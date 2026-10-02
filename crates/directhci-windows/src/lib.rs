//! Windows controller discovery and, in later milestones, ownership backends.

use directhci_core::ControllerObservation;

#[derive(Debug)]
pub enum Error {
    UnsupportedPlatform,
    WindowsApi(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnsupportedPlatform => {
                formatter.write_str("Windows controller discovery is only available on Windows")
            }
            Self::WindowsApi(message) => formatter.write_str(message),
        }
    }
}

impl std::error::Error for Error {}

#[cfg(windows)]
mod enumeration;
mod journal;
#[cfg(windows)]
mod preferences;
mod provisioning;
#[cfg(windows)]
mod provisioning_windows;
mod raw_hci;
mod rebind;
#[cfg(windows)]
mod security;
mod takeover;
mod usbdk;
mod winusb;

pub use journal::{JournalError, JournalLoad, JournalStore};
#[cfg(windows)]
pub use preferences::PreferencesStore;
pub use provisioning::{
    DeviceSpecificPackageBlueprint, device_specific_package_blueprint,
    eligible_device_specific_package,
};
#[cfg(windows)]
pub use provisioning_windows::{
    ControllerPreparationStatus, PreparedController, controller_preparation_status,
    prepare_controller,
};
pub use raw_hci::{
    DEFAULT_COMMAND_TIMEOUT, HciTraceCallback, HciTraceDirection, HciTracePacketType,
    HciTraceRecord, RawHciError, RawHciSession, RawHciSessionOptions, RawHciShutdownReport,
    RawHciTransportSummary,
};
pub use rebind::{
    CompatibleDriverObservation, DIRECTHCI_WINUSB_INTERFACE_GUID, DirectHciDefaultSelectionRisk,
    DirectHciPackageReadiness, DirectHciWinUsbPackageSpec, DriverInstallOutcome,
    RebindObservedState, RebindPlanBlocker, RebindSafetyPrerequisites, TemporaryRebindPlan,
    plan_temporary_device_specific_rebind, plan_temporary_winusb_rebind,
};
#[cfg(windows)]
pub use security::{repair_program_data_directory, validate_service_executable};
pub use takeover::{
    DriverInstallStep, HciBringUpReport, HciInformationReport, OfflineRecoveryReport,
    OfflineRecoveryStatus, PreflightJournalStatus, RoundTripStatus, RuntimeControllerSession,
    TakeoverPreflightReport, TakeoverRoundTripReport, acquire_runtime_controller_session,
    execute_takeover_hci_info, execute_takeover_roundtrip, plan_takeover, recover_offline,
};
pub use usbdk::{
    UsbDkAmbiguityReason, UsbDkApiStatus, UsbDkControllerCorrelation,
    UsbDkControllerCorrelationStatus, UsbDkDeviceKey, UsbDkDeviceObservation, UsbDkDeviceSpeed,
    UsbDkEnumerationStatus, UsbDkHelperStatus, UsbDkNoMatchReason, UsbDkProbeReport,
    UsbDkServiceStatus, probe_usbdk,
};

pub use winusb::{
    DedicatedWinUsbControllerReadiness, DedicatedWinUsbProbeReport, DedicatedWinUsbProbeStatus,
    DedicatedWinUsbReadinessStatus, WinUsbApplicationInterface, WinUsbInterfaceDescriptor,
    WinUsbPipe, WinUsbPipeDirection, WinUsbPipeType, probe_dedicated_winusb,
    probe_winusb_controller,
};
/// Enumerate present USB Bluetooth controller candidates without changing any
/// PnP or driver state.
#[cfg(windows)]
pub fn enumerate_controllers() -> Result<Vec<ControllerObservation>, Error> {
    enumeration::enumerate_controllers()
}

#[cfg(not(windows))]
pub fn enumerate_controllers() -> Result<Vec<ControllerObservation>, Error> {
    Err(Error::UnsupportedPlatform)
}
