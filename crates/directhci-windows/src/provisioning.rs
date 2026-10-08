//! Device-specific WinUSB package template. This module does not stage or install drivers.

use crate::rebind::select_exact_usb_hardware_id;

const INF_TEMPLATE: &str =
    include_str!("../../../driver/winusb-device-specific/directhci-winusb.inf.in");

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DeviceSpecificPackageBlueprint {
    pub hardware_id: String,
    pub package_key: String,
    pub inf: String,
}

pub fn eligible_device_specific_package(
    controller: &directhci_core::ControllerObservation,
) -> Result<crate::rebind::DirectHciWinUsbPackageSpec, String> {
    if !controller.status.present || controller.status.problem_code.is_some_and(|code| code != 0) {
        return Err("controller is not present and problem-free".into());
    }
    if !controller
        .identity
        .instance_id
        .to_ascii_uppercase()
        .starts_with("USB\\")
    {
        return Err("controller is not a USB devnode".into());
    }
    if !controller.service.as_deref().is_some_and(|service| {
        service.eq_ignore_ascii_case("BTHUSB") || service.eq_ignore_ascii_case("IBTUSB")
    }) {
        return Err("controller is not currently Windows Bluetooth-owned".into());
    }
    let bluetooth_class = controller
        .identity
        .compatible_ids
        .iter()
        .any(|value| value.eq_ignore_ascii_case("USB\\CLASS_E0&SUBCLASS_01&PROT_01"));
    let bluetooth_setup_class = controller
        .class_guid
        .as_deref()
        .is_some_and(|value| value.eq_ignore_ascii_case("{E0CBF06C-CD8B-4647-BB8A-263B43F0F974}"));
    if !bluetooth_class && !bluetooth_setup_class {
        return Err("PnP observation does not establish a USB Bluetooth controller".into());
    }
    let hardware_id = select_exact_usb_hardware_id(&controller.identity.hardware_ids)
        .ok_or("no unique exact USB VID/PID[/MI] Hardware ID was observed")?;
    let mut package = crate::rebind::DirectHciWinUsbPackageSpec::device_specific();
    package.supported_hardware_ids.push(hardware_id);
    Ok(package)
}

pub fn device_specific_package_blueprint(
    hardware_id: &str,
) -> Result<DeviceSpecificPackageBlueprint, String> {
    let normalized = select_exact_usb_hardware_id(&[hardware_id.to_owned()])
        .ok_or("not an exact USB VID/PID[/MI] PnP Hardware ID")?;
    if INF_TEMPLATE.matches("@HARDWARE_ID@").count() != 1 {
        return Err("device-specific INF template has an unexpected placeholder count".into());
    }
    let package_key = normalized
        .strip_prefix("USB\\")
        .ok_or("USB hardware ID prefix is missing")?
        .replace('&', "-")
        .to_ascii_lowercase();
    Ok(DeviceSpecificPackageBlueprint {
        hardware_id: normalized.clone(),
        package_key,
        inf: INF_TEMPLATE.replacen("@HARDWARE_ID@", &normalized, 1),
    })
}
