//! Portable ownership intent and recovery journal data model.
//!
//! These types describe durable intent. Windows device state remains the source
//! of truth and must be freshly observed before any transition or recovery.

use serde::{Deserialize, Serialize};

use crate::{ControllerIdentity, ControllerObservation};

pub const OWNERSHIP_JOURNAL_SCHEMA_VERSION: u32 = 1;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct LeaseId(String);

impl LeaseId {
    pub fn new(value: impl Into<String>) -> Option<Self> {
        let value = value.into();
        let trimmed = value.trim();
        (!trimmed.is_empty()).then(|| Self(trimmed.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OwnershipBackend {
    TemporaryWinUsbRebind,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DesiredControllerState {
    WindowsOwned,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OwnershipPhase {
    WindowsOwned,
    AcquirePrepared,
    RebindingToDirectHci,
    DirectHciReady,
    DirectHciOwned,
    RestoringWindows,
    RecoveryRequired,
}

impl OwnershipPhase {
    pub fn can_transition_to(self, next: Self) -> bool {
        use OwnershipPhase::{
            AcquirePrepared, DirectHciOwned, DirectHciReady, RebindingToDirectHci,
            RecoveryRequired, RestoringWindows, WindowsOwned,
        };

        matches!(
            (self, next),
            (WindowsOwned, AcquirePrepared)
                | (WindowsOwned, RecoveryRequired)
                | (AcquirePrepared, RebindingToDirectHci)
                | (AcquirePrepared, RestoringWindows)
                | (AcquirePrepared, RecoveryRequired)
                | (RebindingToDirectHci, DirectHciReady)
                | (RebindingToDirectHci, RestoringWindows)
                | (RebindingToDirectHci, RecoveryRequired)
                | (DirectHciReady, DirectHciOwned)
                | (DirectHciReady, RestoringWindows)
                | (DirectHciReady, RecoveryRequired)
                | (DirectHciOwned, RestoringWindows)
                | (DirectHciOwned, RecoveryRequired)
                | (RestoringWindows, WindowsOwned)
                | (RestoringWindows, RecoveryRequired)
                | (RecoveryRequired, RestoringWindows)
                | (RecoveryRequired, WindowsOwned)
        )
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct DriverPackageIdentity {
    pub provider: String,
    pub description: String,
    pub published_inf: Option<String>,
    pub version: Option<String>,
    pub device_interface_guid: String,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct LeaseOwnerMetadata {
    pub process_id: Option<u32>,
    pub session_id: Option<String>,
    pub client_label: Option<String>,
    /// Kernel boot identifier: a resumed Fast Startup session is not a restart.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub boot_identifier: Option<String>,
    /// GetProcessTimes creation FILETIME, not a reusable PID alone.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub process_creation_time: Option<u64>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct DeviceSecurityBaseline {
    /// None means the device had no explicit security override. An absent
    /// baseline in an older journal means unknown, NOT an absent override.
    pub security_sddl: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct OwnershipJournal {
    pub schema_version: u32,
    pub lease_id: LeaseId,
    pub backend: OwnershipBackend,
    pub phase: OwnershipPhase,
    pub recovery_desired_state: DesiredControllerState,
    pub controller_identity: ControllerIdentity,
    /// Historical evidence only. Interface paths and driver names in this
    /// observation must never be replayed without a fresh observation.
    pub pre_acquire_observation: ControllerObservation,
    pub directhci_driver_package: DriverPackageIdentity,
    pub created_unix_ms: u64,
    pub updated_unix_ms: u64,
    pub owner: LeaseOwnerMetadata,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device_security_baseline: Option<DeviceSecurityBaseline>,
}

impl OwnershipJournal {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        lease_id: LeaseId,
        controller_identity: ControllerIdentity,
        pre_acquire_observation: ControllerObservation,
        directhci_driver_package: DriverPackageIdentity,
        created_unix_ms: u64,
        updated_unix_ms: u64,
        owner: LeaseOwnerMetadata,
    ) -> Self {
        Self {
            schema_version: OWNERSHIP_JOURNAL_SCHEMA_VERSION,
            lease_id,
            backend: OwnershipBackend::TemporaryWinUsbRebind,
            phase: OwnershipPhase::AcquirePrepared,
            recovery_desired_state: DesiredControllerState::WindowsOwned,
            controller_identity,
            pre_acquire_observation,
            directhci_driver_package,
            created_unix_ms,
            updated_unix_ms,
            owner,
            device_security_baseline: None,
        }
    }

    pub fn transition_to(
        &mut self,
        next: OwnershipPhase,
        updated_unix_ms: u64,
    ) -> Result<(), InvalidOwnershipTransition> {
        if !self.phase.can_transition_to(next) {
            return Err(InvalidOwnershipTransition {
                from: self.phase,
                to: next,
            });
        }
        self.phase = next;
        self.updated_unix_ms = updated_unix_ms;
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InvalidOwnershipTransition {
    pub from: OwnershipPhase,
    pub to: OwnershipPhase,
}

impl std::fmt::Display for InvalidOwnershipTransition {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "invalid ownership transition from {:?} to {:?}",
            self.from, self.to
        )
    }
}

impl std::error::Error for InvalidOwnershipTransition {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ControllerIdentityConfidence, DeviceStatus, DriverObservation};

    fn observation() -> ControllerObservation {
        let identity = ControllerIdentity::new(
            Some(0x1234),
            Some(0xabcd),
            None,
            None,
            "USB\\VID_1234&PID_ABCD\\INSTANCE".into(),
            vec!["USBROOT(0)#USB(1)".into()],
            Some("PCI\\VEN_1234".into()),
            vec!["USB\\VID_1234&PID_ABCD".into()],
            vec!["USB\\CLASS_E0&SUBCLASS_01&PROT_01".into()],
        );
        assert_eq!(identity.confidence, ControllerIdentityConfidence::Topology);
        ControllerObservation::new(
            identity,
            Some("Bluetooth controller".into()),
            None,
            None,
            None,
            None,
            Some("USB".into()),
            Some("BTHUSB".into()),
            DriverObservation {
                inf_path: Some("oem1.inf".into()),
                provider: Some("Vendor".into()),
                version: Some("1.0".into()),
                driver_key: None,
            },
            vec!["historical-path".into()],
            DeviceStatus {
                present: true,
                status_flags: Some(1),
                problem_code: Some(0),
            },
        )
    }

    #[test]
    fn journal_defaults_recovery_to_windows_owned() {
        let observation = observation();
        let journal = OwnershipJournal::new(
            LeaseId::new("lease-1").unwrap(),
            observation.identity.clone(),
            observation,
            DriverPackageIdentity {
                provider: "DirectHCI Project".into(),
                description: "DirectHCI WinUSB Controller".into(),
                published_inf: None,
                version: Some("0.1".into()),
                device_interface_guid: "{00000000-0000-0000-0000-000000000001}".into(),
            },
            10,
            10,
            LeaseOwnerMetadata::default(),
        );

        assert_eq!(journal.schema_version, OWNERSHIP_JOURNAL_SCHEMA_VERSION);
        assert_eq!(journal.phase, OwnershipPhase::AcquirePrepared);
        assert_eq!(
            journal.recovery_desired_state,
            DesiredControllerState::WindowsOwned
        );
    }

    #[test]
    fn phase_graph_requires_recovery_or_restore_after_side_effect_intent() {
        assert!(OwnershipPhase::WindowsOwned.can_transition_to(OwnershipPhase::AcquirePrepared));
        assert!(
            OwnershipPhase::RebindingToDirectHci
                .can_transition_to(OwnershipPhase::RecoveryRequired)
        );
        assert!(
            OwnershipPhase::RecoveryRequired.can_transition_to(OwnershipPhase::RestoringWindows)
        );
        assert!(OwnershipPhase::RestoringWindows.can_transition_to(OwnershipPhase::WindowsOwned));
        assert!(!OwnershipPhase::DirectHciOwned.can_transition_to(OwnershipPhase::WindowsOwned));
    }

    #[test]
    fn journal_transition_is_atomic_at_the_model_boundary() {
        let observation = observation();
        let mut journal = OwnershipJournal::new(
            LeaseId::new("lease-2").unwrap(),
            observation.identity.clone(),
            observation,
            DriverPackageIdentity {
                provider: "DirectHCI Project".into(),
                description: "DirectHCI WinUSB Controller".into(),
                published_inf: None,
                version: None,
                device_interface_guid: "{00000000-0000-0000-0000-000000000001}".into(),
            },
            10,
            10,
            LeaseOwnerMetadata::default(),
        );

        journal
            .transition_to(OwnershipPhase::RebindingToDirectHci, 20)
            .unwrap();
        assert_eq!(journal.phase, OwnershipPhase::RebindingToDirectHci);
        assert_eq!(journal.updated_unix_ms, 20);
        assert!(
            journal
                .transition_to(OwnershipPhase::DirectHciOwned, 30)
                .is_err()
        );
        assert_eq!(journal.phase, OwnershipPhase::RebindingToDirectHci);
        assert_eq!(journal.updated_unix_ms, 20);
    }
}
