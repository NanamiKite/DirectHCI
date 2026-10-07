//! Local DirectHCI runtime client SDK.

use std::time::Duration;

use directhci_core::{
    ControllerObservation, HciAclPacket, HciCommandResponse, HciEventPacket, HciIncomingPacket,
    RuntimeControllerStatus, RuntimePreferences, RuntimeStatus,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ClientError {
    UnsupportedPlatform,
    Connection(String),
    Protocol(String),
    Server {
        code: directhci_core::IpcErrorCode,
        message: String,
    },
    Timeout,
    Disconnected(String),
    Backpressure,
    InvalidSession(String),
}

impl std::fmt::Display for ClientError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnsupportedPlatform => {
                formatter.write_str("DirectHCI client is only available on Windows")
            }
            Self::Connection(message) => write!(formatter, "connection: {message}"),
            Self::Protocol(message) => write!(formatter, "protocol: {message}"),
            Self::Server { code, message } => write!(formatter, "server {code:?}: {message}"),
            Self::Timeout => formatter.write_str("runtime request timed out"),
            Self::Disconnected(message) => write!(formatter, "runtime disconnected: {message}"),
            Self::Backpressure => formatter.write_str("client receive queue overflow"),
            Self::InvalidSession(message) => write!(formatter, "invalid session: {message}"),
        }
    }
}
impl std::error::Error for ClientError {}

pub struct DirectHciClient {
    inner: platform::Client,
}

impl DirectHciClient {
    pub fn connect(
        client_name: impl Into<String>,
        client_version: Option<String>,
    ) -> Result<Self, ClientError> {
        Ok(Self {
            inner: platform::Client::connect(client_name.into(), client_version)?,
        })
    }

    /// Connect to the local runtime, starting the demand-start Windows service
    /// if it is not listening. Starting the service still requires Windows
    /// SERVICE_START permission; this does not broaden Raw HCI authorization.
    pub fn connect_or_start(
        client_name: impl Into<String>,
        client_version: Option<String>,
    ) -> Result<Self, ClientError> {
        let client_name = client_name.into();
        match Self::connect(client_name.clone(), client_version.clone()) {
            Ok(client) => return Ok(client),
            Err(ClientError::Connection(_)) => platform::start_service()?,
            Err(error) => return Err(error),
        }
        let mut last_error = ClientError::Connection("runtime service did not become ready".into());
        for _ in 0..6 {
            match Self::connect(client_name.clone(), client_version.clone()) {
                Ok(client) => return Ok(client),
                Err(error @ ClientError::Connection(_)) => last_error = error,
                Err(error) => return Err(error),
            }
            std::thread::sleep(Duration::from_secs(1));
        }
        Err(last_error)
    }
    pub fn server_hello(&self) -> &directhci_core::ServerHello {
        self.inner.server_hello()
    }
    pub fn list_controllers(&self) -> Result<Vec<ControllerObservation>, ClientError> {
        self.inner.list_controllers()
    }
    pub fn runtime_status(&self) -> Result<RuntimeStatus, ClientError> {
        self.inner.runtime_status()
    }
    pub fn preferences(&self) -> Result<RuntimePreferences, ClientError> {
        self.inner.preferences()
    }
    pub fn set_preferred_controller(
        &self,
        controller_id: &str,
    ) -> Result<RuntimePreferences, ClientError> {
        self.inner.set_preferred_controller(controller_id)
    }
    pub fn controller_preparation_status(
        &self,
        controller_id: &str,
    ) -> Result<directhci_core::ControllerPreparationStatus, ClientError> {
        self.inner.controller_preparation_status(controller_id)
    }
    /// Requires an administrator and an explicit acknowledgement that libwdi
    /// will temporarily trust a one-time package-signing certificate.
    pub fn prepare_controller(
        &self,
        controller_id: &str,
        trust_acknowledged: bool,
    ) -> Result<directhci_core::PreparedController, ClientError> {
        self.inner
            .prepare_controller(controller_id, trust_acknowledged)
    }
    pub fn restore_windows(&self) -> Result<(), ClientError> {
        self.inner.restore_windows()
    }
    pub fn controller_status(
        &self,
        controller_id: &str,
    ) -> Result<Option<RuntimeControllerStatus>, ClientError> {
        Ok(self
            .runtime_status()?
            .controllers
            .into_iter()
            .find(|status| {
                status
                    .controller
                    .id
                    .as_str()
                    .eq_ignore_ascii_case(controller_id)
            }))
    }
    pub fn acquire_raw_hci(&self, controller_id: &str) -> Result<RawHciClientSession, ClientError> {
        self.inner
            .acquire_raw_hci(controller_id)
            .map(|inner| RawHciClientSession { inner })
    }
}

pub struct RawHciClientSession {
    inner: platform::Session,
}
impl RawHciClientSession {
    pub fn session_id(&self) -> u64 {
        self.inner.session_id()
    }
    pub fn controller(&self) -> &ControllerObservation {
        self.inner.controller()
    }
    pub fn transport(&self) -> &directhci_core::IpcTransportInfo {
        self.inner.transport()
    }
    pub fn send_command(
        &self,
        opcode: u16,
        parameters: &[u8],
    ) -> Result<HciCommandResponse, ClientError> {
        self.inner.send_command(opcode, parameters)
    }
    pub fn send_acl(&self, packet: &HciAclPacket) -> Result<(), ClientError> {
        self.inner.send_acl(packet)
    }
    pub fn receive_event(&self, timeout: Duration) -> Result<Option<HciEventPacket>, ClientError> {
        self.inner.receive_event(timeout)
    }
    pub fn receive_acl(&self, timeout: Duration) -> Result<Option<HciAclPacket>, ClientError> {
        self.inner.receive_acl(timeout)
    }
    /// Receive Event and ACL traffic in IPC arrival order. Do not mix this
    /// with typed receives when ordering between packet kinds matters.
    pub fn receive_packet(
        &self,
        timeout: Duration,
    ) -> Result<Option<HciIncomingPacket>, ClientError> {
        self.inner.receive_packet(timeout)
    }
    pub fn release(&mut self) -> Result<Option<ControllerObservation>, ClientError> {
        self.inner.release()
    }
}

#[cfg(not(windows))]
mod platform {
    use super::*;
    pub(super) fn start_service() -> Result<(), ClientError> {
        Err(ClientError::UnsupportedPlatform)
    }
    pub(super) struct Client;
    pub(super) struct Session;
    impl Client {
        pub(super) fn connect(_: String, _: Option<String>) -> Result<Self, ClientError> {
            Err(ClientError::UnsupportedPlatform)
        }
        pub(super) fn server_hello(&self) -> &directhci_core::ServerHello {
            panic!("unsupported platform")
        }
        pub(super) fn list_controllers(&self) -> Result<Vec<ControllerObservation>, ClientError> {
            Err(ClientError::UnsupportedPlatform)
        }
        pub(super) fn runtime_status(&self) -> Result<RuntimeStatus, ClientError> {
            Err(ClientError::UnsupportedPlatform)
        }
        pub(super) fn preferences(&self) -> Result<RuntimePreferences, ClientError> {
            Err(ClientError::UnsupportedPlatform)
        }
        pub(super) fn set_preferred_controller(
            &self,
            _: &str,
        ) -> Result<RuntimePreferences, ClientError> {
            Err(ClientError::UnsupportedPlatform)
        }
        pub(super) fn controller_preparation_status(
            &self,
            _: &str,
        ) -> Result<directhci_core::ControllerPreparationStatus, ClientError> {
            Err(ClientError::UnsupportedPlatform)
        }
        pub(super) fn prepare_controller(
            &self,
            _: &str,
            _: bool,
        ) -> Result<directhci_core::PreparedController, ClientError> {
            Err(ClientError::UnsupportedPlatform)
        }
        pub(super) fn restore_windows(&self) -> Result<(), ClientError> {
            Err(ClientError::UnsupportedPlatform)
        }
        pub(super) fn acquire_raw_hci(&self, _: &str) -> Result<Session, ClientError> {
            Err(ClientError::UnsupportedPlatform)
        }
    }
    impl Session {
        pub(super) fn session_id(&self) -> u64 {
            0
        }
        pub(super) fn controller(&self) -> &ControllerObservation {
            panic!("unsupported platform")
        }
        pub(super) fn transport(&self) -> &directhci_core::IpcTransportInfo {
            panic!("unsupported platform")
        }
        pub(super) fn send_command(
            &self,
            _: u16,
            _: &[u8],
        ) -> Result<HciCommandResponse, ClientError> {
            Err(ClientError::UnsupportedPlatform)
        }
        pub(super) fn send_acl(&self, _: &HciAclPacket) -> Result<(), ClientError> {
            Err(ClientError::UnsupportedPlatform)
        }
        pub(super) fn receive_event(
            &self,
            _: Duration,
        ) -> Result<Option<HciEventPacket>, ClientError> {
            Err(ClientError::UnsupportedPlatform)
        }
        pub(super) fn receive_acl(&self, _: Duration) -> Result<Option<HciAclPacket>, ClientError> {
            Err(ClientError::UnsupportedPlatform)
        }
        pub(super) fn receive_packet(
            &self,
            _: Duration,
        ) -> Result<Option<HciIncomingPacket>, ClientError> {
            Err(ClientError::UnsupportedPlatform)
        }
        pub(super) fn release(&mut self) -> Result<Option<ControllerObservation>, ClientError> {
            Err(ClientError::UnsupportedPlatform)
        }
    }
}

#[cfg(windows)]
mod platform {
    use std::collections::{HashMap, VecDeque};
    use std::io::{Read, Write};
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::{Arc, Condvar, Mutex, TryLockError, mpsc};
    use std::thread::{self, JoinHandle};
    use std::time::Instant;

    use directhci_core::*;
    use windows::Win32::Foundation::{
        CloseHandle, ERROR_IO_PENDING, ERROR_SERVICE_ALREADY_RUNNING, HANDLE, WAIT_OBJECT_0,
        WAIT_TIMEOUT,
    };
    use windows::Win32::Storage::FileSystem::{
        CreateFileW, FILE_FLAG_OVERLAPPED, FILE_READ_ATTRIBUTES, FILE_READ_DATA, FILE_SHARE_READ,
        FILE_SHARE_WRITE, FILE_WRITE_ATTRIBUTES, FILE_WRITE_DATA, OPEN_EXISTING, READ_CONTROL,
        ReadFile, SECURITY_IMPERSONATION, SECURITY_SQOS_PRESENT, SYNCHRONIZE, WriteFile,
    };
    use windows::Win32::System::IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED};
    use windows::Win32::System::Pipes::WaitNamedPipeW;
    use windows::Win32::System::Services::{
        CloseServiceHandle, OpenSCManagerW, OpenServiceW, SC_MANAGER_CONNECT, SERVICE_START,
        StartServiceW,
    };
    use windows::Win32::System::Threading::{CreateEventW, WaitForSingleObject};
    use windows::core::{HRESULT, PCWSTR};

    use super::*;

    const REQUEST_TIMEOUT: Duration = Duration::from_secs(60);
    const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(5);
    // The runtime's default HCI command budget is 5 seconds. Keep a margin
    // for IPC delivery without stalling release behind a 60-second command.
    const HCI_REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
    // Separate from ordinary requests; allow the daemon's 45-second stop
    // budget plus IPC margin. A timeout is NOT confirmed Windows restore.
    const RELEASE_TIMEOUT: Duration = Duration::from_secs(50);
    const PREPARE_TIMEOUT: Duration = Duration::from_secs(180);
    // Keep the reader available for command/release responses during a burst.
    // Sustained non-consumption still fails explicitly at a bounded hard limit.
    const STREAM_QUEUE_DEPTH: usize = 4096;
    const STREAM_QUEUE_BYTES: usize = 16 * 1024 * 1024;

    pub(super) fn start_service() -> Result<(), ClientError> {
        let manager = unsafe { OpenSCManagerW(PCWSTR::null(), PCWSTR::null(), SC_MANAGER_CONNECT) }
            .map_err(|error| ClientError::Connection(format!("open service manager: {error}")))?;
        let name: Vec<u16> = "DirectHCI".encode_utf16().chain(Some(0)).collect();
        let service = unsafe { OpenServiceW(manager, PCWSTR(name.as_ptr()), SERVICE_START) };
        let _ = unsafe { CloseServiceHandle(manager) };
        let service = service.map_err(|error| {
            ClientError::Connection(format!(
                "open DirectHCI service for on-demand start (SERVICE_START permission required): {error}"
            ))
        })?;
        let result = unsafe { StartServiceW(service, None) };
        let _ = unsafe { CloseServiceHandle(service) };
        match result {
            Ok(()) => Ok(()),
            Err(error) if error.code() == HRESULT::from_win32(ERROR_SERVICE_ALREADY_RUNNING.0) => {
                Ok(())
            }
            Err(error) => Err(ClientError::Connection(format!(
                "start DirectHCI service (administrator or SERVICE_START permission required): {error}"
            ))),
        }
    }

    struct HandleState {
        handle: HANDLE,
        lifecycle: Mutex<HandleLifecycle>,
        completed: Condvar,
    }
    struct HandleLifecycle {
        closing: bool,
        closed: bool,
        active: usize,
    }
    // SAFETY: the lifecycle mutex serializes submission with cancellation.
    // The handle is closed only after every tracked overlapped I/O completes.
    unsafe impl Send for HandleState {}
    unsafe impl Sync for HandleState {}
    impl HandleState {
        fn close_once(&self) {
            let mut lifecycle = self
                .lifecycle
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if lifecycle.closing {
                while !lifecycle.closed {
                    lifecycle = self
                        .completed
                        .wait(lifecycle)
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                }
                return;
            }
            lifecycle.closing = true;
            // No new submission can pass the mutex after this point.
            let _ = unsafe { CancelIoEx(self.handle, None) };
            while lifecycle.active != 0 {
                lifecycle = self
                    .completed
                    .wait(lifecycle)
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
            }
            let _ = unsafe { CloseHandle(self.handle) };
            lifecycle.closed = true;
            self.completed.notify_all();
        }
    }
    impl Drop for HandleState {
        fn drop(&mut self) {
            self.close_once();
        }
    }

    struct ConnectionState {
        pending: Mutex<HashMap<u32, mpsc::SyncSender<Result<IpcFrame, ClientError>>>>,
        stream: Mutex<StreamState>,
        stream_changed: Condvar,
        terminal: Mutex<Option<ClientError>>,
    }

    #[derive(Default)]
    struct StreamState {
        packets: VecDeque<(u64, HciIncomingPacket)>,
        bytes: usize,
        terminal: Option<ClientError>,
    }

    impl ConnectionState {
        fn enqueue(&self, session: u64, packet: HciIncomingPacket) -> Result<(), ClientError> {
            let mut stream = self
                .stream
                .lock()
                .map_err(|_| protocol("stream lock poisoned"))?;
            let size = packet.encoded_len();
            if stream.packets.len() >= STREAM_QUEUE_DEPTH
                || stream.bytes + size > STREAM_QUEUE_BYTES
            {
                return Err(ClientError::Backpressure);
            }
            stream.bytes += size;
            stream.packets.push_back((session, packet));
            self.stream_changed.notify_all();
            Ok(())
        }

        fn receive(
            &self,
            session: u64,
            timeout: Duration,
            kind: Option<bool>,
        ) -> Result<Option<HciIncomingPacket>, ClientError> {
            let deadline = std::time::Instant::now() + timeout;
            let mut stream = self
                .stream
                .lock()
                .map_err(|_| protocol("stream lock poisoned"))?;
            loop {
                let index = stream.packets.iter().position(|(_, packet)| {
                    kind.is_none_or(|event| event == matches!(packet, HciIncomingPacket::Event(_)))
                });
                if let Some(index) = index {
                    let (actual_session, packet) =
                        stream.packets.remove(index).expect("position exists");
                    stream.bytes -= packet.encoded_len();
                    if actual_session != session {
                        return Err(ClientError::InvalidSession(format!(
                            "stream belongs to session {actual_session}"
                        )));
                    }
                    return Ok(Some(packet));
                }
                if let Some(error) = &stream.terminal {
                    return Err(error.clone());
                }
                let remaining = deadline.saturating_duration_since(std::time::Instant::now());
                if remaining.is_zero() {
                    return Ok(None);
                }
                stream = self
                    .stream_changed
                    .wait_timeout(stream, remaining)
                    .map_err(|_| protocol("stream lock poisoned"))?
                    .0;
            }
        }
    }

    struct Connection {
        handle: Arc<HandleState>,
        state: Arc<ConnectionState>,
        writer_lock: Mutex<()>,
        next_request: AtomicU32,
        active_session: Mutex<Option<u64>>,
        reader: Mutex<Option<JoinHandle<()>>>,
    }

    impl Connection {
        fn request_with_timeout(
            &self,
            kind: IpcMessageKind,
            payload: Vec<u8>,
            timeout: Duration,
        ) -> Result<IpcFrame, ClientError> {
            let deadline = Instant::now() + timeout;
            if let Some(error) = self
                .state
                .terminal
                .lock()
                .ok()
                .and_then(|value| value.clone())
            {
                return Err(error);
            }
            let mut request_id = self.next_request.fetch_add(1, Ordering::Relaxed);
            if request_id == 0 {
                request_id = self.next_request.fetch_add(1, Ordering::Relaxed);
            }
            let (sender, receiver) = mpsc::sync_channel(1);
            self.state
                .pending
                .lock()
                .map_err(|_| protocol("pending map poisoned"))?
                .insert(request_id, sender);
            let frame = IpcFrame::v1(kind, request_id, payload).map_err(frame_error)?;
            let write_result = (|| {
                let _guard = loop {
                    match self.writer_lock.try_lock() {
                        Ok(guard) => break guard,
                        Err(TryLockError::Poisoned(_)) => {
                            return Err(protocol("writer lock poisoned"));
                        }
                        Err(TryLockError::WouldBlock) => {
                            if Instant::now() >= deadline {
                                return Err(ClientError::Timeout);
                            }
                            thread::sleep(Duration::from_millis(1));
                        }
                    }
                };
                let mut writer = PipeWriter {
                    handle: &self.handle,
                    deadline: Some(deadline),
                };
                write_ipc_frame(&mut writer, &frame).map_err(frame_error)
            })();
            if let Err(error) = write_result {
                self.state
                    .pending
                    .lock()
                    .ok()
                    .and_then(|mut map| map.remove(&request_id));
                // A timed-out/partial frame cannot be resumed safely.
                self.abort();
                return Err(error);
            }
            let response =
                match receiver.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
                    Ok(response) => response?,
                    Err(error) => {
                        self.state
                            .pending
                            .lock()
                            .ok()
                            .and_then(|mut map| map.remove(&request_id));
                        let error = match error {
                            mpsc::RecvTimeoutError::Timeout => ClientError::Timeout,
                            mpsc::RecvTimeoutError::Disconnected => {
                                ClientError::Disconnected("response channel closed".into())
                            }
                        };
                        self.abort();
                        return Err(error);
                    }
                };
            if response.kind == IpcMessageKind::Error {
                let error: IpcErrorResponse =
                    decode_ipc_json(&response.payload).map_err(frame_error)?;
                return Err(ClientError::Server {
                    code: error.code,
                    message: error.message,
                });
            }
            Ok(response)
        }

        fn control(&self, request: &ControlRequest) -> Result<ControlResponse, ClientError> {
            let payload = encode_ipc_json(request).map_err(frame_error)?;
            let timeout = match request {
                ControlRequest::PrepareController { .. } => PREPARE_TIMEOUT,
                ControlRequest::ReleaseSession { .. } => RELEASE_TIMEOUT,
                _ => REQUEST_TIMEOUT,
            };
            let response =
                self.request_with_timeout(IpcMessageKind::ControlRequest, payload, timeout)?;
            if response.kind != IpcMessageKind::ControlResponse {
                return Err(protocol("expected ControlResponse"));
            }
            decode_ipc_json(&response.payload).map_err(frame_error)
        }

        fn abort(&self) {
            self.handle.close_once();
        }
    }

    impl Drop for Connection {
        fn drop(&mut self) {
            self.handle.close_once();
            if let Ok(mut reader) = self.reader.lock() {
                if let Some(reader) = reader.take() {
                    let _ = reader.join();
                }
            }
        }
    }

    pub(super) struct Client {
        connection: Arc<Connection>,
        hello: ServerHello,
    }

    impl Client {
        pub(super) fn connect(
            client_name: String,
            client_version: Option<String>,
        ) -> Result<Self, ClientError> {
            let handle = open_pipe()?;
            let handle = Arc::new(HandleState {
                handle,
                lifecycle: Mutex::new(HandleLifecycle {
                    closing: false,
                    closed: false,
                    active: 0,
                }),
                completed: Condvar::new(),
            });
            // One budget for the entire ClientHello/ServerHello exchange,
            // including partial frame reads. A silent server cannot renew it.
            let deadline = Instant::now() + HANDSHAKE_TIMEOUT;
            let hello = ClientHello {
                protocol_version: IPC_PROTOCOL_VERSION,
                client_name,
                client_version,
            };
            let frame = IpcFrame::v1(
                IpcMessageKind::ClientHello,
                1,
                encode_ipc_json(&hello).map_err(frame_error)?,
            )
            .map_err(frame_error)?;
            {
                let mut writer = PipeWriter {
                    handle: &handle,
                    deadline: Some(deadline),
                };
                write_ipc_frame(&mut writer, &frame).map_err(frame_error)?;
            }
            let response = {
                let mut reader = PipeReader {
                    handle: &handle,
                    deadline: Some(deadline),
                };
                read_ipc_frame(&mut reader)
                    .map_err(frame_error)?
                    .ok_or_else(|| {
                        ClientError::Disconnected("server closed during handshake".into())
                    })?
            };
            if response.kind == IpcMessageKind::Error {
                let error: IpcErrorResponse =
                    decode_ipc_json(&response.payload).map_err(frame_error)?;
                handle.close_once();
                return Err(ClientError::Server {
                    code: error.code,
                    message: error.message,
                });
            }
            if response.kind != IpcMessageKind::ServerHello || response.request_id != 1 {
                handle.close_once();
                return Err(protocol("invalid ServerHello"));
            }
            let server_hello: ServerHello =
                decode_ipc_json(&response.payload).map_err(frame_error)?;
            if server_hello.protocol_version != IPC_PROTOCOL_VERSION {
                handle.close_once();
                return Err(protocol("protocol version mismatch"));
            }

            let state = Arc::new(ConnectionState {
                pending: Mutex::new(HashMap::new()),
                stream: Mutex::new(StreamState::default()),
                stream_changed: Condvar::new(),
                terminal: Mutex::new(None),
            });
            let connection = Arc::new(Connection {
                handle: Arc::clone(&handle),
                state: Arc::clone(&state),
                writer_lock: Mutex::new(()),
                next_request: AtomicU32::new(2),
                active_session: Mutex::new(None),
                reader: Mutex::new(None),
            });
            let reader_handle = Arc::clone(&handle);
            let reader = thread::Builder::new()
                .name("directhci-client-rx".into())
                .spawn(move || reader_loop(reader_handle, state))
                .map_err(|error| ClientError::Connection(error.to_string()))?;
            *connection
                .reader
                .lock()
                .map_err(|_| protocol("reader lock poisoned"))? = Some(reader);
            Ok(Self {
                connection,
                hello: server_hello,
            })
        }

        pub(super) fn server_hello(&self) -> &ServerHello {
            &self.hello
        }
        pub(super) fn list_controllers(&self) -> Result<Vec<ControllerObservation>, ClientError> {
            match self.connection.control(&ControlRequest::ListControllers)? {
                ControlResponse::Controllers { controllers } => Ok(controllers),
                _ => Err(protocol("unexpected list_controllers response")),
            }
        }
        pub(super) fn runtime_status(&self) -> Result<RuntimeStatus, ClientError> {
            match self.connection.control(&ControlRequest::RuntimeStatus)? {
                ControlResponse::RuntimeStatus { status } => Ok(status),
                _ => Err(protocol("unexpected runtime_status response")),
            }
        }
        pub(super) fn preferences(&self) -> Result<RuntimePreferences, ClientError> {
            match self.connection.control(&ControlRequest::GetPreferences)? {
                ControlResponse::Preferences { preferences } => Ok(preferences),
                _ => Err(protocol("unexpected preferences response")),
            }
        }
        pub(super) fn set_preferred_controller(
            &self,
            controller_id: &str,
        ) -> Result<RuntimePreferences, ClientError> {
            match self
                .connection
                .control(&ControlRequest::SetPreferredController {
                    controller_id: controller_id.into(),
                })? {
                ControlResponse::Preferences { preferences } => Ok(preferences),
                _ => Err(protocol("unexpected preferences response")),
            }
        }
        pub(super) fn controller_preparation_status(
            &self,
            controller_id: &str,
        ) -> Result<directhci_core::ControllerPreparationStatus, ClientError> {
            match self
                .connection
                .control(&ControlRequest::ControllerPreparationStatus {
                    controller_id: controller_id.into(),
                })? {
                ControlResponse::ControllerPreparationStatus { status } => Ok(status),
                _ => Err(protocol(
                    "unexpected controller preparation status response",
                )),
            }
        }
        pub(super) fn prepare_controller(
            &self,
            controller_id: &str,
            trust_acknowledged: bool,
        ) -> Result<directhci_core::PreparedController, ClientError> {
            match self
                .connection
                .control(&ControlRequest::PrepareController {
                    controller_id: controller_id.into(),
                    trust_acknowledged,
                })? {
                ControlResponse::ControllerPrepared { preparation } => Ok(preparation),
                _ => Err(protocol("unexpected prepare controller response")),
            }
        }
        pub(super) fn restore_windows(&self) -> Result<(), ClientError> {
            match self.connection.control(&ControlRequest::RestoreWindows)? {
                ControlResponse::Accepted => Ok(()),
                _ => Err(protocol("unexpected restore response")),
            }
        }
        pub(super) fn acquire_raw_hci(&self, controller_id: &str) -> Result<Session, ClientError> {
            if self
                .connection
                .active_session
                .lock()
                .map_err(|_| protocol("session lock poisoned"))?
                .is_some()
            {
                return Err(ClientError::InvalidSession(
                    "this connection already owns a session".into(),
                ));
            }
            match self.connection.control(&ControlRequest::AcquireRawHci {
                controller_id: controller_id.into(),
            })? {
                ControlResponse::SessionReady {
                    session_id,
                    controller,
                    transport,
                } => {
                    *self
                        .connection
                        .active_session
                        .lock()
                        .map_err(|_| protocol("session lock poisoned"))? = Some(session_id);
                    Ok(Session {
                        connection: Arc::clone(&self.connection),
                        session_id,
                        controller,
                        transport,
                        released: false,
                    })
                }
                _ => Err(protocol("unexpected acquire response")),
            }
        }
    }

    pub(super) struct Session {
        connection: Arc<Connection>,
        session_id: u64,
        controller: ControllerObservation,
        transport: IpcTransportInfo,
        released: bool,
    }
    impl Session {
        pub(super) fn session_id(&self) -> u64 {
            self.session_id
        }
        pub(super) fn controller(&self) -> &ControllerObservation {
            &self.controller
        }
        pub(super) fn transport(&self) -> &IpcTransportInfo {
            &self.transport
        }
        pub(super) fn send_command(
            &self,
            opcode: u16,
            parameters: &[u8],
        ) -> Result<HciCommandResponse, ClientError> {
            self.ensure_active()?;
            let response = self.connection.request_with_timeout(
                IpcMessageKind::HciCommand,
                encode_hci_command(self.session_id, opcode, parameters).map_err(frame_error)?,
                HCI_REQUEST_TIMEOUT,
            )?;
            if response.kind != IpcMessageKind::HciCommandResponse {
                return Err(protocol("expected HciCommandResponse"));
            }
            let (session_id, response) =
                decode_hci_command_response(&response.payload).map_err(frame_error)?;
            if session_id != self.session_id {
                return Err(ClientError::InvalidSession(
                    "command response session mismatch".into(),
                ));
            }
            Ok(response)
        }
        pub(super) fn send_acl(&self, packet: &HciAclPacket) -> Result<(), ClientError> {
            self.ensure_active()?;
            let response = self.connection.request_with_timeout(
                IpcMessageKind::AclTx,
                encode_acl_packet(self.session_id, packet).map_err(frame_error)?,
                HCI_REQUEST_TIMEOUT,
            )?;
            if response.kind != IpcMessageKind::ControlResponse {
                return Err(protocol("expected ACL acknowledgement"));
            }
            match decode_ipc_json(&response.payload).map_err(frame_error)? {
                ControlResponse::Accepted => Ok(()),
                _ => Err(protocol("unexpected ACL acknowledgement")),
            }
        }
        pub(super) fn receive_event(
            &self,
            timeout: Duration,
        ) -> Result<Option<HciEventPacket>, ClientError> {
            self.ensure_active()?;
            self.connection
                .state
                .receive(self.session_id, timeout, Some(true))
                .map(|value| {
                    value.map(|packet| match packet {
                        HciIncomingPacket::Event(event) => event,
                        _ => unreachable!("typed event receive"),
                    })
                })
        }
        pub(super) fn receive_acl(
            &self,
            timeout: Duration,
        ) -> Result<Option<HciAclPacket>, ClientError> {
            self.ensure_active()?;
            self.connection
                .state
                .receive(self.session_id, timeout, Some(false))
                .map(|value| {
                    value.map(|packet| match packet {
                        HciIncomingPacket::Acl(acl) => acl,
                        _ => unreachable!("typed ACL receive"),
                    })
                })
        }
        pub(super) fn receive_packet(
            &self,
            timeout: Duration,
        ) -> Result<Option<HciIncomingPacket>, ClientError> {
            self.ensure_active()?;
            self.connection
                .state
                .receive(self.session_id, timeout, None)
        }
        pub(super) fn release(&mut self) -> Result<Option<ControllerObservation>, ClientError> {
            if self.released {
                return Ok(None);
            }
            // Only attempt release once, including on failure. In particular,
            // Drop must not silently start another full release timeout.
            self.released = true;
            let result = self.connection.control(&ControlRequest::ReleaseSession {
                session_id: self.session_id,
            });
            match result {
                Ok(ControlResponse::Released {
                    session_id,
                    controller,
                }) if session_id == self.session_id => {
                    // The release response follows every event from this worker
                    // on the same pipe. The reader has already queued any old
                    // stream frames before delivering this response.
                    {
                        let mut stream = self
                            .connection
                            .state
                            .stream
                            .lock()
                            .map_err(|_| protocol("stream lock poisoned"))?;
                        stream.packets.clear();
                        stream.bytes = 0;
                    }
                    *self
                        .connection
                        .active_session
                        .lock()
                        .map_err(|_| protocol("session lock poisoned"))? = None;
                    Ok(controller)
                }
                Ok(_) => {
                    self.connection.abort();
                    Err(protocol("unexpected release response"))
                }
                Err(error) => {
                    self.connection.abort();
                    Err(error)
                }
            }
        }
        fn ensure_active(&self) -> Result<(), ClientError> {
            if self.released {
                Err(ClientError::InvalidSession("session was released".into()))
            } else {
                Ok(())
            }
        }
    }
    impl Drop for Session {
        fn drop(&mut self) {
            if !self.released {
                let _ = self.release();
            }
        }
    }

    fn reader_loop(handle: Arc<HandleState>, state: Arc<ConnectionState>) {
        let terminal = loop {
            let frame = match read_ipc_frame(&mut PipeReader {
                handle: &handle,
                deadline: None,
            }) {
                Ok(Some(frame)) => frame,
                Ok(None) => break ClientError::Disconnected("pipe closed".into()),
                Err(error) => break frame_error(error),
            };
            match frame.kind {
                IpcMessageKind::HciEvent => match decode_hci_event(&frame.payload) {
                    Ok((session, packet)) => {
                        if let Err(error) = state.enqueue(session, HciIncomingPacket::Event(packet))
                        {
                            break error;
                        }
                    }
                    Err(error) => break frame_error(error),
                },
                IpcMessageKind::AclRx => match decode_acl_packet(&frame.payload) {
                    Ok((session, packet)) => {
                        if let Err(error) = state.enqueue(session, HciIncomingPacket::Acl(packet)) {
                            break error;
                        }
                    }
                    Err(error) => break frame_error(error),
                },
                _ if frame.request_id != 0 => {
                    if let Ok(mut pending) = state.pending.lock() {
                        if let Some(sender) = pending.remove(&frame.request_id) {
                            let _ = sender.send(Ok(frame));
                        }
                    }
                }
                _ => break protocol("unsolicited control frame"),
            }
        };
        if let Ok(mut value) = state.terminal.lock() {
            *value = Some(terminal.clone());
        }
        if let Ok(mut pending) = state.pending.lock() {
            for (_, sender) in pending.drain() {
                let _ = sender.send(Err(terminal.clone()));
            }
        }
        if let Ok(mut stream) = state.stream.lock() {
            stream.terminal = Some(terminal);
        }
        state.stream_changed.notify_all();
        handle.close_once();
    }

    struct PipeReader<'a> {
        handle: &'a HandleState,
        deadline: Option<Instant>,
    }
    struct PipeWriter<'a> {
        handle: &'a HandleState,
        deadline: Option<Instant>,
    }
    impl Read for PipeReader<'_> {
        fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
            pipe_read(self.handle, buffer, self.deadline)
        }
    }
    impl Write for PipeWriter<'_> {
        fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
            pipe_write(self.handle, buffer, self.deadline)
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    fn pipe_read(
        handle: &HandleState,
        buffer: &mut [u8],
        deadline: Option<Instant>,
    ) -> std::io::Result<usize> {
        overlapped_io(handle, deadline, |overlapped, transferred| unsafe {
            ReadFile(
                handle.handle,
                Some(buffer),
                Some(transferred),
                Some(overlapped),
            )
        })
    }
    fn pipe_write(
        handle: &HandleState,
        buffer: &[u8],
        deadline: Option<Instant>,
    ) -> std::io::Result<usize> {
        overlapped_io(handle, deadline, |overlapped, transferred| unsafe {
            WriteFile(
                handle.handle,
                Some(buffer),
                Some(transferred),
                Some(overlapped),
            )
        })
    }
    fn overlapped_io(
        handle: &HandleState,
        deadline: Option<Instant>,
        operation: impl FnOnce(*mut OVERLAPPED, *mut u32) -> windows::core::Result<()>,
    ) -> std::io::Result<usize> {
        // Hold the gate through submission only. close_once first blocks new
        // submissions, then cancels and waits for every active completion.
        let mut lifecycle = handle
            .lifecycle
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if lifecycle.closing {
            return Err(std::io::Error::new(
                std::io::ErrorKind::BrokenPipe,
                "pipe closed",
            ));
        }
        if deadline.is_some_and(|deadline| Instant::now() >= deadline) {
            return Err(std::io::ErrorKind::TimedOut.into());
        }
        // SAFETY: event, OVERLAPPED and caller buffer live until completion.
        let event = unsafe { CreateEventW(None, true, false, PCWSTR::null()) }.map_err(win_io)?;
        let mut overlapped = OVERLAPPED {
            hEvent: event,
            ..Default::default()
        };
        let mut transferred = 0u32;
        lifecycle.active += 1;
        let initial = operation(&mut overlapped, &mut transferred);
        drop(lifecycle);
        let result = match initial {
            Ok(()) => Ok(()),
            Err(error) if error.code() == HRESULT::from_win32(ERROR_IO_PENDING.0) => {
                wait_for_io(handle.handle, &overlapped, &mut transferred, deadline)
            }
            Err(error) => Err(win_io(error)),
        };
        let _ = unsafe { CloseHandle(event) };
        let mut lifecycle = handle
            .lifecycle
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        lifecycle.active -= 1;
        if lifecycle.active == 0 {
            handle.completed.notify_all();
        }
        drop(lifecycle);
        result?;
        Ok(transferred as usize)
    }

    fn wait_for_io(
        handle: HANDLE,
        overlapped: &OVERLAPPED,
        transferred: &mut u32,
        deadline: Option<Instant>,
    ) -> std::io::Result<()> {
        if let Some(deadline) = deadline {
            let remaining = deadline.saturating_duration_since(Instant::now());
            let wait_ms = remaining.as_millis().min((u32::MAX - 1) as u128) as u32;
            let waited = unsafe { WaitForSingleObject(overlapped.hEvent, wait_ms) };
            if waited != WAIT_OBJECT_0 {
                let error = if waited == WAIT_TIMEOUT {
                    std::io::ErrorKind::TimedOut.into()
                } else {
                    std::io::Error::last_os_error()
                };
                // The deadline ends waiting for a server reply, not the
                // kernel's ownership of OVERLAPPED/event/buffer storage.
                let _ = unsafe { CancelIoEx(handle, Some(overlapped)) };
                let _ = unsafe { GetOverlappedResult(handle, overlapped, transferred, true) };
                return Err(error);
            }
        }
        unsafe { GetOverlappedResult(handle, overlapped, transferred, true) }.map_err(win_io)
    }

    fn open_pipe() -> Result<HANDLE, ClientError> {
        let wide: Vec<u16> = IPC_PIPE_NAME
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();
        // SAFETY: stable NUL-terminated pipe name; no state is modified beyond opening the local pipe.
        let _ = unsafe { WaitNamedPipeW(PCWSTR(wide.as_ptr()), 5000) };
        unsafe {
            CreateFileW(
                PCWSTR(wide.as_ptr()),
                // Match the pipe's narrow AU ACE; GENERIC_WRITE also grants
                // FILE_CREATE_PIPE_INSTANCE and must not be requested here.
                (FILE_READ_DATA
                    | FILE_WRITE_DATA
                    | FILE_READ_ATTRIBUTES
                    | FILE_WRITE_ATTRIBUTES
                    | READ_CONTROL
                    | SYNCHRONIZE)
                    .0,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                None,
                OPEN_EXISTING,
                FILE_FLAG_OVERLAPPED | SECURITY_SQOS_PRESENT | SECURITY_IMPERSONATION,
                None,
            )
        }
        .map_err(|error| ClientError::Connection(error.to_string()))
    }
    fn win_io(error: windows::core::Error) -> std::io::Error {
        std::io::Error::other(error.to_string())
    }
    fn frame_error(error: IpcFrameError) -> ClientError {
        match error {
            IpcFrameError::Io(error) if error.kind() == std::io::ErrorKind::TimedOut => {
                ClientError::Timeout
            }
            error => ClientError::Protocol(error.to_string()),
        }
    }
    fn protocol(message: &str) -> ClientError {
        ClientError::Protocol(message.into())
    }
}
