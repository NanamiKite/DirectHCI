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
mod rebind;
mod takeover;
mod usbdk;
mod winusb;

pub use journal::{JournalError, JournalLoad, JournalStore};
pub use rebind::{
    CompatibleDriverObservation, DIRECTHCI_WINUSB_INTERFACE_GUID, DirectHciDefaultSelectionRisk,
    DirectHciPackageReadiness, DirectHciWinUsbPackageSpec, DriverInstallOutcome,
    RebindObservedState, RebindPlanBlocker, RebindSafetyPrerequisites, TemporaryRebindPlan,
    plan_temporary_winusb_rebind,
};
pub use takeover::{
    DriverInstallStep, OfflineRecoveryReport, OfflineRecoveryStatus, PreflightJournalStatus,
    RoundTripStatus, TakeoverPreflightReport, TakeoverRoundTripReport, execute_takeover_roundtrip,
    plan_takeover, recover_offline,
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
