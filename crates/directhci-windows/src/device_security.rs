//! Device permissions belong to the ownership lease, not to a driver name.
//!
//! Save both the persistent devnode override and the effective BTHPORT DACL.
//! Deleting an INF/override alone does not establish the live object's rights.
//! Never synthesize an ACL, add a user/service SID, or edit a setup-class ACL.

use directhci_core::{ControllerObservation, ControllerSecurityBaseline};

pub(crate) fn capture(
    controller: &ControllerObservation,
) -> Result<ControllerSecurityBaseline, String> {
    platform::capture(controller)
}

pub(crate) fn restore_override(
    controller: &ControllerObservation,
    baseline: &ControllerSecurityBaseline,
) -> Result<(), String> {
    platform::restore_override(controller, baseline)
}

pub(crate) fn restore_radio(
    controller: &ControllerObservation,
    baseline: &ControllerSecurityBaseline,
) -> Result<(), String> {
    platform::restore_radio(controller, baseline)
}

pub(crate) fn verify(
    controller: &ControllerObservation,
    baseline: &ControllerSecurityBaseline,
) -> Result<(), String> {
    platform::verify(controller, baseline)
}

/// Read-only recovery readiness probe. None means the exact controller has
/// zero present BTHPORT interfaces, not an ACL mismatch. Native API failures,
/// ambiguous interfaces and identity changes remain errors, not retry hints.
/// Compare both the persistent override and live radio security; a read error
/// must not be treated as a mismatch that triggers permission writes/restart.
pub(crate) fn try_matches_baseline(
    controller: &ControllerObservation,
    baseline: &ControllerSecurityBaseline,
) -> Result<Option<bool>, String> {
    platform::try_matches_baseline(controller, baseline)
}

#[cfg(not(windows))]
mod platform {
    use super::*;
    pub(super) fn capture(_: &ControllerObservation) -> Result<ControllerSecurityBaseline, String> {
        Err("controller security requires Windows".into())
    }
    pub(super) fn restore_override(
        _: &ControllerObservation,
        _: &ControllerSecurityBaseline,
    ) -> Result<(), String> {
        Err("controller security requires Windows".into())
    }
    pub(super) fn restore_radio(
        _: &ControllerObservation,
        _: &ControllerSecurityBaseline,
    ) -> Result<(), String> {
        Err("controller security requires Windows".into())
    }
    pub(super) fn verify(
        _: &ControllerObservation,
        _: &ControllerSecurityBaseline,
    ) -> Result<(), String> {
        Err("controller security requires Windows".into())
    }
    pub(super) fn try_matches_baseline(
        _: &ControllerObservation,
        _: &ControllerSecurityBaseline,
    ) -> Result<Option<bool>, String> {
        Err("controller security requires Windows".into())
    }
}

#[cfg(windows)]
mod platform {
    use super::*;
    use std::ptr;
    use windows::Win32::Devices::DeviceAndDriverInstallation::{
        CM_GET_DEVICE_INTERFACE_LIST_PRESENT, CM_Get_Device_Interface_List_SizeW,
        CM_Get_Device_Interface_ListW, CR_BUFFER_SMALL, CR_SUCCESS, HDEVINFO, SP_DEVINFO_DATA,
        SPDRP_SECURITY, SetupDiCreateDeviceInfoList, SetupDiDestroyDeviceInfoList,
        SetupDiGetDevicePropertyW, SetupDiOpenDeviceInfoW, SetupDiSetDevicePropertyW,
        SetupDiSetDeviceRegistryPropertyW,
    };
    use windows::Win32::Devices::Properties::{
        DEVPKEY_Device_SecuritySDS, DEVPROP_TYPE_SECURITY_DESCRIPTOR_STRING, DEVPROPTYPE,
    };
    use windows::Win32::Foundation::{
        CloseHandle, ERROR_INSUFFICIENT_BUFFER, ERROR_NOT_FOUND, HANDLE, HLOCAL, LocalFree,
    };
    use windows::Win32::Security::Authorization::{
        ConvertSecurityDescriptorToStringSecurityDescriptorW,
        ConvertStringSecurityDescriptorToSecurityDescriptorW, GetSecurityInfo, SDDL_REVISION_1,
        SE_FILE_OBJECT, SetSecurityInfo,
    };
    use windows::Win32::Security::{
        ACE_HEADER, ACL, DACL_SECURITY_INFORMATION, GROUP_SECURITY_INFORMATION, GetAce,
        GetSecurityDescriptorControl, GetSecurityDescriptorDacl, IsValidAcl,
        OBJECT_SECURITY_INFORMATION, OWNER_SECURITY_INFORMATION,
        PROTECTED_DACL_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, SACL_SECURITY_INFORMATION,
        UNPROTECTED_DACL_SECURITY_INFORMATION,
    };
    use windows::Win32::Storage::FileSystem::{
        CreateFileW, FILE_FLAGS_AND_ATTRIBUTES, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
        READ_CONTROL, WRITE_DAC,
    };
    use windows::core::{BOOL, GUID, HRESULT, PCWSTR, PWSTR};

    const RADIO_GUID: GUID = GUID::from_u128(0x0850302a_b344_4fda_9be9_90576b8d46f0);
    const DIRECTHCI_DACL: &str = "D:P(A;;GA;;;SY)(A;;GA;;;BA)";
    const MAX_SECURITY_BYTES: usize = 128 * 1024;
    const RADIO_INFO: OBJECT_SECURITY_INFORMATION = OBJECT_SECURITY_INFORMATION(1 | 2 | 4);

    struct Descriptor(PSECURITY_DESCRIPTOR);
    impl Drop for Descriptor {
        fn drop(&mut self) {
            unsafe {
                let _ = LocalFree(Some(HLOCAL(self.0.0)));
            }
        }
    }
    struct Radio(HANDLE);
    impl Drop for Radio {
        fn drop(&mut self) {
            unsafe {
                let _ = CloseHandle(self.0);
            }
        }
    }
    struct Device(HDEVINFO, SP_DEVINFO_DATA);
    impl Drop for Device {
        fn drop(&mut self) {
            unsafe {
                let _ = SetupDiDestroyDeviceInfoList(self.0);
            }
        }
    }

    fn wide(text: &str) -> Result<Vec<u16>, String> {
        if text.contains('\0') || text.len() > MAX_SECURITY_BYTES {
            return Err("invalid or oversized controller security input".into());
        }
        Ok(text.encode_utf16().chain(Some(0)).collect())
    }

    impl Descriptor {
        fn parse(sddl: &str) -> Result<Self, String> {
            let text = wide(sddl)?;
            let mut sd = PSECURITY_DESCRIPTOR::default();
            // The native parser validates the bounded, terminated string;
            // no offsets/pointers from journal bytes are dereferenced.
            unsafe {
                ConvertStringSecurityDescriptorToSecurityDescriptorW(
                    PCWSTR(text.as_ptr()),
                    SDDL_REVISION_1,
                    &mut sd,
                    None,
                )
            }
            .map_err(|e| format!("parse saved controller security: {e}"))?;
            Ok(Self(sd))
        }

        fn text(&self, info: OBJECT_SECURITY_INFORMATION) -> Result<String, String> {
            let mut text = PWSTR::null();
            unsafe {
                ConvertSecurityDescriptorToStringSecurityDescriptorW(
                    self.0,
                    SDDL_REVISION_1,
                    info,
                    &mut text,
                    None,
                )
            }
            .map_err(|e| format!("serialize controller security: {e}"))?;
            let result =
                unsafe { text.to_string() }.map_err(|e| format!("security SDDL encoding: {e}"));
            unsafe {
                let _ = LocalFree(Some(HLOCAL(text.0.cast())));
            }
            result
        }

        fn dacl(&self) -> Result<*mut ACL, String> {
            let mut present = BOOL::default();
            let mut defaulted = BOOL::default();
            let mut acl = ptr::null_mut();
            unsafe { GetSecurityDescriptorDacl(self.0, &mut present, &mut acl, &mut defaulted) }
                .map_err(|e| format!("read controller DACL: {e}"))?;
            if !present.as_bool()
                || acl.is_null()
                || !unsafe { IsValidAcl(acl) }.as_bool()
                || unsafe { (*acl).AceCount } == 0
            {
                return Err(
                    "missing, null or empty controller DACL; refusing permission replay".into(),
                );
            }
            Ok(acl)
        }

        fn protected(&self) -> Result<bool, String> {
            let mut control = 0;
            let mut revision = 0;
            unsafe { GetSecurityDescriptorControl(self.0, &mut control, &mut revision) }
                .map_err(|e| format!("read controller DACL control: {e}"))?;
            Ok(control & 0x1000 != 0) // SE_DACL_PROTECTED
        }

        fn aces(&self) -> Result<Vec<Vec<u8>>, String> {
            let acl = self.dacl()?;
            let mut entries = Vec::new();
            for index in 0..unsafe { (*acl).AceCount } {
                let mut entry = ptr::null_mut();
                unsafe { GetAce(acl, u32::from(index), &mut entry) }
                    .map_err(|e| format!("read controller ACE: {e}"))?;
                let header = unsafe { &*entry.cast::<ACE_HEADER>() };
                let mut bytes = unsafe {
                    std::slice::from_raw_parts(entry.cast::<u8>(), usize::from(header.AceSize))
                }
                .to_vec();
                // PnP maps generic file rights (e.g. GA -> FA). Compare their
                // mapped value without changing the original saved descriptor.
                if (header.AceType == 0 || header.AceType == 1) && bytes.len() >= 8 {
                    let mask = u32::from_le_bytes(bytes[4..8].try_into().unwrap());
                    let mapped = (mask & 0x0fff_ffff)
                        | if mask & 0x8000_0000 != 0 { 0x120089 } else { 0 }
                        | if mask & 0x4000_0000 != 0 { 0x120116 } else { 0 }
                        | if mask & 0x2000_0000 != 0 { 0x1200a0 } else { 0 }
                        | if mask & 0x1000_0000 != 0 { 0x1f01ff } else { 0 };
                    bytes[4..8].copy_from_slice(&mapped.to_le_bytes());
                }
                entries.push(bytes);
            }
            Ok(entries)
        }
    }

    fn same_dacl(a: &Descriptor, b: &Descriptor) -> Result<bool, String> {
        // AI/AR/defaulted bookkeeping can change on SetSecurityInfo; compare
        // actual ACE content/order and protection, NOT a complete SDDL string.
        Ok(a.protected()? == b.protected()? && a.aces()? == b.aces()?)
    }

    fn same_security(a: &Descriptor, b: &Descriptor, include_sacl: bool) -> Result<bool, String> {
        Ok(same_dacl(a, b)?
            && a.text(OWNER_SECURITY_INFORMATION | GROUP_SECURITY_INFORMATION)?
                == b.text(OWNER_SECURITY_INFORMATION | GROUP_SECURITY_INFORMATION)?
            && (!include_sacl
                || a.text(SACL_SECURITY_INFORMATION)? == b.text(SACL_SECURITY_INFORMATION)?))
    }

    fn same_override(a: Option<&str>, b: Option<&str>) -> Result<bool, String> {
        match (a, b) {
            (None, None) => Ok(true),
            (Some(a), Some(b)) => {
                same_security(&Descriptor::parse(a)?, &Descriptor::parse(b)?, true)
            }
            _ => Ok(false),
        }
    }

    fn fresh(controller: &ControllerObservation, windows_only: bool) -> Result<(), String> {
        let current = crate::enumerate_controllers().map_err(|e| e.to_string())?;
        let matches: Vec<_> = current
            .iter()
            .filter(|c| {
                c.identity
                    .instance_id
                    .eq_ignore_ascii_case(&controller.identity.instance_id)
                    && crate::rebind::same_physical_controller(&controller.identity, &c.identity)
            })
            .collect();
        let [current] = matches.as_slice() else {
            return Err("controller security identity is missing or ambiguous".into());
        };
        if !current.status.present
            || !current
                .identity
                .instance_id
                .to_ascii_uppercase()
                .starts_with("USB\\")
        {
            return Err("controller security target is not a present USB controller".into());
        }
        if windows_only
            && (!current.service.as_deref().is_some_and(|s| {
                s.eq_ignore_ascii_case("BTHUSB") || s.eq_ignore_ascii_case("IBTUSB")
            }) || current.status.problem_code != Some(0))
        {
            return Err("live radio security requires a healthy Windows Bluetooth binding".into());
        }
        if current.driver != controller.driver || current.service != controller.service {
            return Err("controller binding changed during security operation".into());
        }
        Ok(())
    }

    fn check_baseline(
        controller: &ControllerObservation,
        baseline: &ControllerSecurityBaseline,
    ) -> Result<(), String> {
        if !baseline
            .instance_id
            .eq_ignore_ascii_case(&controller.identity.instance_id)
        {
            return Err(
                "saved controller permissions belong to a different device instance".into(),
            );
        }
        Descriptor::parse(&baseline.radio_security_sddl)?.dacl()?;
        if let Some(text) = baseline.device_security_sddl.as_deref() {
            Descriptor::parse(text)?.dacl()?;
        }
        Ok(())
    }

    impl Device {
        fn open(controller: &ControllerObservation) -> Result<Self, String> {
            fresh(controller, false)?;
            let set = unsafe { SetupDiCreateDeviceInfoList(None, None) }
                .map_err(|e| format!("create security device set: {e}"))?;
            let mut device = Self(
                set,
                SP_DEVINFO_DATA {
                    cbSize: std::mem::size_of::<SP_DEVINFO_DATA>() as u32,
                    ..Default::default()
                },
            );
            let id = wide(&controller.identity.instance_id)?;
            unsafe {
                SetupDiOpenDeviceInfoW(set, PCWSTR(id.as_ptr()), None, 0, Some(&mut device.1))
            }
            .map_err(|e| format!("open security devnode: {e}"))?;
            Ok(device)
        }

        fn read(&self) -> Result<Option<String>, String> {
            let mut size = 0u32;
            for _ in 0..3 {
                if size as usize > MAX_SECURITY_BYTES {
                    return Err("oversized device security property".into());
                }
                let mut bytes = vec![0u8; size as usize];
                let mut kind = DEVPROPTYPE::default();
                let result = unsafe {
                    SetupDiGetDevicePropertyW(
                        self.0,
                        &self.1,
                        &DEVPKEY_Device_SecuritySDS,
                        &mut kind,
                        if bytes.is_empty() {
                            None
                        } else {
                            Some(&mut bytes)
                        },
                        Some(&mut size),
                        0,
                    )
                };
                match result {
                    Err(e) if e.code() == HRESULT::from_win32(ERROR_NOT_FOUND.0) => {
                        return Ok(None);
                    }
                    Err(e) if e.code() == HRESULT::from_win32(ERROR_INSUFFICIENT_BUFFER.0) => {
                        continue;
                    }
                    Err(e) => {
                        return Err(format!(
                            "read device SecuritySDS (not treated as absent): {e}"
                        ));
                    }
                    Ok(()) => {}
                }
                if kind != DEVPROP_TYPE_SECURITY_DESCRIPTOR_STRING
                    || size < 2
                    || size as usize > bytes.len()
                    || size % 2 != 0
                {
                    return Err("invalid device security property type/size".into());
                }
                let words: Vec<_> = bytes[..size as usize]
                    .chunks_exact(2)
                    .map(|b| u16::from_le_bytes([b[0], b[1]]))
                    .collect();
                if words.last() != Some(&0) || words[..words.len() - 1].contains(&0) {
                    return Err("invalid device security string termination".into());
                }
                let sddl = String::from_utf16(&words[..words.len() - 1])
                    .map_err(|e| format!("device security encoding: {e}"))?;
                Descriptor::parse(&sddl)?.dacl()?;
                return Ok(Some(sddl));
            }
            Err("device security property kept changing".into())
        }

        fn write(&mut self, sddl: Option<&str>) -> Result<(), String> {
            let Some(sddl) = sddl else {
                // A missing baseline means REMOVE the device override, not
                // write an empty SDDL or install a null DACL. The reported
                // Windows host rejects SecuritySDS + DEVPROP_TYPE_EMPTY with
                // ERROR_INVALID_DATA. Clear the underlying binary property,
                // as in the incident repair helper, using the documented
                // NULL-buffer/zero-size deletion contract. restore_override
                // must still read back confirmed absence before proceeding.
                // SAFETY: this owns a valid set and its exact device element;
                // None is passed by windows-rs as NULL with length zero.
                return unsafe {
                    SetupDiSetDeviceRegistryPropertyW(
                        self.0,
                        &mut self.1,
                        SPDRP_SECURITY,
                        None,
                    )
                }
                .map_err(|e| {
                    format!(
                        "clear device security override (SetupDiSetDeviceRegistryPropertyW/SPDRP_SECURITY): {e}"
                    )
                });
            };
            let bytes: Vec<u8> = wide(sddl)?.into_iter().flat_map(u16::to_le_bytes).collect();
            unsafe {
                SetupDiSetDevicePropertyW(
                    self.0,
                    &self.1,
                    &DEVPKEY_Device_SecuritySDS,
                    DEVPROP_TYPE_SECURITY_DESCRIPTOR_STRING,
                    Some(&bytes),
                    0,
                )
            }
            .map_err(|e| format!("restore saved device SecuritySDS: {e}"))
        }
    }

    fn radio_not_ready(controller: &ControllerObservation) -> String {
        format!(
            "BthportInterfaceNotReady: 0 present interfaces for controller {} (GUID {RADIO_GUID:?})",
            controller.identity.instance_id
        )
    }

    fn radio_path(controller: &ControllerObservation) -> Result<Option<Vec<u16>>, String> {
        let id = wide(&controller.identity.instance_id)?;
        for _ in 0..4 {
            let mut size = 0;
            let cr = unsafe {
                CM_Get_Device_Interface_List_SizeW(
                    &mut size,
                    &RADIO_GUID,
                    PCWSTR(id.as_ptr()),
                    CM_GET_DEVICE_INTERFACE_LIST_PRESENT,
                )
            };
            if cr != CR_SUCCESS {
                return Err(format!("read exact BTHPORT interface size: {cr:?}"));
            }
            if size == 0 || size > 65536 {
                return Err(format!(
                    "invalid exact BTHPORT interface buffer size: {size}"
                ));
            }
            let mut list = vec![0u16; size as usize];
            let cr = unsafe {
                CM_Get_Device_Interface_ListW(
                    &RADIO_GUID,
                    PCWSTR(id.as_ptr()),
                    &mut list,
                    CM_GET_DEVICE_INTERFACE_LIST_PRESENT,
                )
            };
            if cr == CR_BUFFER_SMALL {
                continue;
            }
            if cr != CR_SUCCESS || list.last() != Some(&0) {
                return Err(format!("read exact BTHPORT interface: {cr:?}"));
            }
            let paths: Vec<_> = list.split(|c| *c == 0).filter(|s| !s.is_empty()).collect();
            let path = match paths.as_slice() {
                [] => return Ok(None),
                [path] => *path,
                paths => {
                    return Err(format!(
                        "BthportInterfaceAmbiguous: {} present interfaces for controller {} (GUID {RADIO_GUID:?}); no interface selected",
                        paths.len(),
                        controller.identity.instance_id
                    ));
                }
            };
            let mut path = path.to_vec();
            path.push(0);
            return Ok(Some(path));
        }
        Err("BTHPORT interface kept changing".into())
    }

    impl Radio {
        fn open(controller: &ControllerObservation, write: bool) -> Result<Self, String> {
            Self::try_open(controller, write)?.ok_or_else(|| radio_not_ready(controller))
        }

        fn try_open(
            controller: &ControllerObservation,
            write: bool,
        ) -> Result<Option<Self>, String> {
            fresh(controller, true)?;
            let Some(path) = radio_path(controller)? else {
                return Ok(None);
            };
            let handle = unsafe {
                CreateFileW(
                    PCWSTR(path.as_ptr()),
                    READ_CONTROL.0 | if write { WRITE_DAC.0 } else { 0 },
                    FILE_SHARE_READ | FILE_SHARE_WRITE,
                    None,
                    OPEN_EXISTING,
                    FILE_FLAGS_AND_ATTRIBUTES::default(),
                    None,
                )
            }
            .map_err(|e| format!("open exact Windows Bluetooth radio security: {e}"))?;
            let radio = Self(handle);
            fresh(controller, true)?;
            match radio_path(controller)? {
                Some(current) if current == path => Ok(Some(radio)),
                // The read-only recovery probe may wait for re-enumeration.
                // The local handle is dropped before returning None.
                None => Ok(None),
                Some(_) => Err("BTHPORT interface changed during open".into()),
            }
        }

        fn read(&self) -> Result<Descriptor, String> {
            let mut sd = PSECURITY_DESCRIPTOR::default();
            let status = unsafe {
                GetSecurityInfo(
                    self.0,
                    SE_FILE_OBJECT,
                    RADIO_INFO,
                    None,
                    None,
                    None,
                    None,
                    Some(&mut sd),
                )
            };
            if status.0 != 0 {
                return Err(format!("read live radio permissions: {status:?}"));
            }
            let descriptor = Descriptor(sd);
            descriptor.dacl()?;
            Ok(descriptor)
        }
    }

    pub(super) fn capture(
        controller: &ControllerObservation,
    ) -> Result<ControllerSecurityBaseline, String> {
        fresh(controller, true)?;
        let device_security_sddl = Device::open(controller)?.read()?;
        let radio = Radio::open(controller, false)?.read()?;
        let restricted = Descriptor::parse(DIRECTHCI_DACL)?;
        if same_dacl(&radio, &restricted)?
            || device_security_sddl
                .as_deref()
                .map(|s| same_dacl(&Descriptor::parse(s)?, &restricted))
                .transpose()?
                .unwrap_or(false)
        {
            return Err("ControllerSecurityBaselineUnavailable: Windows radio still has DirectHCI-style SYSTEM/Administrators-only permissions; refuse to save them as a healthy baseline".into());
        }
        fresh(controller, true)?;
        Ok(ControllerSecurityBaseline {
            instance_id: controller.identity.instance_id.clone(),
            device_security_sddl,
            radio_security_sddl: radio.text(RADIO_INFO)?,
        })
        // All Windows radio handles have been dropped BEFORE a PnP transition.
    }

    pub(super) fn restore_override(
        controller: &ControllerObservation,
        baseline: &ControllerSecurityBaseline,
    ) -> Result<(), String> {
        check_baseline(controller, baseline)?;
        fresh(controller, true)?;
        let mut device = Device::open(controller)?;
        let current = device.read()?;
        if same_override(current.as_deref(), baseline.device_security_sddl.as_deref())? {
            return Ok(());
        }
        // Undo our known override, or restore a saved override if Windows
        // removed it. Never overwrite an unexpected third-party descriptor.
        if current.is_some() && !same_override(current.as_deref(), Some(DIRECTHCI_DACL))? {
            return Err("ControllerSecurityConflict: device security is neither the saved baseline nor the DirectHCI override".into());
        }
        fresh(controller, true)?;
        device.write(baseline.device_security_sddl.as_deref())?;
        if !same_override(
            device.read()?.as_deref(),
            baseline.device_security_sddl.as_deref(),
        )? {
            return Err(
                "ControllerSecurityRestoreFailed: persistent security readback differs".into(),
            );
        }
        Ok(())
    }

    pub(super) fn restore_radio(
        controller: &ControllerObservation,
        baseline: &ControllerSecurityBaseline,
    ) -> Result<(), String> {
        check_baseline(controller, baseline)?;
        let expected = Descriptor::parse(&baseline.radio_security_sddl)?;
        let current = Radio::open(controller, false)?.read()?;
        if same_security(&current, &expected, false)? {
            return Ok(());
        }
        if current.text(OWNER_SECURITY_INFORMATION | GROUP_SECURITY_INFORMATION)?
            != expected.text(OWNER_SECURITY_INFORMATION | GROUP_SECURITY_INFORMATION)?
        {
            return Err(
                "ControllerSecurityConflict: radio owner/group changed; not overwritten".into(),
            );
        }
        if !same_dacl(&current, &Descriptor::parse(DIRECTHCI_DACL)?)? {
            return Err("ControllerSecurityConflict: live radio DACL is neither the baseline nor the DirectHCI restriction".into());
        }
        // A matching baseline needs no WRITE_DAC access. Request that access
        // only for an actual repair and reject drift since the read-only open.
        let radio = Radio::open(controller, true)?;
        if !same_security(&radio.read()?, &current, false)? {
            return Err("ControllerSecurityConflict: live permissions changed while opening the repair handle".into());
        }
        fresh(controller, true)?;
        let protection = if expected.protected()? {
            PROTECTED_DACL_SECURITY_INFORMATION
        } else {
            UNPROTECTED_DACL_SECURITY_INFORMATION
        };
        // Replay only the saved DACL. Owner/group/SACL and pairing secrets are
        // never modified; no privileges are enabled and no new ACEs invented.
        let status = unsafe {
            SetSecurityInfo(
                radio.0,
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION | protection,
                None,
                None,
                Some(expected.dacl()?),
                None,
            )
        };
        if status.0 != 0 {
            return Err(format!(
                "ControllerSecurityRestoreFailed: live DACL write: {status:?}"
            ));
        }
        if !same_security(&radio.read()?, &expected, false)? {
            return Err(
                "ControllerSecurityRestoreFailed: live radio security readback differs".into(),
            );
        }
        Ok(())
    }

    pub(super) fn verify(
        controller: &ControllerObservation,
        baseline: &ControllerSecurityBaseline,
    ) -> Result<(), String> {
        if !matches_baseline(controller, baseline)? {
            return Err("ControllerSecurityRestoreFailed: Windows binding is present but controller permissions do not match the pre-acquire baseline".into());
        }
        Ok(())
    }

    fn matches_baseline(
        controller: &ControllerObservation,
        baseline: &ControllerSecurityBaseline,
    ) -> Result<bool, String> {
        try_matches_baseline(controller, baseline)?.ok_or_else(|| radio_not_ready(controller))
    }

    pub(super) fn try_matches_baseline(
        controller: &ControllerObservation,
        baseline: &ControllerSecurityBaseline,
    ) -> Result<Option<bool>, String> {
        check_baseline(controller, baseline)?;
        let override_matches = same_override(
            Device::open(controller)?.read()?.as_deref(),
            baseline.device_security_sddl.as_deref(),
        )?;
        let Some(radio) = Radio::try_open(controller, false)? else {
            return Ok(None);
        };
        let radio_matches = same_security(
            &radio.read()?,
            &Descriptor::parse(&baseline.radio_security_sddl)?,
            false,
        )?;
        fresh(controller, true)?;
        Ok(Some(override_matches && radio_matches))
    }
}
