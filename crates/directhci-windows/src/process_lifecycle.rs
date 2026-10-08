//! Startup/installer coordination only. Never used by an active HCI session,
//! shutdown, or recovery. The names are a contract with installer/process-guard.iss.

use std::marker::PhantomData;
use std::rc::Rc;

use windows::Win32::Foundation::{
    CloseHandle, HANDLE, HLOCAL, LocalFree, WAIT_ABANDONED, WAIT_OBJECT_0, WAIT_TIMEOUT,
};
use windows::Win32::Security::Authorization::{
    ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
};
use windows::Win32::Security::{PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES};
use windows::Win32::System::Threading::{
    CreateMutexExW, MUTEX_MODIFY_STATE, ReleaseMutex, SYNCHRONIZATION_SYNCHRONIZE,
    WaitForSingleObject,
};
use windows::core::{PCWSTR, w};

const STARTUP_GATE: PCWSTR = w!("Global\\DirectHCI.StartupGate.v1");
const ACTIVE_APPLICATIONS: PCWSTR = w!("Global\\DirectHCI.ActiveApplications.v1");

struct Handle(HANDLE);

impl Drop for Handle {
    fn drop(&mut self) {
        // SAFETY: this object owns the non-inheritable kernel handle.
        let _ = unsafe { CloseHandle(self.0) };
    }
}

struct StartupGate {
    handle: Handle,
    // Windows mutex ownership is thread-affine.
    _thread: PhantomData<Rc<()>>,
}

impl Drop for StartupGate {
    fn drop(&mut self) {
        // SAFETY: constructed only after this thread acquires the mutex.
        let _ = unsafe { ReleaseMutex(self.handle.0) };
    }
}

/// Keep this token alive for the application lifetime. It is a presence marker,
/// not an owned mutex: multiple applications/sessions may run concurrently.
#[must_use]
pub struct ApplicationPresence {
    _handle: Handle,
}

/// Admit a normal application before starting workers, opening IPC, or touching
/// controllers. Setup holds the gate until it exits. Applications hold it only
/// while publishing their presence, so it never serializes normal operations.
pub fn enter_application() -> Result<ApplicationPresence, String> {
    let handle = create_mutex(STARTUP_GATE)?;
    // SAFETY: handle remains live; this startup-only wait is bounded.
    match unsafe { WaitForSingleObject(handle.0, 2_000) } {
        WAIT_OBJECT_0 | WAIT_ABANDONED => {}
        WAIT_TIMEOUT => return Err("DirectHCI installation/uninstallation or another startup is in progress. Close Setup before starting DirectHCI; no controller was changed.".into()),
        _ => return Err(format!("wait for DirectHCI startup gate: {}", windows::core::Error::from_thread())),
    }
    let _gate = StartupGate {
        handle,
        _thread: PhantomData,
    };
    let presence = create_mutex(ACTIVE_APPLICATIONS)?;
    Ok(ApplicationPresence { _handle: presence })
}

fn create_mutex(name: PCWSTR) -> Result<Handle, String> {
    // Ordinary users can run read-only CLI queries. Give authenticated users
    // only synchronize/modify-state on these coordination objects, not control
    // of their ACLs. This grants NO device, service, pipe, or journal access.
    let mut descriptor = PSECURITY_DESCRIPTOR::default();
    // SAFETY: literal is terminated; Windows allocates descriptor below.
    unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            w!("D:P(A;;GA;;;SY)(A;;GA;;;BA)(A;;0x00100001;;;AU)"),
            SDDL_REVISION_1,
            &mut descriptor,
            None,
        )
    }
    .map_err(|error| format!("create DirectHCI coordination ACL: {error}"))?;
    let attributes = SECURITY_ATTRIBUTES {
        nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: descriptor.0,
        bInheritHandle: false.into(),
    };
    // SAFETY: attributes and allocated descriptor outlive the call. Request
    // only the rights also allowed when another account created the object.
    let result = unsafe {
        CreateMutexExW(
            Some(&attributes),
            name,
            0,
            (SYNCHRONIZATION_SYNCHRONIZE | MUTEX_MODIFY_STATE).0,
        )
    };
    let _ = unsafe { LocalFree(Some(HLOCAL(descriptor.0))) };
    result.map(Handle).map_err(|error| {
        format!("open DirectHCI startup coordination (close any installer and retry): {error}")
    })
}
