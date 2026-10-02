use std::io::{Read, Write};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use directhci_core::*;
use windows::Win32::Foundation::{
    CloseHandle, ERROR_BROKEN_PIPE, ERROR_IO_PENDING, ERROR_NO_DATA, ERROR_PIPE_CONNECTED, HANDLE,
    HLOCAL, LocalFree, WAIT_OBJECT_0, WAIT_TIMEOUT,
};
use windows::Win32::Security::Authorization::{
    ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
};
use windows::Win32::Security::{
    CheckTokenMembership, CreateWellKnownSid, PSECURITY_DESCRIPTOR, PSID, RevertToSelf,
    SECURITY_ATTRIBUTES, TOKEN_QUERY, WinBuiltinAdministratorsSid,
};
use windows::Win32::Storage::FileSystem::{
    FILE_FLAG_FIRST_PIPE_INSTANCE, FILE_FLAG_OVERLAPPED, PIPE_ACCESS_DUPLEX, ReadFile, WriteFile,
};
use windows::Win32::System::IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED};
use windows::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, ImpersonateNamedPipeClient,
    PIPE_READMODE_BYTE, PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE, PIPE_UNLIMITED_INSTANCES,
    PIPE_WAIT,
};
use windows::Win32::System::Threading::{
    CreateEventW, GetCurrentThread, OpenThreadToken, WaitForSingleObject,
};
use windows::core::{BOOL, HRESULT, PCWSTR};

use crate::runtime::{DirectHciRuntime, Outbound};

const OUTBOUND_DEPTH: usize = 128;
const PIPE_BUFFER_SIZE: u32 = 64 * 1024;
const MAX_CLIENT_NAME: usize = 128;
const MAX_CONNECTIONS: usize = 16;
const HELLO_TIMEOUT: Duration = Duration::from_secs(5);
const DIAGNOSTIC_IDLE_TIMEOUT: Duration = Duration::from_secs(10 * 60);

static NEXT_CONNECTION_ID: AtomicU64 = AtomicU64::new(1);

pub fn serve(runtime: Arc<DirectHciRuntime>, stop: Arc<AtomicBool>) -> Result<(), String> {
    let connections: Arc<Mutex<Vec<Arc<PipeHandle>>>> = Arc::new(Mutex::new(Vec::new()));
    let mut workers: Vec<JoinHandle<()>> = Vec::new();
    let mut first_instance = true;
    while !stop.load(Ordering::Acquire) {
        reap_finished(&mut workers);
        let handle = create_pipe(first_instance)?;
        first_instance = false;
        if let Err(error) = connect_pipe(handle, &stop) {
            let _ = unsafe { CloseHandle(handle) };
            if stop.load(Ordering::Acquire) {
                break;
            }
            return Err(error);
        }
        if stop.load(Ordering::Acquire) {
            let _ = unsafe { DisconnectNamedPipe(handle) };
            let _ = unsafe { CloseHandle(handle) };
            break;
        }
        if connections
            .lock()
            .map_err(|_| "connection registry lock poisoned")?
            .len()
            >= MAX_CONNECTIONS
        {
            let _ = unsafe { DisconnectNamedPipe(handle) };
            let _ = unsafe { CloseHandle(handle) };
            continue;
        }
        let handle = Arc::new(PipeHandle {
            raw: handle,
            stopping: AtomicBool::new(false),
            submission: Mutex::new(()),
        });
        connections
            .lock()
            .map_err(|_| "connection registry lock poisoned")?
            .push(Arc::clone(&handle));
        let connection_id = NEXT_CONNECTION_ID.fetch_add(1, Ordering::Relaxed);
        let runtime = Arc::clone(&runtime);
        let registry = Arc::clone(&connections);
        workers.push(
            thread::Builder::new()
                .name(format!("directhci-ipc-{connection_id}"))
                .spawn(move || {
                    if let Err(error) =
                        handle_connection(connection_id, Arc::clone(&handle), Arc::clone(&runtime))
                    {
                        eprintln!("directhcid: IPC connection {connection_id} ended: {error}");
                    }
                    runtime.disconnect(connection_id);
                    handle.request_cancel();
                    if let Ok(mut values) = registry.lock() {
                        values.retain(|value| !Arc::ptr_eq(value, &handle));
                    }
                })
                .map_err(|error| format!("spawn IPC connection: {error}"))?,
        );
    }

    runtime.shutdown();
    if let Ok(values) = connections.lock() {
        for handle in values.iter() {
            handle.request_cancel();
        }
    }
    for worker in workers {
        if worker.is_finished() {
            let _ = worker.join();
        }
    }
    Ok(())
}

fn reap_finished(workers: &mut Vec<JoinHandle<()>>) {
    let mut index = 0;
    while index < workers.len() {
        if workers[index].is_finished() {
            let _ = workers.swap_remove(index).join();
        } else {
            index += 1;
        }
    }
}

fn handle_connection(
    connection_id: u64,
    handle: Arc<PipeHandle>,
    runtime: Arc<DirectHciRuntime>,
) -> Result<(), String> {
    let hello_frame = read_ipc_frame(&mut PipeReader {
        handle: &handle,
        deadline: Some(Instant::now() + HELLO_TIMEOUT),
    })
    .map_err(|error| error.to_string())?
    .ok_or_else(|| "client closed before ClientHello".to_owned())?;
    if hello_frame.kind != IpcMessageKind::ClientHello {
        return Err("first frame must be ClientHello".into());
    }
    let hello: ClientHello =
        decode_ipc_json(&hello_frame.payload).map_err(|error| error.to_string())?;
    if hello.protocol_version != IPC_PROTOCOL_VERSION
        || hello.client_name.is_empty()
        || hello.client_name.len() > MAX_CLIENT_NAME
    {
        return Err("invalid ClientHello".into());
    }
    // Impersonate only after the first validated read. Win32 associates
    // ImpersonateNamedPipeClient with the client context of the last pipe read.
    let is_admin = client_is_administrator(handle.raw)?;
    let server_hello = ServerHello {
        protocol_version: IPC_PROTOCOL_VERSION,
        directhci_version: env!("CARGO_PKG_VERSION").into(),
        capabilities: vec![
            "controller_enumeration".into(),
            "temporary_takeover".into(),
            "raw_hci_command".into(),
            "raw_hci_event".into(),
            "raw_hci_acl".into(),
            "offline_recovery_status".into(),
        ],
    };
    let response = IpcFrame::v1(
        IpcMessageKind::ServerHello,
        hello_frame.request_id,
        encode_ipc_json(&server_hello).map_err(|error| error.to_string())?,
    )
    .map_err(|error| error.to_string())?;
    write_ipc_frame(&mut PipeWriter(&handle), &response).map_err(|error| error.to_string())?;
    eprintln!(
        "directhcid: client `{}` connected (administrator={is_admin})",
        hello.client_name
    );

    let (outbound, outgoing) = mpsc::sync_channel::<IpcFrame>(OUTBOUND_DEPTH);
    let writer_handle = Arc::clone(&handle);
    let writer = thread::Builder::new()
        .name(format!("directhci-ipc-writer-{connection_id}"))
        .spawn(move || writer_loop(writer_handle, outgoing))
        .map_err(|error| error.to_string())?;

    let request_result = (|| {
        loop {
            // A writer session may legitimately stay idle for a long time while
            // receiving HCI traffic; only diagnostic clients get an idle limit.
            let deadline = (!runtime.owns_connection(connection_id))
                .then(|| Instant::now() + DIAGNOSTIC_IDLE_TIMEOUT);
            let frame = match read_ipc_frame(&mut PipeReader {
                handle: &handle,
                deadline,
            }) {
                Ok(Some(frame)) => frame,
                Ok(None) => break,
                Err(error) if is_normal_disconnect(&error) => break,
                Err(error) => return Err(error.to_string()),
            };
            let result = dispatch_frame(
                connection_id,
                &hello.client_name,
                is_admin,
                &runtime,
                &outbound,
                frame,
            );
            if let Err((request_id, code, message)) = result {
                if send_error(&outbound, request_id, code, message).is_err() {
                    break;
                }
            }
        }
        Ok(())
    })();
    runtime.disconnect(connection_id);
    drop(outbound);
    handle.request_cancel();
    let _ = writer.join();
    request_result
}

fn dispatch_frame(
    connection_id: u64,
    client_name: &str,
    is_admin: bool,
    runtime: &Arc<DirectHciRuntime>,
    outbound: &Outbound,
    frame: IpcFrame,
) -> Result<(), (u32, IpcErrorCode, String)> {
    let request_id = frame.request_id;
    if request_id == 0 {
        return Err((
            0,
            IpcErrorCode::Protocol,
            "client request ID must not be zero".into(),
        ));
    }
    match frame.kind {
        IpcMessageKind::ControlRequest => {
            let request: ControlRequest = decode_ipc_json(&frame.payload)
                .map_err(|error| (request_id, IpcErrorCode::Protocol, error.to_string()))?;
            match request {
                ControlRequest::ListControllers => {
                    let controllers = runtime
                        .list_controllers()
                        .map_err(|error| (request_id, IpcErrorCode::Runtime, error))?;
                    send_json(
                        outbound,
                        request_id,
                        &ControlResponse::Controllers { controllers },
                    )
                }
                ControlRequest::RuntimeStatus => {
                    let status = runtime
                        .status()
                        .map_err(|error| (request_id, IpcErrorCode::Runtime, error))?;
                    send_json(
                        outbound,
                        request_id,
                        &ControlResponse::RuntimeStatus { status },
                    )
                }
                ControlRequest::GetPreferences => {
                    let preferences = runtime
                        .preferences()
                        .map_err(|error| (request_id, IpcErrorCode::Runtime, error))?;
                    send_json(
                        outbound,
                        request_id,
                        &ControlResponse::Preferences { preferences },
                    )
                }
                ControlRequest::SetPreferredController { controller_id } => {
                    require_admin(is_admin, request_id)?;
                    let preferences = runtime
                        .set_preferred_controller(&controller_id)
                        .map_err(|(code, message)| (request_id, code, message))?;
                    send_json(
                        outbound,
                        request_id,
                        &ControlResponse::Preferences { preferences },
                    )
                }
                ControlRequest::ControllerPreparationStatus { controller_id } => {
                    let status = runtime
                        .controller_preparation_status(&controller_id)
                        .map_err(|(code, message)| (request_id, code, message))?;
                    send_json(
                        outbound,
                        request_id,
                        &ControlResponse::ControllerPreparationStatus { status },
                    )
                }
                ControlRequest::PrepareController {
                    controller_id,
                    trust_acknowledged,
                } => {
                    require_admin(is_admin, request_id)?;
                    if !trust_acknowledged {
                        return Err((
                            request_id,
                            IpcErrorCode::Unauthorized,
                            "explicit local certificate trust authorization is required".into(),
                        ));
                    }
                    let result = runtime
                        .prepare_controller(&controller_id)
                        .map_err(|(code, message)| (request_id, code, message))?;
                    send_json(
                        outbound,
                        request_id,
                        &ControlResponse::ControllerPrepared {
                            preparation: result,
                        },
                    )
                }
                ControlRequest::RestoreWindows => {
                    require_admin(is_admin, request_id)?;
                    runtime
                        .restore_windows()
                        .map_err(|(code, message)| (request_id, code, message))?;
                    send_json(outbound, request_id, &ControlResponse::Accepted)
                }
                ControlRequest::AcquireRawHci { controller_id } => {
                    require_admin(is_admin, request_id)?;
                    let response = runtime
                        .acquire(
                            connection_id,
                            client_name.into(),
                            &controller_id,
                            outbound.clone(),
                        )
                        .map_err(|(code, message)| (request_id, code, message))?;
                    send_json(outbound, request_id, &response)
                }
                ControlRequest::ReleaseSession { session_id } => {
                    require_admin(is_admin, request_id)?;
                    runtime
                        .release(connection_id, session_id, request_id)
                        .map_err(|(code, message)| (request_id, code, message))
                }
            }
        }
        IpcMessageKind::HciCommand => {
            require_admin(is_admin, request_id)?;
            let (session_id, opcode, parameters) = decode_hci_command(&frame.payload)
                .map_err(|error| (request_id, IpcErrorCode::Protocol, error.to_string()))?;
            runtime
                .send_command(connection_id, session_id, request_id, opcode, parameters)
                .map_err(|(code, message)| (request_id, code, message))
        }
        IpcMessageKind::AclTx => {
            require_admin(is_admin, request_id)?;
            let (session_id, packet) = decode_acl_packet(&frame.payload)
                .map_err(|error| (request_id, IpcErrorCode::Protocol, error.to_string()))?;
            runtime
                .send_acl(connection_id, session_id, request_id, packet)
                .map_err(|(code, message)| (request_id, code, message))
        }
        _ => Err((
            request_id,
            IpcErrorCode::Protocol,
            "message kind is not valid from a client".into(),
        )),
    }
}

fn require_admin(is_admin: bool, request_id: u32) -> Result<(), (u32, IpcErrorCode, String)> {
    if is_admin {
        Ok(())
    } else {
        Err((
            request_id,
            IpcErrorCode::Unauthorized,
            "administrator membership is required for this operation".into(),
        ))
    }
}

fn send_json(
    outbound: &Outbound,
    request_id: u32,
    response: &ControlResponse,
) -> Result<(), (u32, IpcErrorCode, String)> {
    let payload = encode_ipc_json(response)
        .map_err(|error| (request_id, IpcErrorCode::Runtime, error.to_string()))?;
    let frame = IpcFrame::v1(IpcMessageKind::ControlResponse, request_id, payload)
        .map_err(|error| (request_id, IpcErrorCode::Runtime, error.to_string()))?;
    outbound.try_send(frame).map_err(|_| {
        (
            request_id,
            IpcErrorCode::Backpressure,
            "client outbound queue is full".into(),
        )
    })
}

fn send_error(
    outbound: &Outbound,
    request_id: u32,
    code: IpcErrorCode,
    message: String,
) -> Result<(), ()> {
    let payload = encode_ipc_json(&IpcErrorResponse { code, message }).map_err(|_| ())?;
    outbound
        .try_send(IpcFrame::v1(IpcMessageKind::Error, request_id, payload).map_err(|_| ())?)
        .map_err(|_| ())
}

fn writer_loop(handle: Arc<PipeHandle>, outgoing: mpsc::Receiver<IpcFrame>) {
    loop {
        if handle.stopping.load(Ordering::Acquire) {
            break;
        }
        let frame = match outgoing.recv_timeout(Duration::from_millis(100)) {
            Ok(frame) => frame,
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        };
        if write_ipc_frame(&mut PipeWriter(&handle), &frame).is_err() {
            break;
        }
    }
    handle.request_cancel();
}

struct PipeHandle {
    raw: HANDLE,
    stopping: AtomicBool,
    submission: Mutex<()>,
}
unsafe impl Send for PipeHandle {}
unsafe impl Sync for PipeHandle {}
impl PipeHandle {
    fn request_cancel(&self) {
        self.stopping.store(true, Ordering::Release);
        // Serialize cancellation with submission: no new OVERLAPPED operation
        // can start after CancelIoEx has inspected the handle.
        let _guard = self
            .submission
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let _ = unsafe { CancelIoEx(self.raw, None) };
    }
}

impl Drop for PipeHandle {
    fn drop(&mut self) {
        // The last Arc is released only after all reader/writer calls return.
        // Pending operations drain completion before releasing their storage.
        self.request_cancel();
        let _ = unsafe { DisconnectNamedPipe(self.raw) };
        let _ = unsafe { CloseHandle(self.raw) };
    }
}

struct PipeReader<'a> {
    handle: &'a PipeHandle,
    deadline: Option<Instant>,
}
struct PipeWriter<'a>(&'a PipeHandle);
impl Read for PipeReader<'_> {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        overlapped_pipe_io(
            self.handle,
            self.deadline,
            |overlapped, transferred| unsafe {
                ReadFile(
                    self.handle.raw,
                    Some(buffer),
                    Some(transferred),
                    Some(overlapped),
                )
            },
        )
    }
}
impl Write for PipeWriter<'_> {
    fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
        overlapped_pipe_io(self.0, None, |overlapped, transferred| unsafe {
            WriteFile(
                self.0.raw,
                Some(buffer),
                Some(transferred),
                Some(overlapped),
            )
        })
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn create_pipe(first_instance: bool) -> Result<HANDLE, String> {
    let name = wide(IPC_PIPE_NAME);
    // AU: SYNCHRONIZE | READ_CONTROL | READ/WRITE_DATA |
    // READ/WRITE_ATTRIBUTES. 0x0004 (FILE_CREATE_PIPE_INSTANCE) is excluded.
    let sddl = wide("D:P(A;;GA;;;SY)(A;;GA;;;BA)(A;;0x00120183;;;AU)");
    let mut descriptor = PSECURITY_DESCRIPTOR::default();
    // SAFETY: SDDL is NUL-terminated; Windows allocates descriptor memory,
    // which is released with LocalFree after CreateNamedPipe consumes it.
    unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            PCWSTR(sddl.as_ptr()),
            SDDL_REVISION_1,
            &mut descriptor,
            None,
        )
    }
    .map_err(|error| format!("build pipe security descriptor: {error}"))?;
    let attributes = SECURITY_ATTRIBUTES {
        nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: descriptor.0,
        bInheritHandle: false.into(),
    };
    // SAFETY: name and SECURITY_ATTRIBUTES remain valid through the call.
    let handle = unsafe {
        CreateNamedPipeW(
            PCWSTR(name.as_ptr()),
            PIPE_ACCESS_DUPLEX
                | FILE_FLAG_OVERLAPPED
                | if first_instance {
                    FILE_FLAG_FIRST_PIPE_INSTANCE
                } else {
                    Default::default()
                },
            PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
            PIPE_UNLIMITED_INSTANCES,
            PIPE_BUFFER_SIZE,
            PIPE_BUFFER_SIZE,
            0,
            Some(&attributes),
        )
    };
    // SAFETY: descriptor was allocated by ConvertStringSecurityDescriptor...
    let _ = unsafe { LocalFree(Some(HLOCAL(descriptor.0))) };
    if handle.is_invalid() {
        Err(format!(
            "CreateNamedPipeW: {}",
            windows::core::Error::from_thread()
        ))
    } else {
        Ok(handle)
    }
}

fn connect_pipe(handle: HANDLE, stop: &AtomicBool) -> Result<(), String> {
    // SAFETY: the event and OVERLAPPED remain live until the connect completes.
    let event = unsafe { CreateEventW(None, true, false, PCWSTR::null()) }
        .map_err(|error| format!("create pipe connect event: {error}"))?;
    let mut overlapped = OVERLAPPED {
        hEvent: event,
        ..Default::default()
    };
    let initial = unsafe { ConnectNamedPipe(handle, Some(&mut overlapped)) };
    let result: Result<(), String> = match initial {
        Ok(()) => Ok(()),
        Err(error) if error.code() == HRESULT::from_win32(ERROR_IO_PENDING.0) => {
            let mut transferred = 0;
            loop {
                if stop.load(Ordering::Acquire) {
                    let _ = unsafe { CancelIoEx(handle, Some(&overlapped)) };
                    // Cancellation is asynchronous; drain before releasing the
                    // stack-owned OVERLAPPED and event.
                    let _ =
                        unsafe { GetOverlappedResult(handle, &overlapped, &mut transferred, true) };
                    break Err("pipe accept cancelled by shutdown".into());
                }
                match unsafe { WaitForSingleObject(event, 100) } {
                    WAIT_OBJECT_0 => {
                        break unsafe {
                            GetOverlappedResult(handle, &overlapped, &mut transferred, true)
                        }
                        .map_err(|error| format!("ConnectNamedPipe: {error}"));
                    }
                    WAIT_TIMEOUT => continue,
                    _ => {
                        let error = windows::core::Error::from_thread();
                        let _ = unsafe { CancelIoEx(handle, Some(&overlapped)) };
                        let _ = unsafe {
                            GetOverlappedResult(handle, &overlapped, &mut transferred, true)
                        };
                        break Err(format!("wait for ConnectNamedPipe: {error}"));
                    }
                }
            }
        }
        Err(error) if error.code() == HRESULT::from_win32(ERROR_PIPE_CONNECTED.0) => Ok(()),
        Err(error) => Err(format!("ConnectNamedPipe: {error}")),
    };
    // SAFETY: the connect is complete (or failed), so the event is no longer in use.
    let _ = unsafe { CloseHandle(event) };
    result
}

fn overlapped_pipe_io(
    handle: &PipeHandle,
    deadline: Option<Instant>,
    operation: impl FnOnce(*mut OVERLAPPED, *mut u32) -> windows::core::Result<()>,
) -> std::io::Result<usize> {
    // Each simultaneous read/write owns an independent manual-reset event and
    // OVERLAPPED structure for the full operation lifetime.
    let event = unsafe { CreateEventW(None, true, false, PCWSTR::null()) }.map_err(win_io)?;
    let mut overlapped = OVERLAPPED {
        hEvent: event,
        ..Default::default()
    };
    let mut transferred = 0u32;
    let initial = {
        let _guard = handle
            .submission
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if handle.stopping.load(Ordering::Acquire) {
            let _ = unsafe { CloseHandle(event) };
            return Err(std::io::ErrorKind::BrokenPipe.into());
        }
        operation(&mut overlapped, &mut transferred)
    };
    let result = match initial {
        Ok(()) => Ok(()),
        Err(error) if error.code() == HRESULT::from_win32(ERROR_IO_PENDING.0) => {
            if let Some(deadline) = deadline {
                let remaining = deadline.saturating_duration_since(Instant::now());
                let wait_ms = remaining.as_millis().min(u32::MAX as u128) as u32;
                if unsafe { WaitForSingleObject(event, wait_ms) } == WAIT_TIMEOUT {
                    let _ = unsafe { CancelIoEx(handle.raw, Some(&overlapped)) };
                    // CancelIoEx only requests cancellation. Do not free the
                    // stack-owned OVERLAPPED/event/buffer until completion.
                    let _ = unsafe {
                        GetOverlappedResult(handle.raw, &overlapped, &mut transferred, true)
                    };
                    let _ = unsafe { CloseHandle(event) };
                    return Err(std::io::ErrorKind::TimedOut.into());
                }
            }
            unsafe { GetOverlappedResult(handle.raw, &overlapped, &mut transferred, true) }
        }
        Err(error) => Err(error),
    };
    // SAFETY: GetOverlappedResult has completed/cancelled the operation before
    // the event and stack-owned OVERLAPPED are released.
    let _ = unsafe { CloseHandle(event) };
    result.map_err(win_io)?;
    Ok(transferred as usize)
}

fn is_normal_disconnect(error: &IpcFrameError) -> bool {
    matches!(
        error,
        IpcFrameError::Io(error)
            if matches!(
                error.kind(),
                std::io::ErrorKind::BrokenPipe | std::io::ErrorKind::UnexpectedEof
            )
    )
}

fn client_is_administrator(pipe: HANDLE) -> Result<bool, String> {
    // SAFETY: a validated ClientHello has already been read from this connected
    // server pipe, establishing the client security context Win32 requires.
    unsafe { ImpersonateNamedPipeClient(pipe) }
        .map_err(|error| format!("impersonate pipe client: {error}"))?;
    let revert = RevertImpersonation;
    let result: Result<bool, String> = (|| {
        let mut raw_token = HANDLE::default();
        // SAFETY: GetCurrentThread returns the current thread pseudo-handle;
        // after successful pipe impersonation its token is the client token.
        unsafe { OpenThreadToken(GetCurrentThread(), TOKEN_QUERY, true, &mut raw_token) }
            .map_err(|error| format!("open pipe client thread token: {error}"))?;
        let token = TokenHandle(raw_token);
        let mut storage = [0u32; 17];
        let mut size = std::mem::size_of_val(&storage) as u32;
        let sid = PSID(storage.as_mut_ptr().cast());
        // SAFETY: storage is sufficiently sized for a well-known SID.
        unsafe { CreateWellKnownSid(WinBuiltinAdministratorsSid, None, Some(sid), &mut size) }
            .map_err(|error| format!("create administrators SID: {error}"))?;
        let mut member = BOOL(0);
        // SAFETY: token is the explicitly opened impersonated client token and
        // sid points at live, sufficiently sized well-known-SID storage.
        unsafe { CheckTokenMembership(Some(token.0), sid, &mut member) }
            .map_err(|error| format!("check administrator membership: {error}"))?;
        Ok(member.as_bool())
    })();
    // SAFETY: balances the successful impersonation above.
    let reverted =
        unsafe { RevertToSelf() }.map_err(|error| format!("revert pipe impersonation: {error}"));
    if reverted.is_ok() {
        std::mem::forget(revert);
    }
    match (result, reverted) {
        (Ok(member), Ok(())) => Ok(member),
        (Err(error), _) | (_, Err(error)) => Err(error),
    }
}

struct RevertImpersonation;
impl Drop for RevertImpersonation {
    fn drop(&mut self) {
        // SAFETY: this guard exists only after successful impersonation. This
        // best-effort fallback also covers early returns and unwinding.
        let _ = unsafe { RevertToSelf() };
    }
}

struct TokenHandle(HANDLE);
impl Drop for TokenHandle {
    fn drop(&mut self) {
        // SAFETY: this wrapper exclusively owns the OpenThreadToken handle.
        let _ = unsafe { CloseHandle(self.0) };
    }
}

fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}
fn win_io(error: windows::core::Error) -> std::io::Error {
    if error.code() == HRESULT::from_win32(ERROR_BROKEN_PIPE.0)
        || error.code() == HRESULT::from_win32(ERROR_NO_DATA.0)
    {
        std::io::Error::new(std::io::ErrorKind::BrokenPipe, error.to_string())
    } else {
        std::io::Error::other(error.to_string())
    }
}
