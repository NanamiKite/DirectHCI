//! Recovery-only boot task. The interactive runtime remains demand-start.
use std::path::Path;
use std::thread;
use std::time::{Duration, Instant};

use directhci_windows::{OfflineRecoveryStatus, recover_offline};
use windows::Win32::Foundation::ERROR_FILE_NOT_FOUND;
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, CoUninitialize,
};
use windows::Win32::System::TaskScheduler::{
    ITaskService, TASK_CREATE_OR_UPDATE, TASK_LOGON_SERVICE_ACCOUNT, TaskScheduler,
};
use windows::Win32::System::Variant::VARIANT;
use windows::core::{BSTR, HRESULT};

const TASK_NAME: &str = "DirectHCI Boot Recovery";

pub fn run() -> Result<(), String> {
    // USB devnodes may not yet be ready. Retry only within a bounded boot
    // recovery window; the mutation lock prevents racing a live daemon.
    let deadline = Instant::now() + Duration::from_secs(90);
    loop {
        let report = recover_offline();
        if matches!(
            report.status,
            OfflineRecoveryStatus::NoJournal
                | OfflineRecoveryStatus::AlreadyWindowsOwned
                | OfflineRecoveryStatus::RestoredWindows
        ) {
            return Ok(());
        }
        let transient = matches!(report.status, OfflineRecoveryStatus::DeviceMissing)
            || report
                .error
                .as_deref()
                .is_some_and(|s| s.starts_with("controller mutation is busy:"));
        if transient && Instant::now() < deadline {
            thread::sleep(Duration::from_secs(2));
            continue;
        }
        let error = format!(
            "boot recovery not confirmed ({:?}): {}; journal retained",
            report.status,
            report
                .error
                .as_deref()
                .unwrap_or("inspect recovery journal")
        );
        crate::service::report_failure(&error);
        return Err(error);
    }
}

pub fn register(executable: &Path) -> Result<(), String> {
    // This is a SYSTEM task: never register an executable in a user-writable
    // or installer-temporary directory.
    directhci_windows::validate_service_executable(executable)?;
    with_scheduler(|service| {
        let folder = unsafe { service.GetFolder(&BSTR::from("\\")) }?;
        // The XML LogonType enumeration has no ServiceAccount value. Configure
        // SYSTEM service-account logon through RegisterTask below instead;
        // including it in XML causes SCHED_E_INVALIDVALUE (0x80041318).
        let xml = format!(
            r#"<?xml version="1.0" encoding="UTF-16"?>
<Task version="1.2" xmlns="http://schemas.microsoft.com/windows/2004/02/mit/task">
  <RegistrationInfo><Description>Reconcile a retained DirectHCI ownership journal after boot; never acquire a controller or start the interactive runtime.</Description></RegistrationInfo>
  <Triggers><BootTrigger><Enabled>true</Enabled><Delay>PT10S</Delay></BootTrigger></Triggers>
  <Principals><Principal id="System"><UserId>S-1-5-18</UserId><RunLevel>HighestAvailable</RunLevel></Principal></Principals>
  <Settings><MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy><DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries><StopIfGoingOnBatteries>false</StopIfGoingOnBatteries><StartWhenAvailable>true</StartWhenAvailable><RunOnlyIfNetworkAvailable>false</RunOnlyIfNetworkAvailable><IdleSettings><StopOnIdleEnd>false</StopOnIdleEnd><RestartOnIdle>false</RestartOnIdle></IdleSettings><Enabled>true</Enabled><Hidden>false</Hidden><ExecutionTimeLimit>PT3M</ExecutionTimeLimit></Settings>
  <Actions Context="System"><Exec><Command>{}</Command><Arguments>boot-recovery</Arguments></Exec></Actions>
</Task>"#,
            escape_xml(&executable.to_string_lossy())
        );
        // Protected task DACL: no ordinary user may edit its action.
        let _task = unsafe {
            folder.RegisterTask(
                &BSTR::from(TASK_NAME),
                &BSTR::from(xml),
                TASK_CREATE_OR_UPDATE.0,
                &VARIANT::from("SYSTEM"),
                &VARIANT::default(),
                TASK_LOGON_SERVICE_ACCOUNT,
                &VARIANT::from("O:BAD:P(A;;GA;;;SY)(A;;GA;;;BA)"),
            )
        }?;
        Ok(())
    })
}

pub fn remove() -> Result<(), String> {
    with_scheduler(|service| {
        let folder = unsafe { service.GetFolder(&BSTR::from("\\")) }?;
        match unsafe { folder.DeleteTask(&BSTR::from(TASK_NAME), 0) } {
            Ok(()) => Ok(()),
            Err(e) if e.code() == HRESULT::from_win32(ERROR_FILE_NOT_FOUND.0) => Ok(()),
            Err(e) => Err(e),
        }
    })
}

struct Com;
impl Drop for Com {
    fn drop(&mut self) {
        unsafe { CoUninitialize() };
    }
}

fn with_scheduler(
    action: impl FnOnce(&ITaskService) -> windows::core::Result<()>,
) -> Result<(), String> {
    unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }
        .ok()
        .map_err(|e| format!("initialize boot-recovery COM: {e}"))?;
    let _com = Com;
    let service: ITaskService =
        unsafe { CoCreateInstance(&TaskScheduler, None, CLSCTX_INPROC_SERVER) }
            .map_err(|e| format!("open Task Scheduler: {e}"))?;
    let empty = VARIANT::default();
    unsafe { service.Connect(&empty, &empty, &empty, &empty) }
        .map_err(|e| format!("connect Task Scheduler: {e}"))?;
    action(&service).map_err(|e| format!("DirectHCI boot-recovery task: {e}"))
}

fn escape_xml(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}
