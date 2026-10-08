use std::collections::HashMap;
use std::mem::{offset_of, size_of};

use directhci_core::{ControllerIdentity, ControllerObservation, DeviceStatus, DriverObservation};
use windows::Win32::Devices::DeviceAndDriverInstallation::{
    CM_Get_DevNode_Status, CM_Get_Device_IDW, CM_Get_Parent, DIGCF_ALLCLASSES,
    DIGCF_DEVICEINTERFACE, DIGCF_PRESENT, HDEVINFO, SP_DEVICE_INTERFACE_DATA,
    SP_DEVICE_INTERFACE_DETAIL_DATA_W, SP_DEVINFO_DATA, SPDRP_CLASS, SPDRP_CLASSGUID,
    SPDRP_COMPATIBLEIDS, SPDRP_DEVICEDESC, SPDRP_DRIVER, SPDRP_ENUMERATOR_NAME, SPDRP_HARDWAREID,
    SPDRP_MFG, SPDRP_SERVICE, SetupDiDestroyDeviceInfoList, SetupDiEnumDeviceInfo,
    SetupDiEnumDeviceInterfaces, SetupDiGetClassDevsW, SetupDiGetDeviceInstanceIdW,
    SetupDiGetDeviceInterfaceDetailW, SetupDiGetDevicePropertyW, SetupDiGetDeviceRegistryPropertyW,
};
use windows::Win32::Devices::Properties::{
    DEVPKEY_Device_BusReportedDeviceDesc, DEVPKEY_Device_ContainerId, DEVPKEY_Device_DriverInfPath,
    DEVPKEY_Device_DriverProvider, DEVPKEY_Device_DriverVersion, DEVPKEY_Device_LocationPaths,
    DEVPROPTYPE,
};
use windows::Win32::Devices::Usb::GUID_DEVINTERFACE_USB_DEVICE;
use windows::Win32::Foundation::{
    DEVPROPKEY, ERROR_INSUFFICIENT_BUFFER, ERROR_NO_MORE_ITEMS, GetLastError,
};
use windows::core::{GUID, PCWSTR};

use crate::Error;

const BLUETOOTH_CLASS_GUID: &str = "{E0CBF06C-CD8B-4647-BB8A-263B43F0F974}";
const MAX_DEVICE_ID_LEN: usize = 200;

pub(super) fn enumerate_controllers() -> Result<Vec<ControllerObservation>, Error> {
    let interface_paths = enumerate_interface_paths(&GUID_DEVINTERFACE_USB_DEVICE)?;
    let device_set = DeviceInfoSet::all_present()?;
    let mut observations = Vec::new();
    let mut index = 0;

    loop {
        let mut device_info = new_device_info_data();
        let result = unsafe { SetupDiEnumDeviceInfo(device_set.0, index, &mut device_info) };
        if let Err(error) = result {
            if unsafe { GetLastError() } == ERROR_NO_MORE_ITEMS {
                break;
            }
            return Err(api_error("SetupDiEnumDeviceInfo", error));
        }
        index += 1;

        let instance_id = get_instance_id(device_set.0, &device_info)?;
        let hardware_ids = get_registry_strings(device_set.0, &device_info, SPDRP_HARDWAREID)?;
        let compatible_ids = get_registry_strings(device_set.0, &device_info, SPDRP_COMPATIBLEIDS)?;
        let class_guid = get_registry_string(device_set.0, &device_info, SPDRP_CLASSGUID)?;
        let service = get_registry_string(device_set.0, &device_info, SPDRP_SERVICE)?;

        if !is_bluetooth_usb_candidate(
            &instance_id,
            &hardware_ids,
            &compatible_ids,
            class_guid.as_deref(),
            service.as_deref(),
        ) {
            continue;
        }

        let (usb_vendor_id, usb_product_id) = parse_vid_pid(&hardware_ids, &instance_id);
        // USB serial extraction needs topology-aware validation. The instance ID
        // suffix is deliberately not treated as a serial in this milestone.
        let identity = ControllerIdentity::new(
            usb_vendor_id,
            usb_product_id,
            None,
            get_device_guid_property(device_set.0, &device_info, &DEVPKEY_Device_ContainerId)?,
            instance_id.clone(),
            get_device_string_list_property(
                device_set.0,
                &device_info,
                &DEVPKEY_Device_LocationPaths,
            )?,
            get_parent_instance_id(device_info.DevInst),
            hardware_ids,
            compatible_ids,
        );

        let observation = ControllerObservation::new(
            identity,
            get_registry_string(device_set.0, &device_info, SPDRP_DEVICEDESC)?,
            get_device_string_property(
                device_set.0,
                &device_info,
                &DEVPKEY_Device_BusReportedDeviceDesc,
            )?,
            get_registry_string(device_set.0, &device_info, SPDRP_MFG)?,
            get_registry_string(device_set.0, &device_info, SPDRP_CLASS)?,
            class_guid,
            get_registry_string(device_set.0, &device_info, SPDRP_ENUMERATOR_NAME)?,
            service,
            DriverObservation {
                inf_path: get_device_string_property(
                    device_set.0,
                    &device_info,
                    &DEVPKEY_Device_DriverInfPath,
                )?,
                provider: get_device_string_property(
                    device_set.0,
                    &device_info,
                    &DEVPKEY_Device_DriverProvider,
                )?,
                version: get_device_string_property(
                    device_set.0,
                    &device_info,
                    &DEVPKEY_Device_DriverVersion,
                )?,
                driver_key: get_registry_string(device_set.0, &device_info, SPDRP_DRIVER)?,
            },
            interface_paths
                .get(&instance_id.to_ascii_uppercase())
                .cloned()
                .unwrap_or_default(),
            get_device_status(device_info.DevInst),
        );
        observations.push(observation);
    }

    observations.sort_by(|left, right| left.id.cmp(&right.id));
    Ok(observations)
}

struct DeviceInfoSet(HDEVINFO);

impl DeviceInfoSet {
    fn all_present() -> Result<Self, Error> {
        let handle = unsafe {
            SetupDiGetClassDevsW(None, PCWSTR::null(), None, DIGCF_ALLCLASSES | DIGCF_PRESENT)
        }
        .map_err(|error| api_error("SetupDiGetClassDevsW", error))?;
        Ok(Self(handle))
    }

    fn interfaces(interface_guid: &GUID) -> Result<Self, Error> {
        let handle = unsafe {
            SetupDiGetClassDevsW(
                Some(interface_guid),
                PCWSTR::null(),
                None,
                DIGCF_DEVICEINTERFACE | DIGCF_PRESENT,
            )
        }
        .map_err(|error| api_error("SetupDiGetClassDevsW(USB interfaces)", error))?;
        Ok(Self(handle))
    }
}

impl Drop for DeviceInfoSet {
    fn drop(&mut self) {
        let _ = unsafe { SetupDiDestroyDeviceInfoList(self.0) };
    }
}

pub(super) fn enumerate_interface_paths(
    interface_guid: &GUID,
) -> Result<HashMap<String, Vec<String>>, Error> {
    let device_set = DeviceInfoSet::interfaces(interface_guid)?;
    let mut result: HashMap<String, Vec<String>> = HashMap::new();
    let mut index = 0;

    loop {
        let mut interface_data = SP_DEVICE_INTERFACE_DATA {
            cbSize: size_of::<SP_DEVICE_INTERFACE_DATA>() as u32,
            ..Default::default()
        };
        let enumeration = unsafe {
            SetupDiEnumDeviceInterfaces(
                device_set.0,
                None,
                interface_guid,
                index,
                &mut interface_data,
            )
        };
        if let Err(error) = enumeration {
            if unsafe { GetLastError() } == ERROR_NO_MORE_ITEMS {
                break;
            }
            return Err(api_error("SetupDiEnumDeviceInterfaces", error));
        }
        index += 1;

        let mut required_size = 0;
        let _ = unsafe {
            SetupDiGetDeviceInterfaceDetailW(
                device_set.0,
                &interface_data,
                None,
                0,
                Some(&mut required_size),
                None,
            )
        };
        if required_size == 0 || unsafe { GetLastError() } != ERROR_INSUFFICIENT_BUFFER {
            continue;
        }

        // SetupAPI writes a structure followed by a variable-length UTF-16
        // path. Vec<u8> would not guarantee the structure's alignment.
        let storage_words = (required_size as usize).div_ceil(size_of::<usize>());
        let mut detail_storage = vec![0_usize; storage_words];
        let detail_ptr = detail_storage
            .as_mut_ptr()
            .cast::<SP_DEVICE_INTERFACE_DETAIL_DATA_W>();
        unsafe {
            (*detail_ptr).cbSize = size_of::<SP_DEVICE_INTERFACE_DETAIL_DATA_W>() as u32;
        }
        let mut device_info = new_device_info_data();
        unsafe {
            SetupDiGetDeviceInterfaceDetailW(
                device_set.0,
                &interface_data,
                Some(detail_ptr),
                required_size,
                None,
                Some(&mut device_info),
            )
        }
        .map_err(|error| api_error("SetupDiGetDeviceInterfaceDetailW", error))?;

        let instance_id = get_instance_id(device_set.0, &device_info)?.to_ascii_uppercase();
        let path_offset = offset_of!(SP_DEVICE_INTERFACE_DETAIL_DATA_W, DevicePath);
        let path_capacity = (required_size as usize).saturating_sub(path_offset) / size_of::<u16>();
        let path_pointer = unsafe {
            detail_storage
                .as_ptr()
                .cast::<u8>()
                .add(path_offset)
                .cast::<u16>()
        };
        let path_units = unsafe { std::slice::from_raw_parts(path_pointer, path_capacity) };
        let path = decode_utf16_string(path_units);
        if !path.is_empty() {
            result.entry(instance_id).or_default().push(path);
        }
    }

    Ok(result)
}

fn new_device_info_data() -> SP_DEVINFO_DATA {
    SP_DEVINFO_DATA {
        cbSize: size_of::<SP_DEVINFO_DATA>() as u32,
        ..Default::default()
    }
}

fn get_instance_id(device_set: HDEVINFO, device_info: &SP_DEVINFO_DATA) -> Result<String, Error> {
    let mut required_size = 0;
    let _ = unsafe {
        SetupDiGetDeviceInstanceIdW(device_set, device_info, None, Some(&mut required_size))
    };
    if required_size == 0 {
        return Err(Error::WindowsApi(
            "SetupDiGetDeviceInstanceIdW returned an empty size".into(),
        ));
    }
    let mut buffer = vec![0_u16; required_size as usize];
    unsafe {
        SetupDiGetDeviceInstanceIdW(
            device_set,
            device_info,
            Some(&mut buffer),
            Some(&mut required_size),
        )
    }
    .map_err(|error| api_error("SetupDiGetDeviceInstanceIdW", error))?;
    Ok(decode_utf16_string(&buffer))
}

fn get_registry_string(
    device_set: HDEVINFO,
    device_info: &SP_DEVINFO_DATA,
    property: windows::Win32::Devices::DeviceAndDriverInstallation::SETUP_DI_REGISTRY_PROPERTY,
) -> Result<Option<String>, Error> {
    Ok(get_registry_strings(device_set, device_info, property)?
        .into_iter()
        .next())
}

fn get_registry_strings(
    device_set: HDEVINFO,
    device_info: &SP_DEVINFO_DATA,
    property: windows::Win32::Devices::DeviceAndDriverInstallation::SETUP_DI_REGISTRY_PROPERTY,
) -> Result<Vec<String>, Error> {
    let Some(buffer) = get_registry_property_bytes(device_set, device_info, property)? else {
        return Ok(Vec::new());
    };
    Ok(decode_utf16_multi_string(&buffer))
}

fn get_registry_property_bytes(
    device_set: HDEVINFO,
    device_info: &SP_DEVINFO_DATA,
    property: windows::Win32::Devices::DeviceAndDriverInstallation::SETUP_DI_REGISTRY_PROPERTY,
) -> Result<Option<Vec<u8>>, Error> {
    let mut required_size = 0;
    let first = unsafe {
        SetupDiGetDeviceRegistryPropertyW(
            device_set,
            device_info,
            property,
            None,
            None,
            Some(&mut required_size),
        )
    };
    if first.is_ok() && required_size == 0 {
        return Ok(None);
    }
    if unsafe { GetLastError() } != ERROR_INSUFFICIENT_BUFFER || required_size == 0 {
        return Ok(None);
    }

    let mut buffer = vec![0_u8; required_size as usize];
    unsafe {
        SetupDiGetDeviceRegistryPropertyW(
            device_set,
            device_info,
            property,
            None,
            Some(&mut buffer),
            Some(&mut required_size),
        )
    }
    .map_err(|error| api_error("SetupDiGetDeviceRegistryPropertyW", error))?;
    Ok(Some(buffer))
}

fn get_device_string_property(
    device_set: HDEVINFO,
    device_info: &SP_DEVINFO_DATA,
    property: &DEVPROPKEY,
) -> Result<Option<String>, Error> {
    let Some(buffer) = get_device_property_bytes(device_set, device_info, property)? else {
        return Ok(None);
    };
    Ok(decode_utf16_multi_string(&buffer).into_iter().next())
}

fn get_device_string_list_property(
    device_set: HDEVINFO,
    device_info: &SP_DEVINFO_DATA,
    property: &DEVPROPKEY,
) -> Result<Vec<String>, Error> {
    let Some(buffer) = get_device_property_bytes(device_set, device_info, property)? else {
        return Ok(Vec::new());
    };
    Ok(decode_utf16_multi_string(&buffer))
}

fn get_device_guid_property(
    device_set: HDEVINFO,
    device_info: &SP_DEVINFO_DATA,
    property: &DEVPROPKEY,
) -> Result<Option<String>, Error> {
    let Some(buffer) = get_device_property_bytes(device_set, device_info, property)? else {
        return Ok(None);
    };
    if buffer.len() < size_of::<GUID>() {
        return Ok(None);
    }
    let guid = unsafe { std::ptr::read_unaligned(buffer.as_ptr().cast::<GUID>()) };
    Ok(Some(format!("{{{guid:?}}}")))
}

fn get_device_property_bytes(
    device_set: HDEVINFO,
    device_info: &SP_DEVINFO_DATA,
    property: &DEVPROPKEY,
) -> Result<Option<Vec<u8>>, Error> {
    let mut required_size = 0;
    let mut property_type = DEVPROPTYPE::default();
    let first = unsafe {
        SetupDiGetDevicePropertyW(
            device_set,
            device_info,
            property,
            &mut property_type,
            None,
            Some(&mut required_size),
            0,
        )
    };
    if first.is_ok() && required_size == 0 {
        return Ok(None);
    }
    if unsafe { GetLastError() } != ERROR_INSUFFICIENT_BUFFER || required_size == 0 {
        return Ok(None);
    }

    let mut buffer = vec![0_u8; required_size as usize];
    unsafe {
        SetupDiGetDevicePropertyW(
            device_set,
            device_info,
            property,
            &mut property_type,
            Some(&mut buffer),
            Some(&mut required_size),
            0,
        )
    }
    .map_err(|error| api_error("SetupDiGetDevicePropertyW", error))?;
    Ok(Some(buffer))
}

fn get_parent_instance_id(devinst: u32) -> Option<String> {
    let mut parent = 0;
    if unsafe { CM_Get_Parent(&mut parent, devinst, 0) }.0 != 0 {
        return None;
    }
    let mut buffer = [0_u16; MAX_DEVICE_ID_LEN];
    if unsafe { CM_Get_Device_IDW(parent, &mut buffer, 0) }.0 != 0 {
        return None;
    }
    Some(decode_utf16_string(&buffer))
}

fn get_device_status(devinst: u32) -> DeviceStatus {
    let mut status = Default::default();
    let mut problem = Default::default();
    if unsafe { CM_Get_DevNode_Status(&mut status, &mut problem, devinst, 0) }.0 == 0 {
        DeviceStatus {
            present: true,
            status_flags: Some(status.0),
            problem_code: Some(problem.0),
        }
    } else {
        DeviceStatus {
            present: true,
            status_flags: None,
            problem_code: None,
        }
    }
}

fn is_bluetooth_usb_candidate(
    instance_id: &str,
    hardware_ids: &[String],
    compatible_ids: &[String],
    class_guid: Option<&str>,
    service: Option<&str>,
) -> bool {
    let instance_id = instance_id.to_ascii_uppercase();
    if !instance_id.starts_with("USB\\") {
        return false;
    }

    let ids = hardware_ids.iter().chain(compatible_ids);
    let bluetooth_usb_class = ids
        .map(|id| id.to_ascii_uppercase())
        .any(|id| id.contains("CLASS_E0") && id.contains("SUBCLASS_01") && id.contains("PROT_01"));
    let bluetooth_setup_class =
        class_guid.is_some_and(|value| value.eq_ignore_ascii_case(BLUETOOTH_CLASS_GUID));
    let bluetooth_service = service.is_some_and(|value| {
        value.eq_ignore_ascii_case("BTHUSB") || value.eq_ignore_ascii_case("IBTUSB")
    });

    bluetooth_usb_class || bluetooth_setup_class || bluetooth_service
}

fn parse_vid_pid(hardware_ids: &[String], instance_id: &str) -> (Option<u16>, Option<u16>) {
    hardware_ids
        .iter()
        .map(String::as_str)
        .chain(std::iter::once(instance_id))
        .find_map(|value| {
            let upper = value.to_ascii_uppercase();
            let vendor = parse_hex_component(&upper, "VID_")?;
            let product = parse_hex_component(&upper, "PID_")?;
            Some((Some(vendor), Some(product)))
        })
        .unwrap_or((None, None))
}

fn parse_hex_component(value: &str, marker: &str) -> Option<u16> {
    let start = value.find(marker)? + marker.len();
    let end = start.checked_add(4)?;
    u16::from_str_radix(value.get(start..end)?, 16).ok()
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

fn decode_utf16_string(units: &[u16]) -> String {
    let length = units
        .iter()
        .position(|unit| *unit == 0)
        .unwrap_or(units.len());
    String::from_utf16_lossy(&units[..length])
}

fn api_error(operation: &str, error: windows::core::Error) -> Error {
    Error::WindowsApi(format!("{operation} failed: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_usb_vid_pid() {
        assert_eq!(
            parse_vid_pid(&["USB\\VID_1234&PID_ABCD&REV_0001".into()], "unused"),
            (Some(0x1234), Some(0xabcd))
        );
    }

    #[test]
    fn recognizes_standard_bluetooth_usb_compatible_id() {
        assert!(is_bluetooth_usb_candidate(
            "USB\\VID_1234&PID_ABCD\\VALUE",
            &[],
            &["USB\\Class_E0&SubClass_01&Prot_01".into()],
            None,
            None,
        ));
    }
}
