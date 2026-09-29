use std::ffi::c_void;
use std::sync::Arc;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, AtomicPtr, Ordering};

use windows::Win32::Foundation::NO_ERROR;
use windows::Win32::System::Console::{
    CTRL_BREAK_EVENT, CTRL_C_EVENT, CTRL_CLOSE_EVENT, CTRL_LOGOFF_EVENT, CTRL_SHUTDOWN_EVENT,
    SetConsoleCtrlHandler,
};
use windows::Win32::System::Services::*;
use windows::core::{PCWSTR, PWSTR};

use crate::ipc;
use crate::runtime::DirectHciRuntime;

const SERVICE_NAME: &str = "DirectHCI";
static STOP: OnceLock<Arc<AtomicBool>> = OnceLock::new();
static STATUS_HANDLE: AtomicPtr<c_void> = AtomicPtr::new(std::ptr::null_mut());

pub fn dispatch(arguments: Vec<String>) -> Result<(), String> {
    match arguments.as_slice() {
        [] => run_console(),
        [command] if command == "run" => run_console(),
        [command] if command == "service" => run_service_dispatcher(),
        [command] if command == "install-service" => install_service(),
        [command] if command == "--help" || command == "-h" => {
            println!("directhcid [run|service|install-service]");
            println!("  run              run in the foreground for development");
            println!("  service          enter the Windows SCM dispatcher");
            println!("  install-service  register this executable as DirectHCI (manual start)");
            Ok(())
        }
        _ => Err("invalid command; run `directhcid --help`".into()),
    }
}

fn run_console() -> Result<(), String> {
    let stop = Arc::new(AtomicBool::new(false));
    let _ = STOP.set(Arc::clone(&stop));
    // SAFETY: the callback only sets an atomic flag whose Arc is process-global.
    unsafe { SetConsoleCtrlHandler(Some(console_handler), true) }
        .map_err(|error| format!("SetConsoleCtrlHandler: {error}"))?;
    eprintln!(
        "directhcid: console runtime starting; pipe={}",
        directhci_core::IPC_PIPE_NAME
    );
    let result = ipc::serve(DirectHciRuntime::start(), stop);
    let _ = unsafe { SetConsoleCtrlHandler(Some(console_handler), false) };
    result
}

fn run_service_dispatcher() -> Result<(), String> {
    let mut service_name = wide(SERVICE_NAME);
    let table = [
        SERVICE_TABLE_ENTRYW {
            lpServiceName: PWSTR(service_name.as_mut_ptr()),
            lpServiceProc: Some(service_main),
        },
        SERVICE_TABLE_ENTRYW::default(),
    ];
    // SAFETY: table and service name remain valid until the dispatcher returns.
    unsafe { StartServiceCtrlDispatcherW(table.as_ptr()) }
        .map_err(|error| format!("StartServiceCtrlDispatcherW: {error}"))
}

unsafe extern "system" fn service_main(_: u32, _: *mut PWSTR) {
    let service_name = wide(SERVICE_NAME);
    // SAFETY: called by SCM in ServiceMain; callback has static lifetime.
    let handle = match unsafe {
        RegisterServiceCtrlHandlerExW(PCWSTR(service_name.as_ptr()), Some(service_handler), None)
    } {
        Ok(handle) => handle,
        Err(error) => {
            eprintln!("directhcid: register service handler: {error}");
            return;
        }
    };
    STATUS_HANDLE.store(handle.0, Ordering::Release);
    set_status(SERVICE_START_PENDING, 0, 30_000);
    let stop = Arc::new(AtomicBool::new(false));
    let _ = STOP.set(Arc::clone(&stop));
    let runtime = DirectHciRuntime::start();
    set_status(SERVICE_RUNNING, SERVICE_ACCEPT_STOP, 0);
    if let Err(error) = ipc::serve(runtime, stop) {
        eprintln!("directhcid: service runtime failed: {error}");
    }
    set_status(SERVICE_STOPPED, 0, 0);
}

unsafe extern "system" fn service_handler(
    control: u32,
    _: u32,
    _: *mut c_void,
    _: *mut c_void,
) -> u32 {
    if control == SERVICE_CONTROL_STOP {
        set_status(SERVICE_STOP_PENDING, 0, 45_000);
        request_stop();
    }
    NO_ERROR.0
}

unsafe extern "system" fn console_handler(control: u32) -> windows::core::BOOL {
    if matches!(
        control,
        CTRL_C_EVENT
            | CTRL_BREAK_EVENT
            | CTRL_CLOSE_EVENT
            | CTRL_LOGOFF_EVENT
            | CTRL_SHUTDOWN_EVENT
    ) {
        request_stop();
        true.into()
    } else {
        false.into()
    }
}

fn request_stop() {
    if let Some(stop) = STOP.get() {
        stop.store(true, Ordering::Release);
    }
}

fn set_status(current: SERVICE_STATUS_CURRENT_STATE, accepted: u32, wait_hint: u32) {
    let raw = STATUS_HANDLE.load(Ordering::Acquire);
    if raw.is_null() {
        return;
    }
    let status = SERVICE_STATUS {
        dwServiceType: SERVICE_WIN32_OWN_PROCESS,
        dwCurrentState: current,
        dwControlsAccepted: accepted,
        dwWin32ExitCode: NO_ERROR.0,
        dwServiceSpecificExitCode: 0,
        dwCheckPoint: 0,
        dwWaitHint: wait_hint,
    };
    // SAFETY: SCM status handle remains valid for the ServiceMain lifetime.
    let _ = unsafe { SetServiceStatus(SERVICE_STATUS_HANDLE(raw), &status) };
}

fn install_service() -> Result<(), String> {
    let executable = std::env::current_exe().map_err(|error| error.to_string())?;
    let binary_path = format!("\"{}\" service", executable.display());
    let service_name = wide(SERVICE_NAME);
    let display_name = wide("DirectHCI Runtime");
    let binary_path = wide(&binary_path);
    // SAFETY: null machine/database select the local active SCM database.
    let manager =
        unsafe { OpenSCManagerW(PCWSTR::null(), PCWSTR::null(), SC_MANAGER_CREATE_SERVICE) }
            .map_err(|error| format!("OpenSCManagerW: {error}"))?;
    // SAFETY: all string buffers remain valid through the call. Null optional
    // strings select LocalSystem and no dependencies/load-order group.
    let service = unsafe {
        CreateServiceW(
            manager,
            PCWSTR(service_name.as_ptr()),
            PCWSTR(display_name.as_ptr()),
            SERVICE_ALL_ACCESS,
            SERVICE_WIN32_OWN_PROCESS,
            SERVICE_DEMAND_START,
            SERVICE_ERROR_NORMAL,
            PCWSTR(binary_path.as_ptr()),
            PCWSTR::null(),
            None,
            PCWSTR::null(),
            PCWSTR::null(),
            PCWSTR::null(),
        )
    };
    let result = service.map_err(|error| format!("CreateServiceW: {error}"));
    if let Ok(service) = result.as_ref() {
        let _ = unsafe { CloseServiceHandle(*service) };
    }
    let _ = unsafe { CloseServiceHandle(manager) };
    result.map(|_| println!("DirectHCI service installed (manual start)."))
}

fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}
