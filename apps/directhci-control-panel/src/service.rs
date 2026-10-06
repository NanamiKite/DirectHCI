//! Small SCM adapter for the Control Panel. It never starts a shell command.

use windows::Win32::Foundation::ERROR_SERVICE_DOES_NOT_EXIST;
use windows::Win32::System::Services::{
    CloseServiceHandle, ControlService, OpenSCManagerW, OpenServiceW, QueryServiceStatusEx,
    SC_HANDLE, SC_MANAGER_CONNECT, SC_STATUS_PROCESS_INFO, SERVICE_CONTROL_STOP,
    SERVICE_QUERY_STATUS, SERVICE_RUNNING, SERVICE_START, SERVICE_START_PENDING, SERVICE_STATUS,
    SERVICE_STATUS_PROCESS, SERVICE_STOP, SERVICE_STOP_PENDING, SERVICE_STOPPED, StartServiceW,
};
use windows::core::{HRESULT, PCWSTR};

const SERVICE_NAME: &str = "DirectHCI";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ServiceState {
    NotInstalled,
    Stopped,
    StartPending,
    Running,
    StopPending,
    Unknown,
}

impl ServiceState {
    pub fn label(self) -> &'static str {
        match self {
            Self::NotInstalled => "Not Installed",
            Self::Stopped => "Stopped",
            Self::StartPending => "Start Pending",
            Self::Running => "Running",
            Self::StopPending => "Stop Pending",
            Self::Unknown => "Unknown",
        }
    }
}

struct ScHandle(SC_HANDLE);
impl Drop for ScHandle {
    fn drop(&mut self) {
        // SAFETY: this wrapper owns the SCM handle returned by Open*W.
        let _ = unsafe { CloseServiceHandle(self.0) };
    }
}

fn open_service(access: u32) -> Result<Option<ScHandle>, String> {
    let manager = ScHandle(
        unsafe { OpenSCManagerW(PCWSTR::null(), PCWSTR::null(), SC_MANAGER_CONNECT) }
            .map_err(|error| format!("open Service Control Manager: {error}"))?,
    );
    let name: Vec<u16> = SERVICE_NAME.encode_utf16().chain(Some(0)).collect();
    match unsafe { OpenServiceW(manager.0, PCWSTR(name.as_ptr()), access) } {
        Ok(handle) => Ok(Some(ScHandle(handle))),
        Err(error) if error.code() == HRESULT::from_win32(ERROR_SERVICE_DOES_NOT_EXIST.0) => {
            Ok(None)
        }
        Err(error) => Err(format!("open DirectHCI service: {error}")),
    }
}

pub fn query() -> Result<ServiceState, String> {
    query_with_failure().map(|(state, _)| state)
}

pub fn query_with_failure() -> Result<(ServiceState, Option<String>), String> {
    let Some(status) = query_status()? else {
        return Ok((ServiceState::NotInstalled, None));
    };
    let failure = (status.dwCurrentState == SERVICE_STOPPED && status.dwWin32ExitCode != 0)
        .then(|| format!("Service stopped with error {} (service-specific {}); Windows Bluetooth recovery was not confirmed. See Windows Event Viewer and recovery diagnostics.", status.dwWin32ExitCode, status.dwServiceSpecificExitCode));
    Ok((
        match status.dwCurrentState {
            SERVICE_STOPPED => ServiceState::Stopped,
            SERVICE_START_PENDING => ServiceState::StartPending,
            SERVICE_RUNNING => ServiceState::Running,
            SERVICE_STOP_PENDING => ServiceState::StopPending,
            _ => ServiceState::Unknown,
        },
        failure,
    ))
}

fn query_status() -> Result<Option<SERVICE_STATUS_PROCESS>, String> {
    let Some(service) = open_service(SERVICE_QUERY_STATUS)? else {
        return Ok(None);
    };
    let mut status = SERVICE_STATUS_PROCESS::default();
    let mut needed = 0;
    // SAFETY: the mutable byte slice is exactly the live status structure.
    let bytes = unsafe {
        std::slice::from_raw_parts_mut(
            (&mut status as *mut SERVICE_STATUS_PROCESS).cast::<u8>(),
            std::mem::size_of::<SERVICE_STATUS_PROCESS>(),
        )
    };
    unsafe { QueryServiceStatusEx(service.0, SC_STATUS_PROCESS_INFO, Some(bytes), &mut needed) }
        .map_err(|error| format!("query DirectHCI service: {error}"))?;
    Ok(Some(status))
}

pub fn start() -> Result<(), String> {
    let service = open_service(SERVICE_START)?.ok_or("DirectHCI service is not installed")?;
    unsafe { StartServiceW(service.0, None) }
        .map_err(|error| format!("start DirectHCI service (administrator required): {error}"))
}

pub fn stop() -> Result<(), String> {
    let service = open_service(SERVICE_STOP)?.ok_or("DirectHCI service is not installed")?;
    let mut status = SERVICE_STATUS::default();
    unsafe { ControlService(service.0, SERVICE_CONTROL_STOP, &mut status) }
        .map_err(|error| format!("stop DirectHCI service (administrator required): {error}"))
}
