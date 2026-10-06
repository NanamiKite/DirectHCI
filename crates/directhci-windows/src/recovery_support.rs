//! Small Windows recovery primitives. No transport or driver-selection policy.

use directhci_core::OwnershipJournal;
#[cfg(windows)]
use directhci_core::{DeviceSecurityBaseline, LeaseOwnerMetadata};

#[cfg(windows)]
pub(crate) use platform::*;

#[cfg(not(windows))]
pub(crate) struct MutationGuard;

#[cfg(not(windows))]
impl MutationGuard {
    pub(crate) fn acquire() -> Result<Self, String> {
        Err("controller mutation requires Windows".into())
    }
}

#[cfg(not(windows))]
pub(crate) fn capture_recovery_baseline(_: &mut OwnershipJournal) -> Result<(), String> {
    Err("recovery baseline requires Windows".into())
}

#[cfg(not(windows))]
pub(crate) fn owner_still_running(_: &OwnershipJournal) -> Result<bool, String> {
    Err("journal owner verification requires Windows".into())
}

#[cfg(not(windows))]
pub(crate) fn restore_device_security(_: &OwnershipJournal, _: &str) -> Result<(), String> {
    Err("device security recovery requires Windows".into())
}

#[cfg(not(windows))]
pub(crate) fn verify_device_security(_: &OwnershipJournal, _: &str) -> Result<(), String> {
    Err("device security verification requires Windows".into())
}

#[cfg(windows)]
mod platform {
    use super::*;
    use std::mem::size_of;
    use windows::Win32::Devices::DeviceAndDriverInstallation::*;
    use windows::Win32::Foundation::{
        CloseHandle, ERROR_INVALID_DATA, ERROR_INVALID_PARAMETER, ERROR_NOT_FOUND,
        ERROR_SHARING_VIOLATION, FILETIME, HANDLE, HLOCAL, LocalFree, NTSTATUS, STILL_ACTIVE,
    };
    use windows::Win32::Security::Authorization::{
        ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
    };
    use windows::Win32::Security::{PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES};
    use windows::Win32::Storage::FileSystem::{
        CreateFileW, FILE_FLAG_OPEN_REPARSE_POINT, FILE_GENERIC_READ, FILE_GENERIC_WRITE,
        FILE_SHARE_MODE, OPEN_ALWAYS,
    };
    use windows::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};
    use windows::Win32::System::Threading::{
        GetExitCodeProcess, GetProcessTimes, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
    };
    use windows::core::{GUID, HRESULT, PCWSTR, s, w};

    /// Share-denied file in the protected state directory, also serializing
    /// boot recovery against a daemon/CLI that starts at the same time.
    pub(crate) struct MutationGuard(HANDLE);
    // The handle owns no thread-affine state and is kept through lease release.
    unsafe impl Send for MutationGuard {}
    unsafe impl Sync for MutationGuard {}

    impl MutationGuard {
        pub(crate) fn acquire() -> Result<Self, String> {
            let store = crate::JournalStore::program_data().map_err(|e| e.to_string())?;
            let directory = store
                .path()
                .parent()
                .ok_or("missing state directory")?
                .to_owned();
            crate::security::ensure_secure_journal_directory(&directory)?;
            let path = directory.join("controller-mutation.lock");
            if path.exists() {
                crate::security::validate_secure_data_file(&path)?;
            }
            let name = wide(&path.to_string_lossy());
            let mut descriptor = PSECURITY_DESCRIPTOR::default();
            unsafe {
                ConvertStringSecurityDescriptorToSecurityDescriptorW(
                    w!("O:BAD:P(A;;GA;;;SY)(A;;GA;;;BA)"),
                    SDDL_REVISION_1,
                    &mut descriptor,
                    None,
                )
            }
            .map_err(|e| format!("mutation lock security: {e}"))?;
            let attributes = SECURITY_ATTRIBUTES {
                nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
                lpSecurityDescriptor: descriptor.0,
                bInheritHandle: false.into(),
            };
            // No sharing: a crash releases the kernel lock without deleting
            // the durable journal. OPEN_REPARSE_POINT never follows a link.
            let opened = unsafe {
                CreateFileW(
                    PCWSTR(name.as_ptr()),
                    FILE_GENERIC_READ.0 | FILE_GENERIC_WRITE.0,
                    FILE_SHARE_MODE(0),
                    Some(&attributes),
                    OPEN_ALWAYS,
                    FILE_FLAG_OPEN_REPARSE_POINT,
                    None,
                )
            };
            let _ = unsafe { LocalFree(Some(HLOCAL(descriptor.0))) };
            let handle = opened.map_err(|e| {
                if e.code() == HRESULT::from_win32(ERROR_SHARING_VIOLATION.0) {
                    format!("controller mutation is busy: {e}")
                } else {
                    format!("controller mutation unavailable: {e}")
                }
            })?;
            let guard = Self(handle);
            crate::security::validate_secure_data_file(&path)?;
            Ok(guard)
        }
    }

    impl Drop for MutationGuard {
        fn drop(&mut self) {
            let _ = unsafe { CloseHandle(self.0) };
        }
    }

    /// SystemBootEnvironmentInformation (0x5a), Windows kernel boot GUID.
    /// This NT information class is not a stable public Win32 contract. Use
    /// dynamic lookup, bound the buffer, validate length, and fail closed if
    /// unavailable. Never substitute wall-clock minus uptime for a boot GUID.
    fn boot_identifier() -> Result<String, String> {
        #[repr(C)]
        #[derive(Default)]
        struct BootEnvironment {
            identifier: GUID,
            firmware_type: u32,
            flags: u64,
        }
        type Query =
            unsafe extern "system" fn(i32, *mut std::ffi::c_void, u32, *mut u32) -> NTSTATUS;
        let module = unsafe { GetModuleHandleW(w!("ntdll.dll")) }.map_err(|e| e.to_string())?;
        let address = unsafe { GetProcAddress(module, s!("NtQuerySystemInformation")) }
            .ok_or("kernel boot identifier query unavailable")?;
        // SAFETY: ntdll exports this exact NT system-information ABI.
        let query: Query = unsafe { std::mem::transmute(address) };
        let mut value = BootEnvironment::default();
        let mut length = 0;
        let status = unsafe {
            query(
                0x5a,
                (&mut value as *mut BootEnvironment).cast(),
                size_of::<BootEnvironment>() as u32,
                &mut length,
            )
        };
        if status.0 < 0
            || length < 20
            || length as usize > size_of::<BootEnvironment>()
            || value.identifier == GUID::zeroed()
        {
            return Err(format!(
                "cannot verify kernel boot identifier: NTSTATUS {:#010x}, length={length}",
                status.0
            ));
        }
        Ok(format!("{:?}", value.identifier))
    }

    fn process_creation_time(pid: u32) -> Result<Option<u64>, String> {
        let handle = match unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) } {
            Ok(h) => h,
            Err(e) if e.code() == HRESULT::from_win32(ERROR_INVALID_PARAMETER.0) => {
                return Ok(None);
            }
            Err(e) => return Err(format!("inspect journal process {pid}: {e}")),
        };
        let mut creation = FILETIME::default();
        let mut exit = FILETIME::default();
        let mut kernel = FILETIME::default();
        let mut user = FILETIME::default();
        let mut code = 0;
        let result = (|| {
            unsafe { GetProcessTimes(handle, &mut creation, &mut exit, &mut kernel, &mut user) }
                .map_err(|e| e.to_string())?;
            unsafe { GetExitCodeProcess(handle, &mut code) }.map_err(|e| e.to_string())?;
            Ok((code == STILL_ACTIVE.0 as u32).then_some(
                ((creation.dwHighDateTime as u64) << 32) | creation.dwLowDateTime as u64,
            ))
        })();
        let _ = unsafe { CloseHandle(handle) };
        result
    }

    pub(crate) fn capture_recovery_baseline(journal: &mut OwnershipJournal) -> Result<(), String> {
        journal.owner = LeaseOwnerMetadata {
            boot_identifier: Some(boot_identifier()?),
            process_creation_time: Some(
                process_creation_time(std::process::id())?.ok_or("current journal owner exited")?,
            ),
            ..journal.owner.clone()
        };
        let security_sddl = device_security(&journal.controller_identity.instance_id)?;
        if security_sddl.as_deref() == Some("D:P(A;;GA;;;SY)(A;;GA;;;BA)") {
            return Err("Windows Bluetooth still has the legacy DirectHCI-only device security override; repair it before another takeover".into());
        }
        journal.device_security_baseline = Some(DeviceSecurityBaseline { security_sddl });
        Ok(())
    }

    pub(crate) fn reboot_completed(journal: &OwnershipJournal) -> Result<bool, String> {
        match &journal.owner.boot_identifier {
            Some(previous) => {
                let previous = GUID::try_from(previous.as_str())
                    .map_err(|_| "invalid boot identifier in recovery journal")?;
                if previous == GUID::zeroed() {
                    return Err("empty boot identifier in recovery journal".into());
                }
                let current = boot_identifier()?;
                let current = GUID::try_from(current.as_str())
                    .map_err(|_| "invalid current boot identifier")?;
                Ok(previous != current)
            }
            // Legacy journals have no proof of boot identity. Keep conservative
            // reconciliation; never infer a restart from an unrelated PID.
            None => Ok(false),
        }
    }

    pub(crate) fn owner_still_running(journal: &OwnershipJournal) -> Result<bool, String> {
        // Boot comparison MUST precede PID access (even access-denied/reuse).
        if reboot_completed(journal)? {
            return Ok(false);
        }
        let Some(pid) = journal.owner.process_id else {
            return Ok(false);
        };
        let Some(actual) = process_creation_time(pid)? else {
            return Ok(false);
        };
        match journal.owner.process_creation_time {
            Some(expected) => Ok(actual == expected),
            // Legacy records cannot prove the current PID belongs to a
            // different owner. Wall-clock comparisons are not sufficient
            // evidence (clock adjustments), so do not authorize by guessing.
            None => Ok(true),
        }
    }

    struct DeviceSet(HDEVINFO);
    impl Drop for DeviceSet {
        fn drop(&mut self) {
            let _ = unsafe { SetupDiDestroyDeviceInfoList(self.0) };
        }
    }

    fn with_device<T>(
        instance: &str,
        action: impl FnOnce(HDEVINFO, &mut SP_DEVINFO_DATA) -> Result<T, String>,
    ) -> Result<T, String> {
        let set = DeviceSet(
            unsafe { SetupDiCreateDeviceInfoList(None, None) }.map_err(|e| e.to_string())?,
        );
        let mut info = SP_DEVINFO_DATA {
            cbSize: size_of::<SP_DEVINFO_DATA>() as u32,
            ..Default::default()
        };
        let name = wide(instance);
        unsafe { SetupDiOpenDeviceInfoW(set.0, PCWSTR(name.as_ptr()), None, 0, Some(&mut info)) }
            .map_err(|e| format!("open recovery devnode: {e}"))?;
        action(set.0, &mut info)
    }

    fn device_security(instance: &str) -> Result<Option<String>, String> {
        with_device(instance, |set, info| {
            let mut size = 0;
            let mut kind = 0;
            match unsafe {
                SetupDiGetDeviceRegistryPropertyW(
                    set,
                    info,
                    SPDRP_SECURITY_SDS,
                    Some(&mut kind),
                    None,
                    Some(&mut size),
                )
            } {
                Err(e)
                    if e.code() == HRESULT::from_win32(ERROR_INVALID_DATA.0)
                        || e.code() == HRESULT::from_win32(ERROR_NOT_FOUND.0) =>
                {
                    return Ok(None);
                }
                Err(e)
                    if e.code()
                        == HRESULT::from_win32(
                            windows::Win32::Foundation::ERROR_INSUFFICIENT_BUFFER.0,
                        ) => {}
                Err(e) => return Err(format!("read original device security: {e}")),
                Ok(()) => return Err("empty device security property".into()),
            }
            if size == 0 || size > 64 * 1024 || size % 2 != 0 {
                return Err("invalid device security property length".into());
            }
            let mut bytes = vec![0u8; size as usize];
            unsafe {
                SetupDiGetDeviceRegistryPropertyW(
                    set,
                    info,
                    SPDRP_SECURITY_SDS,
                    Some(&mut kind),
                    Some(&mut bytes),
                    Some(&mut size),
                )
            }
            .map_err(|e| format!("read device security: {e}"))?;
            if kind != 1 || size as usize > bytes.len() || size % 2 != 0 {
                return Err("invalid device security property type".into());
            }
            let words: Vec<u16> = bytes[..size as usize]
                .chunks_exact(2)
                .map(|b| u16::from_le_bytes([b[0], b[1]]))
                .collect();
            if words.last() != Some(&0) {
                return Err("unterminated device security SDDL".into());
            }
            String::from_utf16(&words[..words.len() - 1])
                .map(Some)
                .map_err(|e| e.to_string())
        })
    }

    pub(crate) fn verify_device_security(
        journal: &OwnershipJournal,
        instance: &str,
    ) -> Result<(), String> {
        let current = device_security(instance)?;
        match &journal.device_security_baseline {
            Some(baseline) if current == baseline.security_sddl => Ok(()),
            Some(_) => Err("original device security has not been restored; journal retained".into()),
            None if current.as_deref() == Some("D:P(A;;GA;;;SY)(A;;GA;;;BA)") => Err("legacy journal has no original device security baseline and the DirectHCI-only override remains; explicit repair required; journal retained".into()),
            None => Ok(()),
        }
    }

    pub(crate) fn restore_device_security(
        journal: &OwnershipJournal,
        instance: &str,
    ) -> Result<(), String> {
        let Some(baseline) = &journal.device_security_baseline else {
            return verify_device_security(journal, instance);
        };
        if device_security(instance)? == baseline.security_sddl {
            return Ok(());
        }
        with_device(instance, |set, info| {
            match &baseline.security_sddl {
                Some(sddl) => {
                    let bytes: Vec<u8> =
                        wide(sddl).into_iter().flat_map(u16::to_le_bytes).collect();
                    unsafe {
                        SetupDiSetDeviceRegistryPropertyW(
                            set,
                            info,
                            SPDRP_SECURITY_SDS,
                            Some(&bytes),
                        )
                    }
                    .map_err(|e| format!("restore original device security: {e}"))?;
                }
                None => {
                    // NULL/0 deletes the explicit property. It does not set a
                    // NULL DACL and does not guess an Everyone-access policy.
                    unsafe { SetupDiSetDeviceRegistryPropertyW(set, info, SPDRP_SECURITY, None) }
                        .map_err(|e| format!("remove takeover device security override: {e}"))?;
                }
            }
            Ok(())
        })?;
        verify_device_security(journal, instance)
    }

    fn wide(value: &str) -> Vec<u16> {
        value.encode_utf16().chain(Some(0)).collect()
    }
}
