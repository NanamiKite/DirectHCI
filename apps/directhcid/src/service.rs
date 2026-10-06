use std::ffi::c_void;
use std::sync::Arc;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, AtomicPtr, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{
    ERROR_SERVICE_DOES_NOT_EXIST, ERROR_SERVICE_MARKED_FOR_DELETE, ERROR_SERVICE_NOT_ACTIVE,
    NO_ERROR,
};
use windows::Win32::System::Console::{
    CTRL_BREAK_EVENT, CTRL_C_EVENT, CTRL_CLOSE_EVENT, CTRL_LOGOFF_EVENT, CTRL_SHUTDOWN_EVENT,
    SetConsoleCtrlHandler,
};
use windows::Win32::System::Services::*;
use windows::core::{HRESULT, PCWSTR, PWSTR};

use directhci_windows::OfflineRecoveryStatus;

use crate::ipc;
use crate::runtime::DirectHciRuntime;

const SERVICE_NAME: &str = "DirectHCI";
// Standard DELETE access right required by DeleteService/OpenServiceW.
const DELETE_ACCESS: u32 = 0x0001_0000;
const PRESHUTDOWN_TIMEOUT_MS: u32 = 90_000;
static STOP: OnceLock<Arc<AtomicBool>> = OnceLock::new();
static STATUS_HANDLE: AtomicPtr<c_void> = AtomicPtr::new(std::ptr::null_mut());

pub fn dispatch(arguments: Vec<String>) -> Result<(), String> {
    match arguments.as_slice() {
        [] => run_console(),
        [command] if command == "run" => run_console(),
        [command] if command == "service" => run_service_dispatcher(),
        [command] if command == "boot-recovery" => crate::boot_recovery::run(),
        [command] if command == "install-service" => install_service(),
        [command] if command == "uninstall-service" => uninstall_service(),
        [command] if command == "--help" || command == "-h" => {
            println!("directhcid [run|service|boot-recovery|install-service|uninstall-service]");
            println!("  run              run in the foreground for development");
            println!("  service          enter the Windows SCM dispatcher");
            println!("  install-service  register this executable as DirectHCI (manual start)");
            println!(
                "  uninstall-service stop, recover Windows Bluetooth, then remove the service"
            );
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
    set_status(SERVICE_START_PENDING, 0, PRESHUTDOWN_TIMEOUT_MS);
    let stop = Arc::new(AtomicBool::new(false));
    let _ = STOP.set(Arc::clone(&stop));
    let runtime = DirectHciRuntime::start();
    set_status(
        SERVICE_RUNNING,
        SERVICE_ACCEPT_STOP | SERVICE_ACCEPT_PRESHUTDOWN,
        0,
    );
    let result = ipc::serve(runtime, stop);
    if let Err(error) = &result {
        eprintln!("directhcid: service runtime failed: {error}");
        report_failure(error);
    }
    set_status_with_error(SERVICE_STOPPED, 0, 0, result.is_err());
}

unsafe extern "system" fn service_handler(
    control: u32,
    _: u32,
    _: *mut c_void,
    _: *mut c_void,
) -> u32 {
    if matches!(control, SERVICE_CONTROL_STOP | SERVICE_CONTROL_PRESHUTDOWN) {
        set_status(SERVICE_STOP_PENDING, 0, PRESHUTDOWN_TIMEOUT_MS);
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
    set_status_with_error(current, accepted, wait_hint, false);
}

fn set_status_with_error(
    current: SERVICE_STATUS_CURRENT_STATE,
    accepted: u32,
    wait_hint: u32,
    failed: bool,
) {
    let raw = STATUS_HANDLE.load(Ordering::Acquire);
    if raw.is_null() {
        return;
    }
    let status = SERVICE_STATUS {
        dwServiceType: SERVICE_WIN32_OWN_PROCESS,
        dwCurrentState: current,
        dwControlsAccepted: accepted,
        dwWin32ExitCode: if failed {
            windows::Win32::Foundation::ERROR_SERVICE_SPECIFIC_ERROR.0
        } else {
            NO_ERROR.0
        },
        dwServiceSpecificExitCode: u32::from(failed),
        dwCheckPoint: u32::from(matches!(
            current,
            SERVICE_START_PENDING | SERVICE_STOP_PENDING
        )),
        dwWaitHint: wait_hint,
    };
    // SAFETY: SCM status handle remains valid for the ServiceMain lifetime.
    let _ = unsafe { SetServiceStatus(SERVICE_STATUS_HANDLE(raw), &status) };
}

fn install_service() -> Result<(), String> {
    let executable = std::env::current_exe().map_err(|error| error.to_string())?;
    directhci_windows::validate_service_executable(&executable)?;
    directhci_windows::repair_program_data_directory()?;
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
    let result = service
        .map_err(|error| format!("CreateServiceW: {error}"))
        .and_then(|service| {
            let service = ServiceHandle(service);
            let configured = (|| {
                let preshutdown = SERVICE_PRESHUTDOWN_INFO {
                    dwPreshutdownTimeout: PRESHUTDOWN_TIMEOUT_MS,
                };
                unsafe {
                    ChangeServiceConfig2W(
                        service.0,
                        SERVICE_CONFIG_PRESHUTDOWN_INFO,
                        Some((&preshutdown as *const SERVICE_PRESHUTDOWN_INFO).cast()),
                    )
                }
                .map_err(|e| format!("configure preshutdown recovery: {e}"))?;
                crate::boot_recovery::register(&executable)
            })();
            if let Err(primary) = configured {
                // This registration is new and was never started. Do not
                // leave a usable runtime missing its required boot trigger.
                return match unsafe { DeleteService(service.0) } {
                    Ok(()) => Err(format!("{primary}; new service registration rolled back")),
                    Err(cleanup) => Err(format!("{primary}; service rollback failed: {cleanup}")),
                };
            }
            Ok(())
        });
    let _ = unsafe { CloseServiceHandle(manager) };
    result.map(|_| {
        println!(
            "DirectHCI service installed (manual start), with independent SYSTEM boot recovery."
        )
    })
}

// The installer calls this before removing any executable. A failed stop or
// recovery deliberately leaves the service and journal in place for diagnosis.
fn uninstall_service() -> Result<(), String> {
    let manager = ServiceHandle(
        unsafe { OpenSCManagerW(PCWSTR::null(), PCWSTR::null(), SC_MANAGER_CONNECT) }
            .map_err(|error| format!("OpenSCManagerW: {error}"))?,
    );
    let name = wide(SERVICE_NAME);
    let mut deletion_pending = false;
    let service = match unsafe {
        OpenServiceW(
            manager.0,
            PCWSTR(name.as_ptr()),
            SERVICE_STOP | SERVICE_QUERY_STATUS | DELETE_ACCESS,
        )
    } {
        Ok(handle) => Some(ServiceHandle(handle)),
        Err(error) if error.code() == HRESULT::from_win32(ERROR_SERVICE_DOES_NOT_EXIST.0) => None,
        Err(error) if error.code() == HRESULT::from_win32(ERROR_SERVICE_MARKED_FOR_DELETE.0) => {
            deletion_pending = true;
            None
        }
        Err(error) => return Err(format!("OpenServiceW: {error}")),
    };

    if deletion_pending {
        wait_service_gone(manager.0, &name)?;
    }

    if let Some(service) = &service {
        if query_service_state(service.0)? != SERVICE_STOPPED {
            let mut status = SERVICE_STATUS::default();
            match unsafe { ControlService(service.0, SERVICE_CONTROL_STOP, &mut status) } {
                Ok(()) => {}
                Err(error) if error.code() == HRESULT::from_win32(ERROR_SERVICE_NOT_ACTIVE.0) => {}
                Err(error) => return Err(format!("ControlService(STOP): {error}")),
            }
            let deadline = Instant::now() + Duration::from_secs(90);
            while query_service_state(service.0)? != SERVICE_STOPPED {
                if Instant::now() >= deadline {
                    return Err("DirectHCI service did not stop within 90 seconds; installation files were not removed".into());
                }
                thread::sleep(Duration::from_millis(250));
            }
        }
    }

    directhci_windows::repair_program_data_directory()?;
    // A clean service stop alone does not prove that an outstanding DirectHCI
    // ownership journal was reconciled; offline recovery observes it afresh.
    let recovery = directhci_windows::recover_offline();
    if !matches!(
        &recovery.status,
        OfflineRecoveryStatus::NoJournal
            | OfflineRecoveryStatus::AlreadyWindowsOwned
            | OfflineRecoveryStatus::RestoredWindows
    ) {
        return Err(format!(
            "Windows Bluetooth recovery not confirmed ({:?}): {}; service retained; run `directhci recover --offline`",
            recovery.status,
            recovery
                .error
                .as_deref()
                .unwrap_or("inspect the ownership journal")
        ));
    }
    if let Some(service) = service {
        unsafe { DeleteService(service.0) }.map_err(|error| format!("DeleteService: {error}"))?;
        drop(service);
        wait_service_gone(manager.0, &name)?;
        println!(
            "DirectHCI service removed; offline recovery status: {:?}.",
            recovery.status
        );
    } else {
        println!(
            "DirectHCI service is not installed; offline recovery status: {:?}.",
            recovery.status
        );
    }
    // Never remove the boot recovery trigger while restoration or service
    // deletion is unresolved. A task-removal failure retains binaries too.
    crate::boot_recovery::remove()?;
    Ok(())
}

pub(crate) fn report_failure(message: &str) {
    use windows::Win32::System::EventLog::{
        DeregisterEventSource, EVENTLOG_ERROR_TYPE, RegisterEventSourceW, ReportEventW,
    };
    let source = wide("DirectHCI");
    let text = wide(message);
    // Event insertion strings remain readable even without a message DLL.
    if let Ok(handle) = unsafe { RegisterEventSourceW(PCWSTR::null(), PCWSTR(source.as_ptr())) } {
        let strings = [PCWSTR(text.as_ptr())];
        let _ = unsafe {
            ReportEventW(
                handle,
                EVENTLOG_ERROR_TYPE,
                0,
                1,
                None,
                0,
                Some(&strings),
                None,
            )
        };
        let _ = unsafe { DeregisterEventSource(handle) };
    }
}

fn wait_service_gone(manager: SC_HANDLE, name: &[u16]) -> Result<(), String> {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        match unsafe { OpenServiceW(manager, PCWSTR(name.as_ptr()), SERVICE_QUERY_STATUS) } {
            Err(error) if error.code() == HRESULT::from_win32(ERROR_SERVICE_DOES_NOT_EXIST.0) => {
                return Ok(());
            }
            Ok(service) => {
                let _ = unsafe { CloseServiceHandle(service) };
            }
            Err(error)
                if error.code() == HRESULT::from_win32(ERROR_SERVICE_MARKED_FOR_DELETE.0) => {}
            Err(error) => return Err(format!("wait for service deletion: {error}")),
        }
        if Instant::now() >= deadline {
            return Err("DirectHCI service is still pending deletion; close service-management tools and retry".into());
        }
        thread::sleep(Duration::from_millis(250));
    }
}

struct ServiceHandle(SC_HANDLE);

impl Drop for ServiceHandle {
    fn drop(&mut self) {
        // SAFETY: this wrapper owns the SCM handle returned by Open*W.
        let _ = unsafe { CloseServiceHandle(self.0) };
    }
}

fn query_service_state(service: SC_HANDLE) -> Result<SERVICE_STATUS_CURRENT_STATE, String> {
    let mut status = SERVICE_STATUS_PROCESS::default();
    let mut needed = 0;
    // SAFETY: the byte slice covers exactly the live output structure.
    let bytes = unsafe {
        std::slice::from_raw_parts_mut(
            (&mut status as *mut SERVICE_STATUS_PROCESS).cast::<u8>(),
            std::mem::size_of::<SERVICE_STATUS_PROCESS>(),
        )
    };
    unsafe { QueryServiceStatusEx(service, SC_STATUS_PROCESS_INFO, Some(bytes), &mut needed) }
        .map_err(|error| format!("QueryServiceStatusEx: {error}"))?;
    Ok(status.dwCurrentState)
}

fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}
