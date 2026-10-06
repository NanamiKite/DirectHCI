//! WinUSB-backed Raw HCI session.
//!
//! The session is the sole owner of both receive pipes. It starts persistent
//! Event and ACL reads before exposing command transmission and joins those
//! workers before releasing the WinUSB/file handles.

use std::sync::Arc;
use std::time::Duration;

use directhci_core::{
    HciAclPacket, HciCommandPacket, HciCommandResponse, HciEventPacket, HciIncomingPacket,
};
use serde::Serialize;

use crate::{DedicatedWinUsbControllerReadiness, DedicatedWinUsbReadinessStatus};

pub const DEFAULT_COMMAND_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HciTraceDirection {
    Tx,
    Rx,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HciTracePacketType {
    Command,
    Event,
    Acl,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct HciTraceRecord {
    pub unix_time_micros: u128,
    pub direction: HciTraceDirection,
    pub packet_type: HciTracePacketType,
    pub length: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub raw: Option<Vec<u8>>,
}

pub type HciTraceCallback = Arc<dyn Fn(HciTraceRecord) + Send + Sync + 'static>;

#[derive(Clone, Default)]
pub struct RawHciSessionOptions {
    pub command_timeout: Option<Duration>,
    pub trace: Option<HciTraceCallback>,
    pub trace_raw_bytes: bool,
}

impl RawHciSessionOptions {
    fn effective_command_timeout(&self) -> Duration {
        self.command_timeout.unwrap_or(DEFAULT_COMMAND_TIMEOUT)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct RawHciTransportSummary {
    pub application_interface: String,
    pub interface_number: u8,
    pub alternate_setting: u8,
    pub event_pipe: u8,
    pub acl_in_pipe: u8,
    pub acl_out_pipe: u8,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct RawHciShutdownReport {
    pub event_rx_joined: bool,
    pub acl_rx_joined: bool,
    pub handles_released: bool,
    pub cancellation_errors: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RawHciError {
    UnsupportedPlatform,
    NotReady {
        status: String,
    },
    InvalidReadiness {
        message: String,
    },
    Open {
        message: String,
    },
    WinUsbInitialize {
        message: String,
    },
    UsbControlTransfer {
        message: String,
    },
    EventRead {
        message: String,
    },
    AclRead {
        message: String,
    },
    AclWrite {
        message: String,
    },
    MalformedHciPacket {
        packet_type: String,
        message: String,
    },
    CommandTimeout {
        opcode: u16,
        timeout_ms: u64,
    },
    CommandRejected {
        opcode: u16,
        status: u8,
    },
    UnexpectedCommandOpcode {
        expected: u16,
        actual: u16,
    },
    DeviceRemoved {
        operation: String,
        message: String,
    },
    Shutdown,
    Worker {
        worker: String,
        message: String,
    },
    Internal {
        message: String,
    },
}

impl std::fmt::Display for RawHciError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnsupportedPlatform => {
                formatter.write_str("Raw HCI WinUSB is only available on Windows")
            }
            Self::NotReady { status } => {
                write!(formatter, "WinUSB controller is not ready ({status})")
            }
            Self::InvalidReadiness { message } => {
                write!(formatter, "invalid WinUSB readiness: {message}")
            }
            Self::Open { message } => {
                write!(formatter, "open WinUSB application interface: {message}")
            }
            Self::WinUsbInitialize { message } => write!(formatter, "WinUsb_Initialize: {message}"),
            Self::UsbControlTransfer { message } => {
                write!(formatter, "HCI command control transfer: {message}")
            }
            Self::EventRead { message } => write!(formatter, "HCI Event read: {message}"),
            Self::AclRead { message } => write!(formatter, "HCI ACL read: {message}"),
            Self::AclWrite { message } => write!(formatter, "HCI ACL write: {message}"),
            Self::MalformedHciPacket {
                packet_type,
                message,
            } => write!(formatter, "malformed HCI {packet_type}: {message}"),
            Self::CommandTimeout { opcode, timeout_ms } => write!(
                formatter,
                "HCI command {opcode:#06x} timed out after {timeout_ms} ms"
            ),
            Self::CommandRejected { opcode, status } => write!(
                formatter,
                "HCI command {opcode:#06x} rejected with status {status:#04x}"
            ),
            Self::UnexpectedCommandOpcode { expected, actual } => write!(
                formatter,
                "expected response for {expected:#06x}, received {actual:#06x}"
            ),
            Self::DeviceRemoved { operation, message } => {
                write!(formatter, "device removed during {operation}: {message}")
            }
            Self::Shutdown => formatter.write_str("Raw HCI session is shutting down"),
            Self::Worker { worker, message } => write!(formatter, "{worker} worker: {message}"),
            Self::Internal { message } => formatter.write_str(message),
        }
    }
}

impl std::error::Error for RawHciError {}

pub struct RawHciSession {
    inner: platform::Session,
}

impl RawHciSession {
    pub fn open(
        readiness: &DedicatedWinUsbControllerReadiness,
        options: RawHciSessionOptions,
    ) -> Result<Self, RawHciError> {
        Ok(Self {
            inner: platform::Session::open(readiness, options)?,
        })
    }

    pub fn transport(&self) -> &RawHciTransportSummary {
        self.inner.transport()
    }

    pub fn send_command(
        &self,
        opcode: u16,
        parameters: &[u8],
    ) -> Result<HciCommandResponse, RawHciError> {
        self.inner.send_command(opcode, parameters)
    }

    pub fn send_acl(&self, packet: &HciAclPacket) -> Result<(), RawHciError> {
        self.inner.send_acl(packet)
    }

    pub fn receive_event(&self, timeout: Duration) -> Result<Option<HciEventPacket>, RawHciError> {
        self.inner.receive_event(timeout)
    }

    pub fn receive_acl(&self, timeout: Duration) -> Result<Option<HciAclPacket>, RawHciError> {
        self.inner.receive_acl(timeout)
    }
    pub fn receive_packet(
        &self,
        timeout: Duration,
    ) -> Result<Option<HciIncomingPacket>, RawHciError> {
        self.inner.receive_packet(timeout)
    }

    pub fn terminal_error(&self) -> Option<RawHciError> {
        self.inner.terminal_error()
    }

    pub fn shutdown(&mut self) -> RawHciShutdownReport {
        self.inner.shutdown()
    }
}

#[cfg(not(windows))]
mod platform {
    use super::*;

    pub(super) struct Session {
        transport: RawHciTransportSummary,
    }

    impl Session {
        pub(super) fn open(
            _: &DedicatedWinUsbControllerReadiness,
            _: RawHciSessionOptions,
        ) -> Result<Self, RawHciError> {
            Err(RawHciError::UnsupportedPlatform)
        }
        pub(super) fn transport(&self) -> &RawHciTransportSummary {
            &self.transport
        }
        pub(super) fn send_command(
            &self,
            _: u16,
            _: &[u8],
        ) -> Result<HciCommandResponse, RawHciError> {
            Err(RawHciError::UnsupportedPlatform)
        }
        pub(super) fn send_acl(&self, _: &HciAclPacket) -> Result<(), RawHciError> {
            Err(RawHciError::UnsupportedPlatform)
        }
        pub(super) fn receive_event(
            &self,
            _: Duration,
        ) -> Result<Option<HciEventPacket>, RawHciError> {
            Err(RawHciError::UnsupportedPlatform)
        }
        pub(super) fn receive_acl(&self, _: Duration) -> Result<Option<HciAclPacket>, RawHciError> {
            Err(RawHciError::UnsupportedPlatform)
        }
        pub(super) fn receive_packet(
            &self,
            _: Duration,
        ) -> Result<Option<HciIncomingPacket>, RawHciError> {
            Err(RawHciError::UnsupportedPlatform)
        }
        pub(super) fn terminal_error(&self) -> Option<RawHciError> {
            Some(RawHciError::UnsupportedPlatform)
        }
        pub(super) fn shutdown(&mut self) -> RawHciShutdownReport {
            RawHciShutdownReport {
                event_rx_joined: true,
                acl_rx_joined: true,
                handles_released: true,
                cancellation_errors: Vec::new(),
            }
        }
    }
}

#[cfg(windows)]
mod platform {
    use std::collections::VecDeque;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Condvar, Mutex, mpsc};
    use std::thread::{self, JoinHandle};
    use std::time::{SystemTime, UNIX_EPOCH};

    use directhci_core::HciPacketError;
    use windows::Win32::Devices::Usb::{
        WINUSB_INTERFACE_HANDLE, WINUSB_SETUP_PACKET, WinUsb_AbortPipe, WinUsb_ControlTransfer,
        WinUsb_Free, WinUsb_GetOverlappedResult, WinUsb_Initialize, WinUsb_ReadPipe,
        WinUsb_WritePipe,
    };
    use windows::Win32::Foundation::{
        CloseHandle, ERROR_DEVICE_NOT_CONNECTED, ERROR_IO_PENDING, ERROR_NO_SUCH_DEVICE,
        ERROR_NOT_FOUND, ERROR_OPERATION_ABORTED, GENERIC_READ, GENERIC_WRITE, HANDLE,
    };
    use windows::Win32::Storage::FileSystem::{
        CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_FLAG_OVERLAPPED, FILE_SHARE_READ,
        FILE_SHARE_WRITE, OPEN_EXISTING,
    };
    use windows::Win32::System::IO::{CancelIoEx, OVERLAPPED};
    use windows::Win32::System::Threading::CreateEventW;
    use windows::core::{HRESULT, PCWSTR};

    use super::*;

    const EVENT_BUFFER_SIZE: usize = 2 + u8::MAX as usize;
    const ACL_BUFFER_SIZE: usize = 4 + u16::MAX as usize;
    const RX_PACKET_LIMIT: usize = 4096;
    const RX_BYTE_LIMIT: usize = 16 * 1024 * 1024;

    #[derive(Default)]
    struct ReceiveState {
        packets: VecDeque<HciIncomingPacket>,
        bytes: usize,
    }

    struct PendingCommand {
        opcode: u16,
        sender: mpsc::SyncSender<Result<HciCommandResponse, RawHciError>>,
    }

    struct SharedState {
        shutdown: AtomicBool,
        accepting_tx: AtomicBool,
        terminal: Mutex<Option<RawHciError>>,
        pending_command: Mutex<Option<PendingCommand>>,
        receive: Mutex<ReceiveState>,
        receive_changed: Condvar,
    }

    impl SharedState {
        fn new() -> Self {
            Self {
                shutdown: AtomicBool::new(false),
                accepting_tx: AtomicBool::new(true),
                terminal: Mutex::new(None),
                pending_command: Mutex::new(None),
                receive: Mutex::new(ReceiveState::default()),
                receive_changed: Condvar::new(),
            }
        }

        fn terminal_error(&self) -> Option<RawHciError> {
            self.terminal.lock().ok().and_then(|error| error.clone())
        }

        fn fail(&self, error: RawHciError) {
            if let Ok(mut terminal) = self.terminal.lock() {
                if terminal.is_none() {
                    *terminal = Some(error.clone());
                }
            }
            self.accepting_tx.store(false, Ordering::Release);
            self.shutdown.store(true, Ordering::Release);
            self.fail_pending(error);
            self.wake_receivers();
        }

        fn fail_pending(&self, error: RawHciError) {
            if let Ok(mut pending) = self.pending_command.lock() {
                if let Some(pending) = pending.take() {
                    let _ = pending.sender.send(Err(error));
                }
            }
        }

        fn wake_receivers(&self) {
            // Synchronize the terminal predicate with wait, avoiding a lost wake.
            let _guard = self.receive.lock().unwrap_or_else(|e| e.into_inner());
            self.receive_changed.notify_all();
        }

        fn enqueue(&self, packet: HciIncomingPacket) -> bool {
            let error = match self.receive.lock() {
                Ok(mut rx)
                    if rx.packets.len() < RX_PACKET_LIMIT
                        && rx.bytes + packet.encoded_len() <= RX_BYTE_LIMIT =>
                {
                    rx.bytes += packet.encoded_len();
                    rx.packets.push_back(packet);
                    self.receive_changed.notify_all();
                    return true;
                }
                Ok(_) => internal("Raw HCI receive queue hard limit exceeded"),
                Err(_) => internal("Raw HCI receive queue poisoned"),
            };
            self.fail(error);
            false
        }

        fn receive_packet(
            &self,
            timeout: Duration,
            kind: Option<bool>,
        ) -> Result<Option<HciIncomingPacket>, RawHciError> {
            let deadline = std::time::Instant::now() + timeout;
            let mut rx = self
                .receive
                .lock()
                .map_err(|_| internal("Raw HCI receive queue poisoned"))?;
            loop {
                if let Some(index) = rx.packets.iter().position(|packet| {
                    kind.is_none_or(|event| event == matches!(packet, HciIncomingPacket::Event(_)))
                }) {
                    let packet = rx.packets.remove(index).expect("position exists");
                    rx.bytes -= packet.encoded_len();
                    return Ok(Some(packet));
                }
                if let Some(error) = self.terminal_error() {
                    return Err(error);
                }
                if self.shutdown.load(Ordering::Acquire) {
                    return Err(RawHciError::Shutdown);
                }
                let remaining = deadline.saturating_duration_since(std::time::Instant::now());
                if remaining.is_zero() {
                    return Ok(None);
                }
                rx = self
                    .receive_changed
                    .wait_timeout(rx, remaining)
                    .map_err(|_| internal("Raw HCI receive queue poisoned"))?
                    .0;
            }
        }
    }

    struct DeviceIo {
        interface: WINUSB_INTERFACE_HANDLE,
        file: HANDLE,
        // Serialize new submissions with shutdown cancellation, not completion.
        submission: Mutex<()>,
    }

    // SAFETY: WinUSB/file handles can service concurrent overlapped operations.
    // Every operation owns its OVERLAPPED, event, and buffer until completion;
    // DeviceIo remains in Arc and is dropped only after all workers are joined.
    unsafe impl Send for DeviceIo {}
    // SAFETY: The same invariants as Send apply; no mutable Rust references to
    // the handle values are shared and WinUsb_Free/CloseHandle happen last.
    unsafe impl Sync for DeviceIo {}

    impl DeviceIo {
        fn open(path: &str) -> Result<Self, RawHciError> {
            let wide: Vec<u16> = path.encode_utf16().chain(std::iter::once(0)).collect();
            // SAFETY: `wide` is NUL-terminated and lives through the call; the
            // returned owned HANDLE is closed on every error/drop path.
            let file = unsafe {
                CreateFileW(
                    PCWSTR(wide.as_ptr()),
                    (GENERIC_READ | GENERIC_WRITE).0,
                    FILE_SHARE_READ | FILE_SHARE_WRITE,
                    None,
                    OPEN_EXISTING,
                    FILE_ATTRIBUTE_NORMAL | FILE_FLAG_OVERLAPPED,
                    None,
                )
            }
            .map_err(|error| RawHciError::Open {
                message: error.to_string(),
            })?;
            let mut interface = WINUSB_INTERFACE_HANDLE::default();
            // SAFETY: `file` is a valid overlapped device handle and the output
            // pointer is valid for the duration of the call.
            if let Err(error) = unsafe { WinUsb_Initialize(file, &mut interface) } {
                let _ = unsafe { CloseHandle(file) };
                return Err(RawHciError::WinUsbInitialize {
                    message: error.to_string(),
                });
            }
            Ok(Self {
                interface,
                file,
                submission: Mutex::new(()),
            })
        }

        fn abort(&self, pipes: [u8; 3]) -> Vec<String> {
            // No new I/O can be submitted after cancellation has inspected the
            // file handle; each operation releases this gate before waiting.
            let _submission = self
                .submission
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let mut errors = Vec::new();
            for pipe in pipes {
                // SAFETY: the interface stays alive and each pipe ID came from
                // its descriptor-driven readiness result.
                if let Err(error) = unsafe { WinUsb_AbortPipe(self.interface, pipe) } {
                    errors.push(format!("WinUsb_AbortPipe(0x{pipe:02X}): {error}"));
                }
            }
            if let Err(error) = unsafe { CancelIoEx(self.file, None) }
                && error.code() != HRESULT::from_win32(ERROR_NOT_FOUND.0)
            {
                errors.push(format!("CancelIoEx: {error}"));
            }
            errors
        }

        fn cancel_all(&self) {
            let _submission = self
                .submission
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            // SAFETY: the file handle remains valid through worker join; NULL
            // intentionally selects every pending operation on that handle.
            let _ = unsafe { CancelIoEx(self.file, None) };
        }
    }

    impl Drop for DeviceIo {
        fn drop(&mut self) {
            // SAFETY: the last Arc drops only after workers have exited, so no
            // OVERLAPPED operation can still reference these handles.
            let _ = unsafe { WinUsb_Free(self.interface) };
            let _ = unsafe { CloseHandle(self.file) };
        }
    }

    struct EventHandle(HANDLE);
    impl EventHandle {
        fn new() -> Result<Self, windows::core::Error> {
            // SAFETY: no attributes/name are supplied; EventHandle takes sole
            // ownership of the returned HANDLE.
            unsafe { CreateEventW(None, false, false, PCWSTR::null()) }.map(Self)
        }
    }
    impl Drop for EventHandle {
        fn drop(&mut self) {
            let _ = unsafe { CloseHandle(self.0) };
        }
    }

    pub(super) struct Session {
        transport: RawHciTransportSummary,
        options: RawHciSessionOptions,
        io: Option<Arc<DeviceIo>>,
        state: Arc<SharedState>,
        command_lock: Mutex<()>,
        acl_tx_lock: Mutex<()>,
        event_worker: Option<JoinHandle<()>>,
        acl_worker: Option<JoinHandle<()>>,
        shutdown_report: Option<RawHciShutdownReport>,
    }

    impl Session {
        pub(super) fn open(
            readiness: &DedicatedWinUsbControllerReadiness,
            options: RawHciSessionOptions,
        ) -> Result<Self, RawHciError> {
            let transport = transport_from_readiness(readiness)?;
            let io = Arc::new(DeviceIo::open(&transport.application_interface)?);
            let state = Arc::new(SharedState::new());

            let (event_started_tx, event_started_rx) = mpsc::sync_channel(1);
            let event_io = Arc::clone(&io);
            let event_state = Arc::clone(&state);
            let event_trace = options.trace.clone();
            let trace_raw = options.trace_raw_bytes;
            let event_pipe = transport.event_pipe;
            let event_worker = thread::Builder::new()
                .name("directhci-event-rx".into())
                .spawn(move || {
                    event_worker_loop(
                        event_io,
                        event_state,
                        event_pipe,
                        event_started_tx,
                        event_trace,
                        trace_raw,
                    )
                })
                .map_err(|error| RawHciError::Worker {
                    worker: "event_rx".into(),
                    message: error.to_string(),
                })?;

            match event_started_rx.recv() {
                Ok(Ok(())) => {}
                Ok(Err(error)) => {
                    state.shutdown.store(true, Ordering::Release);
                    let _ = io.abort([
                        transport.event_pipe,
                        transport.acl_in_pipe,
                        transport.acl_out_pipe,
                    ]);
                    let _ = event_worker.join();
                    return Err(error);
                }
                Err(error) => {
                    state.shutdown.store(true, Ordering::Release);
                    let _ = io.abort([
                        transport.event_pipe,
                        transport.acl_in_pipe,
                        transport.acl_out_pipe,
                    ]);
                    let _ = event_worker.join();
                    return Err(RawHciError::Worker {
                        worker: "event_rx".into(),
                        message: format!("startup channel closed: {error}"),
                    });
                }
            }

            let (acl_started_tx, acl_started_rx) = mpsc::sync_channel(1);
            let acl_io = Arc::clone(&io);
            let acl_state = Arc::clone(&state);
            let acl_trace = options.trace.clone();
            let acl_pipe = transport.acl_in_pipe;
            let acl_worker = match thread::Builder::new()
                .name("directhci-acl-rx".into())
                .spawn(move || {
                    acl_worker_loop(
                        acl_io,
                        acl_state,
                        acl_pipe,
                        acl_started_tx,
                        acl_trace,
                        trace_raw,
                    )
                }) {
                Ok(worker) => worker,
                Err(error) => {
                    state.shutdown.store(true, Ordering::Release);
                    let _ = io.abort([
                        transport.event_pipe,
                        transport.acl_in_pipe,
                        transport.acl_out_pipe,
                    ]);
                    let _ = event_worker.join();
                    return Err(RawHciError::Worker {
                        worker: "acl_rx".into(),
                        message: error.to_string(),
                    });
                }
            };
            match acl_started_rx.recv() {
                Ok(Ok(())) => {}
                Ok(Err(error)) => {
                    state.shutdown.store(true, Ordering::Release);
                    let _ = io.abort([
                        transport.event_pipe,
                        transport.acl_in_pipe,
                        transport.acl_out_pipe,
                    ]);
                    let _ = event_worker.join();
                    let _ = acl_worker.join();
                    return Err(error);
                }
                Err(error) => {
                    state.shutdown.store(true, Ordering::Release);
                    let _ = io.abort([
                        transport.event_pipe,
                        transport.acl_in_pipe,
                        transport.acl_out_pipe,
                    ]);
                    let _ = event_worker.join();
                    let _ = acl_worker.join();
                    return Err(RawHciError::Worker {
                        worker: "acl_rx".into(),
                        message: format!("startup channel closed: {error}"),
                    });
                }
            }

            Ok(Self {
                transport,
                options,
                io: Some(io),
                state,
                command_lock: Mutex::new(()),
                acl_tx_lock: Mutex::new(()),
                event_worker: Some(event_worker),
                acl_worker: Some(acl_worker),
                shutdown_report: None,
            })
        }

        pub(super) fn transport(&self) -> &RawHciTransportSummary {
            &self.transport
        }

        pub(super) fn send_command(
            &self,
            opcode: u16,
            parameters: &[u8],
        ) -> Result<HciCommandResponse, RawHciError> {
            self.ensure_operational()?;
            let _guard = self
                .command_lock
                .lock()
                .map_err(|_| internal("command lock poisoned"))?;
            self.ensure_operational()?;
            let mut bytes = HciCommandPacket::new(opcode, parameters)
                .encode()
                .map_err(packet_error)?;
            let (sender, receiver) = mpsc::sync_channel(1);
            {
                let mut pending = self
                    .state
                    .pending_command
                    .lock()
                    .map_err(|_| internal("pending command lock poisoned"))?;
                if pending.is_some() {
                    return Err(internal("another HCI command is already outstanding"));
                }
                *pending = Some(PendingCommand { opcode, sender });
            }

            let io = self.io.as_ref().ok_or(RawHciError::Shutdown)?;
            if let Err(error) = control_transfer(io, &mut bytes, &self.state) {
                clear_pending(&self.state, opcode);
                self.state.fail(error.clone());
                io.cancel_all();
                return Err(error);
            }
            trace(
                &self.options,
                HciTraceDirection::Tx,
                HciTracePacketType::Command,
                &bytes,
            );

            match receiver.recv_timeout(self.options.effective_command_timeout()) {
                Ok(Ok(HciCommandResponse::Status {
                    status,
                    command_opcode,
                    ..
                })) if status != 0 => Err(RawHciError::CommandRejected {
                    opcode: command_opcode,
                    status,
                }),
                Ok(result) => result,
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    clear_pending(&self.state, opcode);
                    let error = RawHciError::CommandTimeout {
                        opcode,
                        timeout_ms: self
                            .options
                            .effective_command_timeout()
                            .as_millis()
                            .min(u64::MAX as u128) as u64,
                    };
                    self.state.fail(error.clone());
                    io.cancel_all();
                    Err(error)
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => Err(self
                    .state
                    .terminal_error()
                    .unwrap_or_else(|| RawHciError::EventRead {
                        message: "event worker stopped before command response".into(),
                    })),
            }
        }

        pub(super) fn send_acl(&self, packet: &HciAclPacket) -> Result<(), RawHciError> {
            self.ensure_operational()?;
            let _guard = self
                .acl_tx_lock
                .lock()
                .map_err(|_| internal("ACL TX lock poisoned"))?;
            self.ensure_operational()?;
            let bytes = packet.encode().map_err(packet_error)?;
            let io = self.io.as_ref().ok_or(RawHciError::Shutdown)?;
            if let Err(error) = pipe_write(io, self.transport.acl_out_pipe, &bytes, &self.state) {
                self.state.fail(error.clone());
                io.cancel_all();
                return Err(error);
            }
            trace(
                &self.options,
                HciTraceDirection::Tx,
                HciTracePacketType::Acl,
                &bytes,
            );
            Ok(())
        }

        pub(super) fn receive_event(
            &self,
            timeout: Duration,
        ) -> Result<Option<HciEventPacket>, RawHciError> {
            self.state
                .receive_packet(timeout, Some(true))
                .map(|packet| {
                    packet.map(|packet| match packet {
                        HciIncomingPacket::Event(event) => event,
                        _ => unreachable!("typed event receive"),
                    })
                })
        }

        pub(super) fn receive_acl(
            &self,
            timeout: Duration,
        ) -> Result<Option<HciAclPacket>, RawHciError> {
            self.state
                .receive_packet(timeout, Some(false))
                .map(|packet| {
                    packet.map(|packet| match packet {
                        HciIncomingPacket::Acl(acl) => acl,
                        _ => unreachable!("typed ACL receive"),
                    })
                })
        }

        pub(super) fn receive_packet(
            &self,
            timeout: Duration,
        ) -> Result<Option<HciIncomingPacket>, RawHciError> {
            self.state.receive_packet(timeout, None)
        }

        pub(super) fn terminal_error(&self) -> Option<RawHciError> {
            self.state.terminal_error()
        }

        fn ensure_operational(&self) -> Result<(), RawHciError> {
            if let Some(error) = self.state.terminal_error() {
                return Err(error);
            }
            if !self.state.accepting_tx.load(Ordering::Acquire)
                || self.state.shutdown.load(Ordering::Acquire)
            {
                return Err(RawHciError::Shutdown);
            }
            Ok(())
        }

        pub(super) fn shutdown(&mut self) -> RawHciShutdownReport {
            if let Some(report) = &self.shutdown_report {
                return report.clone();
            }
            self.state.accepting_tx.store(false, Ordering::Release);
            self.state.shutdown.store(true, Ordering::Release);
            self.state.fail_pending(RawHciError::Shutdown);
            self.state.wake_receivers();
            let mut cancellation_errors = self.io.as_ref().map_or_else(Vec::new, |io| {
                io.abort([
                    self.transport.event_pipe,
                    self.transport.acl_in_pipe,
                    self.transport.acl_out_pipe,
                ])
            });
            let event_rx_joined = join_worker(
                self.event_worker.take(),
                "event_rx",
                &mut cancellation_errors,
            );
            let acl_rx_joined =
                join_worker(self.acl_worker.take(), "acl_rx", &mut cancellation_errors);
            self.io.take();
            let report = RawHciShutdownReport {
                event_rx_joined,
                acl_rx_joined,
                handles_released: true,
                cancellation_errors,
            };
            self.shutdown_report = Some(report.clone());
            report
        }
    }

    impl Drop for Session {
        fn drop(&mut self) {
            let _ = self.shutdown();
        }
    }

    fn transport_from_readiness(
        readiness: &DedicatedWinUsbControllerReadiness,
    ) -> Result<RawHciTransportSummary, RawHciError> {
        if !matches!(readiness.status, DedicatedWinUsbReadinessStatus::Ready) {
            return Err(RawHciError::NotReady {
                status: format!("{:?}", readiness.status),
            });
        }
        let [application] = readiness.application_interfaces.as_slice() else {
            return Err(RawHciError::InvalidReadiness {
                message: format!(
                    "expected one application interface, got {}",
                    readiness.application_interfaces.len()
                ),
            });
        };
        let interface = readiness
            .interface
            .ok_or_else(|| RawHciError::InvalidReadiness {
                message: "missing interface descriptor".into(),
            })?;
        Ok(RawHciTransportSummary {
            application_interface: application.path.clone(),
            interface_number: interface.number,
            alternate_setting: interface.alternate_setting,
            event_pipe: readiness
                .event_pipe
                .ok_or_else(|| RawHciError::InvalidReadiness {
                    message: "missing Event pipe".into(),
                })?,
            acl_in_pipe: readiness
                .acl_in_pipe
                .ok_or_else(|| RawHciError::InvalidReadiness {
                    message: "missing ACL IN pipe".into(),
                })?,
            acl_out_pipe: readiness
                .acl_out_pipe
                .ok_or_else(|| RawHciError::InvalidReadiness {
                    message: "missing ACL OUT pipe".into(),
                })?,
        })
    }

    fn event_worker_loop(
        io: Arc<DeviceIo>,
        state: Arc<SharedState>,
        pipe: u8,
        started: mpsc::SyncSender<Result<(), RawHciError>>,
        trace_sink: Option<HciTraceCallback>,
        trace_raw: bool,
    ) {
        let mut first = Some(started);
        loop {
            if state.shutdown.load(Ordering::Acquire) {
                break;
            }
            let bytes = match pipe_read(&io, pipe, EVENT_BUFFER_SIZE, &state, first.take(), "event")
            {
                Ok(bytes) => bytes,
                Err(RawHciError::Shutdown) => break,
                Err(error) => {
                    state.fail(error);
                    io.cancel_all();
                    break;
                }
            };
            trace_parts(
                &trace_sink,
                trace_raw,
                HciTraceDirection::Rx,
                HciTracePacketType::Event,
                &bytes,
            );
            let event = match HciEventPacket::parse(&bytes) {
                Ok(event) => event,
                Err(error) => {
                    let error = RawHciError::MalformedHciPacket {
                        packet_type: "event".into(),
                        message: error.to_string(),
                    };
                    state.fail(error);
                    io.cancel_all();
                    break;
                }
            };
            match event.command_response() {
                Ok(Some(response)) => {
                    if !route_command_response(&state, event, response) {
                        io.cancel_all();
                        break;
                    }
                }
                Ok(None) => {
                    if !state.enqueue(HciIncomingPacket::Event(event)) {
                        io.cancel_all();
                        break;
                    }
                }
                Err(error) => {
                    let error = RawHciError::MalformedHciPacket {
                        packet_type: "command event".into(),
                        message: error.to_string(),
                    };
                    state.fail(error);
                    io.cancel_all();
                    break;
                }
            }
        }
    }

    fn route_command_response(
        state: &SharedState,
        event: HciEventPacket,
        response: HciCommandResponse,
    ) -> bool {
        let mut pending = match state.pending_command.lock() {
            Ok(pending) => pending,
            Err(poisoned) => {
                drop(poisoned.into_inner());
                state.fail(internal("pending command lock poisoned"));
                return false;
            }
        };
        match pending.as_ref() {
            Some(command) if command.opcode == response.opcode() => {
                if let Some(command) = pending.take() {
                    let _ = command.sender.send(Ok(response));
                }
                true
            }
            Some(command) => {
                let error = RawHciError::UnexpectedCommandOpcode {
                    expected: command.opcode,
                    actual: response.opcode(),
                };
                if let Some(command) = pending.take() {
                    let _ = command.sender.send(Err(error.clone()));
                }
                drop(pending);
                let queued = state.enqueue(HciIncomingPacket::Event(event));
                if queued {
                    state.fail(error);
                }
                false
            }
            None => {
                drop(pending);
                state.enqueue(HciIncomingPacket::Event(event))
            }
        }
    }

    fn acl_worker_loop(
        io: Arc<DeviceIo>,
        state: Arc<SharedState>,
        pipe: u8,
        started: mpsc::SyncSender<Result<(), RawHciError>>,
        trace_sink: Option<HciTraceCallback>,
        trace_raw: bool,
    ) {
        let mut first = Some(started);
        loop {
            if state.shutdown.load(Ordering::Acquire) {
                break;
            }
            let bytes = match pipe_read(&io, pipe, ACL_BUFFER_SIZE, &state, first.take(), "acl") {
                Ok(bytes) => bytes,
                Err(RawHciError::Shutdown) => break,
                Err(error) => {
                    state.fail(error);
                    io.cancel_all();
                    break;
                }
            };
            trace_parts(
                &trace_sink,
                trace_raw,
                HciTraceDirection::Rx,
                HciTracePacketType::Acl,
                &bytes,
            );
            match HciAclPacket::parse(&bytes) {
                Ok(packet) => {
                    if !state.enqueue(HciIncomingPacket::Acl(packet)) {
                        io.cancel_all();
                        break;
                    }
                }
                Err(error) => {
                    let error = RawHciError::MalformedHciPacket {
                        packet_type: "ACL".into(),
                        message: error.to_string(),
                    };
                    state.fail(error);
                    io.cancel_all();
                    break;
                }
            }
        }
    }

    fn pipe_read(
        io: &DeviceIo,
        pipe: u8,
        capacity: usize,
        state: &SharedState,
        started: Option<mpsc::SyncSender<Result<(), RawHciError>>>,
        kind: &str,
    ) -> Result<Vec<u8>, RawHciError> {
        let event = EventHandle::new().map_err(|error| read_error(kind, error, state))?;
        let mut overlapped = OVERLAPPED {
            hEvent: event.0,
            ..Default::default()
        };
        let mut buffer = vec![0u8; capacity];
        let mut transferred = 0u32;
        // SAFETY: buffer, OVERLAPPED, and event stay alive until the completion
        // query below confirms completion or cancellation.
        let result = {
            let _submission = io
                .submission
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if state.shutdown.load(Ordering::Acquire) {
                return Err(RawHciError::Shutdown);
            }
            unsafe {
                WinUsb_ReadPipe(
                    io.interface,
                    pipe,
                    Some(&mut buffer),
                    Some(&mut transferred),
                    Some(&mut overlapped),
                )
            }
        };
        let accepted = result.is_ok() || result.as_ref().is_err_and(is_io_pending);
        if let Some(started) = started {
            if accepted {
                let _ = started.send(Ok(()));
            } else {
                let error = read_error(kind, result.expect_err("not accepted"), state);
                let _ = started.send(Err(error.clone()));
                return Err(error);
            }
        }
        complete_overlapped(io, &overlapped, &mut transferred, result, kind, state)?;
        buffer.truncate(transferred as usize);
        Ok(buffer)
    }

    fn pipe_write(
        io: &DeviceIo,
        pipe: u8,
        bytes: &[u8],
        state: &SharedState,
    ) -> Result<(), RawHciError> {
        let event = EventHandle::new().map_err(|error| RawHciError::AclWrite {
            message: error.to_string(),
        })?;
        let overlapped = OVERLAPPED {
            hEvent: event.0,
            ..Default::default()
        };
        let mut transferred = 0u32;
        // SAFETY: bytes, OVERLAPPED, and event stay alive until completion is
        // confirmed below.
        let result = unsafe {
            WinUsb_WritePipe(
                io.interface,
                pipe,
                bytes,
                Some(&mut transferred),
                Some(&overlapped),
            )
        };
        complete_overlapped(
            io,
            &overlapped,
            &mut transferred,
            result,
            "ACL write",
            state,
        )
        .map_err(|error| match error {
            RawHciError::DeviceRemoved { .. } | RawHciError::Shutdown => error,
            other => RawHciError::AclWrite {
                message: other.to_string(),
            },
        })?;
        if transferred as usize != bytes.len() {
            return Err(RawHciError::AclWrite {
                message: format!("short write: {} of {} bytes", transferred, bytes.len()),
            });
        }
        Ok(())
    }

    fn control_transfer(
        io: &DeviceIo,
        bytes: &mut [u8],
        state: &SharedState,
    ) -> Result<(), RawHciError> {
        let length = u16::try_from(bytes.len()).map_err(|_| RawHciError::UsbControlTransfer {
            message: "HCI command exceeds USB control transfer length".into(),
        })?;
        let setup = WINUSB_SETUP_PACKET {
            RequestType: 0x20,
            Request: 0,
            Value: 0,
            Index: 0,
            Length: length,
        };
        let event = EventHandle::new().map_err(|error| RawHciError::UsbControlTransfer {
            message: error.to_string(),
        })?;
        let overlapped = OVERLAPPED {
            hEvent: event.0,
            ..Default::default()
        };
        let mut transferred = 0u32;
        // SAFETY: the complete command buffer, OVERLAPPED, and event stay alive
        // through completion; setup.Length exactly matches the slice.
        let result = unsafe {
            WinUsb_ControlTransfer(
                io.interface,
                setup,
                Some(bytes),
                Some(&mut transferred),
                Some(&overlapped),
            )
        };
        complete_overlapped(
            io,
            &overlapped,
            &mut transferred,
            result,
            "control transfer",
            state,
        )
        .map_err(|error| match error {
            RawHciError::DeviceRemoved { .. } | RawHciError::Shutdown => error,
            other => RawHciError::UsbControlTransfer {
                message: other.to_string(),
            },
        })?;
        if transferred as usize != bytes.len() {
            return Err(RawHciError::UsbControlTransfer {
                message: format!("short write: {} of {} bytes", transferred, bytes.len()),
            });
        }
        Ok(())
    }

    fn complete_overlapped(
        io: &DeviceIo,
        overlapped: &OVERLAPPED,
        transferred: &mut u32,
        initial: windows::core::Result<()>,
        operation: &str,
        state: &SharedState,
    ) -> Result<(), RawHciError> {
        match initial {
            Ok(()) => Ok(()),
            Err(error) if is_io_pending(&error) => {
                // SAFETY: caller retains the operation's buffer, OVERLAPPED,
                // and event until this blocking completion query returns.
                unsafe { WinUsb_GetOverlappedResult(io.interface, overlapped, transferred, true) }
                    .map_err(|error| windows_io_error(operation, error, state))
            }
            Err(error) => Err(windows_io_error(operation, error, state)),
        }
    }

    fn is_io_pending(error: &windows::core::Error) -> bool {
        error.code() == HRESULT::from_win32(ERROR_IO_PENDING.0)
    }

    fn windows_io_error(
        operation: &str,
        error: windows::core::Error,
        state: &SharedState,
    ) -> RawHciError {
        if state.shutdown.load(Ordering::Acquire)
            && error.code() == HRESULT::from_win32(ERROR_OPERATION_ABORTED.0)
        {
            return RawHciError::Shutdown;
        }
        if error.code() == HRESULT::from_win32(ERROR_DEVICE_NOT_CONNECTED.0)
            || error.code() == HRESULT::from_win32(ERROR_NO_SUCH_DEVICE.0)
        {
            return RawHciError::DeviceRemoved {
                operation: operation.into(),
                message: error.to_string(),
            };
        }
        match operation {
            "event" => RawHciError::EventRead {
                message: error.to_string(),
            },
            "acl" => RawHciError::AclRead {
                message: error.to_string(),
            },
            _ => RawHciError::Internal {
                message: format!("{operation}: {error}"),
            },
        }
    }

    fn read_error(kind: &str, error: windows::core::Error, state: &SharedState) -> RawHciError {
        windows_io_error(kind, error, state)
    }

    fn clear_pending(state: &SharedState, opcode: u16) {
        if let Ok(mut pending) = state.pending_command.lock() {
            if pending
                .as_ref()
                .is_some_and(|pending| pending.opcode == opcode)
            {
                pending.take();
            }
        }
    }

    fn join_worker(worker: Option<JoinHandle<()>>, name: &str, errors: &mut Vec<String>) -> bool {
        match worker {
            None => true,
            Some(worker) => match worker.join() {
                Ok(()) => true,
                Err(_) => {
                    errors.push(format!("{name} worker panicked"));
                    false
                }
            },
        }
    }

    fn packet_error(error: HciPacketError) -> RawHciError {
        RawHciError::MalformedHciPacket {
            packet_type: "outbound".into(),
            message: error.to_string(),
        }
    }
    fn internal(message: &str) -> RawHciError {
        RawHciError::Internal {
            message: message.into(),
        }
    }

    fn trace(
        options: &RawHciSessionOptions,
        direction: HciTraceDirection,
        packet_type: HciTracePacketType,
        bytes: &[u8],
    ) {
        trace_parts(
            &options.trace,
            options.trace_raw_bytes,
            direction,
            packet_type,
            bytes,
        )
    }

    fn trace_parts(
        sink: &Option<HciTraceCallback>,
        include_raw: bool,
        direction: HciTraceDirection,
        packet_type: HciTracePacketType,
        bytes: &[u8],
    ) {
        let Some(sink) = sink else {
            return;
        };
        let unix_time_micros = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |duration| duration.as_micros());
        sink(HciTraceRecord {
            unix_time_micros,
            direction,
            packet_type,
            length: bytes.len(),
            raw: include_raw.then(|| bytes.to_vec()),
        });
    }
}
