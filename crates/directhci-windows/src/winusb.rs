//! Read-only WinUSB readiness probing for an already provisioned controller.
//!
//! No driver provisioning or USB transfer is performed here. The Windows
//! implementation only opens the registered application interface, queries
//! cached interface/pipe metadata, and deterministically closes its handles.

use directhci_core::ControllerId;
use serde::Serialize;

#[cfg(any(windows, test))]
const BLUETOOTH_INTERFACE_CLASS: u8 = 0xe0;
#[cfg(any(windows, test))]
const BLUETOOTH_INTERFACE_SUBCLASS: u8 = 0x01;
#[cfg(any(windows, test))]
const BLUETOOTH_INTERFACE_PROTOCOL: u8 = 0x01;

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct DedicatedWinUsbProbeReport {
    pub status: DedicatedWinUsbProbeStatus,
    pub controllers: Vec<DedicatedWinUsbControllerReadiness>,
}

impl DedicatedWinUsbProbeReport {
    pub fn candidate_count(&self) -> usize {
        self.controllers
            .iter()
            .filter(|controller| controller.status.is_candidate())
            .count()
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum DedicatedWinUsbProbeStatus {
    UnsupportedPlatform,
    EnumerationFailed { message: String },
    Succeeded,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct DedicatedWinUsbControllerReadiness {
    pub controller_id: ControllerId,
    pub instance_id: String,
    pub service: Option<String>,
    pub driver_inf: Option<String>,
    pub driver_provider: Option<String>,
    pub driver_version: Option<String>,
    pub registered_interface_guids: Vec<String>,
    pub application_interfaces: Vec<WinUsbApplicationInterface>,
    pub interface: Option<WinUsbInterfaceDescriptor>,
    pub pipes: Vec<WinUsbPipe>,
    pub event_pipe: Option<u8>,
    pub acl_in_pipe: Option<u8>,
    pub acl_out_pipe: Option<u8>,
    pub extra_pipes: Vec<WinUsbPipe>,
    pub status: DedicatedWinUsbReadinessStatus,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct WinUsbApplicationInterface {
    pub class_guid: String,
    pub path: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub struct WinUsbInterfaceDescriptor {
    pub number: u8,
    pub alternate_setting: u8,
    pub endpoint_count: u8,
    pub class: u8,
    pub subclass: u8,
    pub protocol: u8,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub struct WinUsbPipe {
    pub pipe_type: WinUsbPipeType,
    pub direction: WinUsbPipeDirection,
    pub pipe_id: u8,
    pub maximum_packet_size: u16,
    pub interval: u8,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(tag = "type", content = "raw", rename_all = "snake_case")]
pub enum WinUsbPipeType {
    Control,
    Isochronous,
    Bulk,
    Interrupt,
    Unknown(i32),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WinUsbPipeDirection {
    In,
    Out,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum DedicatedWinUsbReadinessStatus {
    NotWinUsbBound,
    AmbiguousControllerId { observations: usize },
    ApplicationInterfaceDiscoveryFailed { message: String },
    InvalidApplicationInterfaceRegistration { values: Vec<String> },
    NoApplicationInterface,
    AmbiguousApplicationInterface { interfaces: usize },
    OpenFailed { message: String },
    WinUsbInitializeFailed { message: String },
    InterfaceQueryFailed { message: String },
    PipeQueryFailed { index: u8, message: String },
    UnexpectedInterface,
    DuplicateEventPipe,
    DuplicateAclInPipe,
    DuplicateAclOutPipe,
    MissingEventPipe,
    MissingAclInPipe,
    MissingAclOutPipe,
    Ready,
}

impl DedicatedWinUsbReadinessStatus {
    fn is_candidate(&self) -> bool {
        !matches!(self, Self::NotWinUsbBound)
    }
}

pub fn probe_dedicated_winusb() -> DedicatedWinUsbProbeReport {
    platform::probe()
}

/// Freshly probes exactly one controller through the generic WinUSB readiness
/// path. The caller must have already established ownership/provisioning.
pub fn probe_winusb_controller(
    controller_id: &str,
) -> Result<DedicatedWinUsbControllerReadiness, String> {
    let report = platform::probe();
    match report.status {
        DedicatedWinUsbProbeStatus::Succeeded => {}
        DedicatedWinUsbProbeStatus::UnsupportedPlatform => {
            return Err("WinUSB readiness is only available on Windows".into());
        }
        DedicatedWinUsbProbeStatus::EnumerationFailed { message } => return Err(message),
    }
    let mut matches = report.controllers.into_iter().filter(|controller| {
        controller
            .controller_id
            .as_str()
            .eq_ignore_ascii_case(controller_id)
    });
    let result = matches
        .next()
        .ok_or_else(|| "controller is missing from the fresh WinUSB probe".to_owned())?;
    if matches.next().is_some() {
        return Err("controller ID is ambiguous in the fresh WinUSB probe".into());
    }
    Ok(result)
}

pub(crate) fn application_interface_is_active(
    instance_id: &str,
    interface_guid: &str,
) -> Result<bool, String> {
    platform::application_interface_is_active(instance_id, interface_guid)
}

#[cfg(any(windows, test))]
fn classify_readiness(
    interface: WinUsbInterfaceDescriptor,
    pipes: &[WinUsbPipe],
) -> DedicatedWinUsbReadinessStatus {
    if (interface.class, interface.subclass, interface.protocol)
        != (
            BLUETOOTH_INTERFACE_CLASS,
            BLUETOOTH_INTERFACE_SUBCLASS,
            BLUETOOTH_INTERFACE_PROTOCOL,
        )
    {
        return DedicatedWinUsbReadinessStatus::UnexpectedInterface;
    }

    let count = |pipe_type, direction| {
        pipes
            .iter()
            .filter(|pipe| pipe.pipe_type == pipe_type && pipe.direction == direction)
            .count()
    };
    let event_count = count(WinUsbPipeType::Interrupt, WinUsbPipeDirection::In);
    let acl_in_count = count(WinUsbPipeType::Bulk, WinUsbPipeDirection::In);
    let acl_out_count = count(WinUsbPipeType::Bulk, WinUsbPipeDirection::Out);

    match (event_count, acl_in_count, acl_out_count) {
        (event, _, _) if event > 1 => DedicatedWinUsbReadinessStatus::DuplicateEventPipe,
        (_, acl_in, _) if acl_in > 1 => DedicatedWinUsbReadinessStatus::DuplicateAclInPipe,
        (_, _, acl_out) if acl_out > 1 => DedicatedWinUsbReadinessStatus::DuplicateAclOutPipe,
        (0, _, _) => DedicatedWinUsbReadinessStatus::MissingEventPipe,
        (_, 0, _) => DedicatedWinUsbReadinessStatus::MissingAclInPipe,
        (_, _, 0) => DedicatedWinUsbReadinessStatus::MissingAclOutPipe,
        _ => DedicatedWinUsbReadinessStatus::Ready,
    }
}

#[cfg(any(windows, test))]
fn pipe_summary(pipes: &[WinUsbPipe]) -> (Option<u8>, Option<u8>, Option<u8>, Vec<WinUsbPipe>) {
    let matching = |pipe_type, direction| {
        pipes
            .iter()
            .filter(move |pipe| pipe.pipe_type == pipe_type && pipe.direction == direction)
            .copied()
            .collect::<Vec<_>>()
    };
    let event = matching(WinUsbPipeType::Interrupt, WinUsbPipeDirection::In);
    let acl_in = matching(WinUsbPipeType::Bulk, WinUsbPipeDirection::In);
    let acl_out = matching(WinUsbPipeType::Bulk, WinUsbPipeDirection::Out);
    let unique_id = |candidates: &[WinUsbPipe]| match candidates {
        [pipe] => Some(pipe.pipe_id),
        _ => None,
    };
    let extras = pipes
        .iter()
        .filter(|pipe| !event.contains(pipe) && !acl_in.contains(pipe) && !acl_out.contains(pipe))
        .copied()
        .collect();

    (
        unique_id(&event),
        unique_id(&acl_in),
        unique_id(&acl_out),
        extras,
    )
}

#[cfg(not(windows))]
mod platform {
    use super::*;

    pub(super) fn probe() -> DedicatedWinUsbProbeReport {
        DedicatedWinUsbProbeReport {
            status: DedicatedWinUsbProbeStatus::UnsupportedPlatform,
            controllers: Vec::new(),
        }
    }

    pub(super) fn application_interface_is_active(
        _instance_id: &str,
        _interface_guid: &str,
    ) -> Result<bool, String> {
        Err("WinUSB application-interface inspection is only available on Windows".into())
    }
}

#[cfg(windows)]
mod platform {
    use std::collections::HashMap;
    use std::mem::size_of;

    use directhci_core::ControllerObservation;
    use windows::Win32::Devices::DeviceAndDriverInstallation::{
        DICS_FLAG_GLOBAL, DIGCF_ALLCLASSES, DIGCF_PRESENT, DIREG_DEV, HDEVINFO, SP_DEVINFO_DATA,
        SetupDiDestroyDeviceInfoList, SetupDiGetClassDevsW, SetupDiOpenDevRegKey,
        SetupDiOpenDeviceInfoW,
    };
    use windows::Win32::Devices::Usb::{
        USB_INTERFACE_DESCRIPTOR, USBD_PIPE_TYPE, UsbdPipeTypeBulk, UsbdPipeTypeControl,
        UsbdPipeTypeInterrupt, UsbdPipeTypeIsochronous, WINUSB_INTERFACE_HANDLE,
        WINUSB_PIPE_INFORMATION, WinUsb_Free, WinUsb_Initialize, WinUsb_QueryInterfaceSettings,
        WinUsb_QueryPipe,
    };
    use windows::Win32::Foundation::{
        CloseHandle, ERROR_FILE_NOT_FOUND, GENERIC_READ, GENERIC_WRITE, HANDLE,
    };
    use windows::Win32::Storage::FileSystem::{
        CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_FLAG_OVERLAPPED, FILE_SHARE_READ,
        FILE_SHARE_WRITE, OPEN_EXISTING,
    };
    use windows::Win32::System::Registry::{
        HKEY, KEY_READ, REG_MULTI_SZ, REG_SZ, REG_VALUE_TYPE, RegCloseKey, RegQueryValueExW,
    };
    use windows::core::{GUID, PCWSTR, w};

    use super::*;
    use crate::enumeration::enumerate_interface_paths;

    pub(super) fn probe() -> DedicatedWinUsbProbeReport {
        let controllers = match crate::enumerate_controllers() {
            Ok(controllers) => controllers,
            Err(error) => {
                return DedicatedWinUsbProbeReport {
                    status: DedicatedWinUsbProbeStatus::EnumerationFailed {
                        message: error.to_string(),
                    },
                    controllers: Vec::new(),
                };
            }
        };

        let id_counts = controllers
            .iter()
            .fold(HashMap::new(), |mut counts, controller| {
                *counts
                    .entry(controller.id.as_str().to_owned())
                    .or_insert(0usize) += 1;
                counts
            });
        let results = controllers
            .iter()
            .map(|controller| {
                probe_controller(
                    controller,
                    id_counts.get(controller.id.as_str()).copied().unwrap_or(0),
                )
            })
            .collect();

        DedicatedWinUsbProbeReport {
            status: DedicatedWinUsbProbeStatus::Succeeded,
            controllers: results,
        }
    }

    pub(super) fn application_interface_is_active(
        instance_id: &str,
        interface_guid: &str,
    ) -> Result<bool, String> {
        let bare = interface_guid
            .trim()
            .trim_start_matches('{')
            .trim_end_matches('}');
        let guid = GUID::try_from(bare)
            .map_err(|_| format!("invalid application interface GUID `{interface_guid}`"))?;
        let paths = enumerate_interface_paths(&guid).map_err(|error| error.to_string())?;
        Ok(paths
            .get(&instance_id.to_ascii_uppercase())
            .is_some_and(|paths| !paths.is_empty()))
    }

    fn probe_controller(
        controller: &ControllerObservation,
        id_observations: usize,
    ) -> DedicatedWinUsbControllerReadiness {
        let mut result = empty_result(controller);
        if !controller
            .service
            .as_deref()
            .is_some_and(|service| service.eq_ignore_ascii_case("WINUSB"))
        {
            return result;
        }
        if id_observations != 1 {
            result.status = DedicatedWinUsbReadinessStatus::AmbiguousControllerId {
                observations: id_observations,
            };
            return result;
        }

        let registration =
            match application_interface_registration(&controller.identity.instance_id) {
                Ok(registration) => registration,
                Err(message) => {
                    result.status =
                        DedicatedWinUsbReadinessStatus::ApplicationInterfaceDiscoveryFailed {
                            message,
                        };
                    return result;
                }
            };
        result.registered_interface_guids = registration
            .guids
            .iter()
            .map(|(_, text)| text.clone())
            .collect();
        if !registration.invalid_values.is_empty() {
            result.status =
                DedicatedWinUsbReadinessStatus::InvalidApplicationInterfaceRegistration {
                    values: registration.invalid_values,
                };
            return result;
        }

        let target_instance = controller.identity.instance_id.to_ascii_uppercase();
        for (guid, guid_text) in registration.guids {
            let paths = match enumerate_interface_paths(&guid) {
                Ok(paths) => paths,
                Err(error) => {
                    result.status =
                        DedicatedWinUsbReadinessStatus::ApplicationInterfaceDiscoveryFailed {
                            message: error.to_string(),
                        };
                    return result;
                }
            };
            for path in paths.get(&target_instance).into_iter().flatten() {
                if !result
                    .application_interfaces
                    .iter()
                    .any(|known| known.path.eq_ignore_ascii_case(path))
                {
                    result
                        .application_interfaces
                        .push(WinUsbApplicationInterface {
                            class_guid: guid_text.clone(),
                            path: path.clone(),
                        });
                }
            }
        }

        match result.application_interfaces.as_slice() {
            [] => {
                result.status = DedicatedWinUsbReadinessStatus::NoApplicationInterface;
                result
            }
            [_] => inspect_application_interface(result),
            interfaces => {
                result.status = DedicatedWinUsbReadinessStatus::AmbiguousApplicationInterface {
                    interfaces: interfaces.len(),
                };
                result
            }
        }
    }

    fn empty_result(controller: &ControllerObservation) -> DedicatedWinUsbControllerReadiness {
        DedicatedWinUsbControllerReadiness {
            controller_id: controller.id.clone(),
            instance_id: controller.identity.instance_id.clone(),
            service: controller.service.clone(),
            driver_inf: controller.driver.inf_path.clone(),
            driver_provider: controller.driver.provider.clone(),
            driver_version: controller.driver.version.clone(),
            registered_interface_guids: Vec::new(),
            application_interfaces: Vec::new(),
            interface: None,
            pipes: Vec::new(),
            event_pipe: None,
            acl_in_pipe: None,
            acl_out_pipe: None,
            extra_pipes: Vec::new(),
            status: DedicatedWinUsbReadinessStatus::NotWinUsbBound,
        }
    }

    fn inspect_application_interface(
        mut result: DedicatedWinUsbControllerReadiness,
    ) -> DedicatedWinUsbControllerReadiness {
        let path = result.application_interfaces[0].path.clone();
        let device = match WinUsbDevice::open(&path) {
            Ok(device) => device,
            Err(OpenError::File(message)) => {
                result.status = DedicatedWinUsbReadinessStatus::OpenFailed { message };
                return result;
            }
            Err(OpenError::Initialize(message)) => {
                result.status = DedicatedWinUsbReadinessStatus::WinUsbInitializeFailed { message };
                return result;
            }
        };
        let (interface, pipes) = match device.inspect() {
            Ok(summary) => summary,
            Err(InspectError::Interface(message)) => {
                result.status = DedicatedWinUsbReadinessStatus::InterfaceQueryFailed { message };
                return result;
            }
            Err(InspectError::Pipe { index, message }) => {
                result.status = DedicatedWinUsbReadinessStatus::PipeQueryFailed { index, message };
                return result;
            }
        };

        let (event_pipe, acl_in_pipe, acl_out_pipe, extra_pipes) = pipe_summary(&pipes);
        result.status = classify_readiness(interface, &pipes);
        result.interface = Some(interface);
        result.event_pipe = event_pipe;
        result.acl_in_pipe = acl_in_pipe;
        result.acl_out_pipe = acl_out_pipe;
        result.extra_pipes = extra_pipes;
        result.pipes = pipes;
        result
    }

    struct ApplicationInterfaceRegistration {
        guids: Vec<(GUID, String)>,
        invalid_values: Vec<String>,
    }

    fn application_interface_registration(
        instance_id: &str,
    ) -> Result<ApplicationInterfaceRegistration, String> {
        let device_set = DeviceInfoSet::all_present()?;
        let mut device_info = SP_DEVINFO_DATA {
            cbSize: size_of::<SP_DEVINFO_DATA>() as u32,
            ..Default::default()
        };
        let instance_id_wide = wide_null(instance_id);
        unsafe {
            SetupDiOpenDeviceInfoW(
                device_set.0,
                PCWSTR(instance_id_wide.as_ptr()),
                None,
                0,
                Some(&mut device_info),
            )
        }
        .map_err(|error| format!("SetupDiOpenDeviceInfoW failed: {error}"))?;

        let key = unsafe {
            SetupDiOpenDevRegKey(
                device_set.0,
                &device_info,
                DICS_FLAG_GLOBAL.0,
                0,
                DIREG_DEV,
                KEY_READ.0,
            )
        }
        .map(RegistryKey)
        .map_err(|error| format!("SetupDiOpenDevRegKey failed: {error}"))?;

        let mut values = registry_strings(key.0, w!("DeviceInterfaceGUIDs"))?;
        values.extend(registry_strings(key.0, w!("DeviceInterfaceGUID"))?);
        values.sort_by_key(|value| value.to_ascii_uppercase());
        values.dedup_by(|left, right| left.eq_ignore_ascii_case(right));

        let mut guids = Vec::new();
        let mut invalid_values = Vec::new();
        for value in values {
            let bare = value.trim().trim_start_matches('{').trim_end_matches('}');
            match GUID::try_from(bare) {
                Ok(guid) => guids.push((guid, format!("{{{guid:?}}}"))),
                Err(_) => invalid_values.push(value),
            }
        }
        Ok(ApplicationInterfaceRegistration {
            guids,
            invalid_values,
        })
    }

    fn registry_strings(key: HKEY, name: PCWSTR) -> Result<Vec<String>, String> {
        let mut value_type = REG_VALUE_TYPE::default();
        let mut byte_count = 0u32;
        let first = unsafe {
            RegQueryValueExW(
                key,
                name,
                None,
                Some(&mut value_type),
                None,
                Some(&mut byte_count),
            )
        };
        if first == ERROR_FILE_NOT_FOUND {
            return Ok(Vec::new());
        }
        if !first.is_ok() {
            return Err(format!("RegQueryValueExW size query failed: {first:?}"));
        }
        if byte_count == 0 {
            return Ok(Vec::new());
        }
        if value_type != REG_SZ && value_type != REG_MULTI_SZ {
            return Err(format!(
                "WinUSB application interface registration has unexpected registry type {}",
                value_type.0
            ));
        }

        let mut buffer = vec![0u8; byte_count as usize];
        let second = unsafe {
            RegQueryValueExW(
                key,
                name,
                None,
                Some(&mut value_type),
                Some(buffer.as_mut_ptr()),
                Some(&mut byte_count),
            )
        };
        if !second.is_ok() {
            return Err(format!("RegQueryValueExW data query failed: {second:?}"));
        }
        buffer.truncate(byte_count as usize);
        Ok(decode_utf16_multi_string(&buffer))
    }

    fn decode_utf16_multi_string(bytes: &[u8]) -> Vec<String> {
        let units: Vec<u16> = bytes
            .chunks_exact(2)
            .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
            .collect();
        units
            .split(|unit| *unit == 0)
            .filter(|part| !part.is_empty())
            .map(String::from_utf16_lossy)
            .collect()
    }

    fn wide_null(value: &str) -> Vec<u16> {
        value.encode_utf16().chain(std::iter::once(0)).collect()
    }

    struct DeviceInfoSet(HDEVINFO);

    impl DeviceInfoSet {
        fn all_present() -> Result<Self, String> {
            unsafe {
                SetupDiGetClassDevsW(None, PCWSTR::null(), None, DIGCF_ALLCLASSES | DIGCF_PRESENT)
            }
            .map(Self)
            .map_err(|error| format!("SetupDiGetClassDevsW failed: {error}"))
        }
    }

    impl Drop for DeviceInfoSet {
        fn drop(&mut self) {
            let _ = unsafe { SetupDiDestroyDeviceInfoList(self.0) };
        }
    }

    struct RegistryKey(HKEY);

    impl Drop for RegistryKey {
        fn drop(&mut self) {
            let _ = unsafe { RegCloseKey(self.0) };
        }
    }

    enum OpenError {
        File(String),
        Initialize(String),
    }

    enum InspectError {
        Interface(String),
        Pipe { index: u8, message: String },
    }

    struct WinUsbDevice {
        // Fields drop in declaration order: WinUSB first, then its file.
        interface: OwnedWinUsbHandle,
        _file: OwnedFileHandle,
    }

    impl WinUsbDevice {
        fn open(path: &str) -> Result<Self, OpenError> {
            let path_wide = wide_null(path);
            let file = unsafe {
                CreateFileW(
                    PCWSTR(path_wide.as_ptr()),
                    (GENERIC_READ | GENERIC_WRITE).0,
                    FILE_SHARE_READ | FILE_SHARE_WRITE,
                    None,
                    OPEN_EXISTING,
                    FILE_ATTRIBUTE_NORMAL | FILE_FLAG_OVERLAPPED,
                    None,
                )
            }
            .map(OwnedFileHandle)
            .map_err(|error| OpenError::File(error.to_string()))?;

            let mut interface = WINUSB_INTERFACE_HANDLE::default();
            unsafe { WinUsb_Initialize(file.0, &mut interface) }
                .map_err(|error| OpenError::Initialize(error.to_string()))?;
            Ok(Self {
                interface: OwnedWinUsbHandle(interface),
                _file: file,
            })
        }

        fn inspect(&self) -> Result<(WinUsbInterfaceDescriptor, Vec<WinUsbPipe>), InspectError> {
            let mut descriptor = USB_INTERFACE_DESCRIPTOR::default();
            unsafe { WinUsb_QueryInterfaceSettings(self.interface.0, 0, &mut descriptor) }
                .map_err(|error| InspectError::Interface(error.to_string()))?;
            let summary = WinUsbInterfaceDescriptor {
                number: descriptor.bInterfaceNumber,
                alternate_setting: descriptor.bAlternateSetting,
                endpoint_count: descriptor.bNumEndpoints,
                class: descriptor.bInterfaceClass,
                subclass: descriptor.bInterfaceSubClass,
                protocol: descriptor.bInterfaceProtocol,
            };

            let mut pipes = Vec::with_capacity(descriptor.bNumEndpoints as usize);
            for index in 0..descriptor.bNumEndpoints {
                let mut pipe = WINUSB_PIPE_INFORMATION::default();
                unsafe { WinUsb_QueryPipe(self.interface.0, 0, index, &mut pipe) }.map_err(
                    |error| InspectError::Pipe {
                        index,
                        message: error.to_string(),
                    },
                )?;
                pipes.push(WinUsbPipe {
                    pipe_type: pipe_type(pipe.PipeType),
                    direction: if pipe.PipeId & 0x80 != 0 {
                        WinUsbPipeDirection::In
                    } else {
                        WinUsbPipeDirection::Out
                    },
                    pipe_id: pipe.PipeId,
                    maximum_packet_size: pipe.MaximumPacketSize,
                    interval: pipe.Interval,
                });
            }
            Ok((summary, pipes))
        }
    }

    fn pipe_type(value: USBD_PIPE_TYPE) -> WinUsbPipeType {
        match value {
            value if value == UsbdPipeTypeControl => WinUsbPipeType::Control,
            value if value == UsbdPipeTypeIsochronous => WinUsbPipeType::Isochronous,
            value if value == UsbdPipeTypeBulk => WinUsbPipeType::Bulk,
            value if value == UsbdPipeTypeInterrupt => WinUsbPipeType::Interrupt,
            value => WinUsbPipeType::Unknown(value.0),
        }
    }

    struct OwnedWinUsbHandle(WINUSB_INTERFACE_HANDLE);

    impl Drop for OwnedWinUsbHandle {
        fn drop(&mut self) {
            let _ = unsafe { WinUsb_Free(self.0) };
        }
    }

    struct OwnedFileHandle(HANDLE);

    impl Drop for OwnedFileHandle {
        fn drop(&mut self) {
            let _ = unsafe { CloseHandle(self.0) };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bluetooth_interface() -> WinUsbInterfaceDescriptor {
        WinUsbInterfaceDescriptor {
            number: 0,
            alternate_setting: 0,
            endpoint_count: 3,
            class: 0xe0,
            subclass: 0x01,
            protocol: 0x01,
        }
    }

    fn pipe(pipe_type: WinUsbPipeType, direction: WinUsbPipeDirection, pipe_id: u8) -> WinUsbPipe {
        WinUsbPipe {
            pipe_type,
            direction,
            pipe_id,
            maximum_packet_size: 64,
            interval: 1,
        }
    }

    fn required_pipes() -> Vec<WinUsbPipe> {
        vec![
            pipe(WinUsbPipeType::Interrupt, WinUsbPipeDirection::In, 0x81),
            pipe(WinUsbPipeType::Bulk, WinUsbPipeDirection::In, 0x82),
            pipe(WinUsbPipeType::Bulk, WinUsbPipeDirection::Out, 0x02),
        ]
    }

    #[test]
    fn accepts_descriptor_driven_hci_topology() {
        assert_eq!(
            classify_readiness(bluetooth_interface(), &required_pipes()),
            DedicatedWinUsbReadinessStatus::Ready
        );
    }

    #[test]
    fn rejects_unexpected_interface_class() {
        let mut interface = bluetooth_interface();
        interface.class = 0xff;
        assert_eq!(
            classify_readiness(interface, &required_pipes()),
            DedicatedWinUsbReadinessStatus::UnexpectedInterface
        );
    }

    #[test]
    fn reports_each_missing_required_pipe() {
        let required = required_pipes();
        let cases = [
            (0, DedicatedWinUsbReadinessStatus::MissingEventPipe),
            (1, DedicatedWinUsbReadinessStatus::MissingAclInPipe),
            (2, DedicatedWinUsbReadinessStatus::MissingAclOutPipe),
        ];
        for (missing, expected) in cases {
            let pipes = required
                .iter()
                .enumerate()
                .filter(|(index, _)| *index != missing)
                .map(|(_, pipe)| *pipe)
                .collect::<Vec<_>>();
            assert_eq!(classify_readiness(bluetooth_interface(), &pipes), expected);
        }
    }

    #[test]
    fn rejects_duplicate_required_pipe_roles() {
        let cases = [
            (
                pipe(WinUsbPipeType::Interrupt, WinUsbPipeDirection::In, 0x83),
                DedicatedWinUsbReadinessStatus::DuplicateEventPipe,
            ),
            (
                pipe(WinUsbPipeType::Bulk, WinUsbPipeDirection::In, 0x83),
                DedicatedWinUsbReadinessStatus::DuplicateAclInPipe,
            ),
            (
                pipe(WinUsbPipeType::Bulk, WinUsbPipeDirection::Out, 0x03),
                DedicatedWinUsbReadinessStatus::DuplicateAclOutPipe,
            ),
        ];
        for (duplicate, expected) in cases {
            let mut pipes = required_pipes();
            pipes.push(duplicate);
            assert_eq!(classify_readiness(bluetooth_interface(), &pipes), expected);
        }
    }

    #[test]
    fn records_isochronous_and_unrelated_pipes_as_extras() {
        let mut pipes = required_pipes();
        pipes.push(pipe(
            WinUsbPipeType::Isochronous,
            WinUsbPipeDirection::In,
            0x84,
        ));
        let (event, acl_in, acl_out, extras) = pipe_summary(&pipes);
        assert_eq!(
            (event, acl_in, acl_out),
            (Some(0x81), Some(0x82), Some(0x02))
        );
        assert_eq!(extras, vec![pipes[3]]);
    }
}
