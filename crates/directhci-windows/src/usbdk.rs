//! Read-only UsbDk runtime probing.
//!
//! The ABI declarations in the Windows implementation are based on UsbDk
//! v1.00-22 `UsbDk/UsbDkData.h` and `UsbDkHelper/UsbDkHelper.h`, licensed
//! Apache-2.0. This module intentionally defines and calls only device-list
//! enumeration functions. Redirect and installation exports are checked by
//! name but have no Rust function declarations and are never called.

use directhci_core::{ControllerId, ControllerObservation};
use serde::Serialize;

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct UsbDkProbeReport {
    pub helper: UsbDkHelperStatus,
    pub api: UsbDkApiStatus,
    pub service: UsbDkServiceStatus,
    pub enumeration: UsbDkEnumerationStatus,
    pub devices: Vec<UsbDkDeviceObservation>,
    pub correlations: Vec<UsbDkControllerCorrelation>,
}

impl UsbDkProbeReport {
    pub fn is_enumeration_available(&self) -> bool {
        matches!(self.enumeration, UsbDkEnumerationStatus::Succeeded)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum UsbDkHelperStatus {
    UnsupportedPlatform,
    Missing { expected_path: String },
    LoadFailed { path: String, message: String },
    Loaded { path: String },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum UsbDkApiStatus {
    NotInspected,
    RequiredExportMissing {
        missing: Vec<String>,
        configuration_descriptor_exports: bool,
        redirect_exports: bool,
    },
    Compatible {
        configuration_descriptor_exports: bool,
        redirect_exports: bool,
    },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum UsbDkServiceStatus {
    UnsupportedPlatform,
    Missing,
    Stopped,
    Running,
    Other { raw_state: u32 },
    QueryFailed { message: String },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum UsbDkEnumerationStatus {
    NotAttempted,
    DriverOrServiceUnavailable { message: String },
    Failed { message: String },
    InvalidUpstreamData { message: String },
    Succeeded,
}

/// Backend-local identity returned by UsbDk. It is not a DirectHCI controller
/// ID and must be re-correlated before every future privileged operation.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct UsbDkDeviceKey {
    pub device_id: String,
    pub instance_id: String,
}

#[cfg(any(windows, test))]
impl UsbDkDeviceKey {
    fn pnp_instance_id(&self) -> Option<String> {
        let device_id = self.device_id.trim().trim_end_matches('\\');
        let instance_id = self.instance_id.trim().trim_start_matches('\\');
        if device_id.is_empty()
            || instance_id.is_empty()
            || !device_id.to_ascii_uppercase().starts_with("USB\\")
        {
            return None;
        }
        Some(format!("{device_id}\\{instance_id}").to_ascii_uppercase())
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct UsbDkDeviceObservation {
    pub key: UsbDkDeviceKey,
    pub vendor_id: u16,
    pub product_id: u16,
    pub usb_version_bcd: u16,
    pub device_release_bcd: u16,
    pub device_class: u8,
    pub device_subclass: u8,
    pub device_protocol: u8,
    pub max_packet_size_0: u8,
    pub manufacturer_string_index: u8,
    pub product_string_index: u8,
    pub serial_number_string_index: u8,
    pub configuration_count: u8,
    pub speed: UsbDkDeviceSpeed,
    pub filter_id: u64,
    pub port: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", content = "raw", rename_all = "snake_case")]
pub enum UsbDkDeviceSpeed {
    None,
    Low,
    Full,
    High,
    Super,
    Unknown(u64),
}

impl From<u64> for UsbDkDeviceSpeed {
    fn from(value: u64) -> Self {
        match value {
            0 => Self::None,
            1 => Self::Low,
            2 => Self::Full,
            3 => Self::High,
            4 => Self::Super,
            value => Self::Unknown(value),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct UsbDkControllerCorrelation {
    pub controller_id: ControllerId,
    pub status: UsbDkControllerCorrelationStatus,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum UsbDkControllerCorrelationStatus {
    NotEvaluated,
    NoMatch {
        reason: UsbDkNoMatchReason,
    },
    Unique {
        device: UsbDkDeviceKey,
    },
    Ambiguous {
        reason: UsbDkAmbiguityReason,
        candidates: Vec<UsbDkDeviceKey>,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum UsbDkNoMatchReason {
    ControllerMissingVidPid,
    NoVidPidCandidate,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum UsbDkAmbiguityReason {
    InsufficientIdentityEvidence,
    MultipleExactInstanceMatches,
}

pub fn probe_usbdk(controllers: &[ControllerObservation]) -> UsbDkProbeReport {
    platform::probe(controllers)
}

fn correlations_not_evaluated(
    controllers: &[ControllerObservation],
) -> Vec<UsbDkControllerCorrelation> {
    controllers
        .iter()
        .map(|controller| UsbDkControllerCorrelation {
            controller_id: controller.id.clone(),
            status: UsbDkControllerCorrelationStatus::NotEvaluated,
        })
        .collect()
}

#[cfg(any(windows, test))]
fn correlate_devices(
    controllers: &[ControllerObservation],
    devices: &[UsbDkDeviceObservation],
) -> Vec<UsbDkControllerCorrelation> {
    controllers
        .iter()
        .map(|controller| {
            let Some((vendor_id, product_id)) = controller
                .identity
                .usb_vendor_id
                .zip(controller.identity.usb_product_id)
            else {
                return UsbDkControllerCorrelation {
                    controller_id: controller.id.clone(),
                    status: UsbDkControllerCorrelationStatus::NoMatch {
                        reason: UsbDkNoMatchReason::ControllerMissingVidPid,
                    },
                };
            };

            let candidates: Vec<_> = devices
                .iter()
                .filter(|device| device.vendor_id == vendor_id && device.product_id == product_id)
                .collect();
            if candidates.is_empty() {
                return UsbDkControllerCorrelation {
                    controller_id: controller.id.clone(),
                    status: UsbDkControllerCorrelationStatus::NoMatch {
                        reason: UsbDkNoMatchReason::NoVidPidCandidate,
                    },
                };
            }

            let windows_instance_id = controller.identity.instance_id.to_ascii_uppercase();
            let exact: Vec<_> = candidates
                .iter()
                .filter(|device| {
                    device.key.pnp_instance_id().as_deref() == Some(&windows_instance_id)
                })
                .collect();

            let status = match exact.as_slice() {
                [device] => UsbDkControllerCorrelationStatus::Unique {
                    device: device.key.clone(),
                },
                [] => UsbDkControllerCorrelationStatus::Ambiguous {
                    reason: UsbDkAmbiguityReason::InsufficientIdentityEvidence,
                    candidates: candidates
                        .into_iter()
                        .map(|device| device.key.clone())
                        .collect(),
                },
                _ => UsbDkControllerCorrelationStatus::Ambiguous {
                    reason: UsbDkAmbiguityReason::MultipleExactInstanceMatches,
                    candidates: exact.into_iter().map(|device| device.key.clone()).collect(),
                },
            };

            UsbDkControllerCorrelation {
                controller_id: controller.id.clone(),
                status,
            }
        })
        .collect()
}

#[cfg(windows)]
mod platform {
    use std::ffi::OsString;
    use std::marker::PhantomData;
    use std::mem::{offset_of, size_of};
    use std::os::windows::ffi::OsStringExt;
    use std::path::PathBuf;
    use std::ptr;

    use windows::Win32::Foundation::{ERROR_SERVICE_DOES_NOT_EXIST, FreeLibrary, HMODULE};
    use windows::Win32::System::LibraryLoader::{
        GetProcAddress, LOAD_LIBRARY_SEARCH_SYSTEM32, LoadLibraryExW,
    };
    use windows::Win32::System::Services::{
        CloseServiceHandle, OpenSCManagerW, OpenServiceW, QueryServiceStatusEx, SC_HANDLE,
        SC_MANAGER_CONNECT, SC_STATUS_PROCESS_INFO, SERVICE_QUERY_STATUS, SERVICE_RUNNING,
        SERVICE_STATUS_PROCESS, SERVICE_STOPPED,
    };
    use windows::Win32::System::SystemInformation::GetSystemDirectoryW;
    use windows::core::{HRESULT, PCSTR, PCWSTR, w};

    use super::*;

    const USBDK_HELPER_DLL: &str = "UsbDkHelper.dll";
    const MAX_DEVICE_ID_LEN: usize = 200;
    const MAX_ENUMERATED_DEVICES: usize = 65_536;

    const GET_DEVICES_LIST: &[u8] = b"UsbDk_GetDevicesList\0";
    const RELEASE_DEVICES_LIST: &[u8] = b"UsbDk_ReleaseDevicesList\0";
    const GET_CONFIG_DESCRIPTOR: &[u8] = b"UsbDk_GetConfigurationDescriptor\0";
    const RELEASE_CONFIG_DESCRIPTOR: &[u8] = b"UsbDk_ReleaseConfigurationDescriptor\0";
    const START_REDIRECT: &[u8] = b"UsbDk_StartRedirect\0";
    const STOP_REDIRECT: &[u8] = b"UsbDk_StopRedirect\0";

    type GetDevicesListFn = unsafe extern "C" fn(*mut *mut RawUsbDkDeviceInfo, *mut u32) -> i32;
    type ReleaseDevicesListFn = unsafe extern "C" fn(*mut RawUsbDkDeviceInfo);

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct RawUsbDkDeviceId {
        device_id: [u16; MAX_DEVICE_ID_LEN],
        instance_id: [u16; MAX_DEVICE_ID_LEN],
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct RawUsbDeviceDescriptor {
        length: u8,
        descriptor_type: u8,
        usb_version_bcd: u16,
        device_class: u8,
        device_subclass: u8,
        device_protocol: u8,
        max_packet_size_0: u8,
        vendor_id: u16,
        product_id: u16,
        device_release_bcd: u16,
        manufacturer_string_index: u8,
        product_string_index: u8,
        serial_number_string_index: u8,
        configuration_count: u8,
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct RawUsbDkDeviceInfo {
        id: RawUsbDkDeviceId,
        filter_id: u64,
        port: u64,
        speed: u64,
        device_descriptor: RawUsbDeviceDescriptor,
    }

    // UsbDk v1.00-22 x64 ABI checks. These fail at compile time if a Rust
    // layout change would make the FFI declaration incompatible.
    const _: () = {
        assert!(size_of::<RawUsbDkDeviceId>() == 800);
        assert!(size_of::<RawUsbDeviceDescriptor>() == 18);
        assert!(offset_of!(RawUsbDkDeviceInfo, id) == 0);
        assert!(offset_of!(RawUsbDkDeviceInfo, filter_id) == 800);
        assert!(offset_of!(RawUsbDkDeviceInfo, port) == 808);
        assert!(offset_of!(RawUsbDkDeviceInfo, speed) == 816);
        assert!(offset_of!(RawUsbDkDeviceInfo, device_descriptor) == 824);
        assert!(size_of::<RawUsbDkDeviceInfo>() == 848);
    };

    pub(super) fn probe(controllers: &[ControllerObservation]) -> UsbDkProbeReport {
        let service = query_service_status();
        let helper_path = match system_helper_path() {
            Ok(path) => path,
            Err(message) => {
                return report_without_enumeration(
                    controllers,
                    UsbDkHelperStatus::LoadFailed {
                        path: USBDK_HELPER_DLL.into(),
                        message,
                    },
                    UsbDkApiStatus::NotInspected,
                    service,
                    UsbDkEnumerationStatus::NotAttempted,
                );
            }
        };
        let helper_path_text = helper_path.display().to_string();
        if !helper_path.is_file() {
            return report_without_enumeration(
                controllers,
                UsbDkHelperStatus::Missing {
                    expected_path: helper_path_text,
                },
                UsbDkApiStatus::NotInspected,
                service,
                UsbDkEnumerationStatus::NotAttempted,
            );
        }

        let module = match LoadedModule::load() {
            Ok(module) => module,
            Err(message) => {
                return report_without_enumeration(
                    controllers,
                    UsbDkHelperStatus::LoadFailed {
                        path: helper_path_text,
                        message,
                    },
                    UsbDkApiStatus::NotInspected,
                    service,
                    UsbDkEnumerationStatus::NotAttempted,
                );
            }
        };
        let helper = UsbDkHelperStatus::Loaded {
            path: helper_path_text,
        };

        let export_probe = ExportProbe::inspect(&module);
        let api = export_probe.status();
        if !export_probe.missing_required.is_empty() {
            return report_without_enumeration(
                controllers,
                helper,
                api,
                service,
                UsbDkEnumerationStatus::NotAttempted,
            );
        }

        // SAFETY: the required exports were resolved from a module that stays
        // owned by UsbDkLibrary for the full lifetime of both function pointers.
        let library = unsafe { UsbDkLibrary::from_module(module) };
        match library.enumerate_devices() {
            Ok(devices) => UsbDkProbeReport {
                helper,
                api,
                service,
                enumeration: UsbDkEnumerationStatus::Succeeded,
                correlations: correlate_devices(controllers, &devices),
                devices,
            },
            Err(EnumerationError::CallFailed(message)) => {
                let enumeration = if matches!(
                    service,
                    UsbDkServiceStatus::Missing | UsbDkServiceStatus::Stopped
                ) {
                    UsbDkEnumerationStatus::DriverOrServiceUnavailable { message }
                } else {
                    UsbDkEnumerationStatus::Failed { message }
                };
                report_without_enumeration(controllers, helper, api, service, enumeration)
            }
            Err(EnumerationError::InvalidData(message)) => report_without_enumeration(
                controllers,
                helper,
                api,
                service,
                UsbDkEnumerationStatus::InvalidUpstreamData { message },
            ),
        }
    }

    fn report_without_enumeration(
        controllers: &[ControllerObservation],
        helper: UsbDkHelperStatus,
        api: UsbDkApiStatus,
        service: UsbDkServiceStatus,
        enumeration: UsbDkEnumerationStatus,
    ) -> UsbDkProbeReport {
        UsbDkProbeReport {
            helper,
            api,
            service,
            enumeration,
            devices: Vec::new(),
            correlations: correlations_not_evaluated(controllers),
        }
    }

    struct ExportProbe {
        missing_required: Vec<String>,
        configuration_descriptor_exports: bool,
        redirect_exports: bool,
    }

    impl ExportProbe {
        fn inspect(module: &LoadedModule) -> Self {
            let mut missing_required = Vec::new();
            if !module.has_export(GET_DEVICES_LIST) {
                missing_required.push("UsbDk_GetDevicesList".into());
            }
            if !module.has_export(RELEASE_DEVICES_LIST) {
                missing_required.push("UsbDk_ReleaseDevicesList".into());
            }
            Self {
                missing_required,
                configuration_descriptor_exports: module.has_export(GET_CONFIG_DESCRIPTOR)
                    && module.has_export(RELEASE_CONFIG_DESCRIPTOR),
                redirect_exports: module.has_export(START_REDIRECT)
                    && module.has_export(STOP_REDIRECT),
            }
        }

        fn status(&self) -> UsbDkApiStatus {
            if self.missing_required.is_empty() {
                UsbDkApiStatus::Compatible {
                    configuration_descriptor_exports: self.configuration_descriptor_exports,
                    redirect_exports: self.redirect_exports,
                }
            } else {
                UsbDkApiStatus::RequiredExportMissing {
                    missing: self.missing_required.clone(),
                    configuration_descriptor_exports: self.configuration_descriptor_exports,
                    redirect_exports: self.redirect_exports,
                }
            }
        }
    }

    struct LoadedModule(HMODULE);

    impl LoadedModule {
        fn load() -> Result<Self, String> {
            // LOAD_LIBRARY_SEARCH_SYSTEM32 avoids loading an attacker-controlled
            // UsbDkHelper.dll from the current working directory.
            let module = unsafe {
                LoadLibraryExW(w!("UsbDkHelper.dll"), None, LOAD_LIBRARY_SEARCH_SYSTEM32)
            }
            .map_err(|error| error.to_string())?;
            Ok(Self(module))
        }

        fn has_export(&self, name: &'static [u8]) -> bool {
            unsafe { GetProcAddress(self.0, PCSTR(name.as_ptr())) }.is_some()
        }

        fn export(&self, name: &'static [u8]) -> unsafe extern "system" fn() -> isize {
            // Caller only asks for exports that were validated immediately
            // before constructing UsbDkLibrary.
            unsafe { GetProcAddress(self.0, PCSTR(name.as_ptr())) }
                .expect("validated UsbDk export disappeared while the module remained loaded")
        }
    }

    impl Drop for LoadedModule {
        fn drop(&mut self) {
            let _ = unsafe { FreeLibrary(self.0) };
        }
    }

    struct UsbDkLibrary {
        module: LoadedModule,
        get_devices_list: GetDevicesListFn,
        release_devices_list: ReleaseDevicesListFn,
    }

    impl UsbDkLibrary {
        unsafe fn from_module(module: LoadedModule) -> Self {
            let raw_get = module.export(GET_DEVICES_LIST);
            let raw_release = module.export(RELEASE_DEVICES_LIST);
            // SAFETY: UsbDkHelper.h declares these exact C ABI signatures, and
            // ExportProbe verified both symbol names in the loaded v1 ABI.
            let get_devices_list = unsafe {
                std::mem::transmute::<unsafe extern "system" fn() -> isize, GetDevicesListFn>(
                    raw_get,
                )
            };
            // SAFETY: same reasoning as above for UsbDk_ReleaseDevicesList.
            let release_devices_list = unsafe {
                std::mem::transmute::<unsafe extern "system" fn() -> isize, ReleaseDevicesListFn>(
                    raw_release,
                )
            };
            Self {
                module,
                get_devices_list,
                release_devices_list,
            }
        }

        fn enumerate_devices(&self) -> Result<Vec<UsbDkDeviceObservation>, EnumerationError> {
            let mut devices_ptr = ptr::null_mut();
            let mut device_count = 0_u32;
            // SAFETY: both out-pointers are valid for writes. UsbDk owns any
            // returned allocation until release_devices_list is called below.
            let succeeded =
                unsafe { (self.get_devices_list)(&mut devices_ptr, &mut device_count) } != 0;
            if !succeeded {
                return Err(EnumerationError::CallFailed(
                    "UsbDk_GetDevicesList returned FALSE".into(),
                ));
            }

            let list = DeviceListGuard {
                pointer: devices_ptr,
                release: self.release_devices_list,
                _library: PhantomData,
            };
            if device_count == 0 {
                return Ok(Vec::new());
            }
            if list.pointer.is_null() {
                return Err(EnumerationError::InvalidData(
                    "UsbDk returned a non-zero device count with a null array".into(),
                ));
            }
            let device_count = device_count as usize;
            if device_count > MAX_ENUMERATED_DEVICES {
                return Err(EnumerationError::InvalidData(format!(
                    "UsbDk returned an unreasonable device count: {device_count}"
                )));
            }

            // SAFETY: successful UsbDk_GetDevicesList returned an array of
            // device_count ABI-compatible elements. DeviceListGuard keeps it
            // alive for the duration of this read and releases it exactly once.
            let raw_devices = unsafe { std::slice::from_raw_parts(list.pointer, device_count) };
            raw_devices
                .iter()
                .enumerate()
                .map(|(index, raw)| parse_device(index, raw))
                .collect()
        }
    }

    impl Drop for UsbDkLibrary {
        fn drop(&mut self) {
            // Read the field so the ownership relation stays explicit: the
            // module is intentionally dropped only after the function pointers
            // can no longer be used.
            let _ = self.module.0;
        }
    }

    struct DeviceListGuard<'library> {
        pointer: *mut RawUsbDkDeviceInfo,
        release: ReleaseDevicesListFn,
        _library: PhantomData<&'library UsbDkLibrary>,
    }

    impl Drop for DeviceListGuard<'_> {
        fn drop(&mut self) {
            if !self.pointer.is_null() {
                // SAFETY: this pointer came from a successful GetDevicesList
                // call and has not been released or converted into owned Rust
                // allocation. UsbDk requires this matching release function.
                unsafe { (self.release)(self.pointer) };
            }
        }
    }

    enum EnumerationError {
        CallFailed(String),
        InvalidData(String),
    }

    fn parse_device(
        index: usize,
        raw: &RawUsbDkDeviceInfo,
    ) -> Result<UsbDkDeviceObservation, EnumerationError> {
        let device_id = decode_id(&raw.id.device_id).map_err(|message| {
            EnumerationError::InvalidData(format!("device {index} DeviceID: {message}"))
        })?;
        let instance_id = decode_id(&raw.id.instance_id).map_err(|message| {
            EnumerationError::InvalidData(format!("device {index} InstanceID: {message}"))
        })?;
        if !device_id.starts_with("USB\\") {
            return Err(EnumerationError::InvalidData(format!(
                "device {index} has a non-USB DeviceID: {device_id}"
            )));
        }
        let descriptor = raw.device_descriptor;
        if descriptor.length != 18 || descriptor.descriptor_type != 1 {
            return Err(EnumerationError::InvalidData(format!(
                "device {index} has an invalid USB device descriptor header"
            )));
        }

        Ok(UsbDkDeviceObservation {
            key: UsbDkDeviceKey {
                device_id,
                instance_id,
            },
            vendor_id: descriptor.vendor_id,
            product_id: descriptor.product_id,
            usb_version_bcd: descriptor.usb_version_bcd,
            device_release_bcd: descriptor.device_release_bcd,
            device_class: descriptor.device_class,
            device_subclass: descriptor.device_subclass,
            device_protocol: descriptor.device_protocol,
            max_packet_size_0: descriptor.max_packet_size_0,
            manufacturer_string_index: descriptor.manufacturer_string_index,
            product_string_index: descriptor.product_string_index,
            serial_number_string_index: descriptor.serial_number_string_index,
            configuration_count: descriptor.configuration_count,
            speed: raw.speed.into(),
            filter_id: raw.filter_id,
            port: raw.port,
        })
    }

    fn decode_id(value: &[u16; MAX_DEVICE_ID_LEN]) -> Result<String, String> {
        let length = value
            .iter()
            .position(|unit| *unit == 0)
            .unwrap_or(value.len());
        if length == 0 {
            return Err("empty string".into());
        }
        String::from_utf16(&value[..length])
            .map(|value| value.trim().to_ascii_uppercase())
            .map_err(|_| "invalid UTF-16".into())
            .and_then(|value| {
                if value.is_empty() {
                    Err("blank string".into())
                } else {
                    Ok(value)
                }
            })
    }

    fn system_helper_path() -> Result<PathBuf, String> {
        let mut buffer = vec![0_u16; 32_768];
        let length = unsafe { GetSystemDirectoryW(Some(&mut buffer)) } as usize;
        if length == 0 {
            return Err("GetSystemDirectoryW failed".into());
        }
        if length >= buffer.len() {
            return Err("GetSystemDirectoryW returned an oversized path".into());
        }
        let mut path = PathBuf::from(OsString::from_wide(&buffer[..length]));
        path.push(USBDK_HELPER_DLL);
        Ok(path)
    }

    fn query_service_status() -> UsbDkServiceStatus {
        let manager =
            match unsafe { OpenSCManagerW(PCWSTR::null(), PCWSTR::null(), SC_MANAGER_CONNECT) } {
                Ok(handle) => ServiceHandle(handle),
                Err(error) => {
                    return UsbDkServiceStatus::QueryFailed {
                        message: error.to_string(),
                    };
                }
            };
        let service = match unsafe { OpenServiceW(manager.0, w!("UsbDk"), SERVICE_QUERY_STATUS) } {
            Ok(handle) => ServiceHandle(handle),
            Err(error) if error.code() == HRESULT::from_win32(ERROR_SERVICE_DOES_NOT_EXIST.0) => {
                return UsbDkServiceStatus::Missing;
            }
            Err(error) => {
                return UsbDkServiceStatus::QueryFailed {
                    message: error.to_string(),
                };
            }
        };

        let mut status = SERVICE_STATUS_PROCESS::default();
        let mut bytes_needed = 0;
        // SAFETY: SERVICE_STATUS_PROCESS is plain C data and the byte slice
        // covers exactly its initialized writable storage for QueryServiceStatusEx.
        let buffer = unsafe {
            std::slice::from_raw_parts_mut(
                (&mut status as *mut SERVICE_STATUS_PROCESS).cast::<u8>(),
                size_of::<SERVICE_STATUS_PROCESS>(),
            )
        };
        if let Err(error) = unsafe {
            QueryServiceStatusEx(
                service.0,
                SC_STATUS_PROCESS_INFO,
                Some(buffer),
                &mut bytes_needed,
            )
        } {
            return UsbDkServiceStatus::QueryFailed {
                message: error.to_string(),
            };
        }

        if status.dwCurrentState == SERVICE_RUNNING {
            UsbDkServiceStatus::Running
        } else if status.dwCurrentState == SERVICE_STOPPED {
            UsbDkServiceStatus::Stopped
        } else {
            UsbDkServiceStatus::Other {
                raw_state: status.dwCurrentState.0,
            }
        }
    }

    struct ServiceHandle(SC_HANDLE);

    impl Drop for ServiceHandle {
        fn drop(&mut self) {
            let _ = unsafe { CloseServiceHandle(self.0) };
        }
    }
}

#[cfg(not(windows))]
mod platform {
    use super::*;

    pub(super) fn probe(controllers: &[ControllerObservation]) -> UsbDkProbeReport {
        UsbDkProbeReport {
            helper: UsbDkHelperStatus::UnsupportedPlatform,
            api: UsbDkApiStatus::NotInspected,
            service: UsbDkServiceStatus::UnsupportedPlatform,
            enumeration: UsbDkEnumerationStatus::NotAttempted,
            devices: Vec::new(),
            correlations: correlations_not_evaluated(controllers),
        }
    }
}

#[cfg(test)]
mod tests {
    use directhci_core::{ControllerIdentity, DeviceStatus, DriverObservation};

    use super::*;

    fn controller(
        instance_id: &str,
        vendor_id: Option<u16>,
        product_id: Option<u16>,
        serial: Option<&str>,
        locations: &[&str],
    ) -> ControllerObservation {
        ControllerObservation::new(
            ControllerIdentity::new(
                vendor_id,
                product_id,
                serial.map(str::to_owned),
                None,
                instance_id.into(),
                locations.iter().map(|value| (*value).into()).collect(),
                None,
                Vec::new(),
                Vec::new(),
            ),
            None,
            None,
            None,
            None,
            None,
            Some("USB".into()),
            None,
            DriverObservation::default(),
            Vec::new(),
            DeviceStatus {
                present: true,
                status_flags: None,
                problem_code: None,
            },
        )
    }

    fn device(
        device_id: &str,
        instance_id: &str,
        vendor_id: u16,
        product_id: u16,
    ) -> UsbDkDeviceObservation {
        UsbDkDeviceObservation {
            key: UsbDkDeviceKey {
                device_id: device_id.into(),
                instance_id: instance_id.into(),
            },
            vendor_id,
            product_id,
            usb_version_bcd: 0x0200,
            device_release_bcd: 0x0100,
            device_class: 0xe0,
            device_subclass: 1,
            device_protocol: 1,
            max_packet_size_0: 64,
            manufacturer_string_index: 1,
            product_string_index: 2,
            serial_number_string_index: 0,
            configuration_count: 1,
            speed: UsbDkDeviceSpeed::High,
            filter_id: 1,
            port: 2,
        }
    }

    fn status(
        controller: ControllerObservation,
        devices: &[UsbDkDeviceObservation],
    ) -> UsbDkControllerCorrelationStatus {
        correlate_devices(&[controller], devices)
            .pop()
            .expect("one controller result")
            .status
    }

    #[test]
    fn zero_vid_pid_candidates_is_no_match() {
        let controller = controller(
            "USB\\VID_1234&PID_ABCD\\ONE",
            Some(0x1234),
            Some(0xabcd),
            None,
            &[],
        );
        assert_eq!(
            status(controller, &[]),
            UsbDkControllerCorrelationStatus::NoMatch {
                reason: UsbDkNoMatchReason::NoVidPidCandidate
            }
        );
    }

    #[test]
    fn exact_pnp_instance_is_unique() {
        let controller = controller(
            "USB\\VID_1234&PID_ABCD\\ONE",
            Some(0x1234),
            Some(0xabcd),
            None,
            &[],
        );
        let matching = device("usb\\vid_1234&pid_abcd", "one", 0x1234, 0xabcd);
        assert!(matches!(
            status(controller, &[matching]),
            UsbDkControllerCorrelationStatus::Unique { .. }
        ));
    }

    #[test]
    fn multiple_exact_records_are_ambiguous() {
        let controller = controller(
            "USB\\VID_1234&PID_ABCD\\ONE",
            Some(0x1234),
            Some(0xabcd),
            None,
            &[],
        );
        let matching = device("USB\\VID_1234&PID_ABCD", "ONE", 0x1234, 0xabcd);
        assert!(matches!(
            status(controller, &[matching.clone(), matching]),
            UsbDkControllerCorrelationStatus::Ambiguous {
                reason: UsbDkAmbiguityReason::MultipleExactInstanceMatches,
                ..
            }
        ));
    }

    #[test]
    fn vid_pid_collision_without_instance_match_is_ambiguous() {
        let controller = controller(
            "USB\\VID_1234&PID_ABCD\\TARGET",
            Some(0x1234),
            Some(0xabcd),
            None,
            &[],
        );
        let first = device("USB\\VID_1234&PID_ABCD", "ONE", 0x1234, 0xabcd);
        let second = device("USB\\VID_1234&PID_ABCD", "TWO", 0x1234, 0xabcd);
        assert!(matches!(
            status(controller, &[first, second]),
            UsbDkControllerCorrelationStatus::Ambiguous {
                reason: UsbDkAmbiguityReason::InsufficientIdentityEvidence,
                ..
            }
        ));
    }

    #[test]
    fn one_sided_serial_is_not_treated_as_backend_evidence() {
        let controller = controller(
            "USB\\VID_1234&PID_ABCD\\TARGET",
            Some(0x1234),
            Some(0xabcd),
            Some("SERIAL"),
            &[],
        );
        let candidate = device("USB\\VID_1234&PID_ABCD", "OTHER", 0x1234, 0xabcd);
        assert!(matches!(
            status(controller, &[candidate]),
            UsbDkControllerCorrelationStatus::Ambiguous { .. }
        ));
    }

    #[test]
    fn one_sided_topology_is_not_treated_as_backend_evidence() {
        let controller = controller(
            "USB\\VID_1234&PID_ABCD\\TARGET",
            Some(0x1234),
            Some(0xabcd),
            None,
            &["PCIROOT(0)#USBROOT(0)#USB(2)"],
        );
        let candidate = device("USB\\VID_1234&PID_ABCD", "OTHER", 0x1234, 0xabcd);
        assert!(matches!(
            status(controller, &[candidate]),
            UsbDkControllerCorrelationStatus::Ambiguous { .. }
        ));
    }

    #[test]
    fn malformed_backend_identity_never_correlates() {
        let controller = controller(
            "USB\\VID_1234&PID_ABCD\\TARGET",
            Some(0x1234),
            Some(0xabcd),
            None,
            &[],
        );
        let candidate = device("USB\\VID_1234&PID_ABCD", "", 0x1234, 0xabcd);
        assert!(matches!(
            status(controller, &[candidate]),
            UsbDkControllerCorrelationStatus::Ambiguous { .. }
        ));
    }
}
