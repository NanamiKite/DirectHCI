//! `bt-hci` controller adapter for a DirectHCI runtime client session.
//!
//! Command Complete/Status correlation remains in DirectHCI. Only genuinely
//! unsolicited events and ACL data are exposed through `Controller::read`.

use std::convert::Infallible;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use bt_hci::cmd::{self, AsyncCmd, SyncCmd};
use bt_hci::controller::{Controller, ControllerCmdAsync, ControllerCmdSync};
use bt_hci::event::CommandCompleteWithStatus;
use bt_hci::param::{RemainingBytes, Status};
use bt_hci::{ControllerToHostPacket, FromHciBytes, PacketKind, WriteHci};
use directhci_client::{ClientError, RawHciClientSession};
use directhci_core::{HciAclPacket, HciCommandResponse, HciIncomingPacket};
use tokio::sync::{Mutex as AsyncMutex, mpsc as async_mpsc, oneshot, watch};

const REQUEST_DEPTH: usize = 32;
const INBOUND_DEPTH: usize = 128;
const POLL_INTERVAL: Duration = Duration::from_millis(5);
const MAX_DRAIN_PER_TICK: usize = 32;
// bt-hci 0.10.1 models Disconnect as SyncCmd<Return = ()>, although the
// controller acknowledges it with Command Status and completes it later with
// Disconnection Complete. Keep this exception opcode-scoped.
const HCI_DISCONNECT_OPCODE: u16 = 0x0406;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AdapterError {
    Client(String),
    Encode(String),
    Decode(String),
    UnexpectedResponse {
        expected: u16,
        actual: u16,
    },
    UnexpectedResponseKind(&'static str),
    UnsupportedPacket(&'static str),
    Backpressure,
    Closed,
    WorkerPanicked,
    Cleanup {
        primary: Box<AdapterError>,
        cleanup: Box<AdapterError>,
    },
}
impl std::fmt::Display for AdapterError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Client(v) => write!(f, "DirectHCI client: {v}"),
            Self::Encode(v) => write!(f, "HCI encode: {v}"),
            Self::Decode(v) => write!(f, "HCI decode: {v}"),
            Self::UnexpectedResponse { expected, actual } => write!(
                f,
                "command response opcode {actual:#06x}, expected {expected:#06x}"
            ),
            Self::UnexpectedResponseKind(v) => write!(f, "unexpected command response kind: {v}"),
            Self::UnsupportedPacket(v) => write!(f, "unsupported HCI packet: {v}"),
            Self::Backpressure => f.write_str("DirectHCI adapter inbound queue overflow"),
            Self::Closed => f.write_str("DirectHCI adapter is closed"),
            Self::WorkerPanicked => f.write_str("DirectHCI adapter worker panicked"),
            Self::Cleanup { primary, cleanup } => {
                write!(f, "primary: {primary}; cleanup: {cleanup}")
            }
        }
    }
}
impl std::error::Error for AdapterError {}
impl embedded_io::Error for AdapterError {
    fn kind(&self) -> embedded_io::ErrorKind {
        embedded_io::ErrorKind::Other
    }
}
impl From<ClientError> for AdapterError {
    fn from(value: ClientError) -> Self {
        Self::Client(value.to_string())
    }
}

enum Request {
    Command {
        opcode: u16,
        params: Vec<u8>,
        reply: oneshot::Sender<Result<HciCommandResponse, AdapterError>>,
    },
    Acl {
        packet: HciAclPacket,
        reply: oneshot::Sender<Result<(), AdapterError>>,
    },
}
enum Inbound {
    Event(Vec<u8>),
    Acl(Vec<u8>),
}
struct Shared {
    requests: async_mpsc::Sender<Request>,
    inbound: AsyncMutex<async_mpsc::Receiver<Result<Inbound, AdapterError>>>,
    worker: Mutex<Option<JoinHandle<()>>>,
    stopping: Arc<AtomicBool>,
    completed: watch::Receiver<Option<Result<(), AdapterError>>>,
}

#[derive(Clone)]
pub struct DirectHciController {
    shared: Arc<Shared>,
}
impl DirectHciController {
    pub fn new(session: RawHciClientSession) -> Result<Self, AdapterError> {
        let (request_tx, request_rx) = async_mpsc::channel(REQUEST_DEPTH);
        let (inbound_tx, inbound_rx) = async_mpsc::channel(INBOUND_DEPTH);
        let stopping = Arc::new(AtomicBool::new(false));
        let worker_stopping = Arc::clone(&stopping);
        let (completed_tx, completed_rx) = watch::channel(None);
        let worker = thread::Builder::new()
            .name("directhci-bt-hci".into())
            .spawn(move || {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    worker_loop(session, request_rx, inbound_tx, worker_stopping)
                }))
                .unwrap_or(Err(AdapterError::WorkerPanicked));
                // worker_loop's session, channel receivers and other owned
                // resources have ALL been dropped before publishing completion.
                completed_tx.send_replace(Some(result));
            })
            .map_err(|e| AdapterError::Client(e.to_string()))?;
        Ok(Self {
            shared: Arc::new(Shared {
                requests: request_tx,
                inbound: AsyncMutex::new(inbound_rx),
                worker: Mutex::new(Some(worker)),
                stopping,
                completed: completed_rx,
            }),
        })
    }
    async fn raw_command(
        &self,
        opcode: u16,
        params: Vec<u8>,
    ) -> Result<HciCommandResponse, AdapterError> {
        if self.shared.stopping.load(Ordering::Acquire) {
            return Err(AdapterError::Closed);
        }
        let (tx, rx) = oneshot::channel();
        self.shared
            .requests
            .send(Request::Command {
                opcode,
                params,
                reply: tx,
            })
            .await
            .map_err(|_| AdapterError::Closed)?;
        rx.await.map_err(|_| AdapterError::Closed)?
    }
    pub async fn shutdown(&self) -> Result<(), AdapterError> {
        // Out-of-band stop cannot be stranded behind a full request queue.
        // The shared completion remains observable if this future is cancelled.
        self.shared.stopping.store(true, Ordering::Release);
        let mut completed = self.shared.completed.clone();
        let result = loop {
            if let Some(result) = completed.borrow().clone() {
                break result;
            }
            if completed.changed().await.is_err() {
                break Err(AdapterError::WorkerPanicked);
            }
        };
        let worker = {
            self.shared
                .worker
                .lock()
                .map_err(|_| AdapterError::Closed)?
                .take()
        };
        if let Some(worker) = worker {
            // A final channel signal precedes OS/TLS thread-exit bookkeeping.
            // Reap off the executor without requiring a Tokio runtime in a
            // standalone bt-hci consumer. Cancelling this await does not kill
            // the reaper or worker and cannot free their resources early.
            let (joined_tx, joined_rx) = oneshot::channel();
            thread::Builder::new()
                .name("directhci-bt-hci-reap".into())
                .spawn(move || {
                    let result = worker.join().map_err(|_| AdapterError::WorkerPanicked);
                    let _ = joined_tx.send(result);
                })
                .map_err(|error| AdapterError::Client(format!("start shutdown reaper: {error}")))?;
            joined_rx
                .await
                .map_err(|_| AdapterError::WorkerPanicked)??;
        }
        result
    }
}
fn worker_loop(
    mut session: RawHciClientSession,
    mut requests: async_mpsc::Receiver<Request>,
    inbound: async_mpsc::Sender<Result<Inbound, AdapterError>>,
    stopping: Arc<AtomicBool>,
) -> Result<(), AdapterError> {
    let primary = loop {
        if stopping.load(Ordering::Acquire) {
            break None;
        }
        match requests.try_recv() {
            Ok(Request::Command {
                opcode,
                params,
                reply,
            }) => {
                let _ = reply.send(session.send_command(opcode, &params).map_err(Into::into));
            }
            Ok(Request::Acl { packet, reply }) => {
                let _ = reply.send(session.send_acl(&packet).map_err(Into::into));
            }
            Err(async_mpsc::error::TryRecvError::Disconnected) => {
                break None;
            }
            Err(async_mpsc::error::TryRecvError::Empty) => thread::sleep(POLL_INTERVAL),
        }
        if let Err(error) = pump_inbound(&session, &inbound) {
            let _ = inbound.try_send(Err(error.clone()));
            break Some(error);
        }
    };
    let cleanup = session.release().map(|_| ()).map_err(AdapterError::from);
    match (primary, cleanup) {
        (None, result) => result,
        (Some(primary), Ok(())) => Err(primary),
        (Some(primary), Err(cleanup)) => Err(AdapterError::Cleanup {
            primary: Box::new(primary),
            cleanup: Box::new(cleanup),
        }),
    }
}
fn pump_inbound(
    session: &RawHciClientSession,
    tx: &async_mpsc::Sender<Result<Inbound, AdapterError>>,
) -> Result<(), AdapterError> {
    for _ in 0..MAX_DRAIN_PER_TICK {
        // Reserve BEFORE consuming the SDK stream. A full queue only pauses
        // RX pumping; it does not drop packets or prevent shutdown commands.
        let permit = match tx.try_reserve() {
            Ok(permit) => permit,
            Err(async_mpsc::error::TrySendError::Full(_)) => break,
            Err(async_mpsc::error::TrySendError::Closed(_)) => return Err(AdapterError::Closed),
        };
        let Some(packet) = session.receive_packet(Duration::ZERO)? else {
            break;
        };
        let inbound = match packet {
            HciIncomingPacket::Event(event) => {
                if event
                    .command_response()
                    .map_err(|e| AdapterError::Decode(e.to_string()))?
                    .is_some()
                {
                    continue;
                }
                Inbound::Event(
                    event
                        .encode()
                        .map_err(|e| AdapterError::Decode(e.to_string()))?,
                )
            }
            HciIncomingPacket::Acl(acl) => Inbound::Acl(
                acl.encode()
                    .map_err(|e| AdapterError::Decode(e.to_string()))?,
            ),
        };
        permit.send(Ok(inbound));
    }
    Ok(())
}
impl embedded_io::ErrorType for DirectHciController {
    type Error = AdapterError;
}
impl Controller for DirectHciController {
    type Buffer<'a> = Vec<u8>;
    fn alloc_buf(&self) -> Result<Self::Buffer<'_>, Self::Error> {
        Ok(vec![0; 4 + u16::MAX as usize])
    }
    async fn write_acl_data(
        &self,
        packet: &bt_hci::data::AclPacket<'_>,
    ) -> Result<(), Self::Error> {
        if self.shared.stopping.load(Ordering::Acquire) {
            return Err(AdapterError::Closed);
        }
        let (tx, rx) = oneshot::channel();
        let p = HciAclPacket {
            handle: packet.handle().into_inner(),
            packet_boundary_flag: packet.boundary_flag() as u8,
            broadcast_flag: packet.broadcast_flag() as u8,
            payload: packet.data().to_vec(),
        };
        self.shared
            .requests
            .send(Request::Acl {
                packet: p,
                reply: tx,
            })
            .await
            .map_err(|_| AdapterError::Closed)?;
        rx.await.map_err(|_| AdapterError::Closed)?
    }
    async fn write_sync_data(&self, _: &bt_hci::data::SyncPacket<'_>) -> Result<(), Self::Error> {
        Err(AdapterError::UnsupportedPacket("synchronous data"))
    }
    async fn write_iso_data(&self, _: &bt_hci::data::IsoPacket<'_>) -> Result<(), Self::Error> {
        Err(AdapterError::UnsupportedPacket("ISO data"))
    }
    async fn read<'a>(
        &self,
        buf: &'a mut Self::Buffer<'_>,
    ) -> Result<ControllerToHostPacket<'a>, Self::Error> {
        let item = self
            .shared
            .inbound
            .lock()
            .await
            .recv()
            .await
            .ok_or(AdapterError::Closed)??;
        let (kind, bytes) = match item {
            Inbound::Event(v) => (PacketKind::Event, v),
            Inbound::Acl(v) => (PacketKind::AclData, v),
        };
        if bytes.len() > buf.len() {
            return Err(AdapterError::Decode(
                "packet exceeds controller buffer".into(),
            ));
        }
        buf[..bytes.len()].copy_from_slice(&bytes);
        let (packet, rest) =
            ControllerToHostPacket::from_hci_bytes_with_kind(kind, &buf[..bytes.len()])
                .map_err(|e| AdapterError::Decode(format!("{e:?}")))?;
        if !rest.is_empty() {
            return Err(AdapterError::Decode("trailing HCI packet bytes".into()));
        }
        Ok(packet)
    }
}
impl<C: SyncCmd> ControllerCmdSync<C> for DirectHciController {
    async fn exec(&self, command: &C) -> Result<C::Return, cmd::Error<Self::Error>> {
        let params = encode_params(command.params()).map_err(cmd::Error::Io)?;
        match self
            .raw_command(C::OPCODE.to_raw(), params)
            .await
            .map_err(cmd::Error::Io)?
        {
            HciCommandResponse::Complete {
                command_opcode,
                return_parameters,
                num_hci_command_packets,
            } => {
                if command_opcode != C::OPCODE.to_raw() {
                    return Err(cmd::Error::Io(AdapterError::UnexpectedResponse {
                        expected: C::OPCODE.to_raw(),
                        actual: command_opcode,
                    }));
                }
                let Some((&status, values)) = return_parameters.split_first() else {
                    return Err(cmd::Error::Io(AdapterError::Decode(
                        "Command Complete omitted status".into(),
                    )));
                };
                let remaining = RemainingBytes::from_hci_bytes_complete(values)
                    .map_err(|e| cmd::Error::Io(AdapterError::Decode(format!("{e:?}"))))?;
                CommandCompleteWithStatus {
                    num_hci_cmd_pkts: num_hci_command_packets,
                    cmd_opcode: C::OPCODE,
                    status: Status::new(status),
                    return_param_bytes: remaining,
                }
                .to_result::<C>()
                .map_err(cmd::Error::Hci)
            }
            HciCommandResponse::Status {
                command_opcode,
                status,
                ..
            } => {
                if command_opcode != C::OPCODE.to_raw() {
                    return Err(cmd::Error::Io(AdapterError::UnexpectedResponse {
                        expected: C::OPCODE.to_raw(),
                        actual: command_opcode,
                    }));
                }
                if C::OPCODE.to_raw() != HCI_DISCONNECT_OPCODE {
                    return Err(cmd::Error::Io(AdapterError::UnexpectedResponseKind(
                        "Command Status for synchronous command",
                    )));
                }
                Status::new(status).to_result().map_err(cmd::Error::Hci)?;
                C::Return::from_hci_bytes_complete(&[])
                    .map_err(|e| cmd::Error::Io(AdapterError::Decode(format!("{e:?}"))))
            }
        }
    }
}
impl<C: AsyncCmd> ControllerCmdAsync<C> for DirectHciController {
    async fn exec(&self, command: &C) -> Result<(), cmd::Error<Self::Error>> {
        let params = encode_params(command.params()).map_err(cmd::Error::Io)?;
        match self
            .raw_command(C::OPCODE.to_raw(), params)
            .await
            .map_err(cmd::Error::Io)?
        {
            HciCommandResponse::Status {
                command_opcode,
                status,
                ..
            } => {
                if command_opcode != C::OPCODE.to_raw() {
                    return Err(cmd::Error::Io(AdapterError::UnexpectedResponse {
                        expected: C::OPCODE.to_raw(),
                        actual: command_opcode,
                    }));
                }
                Status::new(status).to_result().map_err(cmd::Error::Hci)
            }
            HciCommandResponse::Complete {
                command_opcode,
                return_parameters,
                ..
            } => {
                if command_opcode != C::OPCODE.to_raw() {
                    return Err(cmd::Error::Io(AdapterError::UnexpectedResponse {
                        expected: C::OPCODE.to_raw(),
                        actual: command_opcode,
                    }));
                }
                let status = return_parameters.first().copied().ok_or_else(|| {
                    cmd::Error::Io(AdapterError::Decode(
                        "Command Complete omitted status".into(),
                    ))
                })?;
                Status::new(status).to_result().map_err(cmd::Error::Hci)
            }
        }
    }
}
fn encode_params(value: &impl WriteHci) -> Result<Vec<u8>, AdapterError> {
    let mut sink = VecSink(Vec::with_capacity(value.size()));
    value.write_hci(&mut sink).expect("VecSink is infallible");
    Ok(sink.0)
}
struct VecSink(Vec<u8>);
impl embedded_io::ErrorType for VecSink {
    type Error = Infallible;
}
impl embedded_io::Write for VecSink {
    fn write(&mut self, buf: &[u8]) -> Result<usize, Self::Error> {
        self.0.extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> Result<(), Self::Error> {
        Ok(())
    }
}
