use std::io::{Read, Write};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use directhci_core::*;
use windows::Win32::Foundation::{
    CloseHandle, ERROR_BROKEN_PIPE, ERROR_IO_PENDING, ERROR_NO_DATA, ERROR_PIPE_CONNECTED,
    GENERIC_READ, GENERIC_WRITE, HANDLE, HLOCAL, LocalFree,
};
use windows::Win32::Security::Authorization::{
    ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
};
use windows::Win32::Security::{
    CheckTokenMembership, CreateWellKnownSid, PSECURITY_DESCRIPTOR, PSID, RevertToSelf,
    SECURITY_ATTRIBUTES, TOKEN_QUERY, WinBuiltinAdministratorsSid,
};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_FLAG_OVERLAPPED, FILE_SHARE_READ, FILE_SHARE_WRITE,
    OPEN_EXISTING, PIPE_ACCESS_DUPLEX, ReadFile, WriteFile,
};
use windows::Win32::System::IO::{CancelIoEx, GetOverlappedResult, OVERLAPPED};
use windows::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, ImpersonateNamedPipeClient,
    PIPE_READMODE_BYTE, PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE, PIPE_UNLIMITED_INSTANCES,
    PIPE_WAIT,
};
use windows::Win32::System::Threading::{CreateEventW, GetCurrentThread, OpenThreadToken};
use windows::core::{BOOL, HRESULT, PCWSTR};

use crate::runtime::{DirectHciRuntime, Outbound};

const OUTBOUND_DEPTH: usize = 128;
const PIPE_BUFFER_SIZE: u32 = 64 * 1024;
const MAX_CLIENT_NAME: usize = 128;

static NEXT_CONNECTION_ID: AtomicU64 = AtomicU64::new(1);

pub fn serve(runtime: Arc<DirectHciRuntime>, stop: Arc<AtomicBool>) -> Result<(), String> {
    let connections: Arc<Mutex<Vec<Arc<PipeHandle>>>> = Arc::new(Mutex::new(Vec::new()));
    let mut workers: Vec<JoinHandle<()>> = Vec::new();
    let wake_stop = Arc::clone(&stop);
    let wake_thread = thread::spawn(move || {
        while !wake_stop.load(Ordering::Acquire) {
            thread::sleep(Duration::from_millis(100));
        }
        wake_listener();
    });

    while !stop.load(Ordering::Acquire) {
        let handle = create_pipe()?;
        if let Err(error) = connect_pipe(handle) {
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
        let handle = Arc::new(PipeHandle {
            raw: handle,
            closed: AtomicBool::new(false),
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
                    handle.close_once();
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
            handle.close_once();
        }
    }
    for worker in workers {
        if worker.is_finished() {
            let _ = worker.join();
        }
    }
    let _ = wake_thread.join();
    Ok(())
}

fn handle_connection(
    connection_id: u64,
    handle: Arc<PipeHandle>,
    runtime: Arc<DirectHciRuntime>,
) -> Result<(), String> {
    let hello_frame = read_ipc_frame(&mut PipeReader(&handle))
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

    loop {
        let frame = match read_ipc_frame(&mut PipeReader(&handle)) {
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
    runtime.disconnect(connection_id);
    drop(outbound);
    handle.close_once();
    let _ = writer.join();
    Ok(())
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
            "administrator membership is required for ownership and Raw HCI".into(),
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
    for frame in outgoing {
        if write_ipc_frame(&mut PipeWriter(&handle), &frame).is_err() {
            break;
        }
    }
    handle.close_once();
}

struct PipeHandle {
    raw: HANDLE,
    closed: AtomicBool,
}
unsafe impl Send for PipeHandle {}
unsafe impl Sync for PipeHandle {}
impl PipeHandle {
    fn close_once(&self) {
        if !self.closed.swap(true, Ordering::AcqRel) {
            // SAFETY: this object uniquely owns the pipe handle. Cancellation
            // wakes the reader/writer before the handle is closed once.
            let _ = unsafe { CancelIoEx(self.raw, None) };
            let _ = unsafe { DisconnectNamedPipe(self.raw) };
            let _ = unsafe { CloseHandle(self.raw) };
        }
    }
}

struct PipeReader<'a>(&'a PipeHandle);
struct PipeWriter<'a>(&'a PipeHandle);
impl Read for PipeReader<'_> {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        overlapped_pipe_io(self.0.raw, |overlapped, transferred| unsafe {
            ReadFile(
                self.0.raw,
                Some(buffer),
                Some(transferred),
                Some(overlapped),
            )
        })
    }
}
impl Write for PipeWriter<'_> {
    fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
        overlapped_pipe_io(self.0.raw, |overlapped, transferred| unsafe {
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

fn create_pipe() -> Result<HANDLE, String> {
    let name = wide(IPC_PIPE_NAME);
    let sddl = wide("D:P(A;;GA;;;SY)(A;;GA;;;BA)(A;;GRGW;;;AU)");
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
            PIPE_ACCESS_DUPLEX | FILE_FLAG_OVERLAPPED,
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

fn connect_pipe(handle: HANDLE) -> Result<(), String> {
    // SAFETY: the event and OVERLAPPED remain live until the connect completes.
    let event = unsafe { CreateEventW(None, true, false, PCWSTR::null()) }
        .map_err(|error| format!("create pipe connect event: {error}"))?;
    let mut overlapped = OVERLAPPED {
        hEvent: event,
        ..Default::default()
    };
    let initial = unsafe { ConnectNamedPipe(handle, Some(&mut overlapped)) };
    let result = match initial {
        Ok(()) => Ok(()),
        Err(error) if error.code() == HRESULT::from_win32(ERROR_IO_PENDING.0) => {
            let mut transferred = 0;
            // SAFETY: handle, OVERLAPPED, and event remain valid while waiting.
            unsafe { GetOverlappedResult(handle, &overlapped, &mut transferred, true) }
        }
        Err(error) if error.code() == HRESULT::from_win32(ERROR_PIPE_CONNECTED.0) => Ok(()),
        Err(error) => Err(error),
    };
    // SAFETY: the connect is complete (or failed), so the event is no longer in use.
    let _ = unsafe { CloseHandle(event) };
    result.map_err(|error| format!("ConnectNamedPipe: {error}"))
}

fn overlapped_pipe_io(
    handle: HANDLE,
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
    let initial = operation(&mut overlapped, &mut transferred);
    let result = match initial {
        Ok(()) => Ok(()),
        Err(error) if error.code() == HRESULT::from_win32(ERROR_IO_PENDING.0) => unsafe {
            GetOverlappedResult(handle, &overlapped, &mut transferred, true)
        },
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

fn wake_listener() {
    let name = wide(IPC_PIPE_NAME);
    // SAFETY: this only opens the local pipe to release ConnectNamedPipe.
    if let Ok(handle) = unsafe {
        CreateFileW(
            PCWSTR(name.as_ptr()),
            (GENERIC_READ | GENERIC_WRITE).0,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            None,
            OPEN_EXISTING,
            FILE_ATTRIBUTE_NORMAL,
            None,
        )
    } {
        let _ = unsafe { CloseHandle(handle) };
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
