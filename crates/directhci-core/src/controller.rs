use std::fmt;

use serde::{Deserialize, Serialize};

const SYSTEM_CONTAINER_ID: &str = "00000000-0000-0000-FFFF-FFFFFFFFFFFF";
const INVALID_CONTAINER_ID: &str = "00000000-0000-0000-0000-000000000000";

/// An opaque DirectHCI identifier derived from the best identity evidence that
/// is currently available.
///
/// The v0 identifier is a discovery aid, not proof of physical identity.
/// Callers must compare the accompanying identity evidence before performing a
/// privileged operation.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ControllerId(String);

impl ControllerId {
    pub fn from_identity(identity: &ControllerIdentity) -> Self {
        let canonical = identity.canonical_key();
        Self(format!("dhci-v0-{:016x}", fnv1a64(canonical.as_bytes())))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ControllerId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// Evidence used to recognize a physical controller across Windows PnP
/// observations.
///
/// No single field is assumed to remain stable across every driver transition.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ControllerIdentity {
    pub usb_vendor_id: Option<u16>,
    pub usb_product_id: Option<u16>,
    pub usb_serial: Option<String>,
    pub container_id: Option<String>,
    pub instance_id: String,
    pub location_paths: Vec<String>,
    pub parent_instance_id: Option<String>,
    pub hardware_ids: Vec<String>,
    pub compatible_ids: Vec<String>,
    pub confidence: ControllerIdentityConfidence,
}

impl ControllerIdentity {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        usb_vendor_id: Option<u16>,
        usb_product_id: Option<u16>,
        usb_serial: Option<String>,
        container_id: Option<String>,
        instance_id: String,
        location_paths: Vec<String>,
        parent_instance_id: Option<String>,
        hardware_ids: Vec<String>,
        compatible_ids: Vec<String>,
    ) -> Self {
        let usb_serial = normalize_identity_optional(usb_serial);
        let container_id = normalize_container_id(container_id);
        let location_paths = normalize_list(location_paths);
        let parent_instance_id = normalize_identity_optional(parent_instance_id);
        let hardware_ids = normalize_list(hardware_ids);
        let compatible_ids = normalize_list(compatible_ids);

        let confidence = if usb_serial.is_some() {
            ControllerIdentityConfidence::Serial
        } else if !location_paths.is_empty() {
            ControllerIdentityConfidence::Topology
        } else {
            ControllerIdentityConfidence::WindowsInstance
        };

        Self {
            usb_vendor_id,
            usb_product_id,
            usb_serial,
            container_id,
            instance_id: normalize(&instance_id),
            location_paths,
            parent_instance_id,
            hardware_ids,
            compatible_ids,
            confidence,
        }
    }

    fn canonical_key(&self) -> String {
        let vid_pid = format!(
            "{:04x}:{:04x}",
            self.usb_vendor_id.unwrap_or_default(),
            self.usb_product_id.unwrap_or_default()
        );

        if let Some(serial) = self.usb_serial.as_deref() {
            return format!("serial|{vid_pid}|{serial}");
        }

        if let Some(location) = self.location_paths.first() {
            return format!("topology|{vid_pid}|{location}");
        }

        format!("instance|{vid_pid}|{}", self.instance_id)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
/// Describes the primary evidence behind the current identity, not a numeric
/// probability or a guarantee that two observations refer to the same device.
pub enum ControllerIdentityConfidence {
    /// A genuine USB serial number is available. The enumerator must not infer
    /// this from an arbitrary device-instance suffix.
    Serial,
    /// Identity is bound to the observed USB/PCI location. Moving an external
    /// controller to another port can intentionally change its ID.
    Topology,
    /// Only the current Windows device instance is usable. Privileged actions
    /// require a fresh observation and an exact evidence comparison.
    WindowsInstance,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ControllerObservation {
    pub id: ControllerId,
    pub identity: ControllerIdentity,
    pub device_description: Option<String>,
    pub bus_reported_description: Option<String>,
    pub manufacturer: Option<String>,
    pub class_name: Option<String>,
    pub class_guid: Option<String>,
    pub enumerator_name: Option<String>,
    pub service: Option<String>,
    pub driver: DriverObservation,
    pub interface_paths: Vec<String>,
    pub status: DeviceStatus,
}

impl ControllerObservation {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        identity: ControllerIdentity,
        device_description: Option<String>,
        bus_reported_description: Option<String>,
        manufacturer: Option<String>,
        class_name: Option<String>,
        class_guid: Option<String>,
        enumerator_name: Option<String>,
        service: Option<String>,
        driver: DriverObservation,
        interface_paths: Vec<String>,
        status: DeviceStatus,
    ) -> Self {
        let id = ControllerId::from_identity(&identity);
        Self {
            id,
            identity,
            device_description: normalize_optional(device_description),
            bus_reported_description: normalize_optional(bus_reported_description),
            manufacturer: normalize_optional(manufacturer),
            class_name: normalize_optional(class_name),
            class_guid: normalize_optional(class_guid),
            enumerator_name: normalize_optional(enumerator_name),
            service: normalize_optional(service),
            driver,
            interface_paths: normalize_list(interface_paths),
            status,
        }
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct DriverObservation {
    pub inf_path: Option<String>,
    pub provider: Option<String>,
    pub version: Option<String>,
    pub driver_key: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct DeviceStatus {
    pub present: bool,
    pub status_flags: Option<u32>,
    pub problem_code: Option<u32>,
}

fn normalize(value: &str) -> String {
    value.trim().to_ascii_uppercase()
}

fn normalize_optional(value: Option<String>) -> Option<String> {
    value
        .map(|value| value.trim().to_owned())
        .filter(|value| is_useful_value(value))
}

fn normalize_identity_optional(value: Option<String>) -> Option<String> {
    value
        .map(|value| normalize(&value))
        .filter(|value| is_useful_value(value))
}

fn normalize_container_id(value: Option<String>) -> Option<String> {
    let value = normalize_identity_optional(value)?;
    let bare = value.trim_start_matches('{').trim_end_matches('}');
    if bare == SYSTEM_CONTAINER_ID || bare == INVALID_CONTAINER_ID {
        None
    } else {
        Some(value)
    }
}

fn normalize_list(values: Vec<String>) -> Vec<String> {
    let mut values: Vec<_> = values
        .into_iter()
        .map(|value| normalize(&value))
        .filter(|value| !value.is_empty())
        .collect();
    values.sort_unstable();
    values.dedup();
    values
}

fn is_useful_value(value: &str) -> bool {
    !value.trim().is_empty()
}

fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf29ce484222325_u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::*;

    fn identity(
        instance_id: &str,
        location_paths: Vec<String>,
        container_id: Option<&str>,
    ) -> ControllerIdentity {
        ControllerIdentity::new(
            Some(0x1234),
            Some(0xabcd),
            None,
            container_id.map(str::to_owned),
            instance_id.into(),
            location_paths,
            Some("PCI\\VEN_1234".into()),
            vec!["USB\\VID_1234&PID_ABCD".into()],
            vec!["USB\\CLASS_E0&SUBCLASS_01&PROT_01".into()],
        )
    }

    #[test]
    fn id_ignores_instance_change_when_stronger_evidence_is_stable() {
        let first = identity(
            "USB\\VID_1234&PID_ABCD\\FIRST",
            vec!["PCIROOT(0)#PCI(1400)#USBROOT(0)#USB(10)".into()],
            Some("{01234567-89ab-cdef-0123-456789abcdef}"),
        );
        let second = identity(
            "USB\\VID_1234&PID_ABCD\\SECOND",
            vec!["pciroot(0)#pci(1400)#usbroot(0)#usb(10)".into()],
            Some("{fedcba98-7654-3210-fedc-ba9876543210}"),
        );

        assert_eq!(
            ControllerId::from_identity(&first),
            ControllerId::from_identity(&second)
        );
    }

    #[test]
    fn id_changes_when_only_location_changes() {
        let first = identity("same", vec!["USBROOT(0)#USB(1)".into()], None);
        let second = identity("same", vec!["USBROOT(0)#USB(2)".into()], None);

        assert_ne!(
            ControllerId::from_identity(&first),
            ControllerId::from_identity(&second)
        );
    }

    #[test]
    fn system_container_id_has_no_identity_value() {
        let with_system_container = identity(
            "USB\\VID_1234&PID_ABCD\\INSTANCE",
            vec!["USBROOT(0)#USB(1)".into()],
            Some("{00000000-0000-0000-FFFF-FFFFFFFFFFFF}"),
        );
        let without_container = identity(
            "USB\\VID_1234&PID_ABCD\\INSTANCE",
            vec!["USBROOT(0)#USB(1)".into()],
            None,
        );

        assert_eq!(with_system_container.container_id, None);
        assert_eq!(
            with_system_container.confidence,
            ControllerIdentityConfidence::Topology
        );
        assert_eq!(
            ControllerId::from_identity(&with_system_container),
            ControllerId::from_identity(&without_container)
        );
    }

    #[test]
    fn all_zero_container_id_has_no_identity_value() {
        let identity = identity(
            "USB\\VID_1234&PID_ABCD\\INSTANCE",
            Vec::new(),
            Some("00000000-0000-0000-0000-000000000000"),
        );

        assert_eq!(identity.container_id, None);
        assert_eq!(
            identity.confidence,
            ControllerIdentityConfidence::WindowsInstance
        );
    }

    #[test]
    fn valid_container_is_corroboration_not_controller_id_input() {
        let first = identity(
            "USB\\VID_1234&PID_ABCD\\FIRST",
            vec!["USBROOT(0)#USB(1)".into()],
            Some("{01234567-89ab-cdef-0123-456789abcdef}"),
        );
        let second = identity(
            "USB\\VID_1234&PID_ABCD\\SECOND",
            vec!["USBROOT(0)#USB(1)".into()],
            Some("{fedcba98-7654-3210-fedc-ba9876543210}"),
        );

        assert!(first.container_id.is_some());
        assert_eq!(
            ControllerId::from_identity(&first),
            ControllerId::from_identity(&second)
        );
    }

    #[test]
    fn genuine_serial_is_the_identity_basis_across_location_changes() {
        let first = ControllerIdentity::new(
            Some(0x1234),
            Some(0xabcd),
            Some("controller-serial".into()),
            None,
            "USB\\VID_1234&PID_ABCD\\FIRST".into(),
            vec!["USBROOT(0)#USB(1)".into()],
            None,
            Vec::new(),
            Vec::new(),
        );
        let second = ControllerIdentity::new(
            Some(0x1234),
            Some(0xabcd),
            Some("CONTROLLER-SERIAL".into()),
            None,
            "USB\\VID_1234&PID_ABCD\\SECOND".into(),
            vec!["USBROOT(0)#USB(2)".into()],
            None,
            Vec::new(),
            Vec::new(),
        );

        assert_eq!(first.confidence, ControllerIdentityConfidence::Serial);
        assert_eq!(
            ControllerId::from_identity(&first),
            ControllerId::from_identity(&second)
        );
    }
}
