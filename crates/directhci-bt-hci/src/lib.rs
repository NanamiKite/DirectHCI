//! `bt-hci` controller adapter for a DirectHCI runtime client session.
//!
//! Command Complete/Status correlation remains in DirectHCI. Only genuinely
//! unsolicited events and ACL data are exposed through `Controller::read`.

use std::convert::Infallible;
use std::sync::{Arc, Mutex, mpsc};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use bt_hci::cmd::{self, AsyncCmd, SyncCmd};
use bt_hci::controller::{Controller, ControllerCmdAsync, ControllerCmdSync};
use bt_hci::event::CommandCompleteWithStatus;
use bt_hci::param::{RemainingBytes, Status};
use bt_hci::{ControllerToHostPacket, FromHciBytes, PacketKind, WriteHci};
use directhci_client::{ClientError, RawHciClientSession};
use directhci_core::{HciAclPacket, HciCommandResponse};
use tokio::sync::{Mutex as AsyncMutex, mpsc as async_mpsc, oneshot};

const REQUEST_DEPTH: usize = 32;
const INBOUND_DEPTH: usize = 128;
const POLL_INTERVAL: Duration = Duration::from_millis(5);
const MAX_DRAIN_PER_TICK: usize = 32;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AdapterError {
    Client(String),
    Encode(String),
    Decode(String),
    UnexpectedResponse { expected: u16, actual: u16 },
    UnexpectedResponseKind(&'static str),
    UnsupportedPacket(&'static str),
    Backpressure,
    Closed,
    WorkerPanicked,
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
    Shutdown {
        reply: oneshot::Sender<Result<(), AdapterError>>,
    },
}
enum Inbound {
    Event(Vec<u8>),
    Acl(Vec<u8>),
}
struct Shared {
    requests: mpsc::SyncSender<Request>,
    inbound: AsyncMutex<async_mpsc::Receiver<Result<Inbound, AdapterError>>>,
    worker: Mutex<Option<JoinHandle<()>>>,
}

#[derive(Clone)]
pub struct DirectHciController {
    shared: Arc<Shared>,
}
impl DirectHciController {
    pub fn new(session: RawHciClientSession) -> Result<Self, AdapterError> {
        let (request_tx, request_rx) = mpsc::sync_channel(REQUEST_DEPTH);
        let (inbound_tx, inbound_rx) = async_mpsc::channel(INBOUND_DEPTH);
        let worker = thread::Builder::new()
            .name("directhci-bt-hci".into())
            .spawn(move || worker_loop(session, request_rx, inbound_tx))
            .map_err(|e| AdapterError::Client(e.to_string()))?;
        Ok(Self {
            shared: Arc::new(Shared {
                requests: request_tx,
                inbound: AsyncMutex::new(inbound_rx),
                worker: Mutex::new(Some(worker)),
            }),
        })
    }
    async fn raw_command(
        &self,
        opcode: u16,
        params: Vec<u8>,
    ) -> Result<HciCommandResponse, AdapterError> {
        let (tx, rx) = oneshot::channel();
        self.shared
            .requests
            .try_send(Request::Command {
                opcode,
                params,
                reply: tx,
            })
            .map_err(map_send)?;
        rx.await.map_err(|_| AdapterError::Closed)?
    }
    pub async fn shutdown(&self) -> Result<(), AdapterError> {
        let worker = self
            .shared
            .worker
            .lock()
            .map_err(|_| AdapterError::Closed)?
            .take();
        let Some(worker) = worker else {
            return Ok(());
        };
        let (tx, rx) = oneshot::channel();
        self.shared
            .requests
            .send(Request::Shutdown { reply: tx })
            .map_err(|_| AdapterError::Closed)?;
        let result = rx.await.map_err(|_| AdapterError::Closed)?;
        worker.join().map_err(|_| AdapterError::WorkerPanicked)?;
        result
    }
}
fn map_send<T>(error: mpsc::TrySendError<T>) -> AdapterError {
    match error {
        mpsc::TrySendError::Full(_) => AdapterError::Backpressure,
        mpsc::TrySendError::Disconnected(_) => AdapterError::Closed,
    }
}
fn worker_loop(
    mut session: RawHciClientSession,
    requests: mpsc::Receiver<Request>,
    inbound: async_mpsc::Sender<Result<Inbound, AdapterError>>,
) {
    loop {
        match requests.recv_timeout(POLL_INTERVAL) {
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
            Ok(Request::Shutdown { reply }) => {
                let result = session.release().map(|_| ()).map_err(Into::into);
                let _ = reply.send(result);
                break;
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                let _ = session.release();
                break;
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
        if let Err(error) = pump_inbound(&session, &inbound) {
            let _ = inbound.try_send(Err(error));
            let _ = session.release();
            break;
        }
    }
}
fn pump_inbound(
    session: &RawHciClientSession,
    tx: &async_mpsc::Sender<Result<Inbound, AdapterError>>,
) -> Result<(), AdapterError> {
    for _ in 0..MAX_DRAIN_PER_TICK {
        let Some(event) = session.receive_event(Duration::ZERO)? else {
            break;
        };
        if event
            .command_response()
            .map_err(|e| AdapterError::Decode(e.to_string()))?
            .is_some()
        {
            continue;
        }
        tx.try_send(Ok(Inbound::Event(
            event
                .encode()
                .map_err(|e| AdapterError::Decode(e.to_string()))?,
        )))
        .map_err(|_| AdapterError::Backpressure)?;
    }
    for _ in 0..MAX_DRAIN_PER_TICK {
        let Some(acl) = session.receive_acl(Duration::ZERO)? else {
            break;
        };
        tx.try_send(Ok(Inbound::Acl(
            acl.encode()
                .map_err(|e| AdapterError::Decode(e.to_string()))?,
        )))
        .map_err(|_| AdapterError::Backpressure)?;
    }
    Ok(())
}
impl embedded_io::ErrorType for DirectHciController {
    type Error = AdapterError;
}
impl Controller for DirectHciController {
    type Buffer<'a> = [u8; 4096];
    fn alloc_buf(&self) -> Result<Self::Buffer<'_>, Self::Error> {
        Ok([0; 4096])
    }
    async fn write_acl_data(
        &self,
        packet: &bt_hci::data::AclPacket<'_>,
    ) -> Result<(), Self::Error> {
        let (tx, rx) = oneshot::channel();
        let p = HciAclPacket {
            handle: packet.handle().into_inner(),
            packet_boundary_flag: packet.boundary_flag() as u8,
            broadcast_flag: packet.broadcast_flag() as u8,
            payload: packet.data().to_vec(),
        };
        self.shared
            .requests
            .try_send(Request::Acl {
                packet: p,
                reply: tx,
            })
            .map_err(map_send)?;
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
            HciCommandResponse::Status { .. } => Err(cmd::Error::Io(
                AdapterError::UnexpectedResponseKind("Command Status for synchronous command"),
            )),
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
