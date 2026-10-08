//! Reusable BLE Central and GATT client API backed by the DirectHCI runtime.
//!
//! This crate owns the bt-hci/TrouBLE lifecycle. Consumers work with BLE
//! addresses, discovered GATT data, raw characteristic values, and structured
//! notification events; they never handle Named Pipes, HCI, WinUSB, or
//! controller ownership directly.

use std::collections::BTreeMap;
use std::fmt;
use std::future::{Future, pending, poll_fn};
use std::pin::Pin;
use std::str::FromStr;
use std::sync::{Arc, Mutex};
use std::task::Poll;
use std::thread::{self, JoinHandle};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use bt_hci::param::{AddrKind, BdAddr, LeAdvEventKind};
use directhci_bt_hci::DirectHciController;
use directhci_client::DirectHciClient;
use embassy_time::Duration as EmbassyDuration;
use tokio::sync::{mpsc, oneshot};
use trouble_host::advertise::AdStructure;
use trouble_host::prelude::*;

const CONNECTIONS: usize = 1;
const L2CAP_CHANNELS: usize = 3;
const MAX_SERVICES: usize = 64;
const MAX_CHARACTERISTICS: usize = 128;
const MAX_DESCRIPTORS: usize = 64;
const VALUE_BUFFER: usize = 4096;
const REQUEST_DEPTH: usize = 32;
const NOTIFICATION_DEPTH: usize = 32;
const DISCONNECT_SETTLE_TIME: Duration = Duration::from_millis(100);
const CONNECT_ATTEMPT_TIMEOUT: Duration = Duration::from_secs(30);
const CONNECT_CANCEL_SETTLE_TIME: Duration = Duration::from_secs(6);
const NOTIFICATION_MTU: usize = trouble_host::config::GATT_CLIENT_NOTIFICATION_MTU;

#[derive(Clone, Debug)]
pub struct BleCentralConfig {
    pub controller_id: Option<String>,
    pub client_name: String,
    pub client_version: Option<String>,
}

impl Default for BleCentralConfig {
    fn default() -> Self {
        Self {
            controller_id: None,
            client_name: "directhci-ble-consumer".into(),
            client_version: None,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum BleAddressKind {
    Public,
    Random,
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct BleAddress {
    pub kind: BleAddressKind,
    pub bytes: [u8; 6],
}

impl fmt::Display for BleAddress {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let kind = match self.kind {
            BleAddressKind::Public => "public",
            BleAddressKind::Random => "random",
        };
        write!(formatter, "{kind}:")?;
        for (index, byte) in self.bytes.iter().enumerate() {
            if index != 0 {
                formatter.write_str(":")?;
            }
            write!(formatter, "{byte:02X}")?;
        }
        Ok(())
    }
}

impl FromStr for BleAddress {
    type Err = BleError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let (kind, bytes) = value.split_once(':').ok_or_else(|| {
            BleError::InvalidInput("address must start with public: or random:".into())
        })?;
        let compact = bytes.replace(':', "");
        if compact.len() != 12 {
            return Err(BleError::InvalidInput(
                "Bluetooth address must contain 6 bytes".into(),
            ));
        }
        let mut parsed = [0u8; 6];
        for (index, byte) in parsed.iter_mut().enumerate() {
            *byte = u8::from_str_radix(&compact[index * 2..index * 2 + 2], 16)
                .map_err(|error| BleError::InvalidInput(error.to_string()))?;
        }
        let kind = match kind.to_ascii_lowercase().as_str() {
            "public" => BleAddressKind::Public,
            "random" => BleAddressKind::Random,
            _ => {
                return Err(BleError::InvalidInput(
                    "address type must be public or random".into(),
                ));
            }
        };
        Ok(Self {
            kind,
            bytes: parsed,
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum BleUuid {
    Uuid16(u16),
    Uuid32(u32),
    Uuid128(u128),
}

impl fmt::Display for BleUuid {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Uuid16(value) => write!(formatter, "{value:04x}"),
            Self::Uuid32(value) => write!(formatter, "{value:08x}"),
            Self::Uuid128(value) => {
                let value = format!("{value:032x}");
                write!(
                    formatter,
                    "{}-{}-{}-{}-{}",
                    &value[0..8],
                    &value[8..12],
                    &value[12..16],
                    &value[16..20],
                    &value[20..32]
                )
            }
        }
    }
}

impl FromStr for BleUuid {
    type Err = BleError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let compact = value.replace('-', "");
        match compact.len() {
            4 => u16::from_str_radix(&compact, 16)
                .map(Self::Uuid16)
                .map_err(|error| BleError::InvalidInput(error.to_string())),
            8 => u32::from_str_radix(&compact, 16)
                .map(Self::Uuid32)
                .map_err(|error| BleError::InvalidInput(error.to_string())),
            32 => u128::from_str_radix(&compact, 16)
                .map(Self::Uuid128)
                .map_err(|error| BleError::InvalidInput(error.to_string())),
            _ => Err(BleError::InvalidInput(
                "UUID must be 4, 8, or 32 hexadecimal digits".into(),
            )),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ScanResult {
    pub address: BleAddress,
    pub rssi: i8,
    pub local_name: Option<String>,
    pub service_uuids: Vec<BleUuid>,
    pub advertisement_data: Vec<Vec<u8>>,
    pub scan_response_data: Vec<Vec<u8>>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GattService {
    pub uuid: BleUuid,
    pub start_handle: u16,
    pub end_handle: u16,
    pub characteristics: Vec<GattCharacteristic>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GattCharacteristic {
    pub service_uuid: BleUuid,
    pub uuid: BleUuid,
    pub value_handle: u16,
    pub properties: u8,
    pub cccd_handle: Option<u16>,
    pub descriptors: Vec<GattDescriptor>,
}

impl GattCharacteristic {
    pub fn supports(&self, property: u8) -> bool {
        self.properties & property != 0
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GattDescriptor {
    pub uuid: BleUuid,
    pub handle: u16,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WriteMode {
    WithResponse,
    WithoutResponse,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SubscriptionMode {
    Notifications,
    Indications,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NotificationKind {
    Notification,
    Indication,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GattEvent {
    pub timestamp: SystemTime,
    pub handle: u16,
    pub kind: NotificationKind,
    pub value: Vec<u8>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BleConnectionInfo {
    pub peer: BleAddress,
    pub att_mtu: u16,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum BleError {
    Runtime(String),
    Acquire(String),
    Connect(String),
    Disconnected(String),
    Gatt(String),
    CccdNotAvailable {
        characteristic: BleUuid,
    },
    UnsupportedWriteMode {
        characteristic: BleUuid,
        mode: WriteMode,
    },
    UnsupportedNotificationMode {
        characteristic: BleUuid,
        mode: SubscriptionMode,
    },
    CharacteristicNotFound(BleUuid),
    CharacteristicChanged {
        uuid: BleUuid,
        handle: u16,
    },
    Listener(String),
    ListenerAlreadyActive,
    Backpressure,
    InvalidState(&'static str),
    InvalidInput(String),
    WorkerStopped,
    WorkerPanicked,
    Cleanup {
        primary: Box<BleError>,
        cleanup: Box<BleError>,
    },
}

impl fmt::Display for BleError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Runtime(value) => write!(formatter, "runtime: {value}"),
            Self::Acquire(value) => write!(formatter, "acquire: {value}"),
            Self::Connect(value) => write!(formatter, "connect: {value}"),
            Self::Disconnected(value) => write!(formatter, "disconnected: {value}"),
            Self::Gatt(value) => write!(formatter, "GATT: {value}"),
            Self::CccdNotAvailable { characteristic } => {
                write!(
                    formatter,
                    "CCCD is not available for characteristic {characteristic}"
                )
            }
            Self::UnsupportedWriteMode {
                characteristic,
                mode,
            } => write!(
                formatter,
                "characteristic {characteristic} does not support {mode:?}"
            ),
            Self::UnsupportedNotificationMode {
                characteristic,
                mode,
            } => write!(
                formatter,
                "characteristic {characteristic} does not support {mode:?}"
            ),
            Self::CharacteristicNotFound(uuid) => {
                write!(formatter, "characteristic {uuid} was not found")
            }
            Self::CharacteristicChanged { uuid, handle } => write!(
                formatter,
                "characteristic {uuid} at handle 0x{handle:04x} is no longer present"
            ),
            Self::Listener(value) => write!(formatter, "listener: {value}"),
            Self::ListenerAlreadyActive => {
                formatter.write_str("this connection already has an active listener")
            }
            Self::Backpressure => formatter.write_str("notification consumer is too slow"),
            Self::InvalidState(value) => write!(formatter, "invalid BLE lifecycle state: {value}"),
            Self::InvalidInput(value) => write!(formatter, "invalid input: {value}"),
            Self::WorkerStopped => formatter.write_str("BLE worker stopped"),
            Self::WorkerPanicked => formatter.write_str("BLE worker panicked"),
            Self::Cleanup { primary, cleanup } => {
                write!(formatter, "primary: {primary}; cleanup: {cleanup}")
            }
        }
    }
}

impl std::error::Error for BleError {}

struct WorkerHandle {
    requests: mpsc::Sender<WorkerRequest>,
    terminal: Arc<Mutex<Option<BleError>>>,
    thread: Option<JoinHandle<()>>,
    shutdown_requested: bool,
}

pub struct DirectHciBleCentral {
    worker: WorkerHandle,
}

impl DirectHciBleCentral {
    pub async fn connect(config: BleCentralConfig) -> Result<Self, BleError> {
        let (request_tx, request_rx) = mpsc::channel(REQUEST_DEPTH);
        let (ready_tx, ready_rx) = oneshot::channel();
        let terminal = Arc::new(Mutex::new(None));
        let worker_terminal = Arc::clone(&terminal);
        let thread = thread::Builder::new()
            .name("directhci-ble".into())
            .spawn(move || worker_thread(config, request_rx, ready_tx, worker_terminal))
            .map_err(|error| BleError::Runtime(error.to_string()))?;
        ready_rx.await.map_err(|_| BleError::WorkerStopped)??;
        Ok(Self {
            worker: WorkerHandle {
                requests: request_tx,
                terminal,
                thread: Some(thread),
                shutdown_requested: false,
            },
        })
    }

    pub async fn scan(&self, duration: Duration) -> Result<Vec<ScanResult>, BleError> {
        request(&self.worker, |reply| WorkerRequest::Scan {
            duration,
            reply,
        })
        .await
    }

    pub async fn connect_device(self, address: BleAddress) -> Result<BleConnection, BleError> {
        let info = request(&self.worker, |reply| WorkerRequest::Connect {
            address,
            reply,
        })
        .await?;
        Ok(BleConnection {
            central: Some(self),
            info,
        })
    }

    pub async fn shutdown(mut self) -> Result<(), BleError> {
        shutdown_worker(&mut self.worker).await
    }
}

impl Drop for DirectHciBleCentral {
    fn drop(&mut self) {
        request_best_effort_shutdown(&mut self.worker);
    }
}

pub struct BleConnection {
    central: Option<DirectHciBleCentral>,
    info: BleConnectionInfo,
}

impl BleConnection {
    pub fn info(&self) -> &BleConnectionInfo {
        &self.info
    }

    pub async fn discover(&self) -> Result<Vec<GattService>, BleError> {
        request(self.worker(), |reply| WorkerRequest::Discover { reply }).await
    }

    pub async fn find_characteristic(&self, uuid: BleUuid) -> Result<GattCharacteristic, BleError> {
        let mut matches = self
            .discover()
            .await?
            .into_iter()
            .flat_map(|service| service.characteristics)
            .filter(|characteristic| characteristic.uuid == uuid);
        let first = matches
            .next()
            .ok_or(BleError::CharacteristicNotFound(uuid))?;
        if matches.next().is_some() {
            return Err(BleError::Gatt(format!(
                "characteristic UUID {uuid} is ambiguous; select a discovered handle"
            )));
        }
        Ok(first)
    }

    pub async fn read(&self, characteristic: &GattCharacteristic) -> Result<Vec<u8>, BleError> {
        let characteristic = characteristic.clone();
        request(self.worker(), |reply| WorkerRequest::Read {
            characteristic,
            reply,
        })
        .await
    }

    pub async fn write(
        &self,
        characteristic: &GattCharacteristic,
        value: impl Into<Vec<u8>>,
        mode: WriteMode,
    ) -> Result<(), BleError> {
        let characteristic = characteristic.clone();
        request(self.worker(), |reply| WorkerRequest::Write {
            characteristic,
            value: value.into(),
            mode,
            reply,
        })
        .await
    }

    pub async fn subscribe(
        &self,
        characteristic: &GattCharacteristic,
        mode: SubscriptionMode,
    ) -> Result<GattNotificationStream, BleError> {
        self.start_listener(ListenerRequest::Subscribe {
            characteristic: characteristic.clone(),
            mode,
        })
        .await
    }

    pub async fn listen(
        &self,
        characteristic: &GattCharacteristic,
    ) -> Result<GattNotificationStream, BleError> {
        self.start_listener(ListenerRequest::Passive {
            characteristic: characteristic.clone(),
        })
        .await
    }

    pub async fn listen_all(&self) -> Result<GattNotificationStream, BleError> {
        self.start_listener(ListenerRequest::All).await
    }

    async fn start_listener(
        &self,
        listener: ListenerRequest,
    ) -> Result<GattNotificationStream, BleError> {
        let (events_tx, events_rx) = mpsc::channel(NOTIFICATION_DEPTH);
        let id = request(self.worker(), |reply| WorkerRequest::StartListener {
            listener,
            events: events_tx,
            reply,
        })
        .await?;
        Ok(GattNotificationStream {
            id: Some(id),
            events: events_rx,
            requests: self.worker().requests.clone(),
            terminal: Arc::clone(&self.worker().terminal),
        })
    }

    pub async fn disconnect(mut self) -> Result<(), BleError> {
        let central = self
            .central
            .take()
            .ok_or(BleError::InvalidState("connection is already closed"))?;
        central.shutdown().await
    }

    fn worker(&self) -> &WorkerHandle {
        &self.central.as_ref().expect("live connection").worker
    }
}

pub struct GattNotificationStream {
    id: Option<u64>,
    events: mpsc::Receiver<Result<GattEvent, BleError>>,
    requests: mpsc::Sender<WorkerRequest>,
    terminal: Arc<Mutex<Option<BleError>>>,
}

impl GattNotificationStream {
    pub async fn recv(&mut self) -> Result<Option<GattEvent>, BleError> {
        match self.events.recv().await {
            Some(value) => value.map(Some),
            None => match terminal_error(&self.terminal) {
                Some(error) => Err(error),
                None => Ok(None),
            },
        }
    }

    pub async fn close(mut self) -> Result<(), BleError> {
        let Some(id) = self.id.take() else {
            return Ok(());
        };
        let (reply_tx, reply_rx) = oneshot::channel();
        self.requests
            .send(WorkerRequest::StopListener {
                id,
                reply: reply_tx,
            })
            .await
            .map_err(|_| BleError::WorkerStopped)?;
        reply_rx.await.map_err(|_| BleError::WorkerStopped)?
    }
}

impl Drop for GattNotificationStream {
    fn drop(&mut self) {
        if let Some(id) = self.id.take() {
            let (reply, _) = oneshot::channel();
            let _ = self
                .requests
                .try_send(WorkerRequest::StopListener { id, reply });
        }
    }
}

async fn request<T>(
    worker: &WorkerHandle,
    build: impl FnOnce(oneshot::Sender<Result<T, BleError>>) -> WorkerRequest,
) -> Result<T, BleError> {
    let (reply_tx, reply_rx) = oneshot::channel();
    worker
        .requests
        .send(build(reply_tx))
        .await
        .map_err(|_| terminal_error(&worker.terminal).unwrap_or(BleError::WorkerStopped))?;
    reply_rx
        .await
        .map_err(|_| terminal_error(&worker.terminal).unwrap_or(BleError::WorkerStopped))?
}

async fn shutdown_worker(worker: &mut WorkerHandle) -> Result<(), BleError> {
    if worker.shutdown_requested {
        return terminal_error(&worker.terminal).map_or(Ok(()), Err);
    }
    worker.shutdown_requested = true;
    let (reply_tx, reply_rx) = oneshot::channel();
    let sent = worker
        .requests
        .send(WorkerRequest::Shutdown {
            reply: Some(reply_tx),
        })
        .await
        .is_ok();
    let result = if sent {
        reply_rx
            .await
            .map_err(|_| terminal_error(&worker.terminal).unwrap_or(BleError::WorkerStopped))?
    } else {
        terminal_error(&worker.terminal).map_or(Err(BleError::WorkerStopped), Err)
    };
    if let Some(thread) = worker.thread.take() {
        if thread.join().is_err() {
            return Err(BleError::WorkerPanicked);
        }
    }
    result
}

fn request_best_effort_shutdown(worker: &mut WorkerHandle) {
    if worker.shutdown_requested {
        return;
    }
    worker.shutdown_requested = true;
    let _ = worker
        .requests
        .try_send(WorkerRequest::Shutdown { reply: None });
}

fn terminal_error(terminal: &Arc<Mutex<Option<BleError>>>) -> Option<BleError> {
    terminal.lock().ok().and_then(|value| value.clone())
}

enum WorkerRequest {
    Scan {
        duration: Duration,
        reply: oneshot::Sender<Result<Vec<ScanResult>, BleError>>,
    },
    Connect {
        address: BleAddress,
        reply: oneshot::Sender<Result<BleConnectionInfo, BleError>>,
    },
    Discover {
        reply: oneshot::Sender<Result<Vec<GattService>, BleError>>,
    },
    Read {
        characteristic: GattCharacteristic,
        reply: oneshot::Sender<Result<Vec<u8>, BleError>>,
    },
    Write {
        characteristic: GattCharacteristic,
        value: Vec<u8>,
        mode: WriteMode,
        reply: oneshot::Sender<Result<(), BleError>>,
    },
    StartListener {
        listener: ListenerRequest,
        events: mpsc::Sender<Result<GattEvent, BleError>>,
        reply: oneshot::Sender<Result<u64, BleError>>,
    },
    StopListener {
        id: u64,
        reply: oneshot::Sender<Result<(), BleError>>,
    },
    Shutdown {
        reply: Option<oneshot::Sender<Result<(), BleError>>>,
    },
}

enum ListenerRequest {
    Passive {
        characteristic: GattCharacteristic,
    },
    All,
    Subscribe {
        characteristic: GattCharacteristic,
        mode: SubscriptionMode,
    },
}

struct ActiveListener<'a> {
    id: u64,
    listener: NotificationListener<'a, NOTIFICATION_MTU>,
    events: mpsc::Sender<Result<GattEvent, BleError>>,
    subscription: Option<Characteristic<[u8]>>,
}

struct WorkerExit {
    primary: Option<BleError>,
    reply: Option<oneshot::Sender<Result<(), BleError>>>,
}

fn worker_thread(
    config: BleCentralConfig,
    requests: mpsc::Receiver<WorkerRequest>,
    ready: oneshot::Sender<Result<(), BleError>>,
    terminal: Arc<Mutex<Option<BleError>>>,
) {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .map_err(|error| BleError::Runtime(error.to_string()));
    let result = match runtime {
        Ok(runtime) => runtime.block_on(worker_main(config, requests, ready)),
        Err(error) => {
            let _ = ready.send(Err(error.clone()));
            Err(error)
        }
    };
    if let Err(error) = result {
        if let Ok(mut value) = terminal.lock() {
            *value = Some(error);
        }
    }
}

async fn worker_main(
    config: BleCentralConfig,
    requests: mpsc::Receiver<WorkerRequest>,
    ready: oneshot::Sender<Result<(), BleError>>,
) -> Result<(), BleError> {
    let runtime_client = match DirectHciClient::connect(config.client_name, config.client_version) {
        Ok(client) => client,
        Err(error) => {
            let error = BleError::Runtime(error.to_string());
            let _ = ready.send(Err(error.clone()));
            return Err(error);
        }
    };
    let controllers = match runtime_client.list_controllers() {
        Ok(controllers) => controllers,
        Err(error) => {
            let error = BleError::Runtime(error.to_string());
            let _ = ready.send(Err(error.clone()));
            return Err(error);
        }
    };
    let controller_id = match config.controller_id {
        Some(id) => id,
        None if controllers.len() == 1 => controllers[0].id.as_str().to_owned(),
        None => {
            let error = BleError::Acquire(format!(
                "controller_id is required when {} controllers are present",
                controllers.len()
            ));
            let _ = ready.send(Err(error.clone()));
            return Err(error);
        }
    };
    let session = match runtime_client.acquire_raw_hci(&controller_id) {
        Ok(session) => session,
        Err(error) => {
            let error = BleError::Acquire(error.to_string());
            let _ = ready.send(Err(error.clone()));
            return Err(error);
        }
    };
    let controller = match DirectHciController::new(session) {
        Ok(controller) => controller,
        Err(error) => {
            let error = BleError::Acquire(error.to_string());
            let _ = ready.send(Err(error.clone()));
            return Err(error);
        }
    };
    let shutdown = controller.clone();
    let collector = ScanCollector::new();
    let mut resources: HostResources<DefaultPacketPool, CONNECTIONS, L2CAP_CHANNELS> =
        HostResources::new();
    let stack = trouble_host::new(controller, &mut resources)
        .set_random_address(local_address())
        .build();
    let mut runner = stack.runner();
    let mut central = stack.central();
    let _ = ready.send(Ok(()));

    let exit = {
        let control = control_loop(&stack, &mut central, &collector, requests);
        tokio::pin!(control);
        tokio::select! {
            biased;
            exit = &mut control => exit,
            result = runner.run_with_handler(&collector) => WorkerExit {
                primary: Some(BleError::Runtime(format!("TrouBLE host runner stopped: {result:?}"))),
                reply: None,
            },
        }
    };
    drop(runner);
    drop(central);
    let cleanup = shutdown
        .shutdown()
        .await
        .map_err(|error| BleError::Runtime(format!("DirectHCI release failed: {error}")));
    let result = combine_result(exit.primary, cleanup.err());
    if let Some(reply) = exit.reply {
        let _ = reply.send(result.clone());
    }
    result
}

async fn control_loop<'stack, 'borrow>(
    stack: &'borrow Stack<'stack, DirectHciController, DefaultPacketPool>,
    central: &mut Central<'borrow, DirectHciController, DefaultPacketPool>,
    collector: &ScanCollector,
    mut requests: mpsc::Receiver<WorkerRequest>,
) -> WorkerExit {
    loop {
        let Some(request) = requests.recv().await else {
            return WorkerExit {
                primary: None,
                reply: None,
            };
        };
        match request {
            WorkerRequest::Scan { duration, reply } => {
                if let Some(exit) =
                    run_scan(central, collector, duration, reply, &mut requests).await
                {
                    return exit;
                }
            }
            WorkerRequest::Connect { address, reply } => {
                eprintln!("directhci-ble worker: connect requested for {address}");
                let filter = [to_trouble_address(address)];
                let config = ConnectConfig {
                    connect_params: Default::default(),
                    scan_config: ScanConfig {
                        filter_accept_list: &filter,
                        active: true,
                        phys: PhySet::M1,
                        ..Default::default()
                    },
                };
                enum ConnectOutcome<T> {
                    Completed(T),
                    Shutdown(Option<oneshot::Sender<Result<(), BleError>>>),
                    ChannelClosed,
                    TimedOut,
                }
                // Keep the host runner active while connecting. Dropping the
                // TrouBLE connect future requests LE Create Connection Cancel.
                let outcome = {
                    let connecting = central.connect(&config);
                    tokio::pin!(connecting);
                    let deadline = tokio::time::sleep(CONNECT_ATTEMPT_TIMEOUT);
                    tokio::pin!(deadline);
                    loop {
                        tokio::select! {
                            result = &mut connecting => break ConnectOutcome::Completed(result),
                            _ = &mut deadline => break ConnectOutcome::TimedOut,
                            request = requests.recv() => match request {
                                Some(WorkerRequest::Shutdown { reply }) =>
                                    break ConnectOutcome::Shutdown(reply),
                                Some(request) => reject_request(
                                    request,
                                    BleError::InvalidState("a BLE connection is being established"),
                                ),
                                None => break ConnectOutcome::ChannelClosed,
                            },
                        }
                    }
                };
                let connection = match outcome {
                    ConnectOutcome::Completed(Ok(connection)) => {
                        eprintln!("directhci-ble worker: connection established");
                        connection
                    }
                    ConnectOutcome::Completed(Err(error)) => {
                        eprintln!("directhci-ble worker: connection failed: {error:?}");
                        let error = BleError::Connect(format!("{error:?}"));
                        let _ = reply.send(Err(error.clone()));
                        return WorkerExit {
                            primary: Some(error),
                            reply: None,
                        };
                    }
                    ConnectOutcome::Shutdown(shutdown_reply) => {
                        let _ = reply.send(Err(BleError::Connect(
                            "connection cancelled by shutdown".into(),
                        )));
                        tokio::time::sleep(CONNECT_CANCEL_SETTLE_TIME).await;
                        return WorkerExit {
                            primary: None,
                            reply: shutdown_reply,
                        };
                    }
                    ConnectOutcome::ChannelClosed => {
                        tokio::time::sleep(CONNECT_CANCEL_SETTLE_TIME).await;
                        return WorkerExit {
                            primary: None,
                            reply: None,
                        };
                    }
                    ConnectOutcome::TimedOut => {
                        let error = BleError::Connect(format!(
                            "connection attempt timed out after {} seconds",
                            CONNECT_ATTEMPT_TIMEOUT.as_secs()
                        ));
                        let _ = reply.send(Err(error.clone()));
                        tokio::time::sleep(CONNECT_CANCEL_SETTLE_TIME).await;
                        return WorkerExit {
                            primary: Some(error),
                            reply: None,
                        };
                    }
                };
                eprintln!("directhci-ble worker: initializing GATT client / ATT MTU");
                let client =
                    match GattClient::<DirectHciController, DefaultPacketPool, MAX_SERVICES>::new(
                        stack,
                        &connection,
                    )
                    .await
                    {
                        Ok(client) => {
                            eprintln!(
                                "directhci-ble worker: GATT client created, mtu={}",
                                connection.att_mtu()
                            );
                            client
                        }
                        Err(error) => {
                            eprintln!(
                                "directhci-ble worker: GATT initialization failed: {error:?}; connected={}",
                                connection.is_connected()
                            );
                            if !connection.is_connected() {
                                if let Ok(event) = tokio::time::timeout(
                                    Duration::from_millis(10),
                                    connection.next(),
                                )
                                .await
                                {
                                    eprintln!(
                                        "directhci-ble worker: connection event before GATT ready: {event:?}"
                                    );
                                }
                            }
                            connection.disconnect();
                            let _ = reply.send(Err(BleError::Gatt(format!("{error:?}"))));
                            continue;
                        }
                    };
                return connected_loop(client, connection, requests, address, reply).await;
            }
            WorkerRequest::Shutdown { reply } => {
                return WorkerExit {
                    primary: None,
                    reply,
                };
            }
            request => reject_request(
                request,
                BleError::InvalidState("a BLE connection has not been established"),
            ),
        }
    }
}

async fn run_scan(
    central: &mut Central<'_, DirectHciController, DefaultPacketPool>,
    collector: &ScanCollector,
    duration: Duration,
    reply: oneshot::Sender<Result<Vec<ScanResult>, BleError>>,
    requests: &mut mpsc::Receiver<WorkerRequest>,
) -> Option<WorkerExit> {
    collector.clear();
    let mut scanner = Scanner::new(central);
    let config = ScanConfig {
        active: true,
        phys: PhySet::M1,
        interval: EmbassyDuration::from_millis(100),
        window: EmbassyDuration::from_millis(100),
        ..Default::default()
    };
    let session = match scanner.scan(&config).await {
        Ok(session) => session,
        Err(error) => {
            let _ = reply.send(Err(BleError::Runtime(format!("scan: {error:?}"))));
            return None;
        }
    };
    let timer = tokio::time::sleep(duration);
    tokio::pin!(timer);
    loop {
        tokio::select! {
            _ = &mut timer => {
                session.stop().await;
                let _ = reply.send(collector.snapshot());
                return None;
            }
            request = requests.recv() => {
                match request {
                    Some(WorkerRequest::Shutdown { reply: shutdown_reply }) => {
                        session.stop().await;
                        let _ = reply.send(Err(BleError::InvalidState("scan was cancelled by shutdown")));
                        return Some(WorkerExit {
                            primary: None,
                            reply: shutdown_reply,
                        });
                    }
                    Some(request) => reject_request(
                        request,
                        BleError::InvalidState("a scan is already active"),
                    ),
                    None => {
                        session.stop().await;
                        return Some(WorkerExit {
                            primary: None,
                            reply: None,
                        });
                    }
                }
            }
        }
    }
}

async fn connected_loop<'reference>(
    client: GattClient<'reference, DirectHciController, DefaultPacketPool, MAX_SERVICES>,
    connection: Connection<'reference, DefaultPacketPool>,
    mut requests: mpsc::Receiver<WorkerRequest>,
    peer: BleAddress,
    ready: oneshot::Sender<Result<BleConnectionInfo, BleError>>,
) -> WorkerExit {
    // Own the pinned future, not just a Pin<&mut _> shadow. In particular,
    // drop(task) below must end the GATT receiver before disconnecting.
    let mut task = Box::pin(client.task());
    eprintln!("directhci-ble worker: GATT task started");
    let stopped = poll_fn(|context| match task.as_mut().poll(context) {
        Poll::Ready(result) => Poll::Ready(Some(result)),
        Poll::Pending => Poll::Ready(None),
    })
    .await;
    if let Some(result) = stopped {
        eprintln!("directhci-ble worker: GATT task ended before ready: {result:?}");
        let error = BleError::Gatt(format!(
            "GATT task stopped before connection ready: {result:?}"
        ));
        let _ = ready.send(Err(error.clone()));
        connection.disconnect();
        return WorkerExit {
            primary: Some(error),
            reply: None,
        };
    }
    let _ = ready.send(Ok(BleConnectionInfo {
        peer,
        att_mtu: connection.att_mtu(),
    }));

    let mut active: Option<ActiveListener<'_>> = None;
    // Discovery results belong to this connection only. Reuse verified value
    // handles for later writes instead of rediscovering the full GATT table.
    let mut discovered_services: Vec<GattService> = Vec::new();
    let mut next_listener_id = 1u64;
    let exit = loop {
        tokio::select! {
            biased;
            request = requests.recv() => {
                let Some(request) = request else {
                    break WorkerExit {
                        primary: None,
                        reply: None,
                    };
                };
                match request {
                    WorkerRequest::Discover { reply } => {
                        eprintln!("directhci-ble worker: request discover");
                        discovered_services.clear();
                        match drive_operation(
                            task.as_mut(),
                            &client,
                            &mut active,
                            discover_gatt(&client),
                        )
                        .await
                        {
                            Ok(result) => {
                                eprintln!("directhci-ble worker: discover completed: {}", if result.is_ok() { "ok" } else { "error" });
                                if let Ok(services) = &result {
                                    discovered_services = services.clone();
                                }
                                let _ = reply.send(result);
                            }
                            Err(error) => {
                                let _ = reply.send(Err(error.clone()));
                                break WorkerExit {
                                    primary: Some(error),
                                    reply: None,
                                };
                            }
                        }
                    }
                    WorkerRequest::Read {
                        characteristic,
                        reply,
                    } => {
                        eprintln!("directhci-ble worker: request read");
                        match drive_operation(
                            task.as_mut(),
                            &client,
                            &mut active,
                            read_value(&client, &characteristic),
                        )
                        .await
                        {
                            Ok(result) => {
                                eprintln!("directhci-ble worker: read completed: {}", if result.is_ok() { "ok" } else { "error" });
                                let _ = reply.send(result);
                            }
                            Err(error) => {
                                let _ = reply.send(Err(error.clone()));
                                break WorkerExit {
                                    primary: Some(error),
                                    reply: None,
                                };
                            }
                        }
                    }
                    WorkerRequest::Write {
                        characteristic,
                        value,
                        mode,
                        reply,
                    } => {
                        eprintln!("directhci-ble worker: request write");
                        let discovered_here = discovered_services.iter().any(|service| {
                            service.characteristics.contains(&characteristic)
                        });
                        match drive_operation(
                            task.as_mut(),
                            &client,
                            &mut active,
                            write_value(&client, &characteristic, &value, mode, discovered_here),
                        )
                        .await
                        {
                            Ok(result) => {
                                eprintln!("directhci-ble worker: write completed: {}", if result.is_ok() { "ok" } else { "error" });
                                let _ = reply.send(result);
                            }
                            Err(error) => {
                                eprintln!(
                                    "directhci-ble worker: write aborted source=gatt_task_ended error={error}"
                                );
                                let _ = reply.send(Err(error.clone()));
                                break WorkerExit {
                                    primary: Some(error),
                                    reply: None,
                                };
                            }
                        }
                    }
                    WorkerRequest::StartListener {
                        listener,
                        events,
                        reply,
                    } => {
                        eprintln!("directhci-ble worker: request start-listener");
                        if active.is_some() {
                            let _ = reply.send(Err(BleError::ListenerAlreadyActive));
                            continue;
                        }
                        match drive_without_listener(
                            task.as_mut(),
                            start_listener(&client, listener, events, next_listener_id),
                        )
                        .await
                        {
                            Ok(Ok(listener)) => {
                                let id = listener.id;
                                active = Some(listener);
                                next_listener_id = next_listener_id.wrapping_add(1).max(1);
                                let _ = reply.send(Ok(id));
                            }
                            Ok(Err(error)) => {
                                let _ = reply.send(Err(error));
                            }
                            Err(error) => {
                                let _ = reply.send(Err(error.clone()));
                                break WorkerExit {
                                    primary: Some(error),
                                    reply: None,
                                };
                            }
                        }
                    }
                    WorkerRequest::StopListener { id, reply } => {
                        eprintln!("directhci-ble worker: request stop-listener");
                        match take_subscription(&mut active, id) {
                            Err(error) => {
                                let _ = reply.send(Err(error));
                            }
                            Ok(None) => {
                                let _ = reply.send(Ok(()));
                            }
                            Ok(Some(characteristic)) => {
                                let operation = async {
                                    client
                                        .unsubscribe(&characteristic)
                                        .await
                                        .map_err(|error| {
                                            BleError::Listener(format!(
                                                "unsubscribe: {error:?}"
                                            ))
                                        })
                                };
                                match drive_without_listener(task.as_mut(), operation).await {
                                    Ok(result) => {
                                        let _ = reply.send(result);
                                    }
                                    Err(error) => {
                                        let _ = reply.send(Err(error.clone()));
                                        break WorkerExit {
                                            primary: Some(error),
                                            reply: None,
                                        };
                                    }
                                }
                            }
                        }
                    }
                    WorkerRequest::Shutdown { reply } => {
                        eprintln!("directhci-ble worker: shutdown requested");
                        break WorkerExit {
                            primary: None,
                            reply,
                        };
                    }
                    request => reject_request(
                        request,
                        BleError::InvalidState("a BLE connection is already active"),
                    ),
                }
            }
            result = &mut task => {
                eprintln!("directhci-ble worker: GATT task ended: {result:?}");
                break WorkerExit {
                    primary: Some(BleError::Disconnected(format!(
                        "GATT task stopped: {result:?}"
                    ))),
                    reply: None,
                };
            }
            event = connection.next() => {
                eprintln!("directhci-ble worker: connection event: {event:?}");
                if let trouble_host::connection::ConnectionEvent::Disconnected { reason } = event {
                    break WorkerExit {
                        primary: Some(BleError::Disconnected(format!("connection disconnected: {reason:?}"))),
                        reply: None,
                    };
                }
            }
            notification = next_notification(&mut active) => {
                let Some(notification) = notification else {
                    continue;
                };
                if let Err(error) =
                    forward_notification(&client, &mut active, notification).await
                {
                    break WorkerExit {
                        primary: Some(error),
                        reply: None,
                    };
                }
            }
        }
    };
    if let (Some(listener), Some(error)) = (active.as_ref(), exit.primary.as_ref()) {
        let _ = listener.events.try_send(Err(error.clone()));
    }
    let shutdown_source = match &exit.primary {
        Some(error) => format!("terminal_error:{error}"),
        None if exit.reply.is_some() => "sdk_shutdown_request".to_owned(),
        None => "request_channel_closed".to_owned(),
    };
    drop(active);
    drop(task);
    eprintln!("directhci-ble worker: session shutdown begin source={shutdown_source}");
    connection.disconnect();
    tokio::time::sleep(DISCONNECT_SETTLE_TIME).await;
    eprintln!("directhci-ble worker: session shutdown complete source={shutdown_source}");
    exit
}

async fn drive_operation<T, F, Task>(
    mut task: Pin<&mut Task>,
    client: &GattClient<'_, DirectHciController, DefaultPacketPool, MAX_SERVICES>,
    active: &mut Option<ActiveListener<'_>>,
    operation: F,
) -> Result<T, BleError>
where
    F: Future<Output = T>,
    Task: Future,
    Task::Output: fmt::Debug,
{
    tokio::pin!(operation);
    loop {
        tokio::select! {
            biased;
            result = &mut operation => return Ok(result),
            result = task.as_mut() => {
                return Err(BleError::Disconnected(format!(
                    "GATT task stopped during operation: {result:?}"
                )));
            }
            notification = next_notification(active) => {
                if let Some(notification) = notification {
                    forward_notification(client, active, notification).await?;
                }
            }
        }
    }
}

async fn drive_without_listener<T, F, Task>(
    mut task: Pin<&mut Task>,
    operation: F,
) -> Result<T, BleError>
where
    F: Future<Output = T>,
    Task: Future,
    Task::Output: fmt::Debug,
{
    tokio::pin!(operation);
    tokio::select! {
        biased;
        result = &mut operation => Ok(result),
        result = task.as_mut() => Err(BleError::Disconnected(format!(
            "GATT task stopped during operation: {result:?}"
        ))),
    }
}

async fn forward_notification(
    client: &GattClient<'_, DirectHciController, DefaultPacketPool, MAX_SERVICES>,
    active: &mut Option<ActiveListener<'_>>,
    notification: Notification<NOTIFICATION_MTU>,
) -> Result<(), BleError> {
    let event = GattEvent {
        timestamp: SystemTime::now(),
        handle: notification.handle(),
        kind: if notification.is_indication() {
            NotificationKind::Indication
        } else {
            NotificationKind::Notification
        },
        value: notification.as_ref().to_vec(),
    };
    if notification.is_indication() {
        client
            .confirm_indication()
            .await
            .map_err(|error| BleError::Gatt(format!("indication confirmation: {error:?}")))?;
    }
    let send = active
        .as_ref()
        .expect("notification requires an active listener")
        .events
        .try_send(Ok(event));
    match send {
        Ok(()) => Ok(()),
        Err(mpsc::error::TrySendError::Closed(_)) => {
            // Stream::drop normally queues StopListener. If its receiver
            // closes first, stop local delivery; the queued request or
            // connection teardown handles a standard CCCD subscription.
            active.take();
            Ok(())
        }
        Err(mpsc::error::TrySendError::Full(_)) => Err(BleError::Backpressure),
    }
}

async fn next_notification(
    active: &mut Option<ActiveListener<'_>>,
) -> Option<Notification<NOTIFICATION_MTU>> {
    match active {
        Some(active) => Some(active.listener.next().await),
        None => pending().await,
    }
}

async fn start_listener<'a>(
    client: &'a GattClient<'_, DirectHciController, DefaultPacketPool, MAX_SERVICES>,
    request: ListenerRequest,
    events: mpsc::Sender<Result<GattEvent, BleError>>,
    id: u64,
) -> Result<ActiveListener<'a>, BleError> {
    match request {
        ListenerRequest::All => Ok(ActiveListener {
            id,
            listener: client
                .listen_all()
                .map_err(|error| BleError::Listener(format!("{error:?}")))?,
            events,
            subscription: None,
        }),
        ListenerRequest::Passive { characteristic } => {
            ensure_notification_supported(&characteristic, None)?;
            let resolved = resolve_characteristic(client, &characteristic).await?;
            Ok(ActiveListener {
                id,
                listener: client
                    .listen(&resolved)
                    .map_err(|error| BleError::Listener(format!("{error:?}")))?,
                events,
                subscription: None,
            })
        }
        ListenerRequest::Subscribe {
            characteristic,
            mode,
        } => {
            ensure_notification_supported(&characteristic, Some(mode))?;
            if characteristic.cccd_handle.is_none() {
                return Err(BleError::CccdNotAvailable {
                    characteristic: characteristic.uuid,
                });
            }
            let resolved = resolve_characteristic(client, &characteristic).await?;
            if resolved.cccd_handle.is_none() {
                return Err(BleError::CccdNotAvailable {
                    characteristic: characteristic.uuid,
                });
            }
            let listener = client
                .subscribe(&resolved, mode == SubscriptionMode::Indications)
                .await
                .map_err(|error| BleError::Listener(format!("{error:?}")))?;
            Ok(ActiveListener {
                id,
                listener,
                events,
                subscription: Some(resolved),
            })
        }
    }
}

fn take_subscription(
    active: &mut Option<ActiveListener<'_>>,
    id: u64,
) -> Result<Option<Characteristic<[u8]>>, BleError> {
    match active {
        Some(listener) if listener.id == id => {
            Ok(active.take().and_then(|listener| listener.subscription))
        }
        Some(_) => Err(BleError::Listener("listener ID does not match".into())),
        None => Ok(None),
    }
}

async fn discover_gatt(
    client: &GattClient<'_, DirectHciController, DefaultPacketPool, MAX_SERVICES>,
) -> Result<Vec<GattService>, BleError> {
    let mut services = Vec::new();
    for service in client
        .services()
        .await
        .map_err(|error| BleError::Gatt(format!("{error:?}")))?
    {
        let uuid = from_trouble_uuid(service.uuid());
        let handles = service.handle_range();
        let mut characteristics = Vec::new();
        for characteristic in client
            .characteristics::<MAX_CHARACTERISTICS>(&service)
            .await
            .map_err(|error| BleError::Gatt(format!("{error:?}")))?
        {
            let mut descriptors = Vec::new();
            for descriptor in client
                .descriptors::<_, MAX_DESCRIPTORS>(&characteristic)
                .await
                .map_err(|error| BleError::Gatt(format!("{error:?}")))?
            {
                descriptors.push(GattDescriptor {
                    uuid: from_trouble_uuid(*descriptor.uuid()),
                    handle: descriptor.handle(),
                });
            }
            characteristics.push(GattCharacteristic {
                service_uuid: uuid,
                uuid: from_trouble_uuid(characteristic.uuid),
                value_handle: characteristic.handle,
                properties: characteristic.props.to_raw(),
                cccd_handle: characteristic.cccd_handle,
                descriptors,
            });
        }
        services.push(GattService {
            uuid,
            start_handle: *handles.start(),
            end_handle: *handles.end(),
            characteristics,
        });
    }
    Ok(services)
}

async fn resolve_characteristic(
    client: &GattClient<'_, DirectHciController, DefaultPacketPool, MAX_SERVICES>,
    expected: &GattCharacteristic,
) -> Result<Characteristic<[u8]>, BleError> {
    for service in client
        .services()
        .await
        .map_err(|error| BleError::Gatt(format!("{error:?}")))?
    {
        if from_trouble_uuid(service.uuid()) != expected.service_uuid {
            continue;
        }
        for characteristic in client
            .characteristics::<MAX_CHARACTERISTICS>(&service)
            .await
            .map_err(|error| BleError::Gatt(format!("{error:?}")))?
        {
            if from_trouble_uuid(characteristic.uuid) == expected.uuid
                && characteristic.handle == expected.value_handle
            {
                return Ok(characteristic);
            }
        }
    }
    Err(BleError::CharacteristicChanged {
        uuid: expected.uuid,
        handle: expected.value_handle,
    })
}

async fn read_value(
    client: &GattClient<'_, DirectHciController, DefaultPacketPool, MAX_SERVICES>,
    characteristic: &GattCharacteristic,
) -> Result<Vec<u8>, BleError> {
    let resolved = resolve_characteristic(client, characteristic).await?;
    let mut value = [0u8; VALUE_BUFFER];
    let length = client
        .read_characteristic(&resolved, &mut value)
        .await
        .map_err(|error| BleError::Gatt(format!("{error:?}")))?;
    Ok(value[..length].to_vec())
}

async fn write_value(
    client: &GattClient<'_, DirectHciController, DefaultPacketPool, MAX_SERVICES>,
    characteristic: &GattCharacteristic,
    value: &[u8],
    mode: WriteMode,
    discovered_here: bool,
) -> Result<(), BleError> {
    ensure_write_supported(characteristic, mode)?;
    if discovered_here {
        // This handle was verified by discovery on the current connection.
        // Trouble's write_characteristic delegates to write_handle, so avoid
        // repeating services()/characteristics() ATT discovery per write.
        match mode {
            WriteMode::WithResponse => {
                client
                    .write_handle(characteristic.value_handle, value)
                    .await
            }
            WriteMode::WithoutResponse => {
                client
                    .write_handle_without_response(characteristic.value_handle, value)
                    .await
            }
        }
        .map_err(|error| BleError::Gatt(format!("{error:?}")))
    } else {
        let resolved = resolve_characteristic(client, characteristic).await?;
        match mode {
            WriteMode::WithResponse => client.write_characteristic(&resolved, value).await,
            WriteMode::WithoutResponse => {
                client
                    .write_characteristic_without_response(&resolved, value)
                    .await
            }
        }
        .map_err(|error| BleError::Gatt(format!("{error:?}")))
    }
}

fn ensure_write_supported(
    characteristic: &GattCharacteristic,
    mode: WriteMode,
) -> Result<(), BleError> {
    let property = match mode {
        WriteMode::WithResponse => CharacteristicProp::Write as u8,
        WriteMode::WithoutResponse => CharacteristicProp::WriteWithoutResponse as u8,
    };
    if characteristic.supports(property) {
        Ok(())
    } else {
        Err(BleError::UnsupportedWriteMode {
            characteristic: characteristic.uuid,
            mode,
        })
    }
}

fn ensure_notification_supported(
    characteristic: &GattCharacteristic,
    mode: Option<SubscriptionMode>,
) -> Result<(), BleError> {
    let notification = characteristic.supports(CharacteristicProp::Notify as u8);
    let indication = characteristic.supports(CharacteristicProp::Indicate as u8);
    let supported = match mode {
        None => notification || indication,
        Some(SubscriptionMode::Notifications) => notification,
        Some(SubscriptionMode::Indications) => indication,
    };
    if supported {
        Ok(())
    } else {
        Err(BleError::UnsupportedNotificationMode {
            characteristic: characteristic.uuid,
            mode: mode.unwrap_or(SubscriptionMode::Notifications),
        })
    }
}

fn reject_request(request: WorkerRequest, error: BleError) {
    match request {
        WorkerRequest::Scan { reply, .. } => {
            let _ = reply.send(Err(error));
        }
        WorkerRequest::Connect { reply, .. } => {
            let _ = reply.send(Err(error));
        }
        WorkerRequest::Discover { reply } => {
            let _ = reply.send(Err(error));
        }
        WorkerRequest::Read { reply, .. } => {
            let _ = reply.send(Err(error));
        }
        WorkerRequest::Write { reply, .. } => {
            let _ = reply.send(Err(error));
        }
        WorkerRequest::StartListener { reply, .. } => {
            let _ = reply.send(Err(error));
        }
        WorkerRequest::StopListener { reply, .. } => {
            let _ = reply.send(Err(error));
        }
        WorkerRequest::Shutdown { reply } => {
            if let Some(reply) = reply {
                let _ = reply.send(Err(error));
            }
        }
    }
}

fn combine_result(primary: Option<BleError>, cleanup: Option<BleError>) -> Result<(), BleError> {
    match (primary, cleanup) {
        (None, None) => Ok(()),
        (Some(primary), None) => Err(primary),
        (None, Some(cleanup)) => Err(cleanup),
        (Some(primary), Some(cleanup)) => Err(BleError::Cleanup {
            primary: Box::new(primary),
            cleanup: Box::new(cleanup),
        }),
    }
}

struct ScanCollector {
    records: Mutex<BTreeMap<BleAddress, ScanResult>>,
}

impl ScanCollector {
    fn new() -> Self {
        Self {
            records: Mutex::new(BTreeMap::new()),
        }
    }

    fn clear(&self) {
        if let Ok(mut records) = self.records.lock() {
            records.clear();
        }
    }

    fn snapshot(&self) -> Result<Vec<ScanResult>, BleError> {
        self.records
            .lock()
            .map(|records| records.values().cloned().collect())
            .map_err(|_| BleError::Runtime("scan record lock poisoned".into()))
    }
}

impl EventHandler for ScanCollector {
    fn on_adv_reports(&self, reports: LeAdvReportsIter<'_>) {
        let Ok(mut records) = self.records.lock() else {
            return;
        };
        for report in reports.flatten() {
            let address = from_trouble_address(Address::new(report.addr_kind, report.addr));
            let (name, service_uuids) = parse_advertisement(report.data);
            let record = records.entry(address).or_insert_with(|| ScanResult {
                address,
                rssi: report.rssi,
                local_name: None,
                service_uuids: Vec::new(),
                advertisement_data: Vec::new(),
                scan_response_data: Vec::new(),
            });
            record.rssi = report.rssi;
            if name.is_some() {
                record.local_name = name;
            }
            let raw = if report.event_kind == LeAdvEventKind::ScanRsp {
                &mut record.scan_response_data
            } else {
                &mut record.advertisement_data
            };
            if !raw.iter().any(|value| value == report.data) {
                raw.push(report.data.to_vec());
            }
            for uuid in service_uuids {
                if !record.service_uuids.contains(&uuid) {
                    record.service_uuids.push(uuid);
                }
            }
        }
    }
}

fn parse_advertisement(data: &[u8]) -> (Option<String>, Vec<BleUuid>) {
    let mut name = None;
    let mut uuids = Vec::new();
    for item in AdStructure::decode(data).filter_map(Result::ok) {
        match item {
            AdStructure::CompleteLocalName(value) | AdStructure::ShortenedLocalName(value) => {
                name = Some(String::from_utf8_lossy(value).into_owned());
            }
            AdStructure::IncompleteServiceUuids16(values)
            | AdStructure::CompleteServiceUuids16(values) => {
                uuids.extend(
                    values
                        .iter()
                        .map(|value| BleUuid::Uuid16(u16::from_le_bytes(*value))),
                );
            }
            AdStructure::IncompleteServiceUuids32(values)
            | AdStructure::CompleteServiceUuids32(values) => {
                uuids.extend(
                    values
                        .iter()
                        .map(|value| BleUuid::Uuid32(u32::from_le_bytes(*value))),
                );
            }
            AdStructure::IncompleteServiceUuids128(values)
            | AdStructure::CompleteServiceUuids128(values) => {
                uuids.extend(
                    values
                        .iter()
                        .map(|value| BleUuid::Uuid128(u128::from_le_bytes(*value))),
                );
            }
            _ => {}
        }
    }
    (name, uuids)
}

fn to_trouble_address(address: BleAddress) -> Address {
    let mut bytes = address.bytes;
    bytes.reverse();
    let kind = match address.kind {
        BleAddressKind::Public => AddrKind::PUBLIC,
        BleAddressKind::Random => AddrKind::RANDOM,
    };
    Address::new(kind, BdAddr::new(bytes))
}

fn from_trouble_address(address: Address) -> BleAddress {
    let mut bytes = address.addr.into_inner();
    bytes.reverse();
    BleAddress {
        kind: if address.kind.as_raw() & 1 == 0 {
            BleAddressKind::Public
        } else {
            BleAddressKind::Random
        },
        bytes,
    }
}

fn from_trouble_uuid(uuid: Uuid) -> BleUuid {
    match uuid {
        Uuid::Uuid16(value) => BleUuid::Uuid16(u16::from_le_bytes(value)),
        Uuid::Uuid32(value) => BleUuid::Uuid32(u32::from_le_bytes(value)),
        Uuid::Uuid128(value) => BleUuid::Uuid128(u128::from_le_bytes(value)),
    }
}

fn local_address() -> Address {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let mut bytes = [0u8; 6];
    bytes.copy_from_slice(&now.to_le_bytes()[..6]);
    bytes[5] = (bytes[5] & 0x3f) | 0xc0;
    Address::random(bytes)
}
