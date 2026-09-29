//! Planning and exact-devnode installation for temporary WinUSB rebind.
//!
//! Driver Store staging remains an explicit user operation. The only mutation
//! primitive here is `DiInstallDevice`; this module never disables/restarts a
//! device and never installs or removes a Driver Store package.

use directhci_core::ControllerObservation;
#[cfg(any(windows, test))]
use directhci_core::DriverObservation;
use serde::Serialize;

pub const DIRECTHCI_WINUSB_INTERFACE_GUID: &str = "{CF97AABE-7898-4D73-B044-A481B26747AA}";

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct DirectHciWinUsbPackageSpec {
    pub provider: String,
    pub description: String,
    pub target_hardware_id: String,
    pub device_interface_guid: String,
}

impl DirectHciWinUsbPackageSpec {
    pub fn ax201_development() -> Self {
        Self {
            provider: "DirectHCI Project".into(),
            description: "DirectHCI WinUSB Controller (AX201 Development)".into(),
            target_hardware_id: "USB\\VID_8087&PID_0026".into(),
            device_interface_guid: DIRECTHCI_WINUSB_INTERFACE_GUID.into(),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize)]
pub struct RebindSafetyPrerequisites {
    pub durable_journal_implemented: bool,
    pub offline_recovery_implemented: bool,
}

impl RebindSafetyPrerequisites {
    pub const fn m1_implemented() -> Self {
        Self {
            durable_journal_implemented: true,
            offline_recovery_implemented: true,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct TemporaryRebindPlan {
    pub requested_controller_id: String,
    pub changes_applied: bool,
    pub controller: Option<ControllerObservation>,
    pub current_state: RebindObservedState,
    pub applicable_drivers: Vec<CompatibleDriverObservation>,
    pub directhci_package: DirectHciPackageReadiness,
    pub package_accepted_by_driver_store: bool,
    pub default_selection_risk: DirectHciDefaultSelectionRisk,
    pub best_windows_recovery_driver: Option<CompatibleDriverObservation>,
    pub driver_switch_candidate_ready: bool,
    pub safe_to_execute: bool,
    pub blockers: Vec<RebindPlanBlocker>,
    pub install_api: &'static str,
    pub recovery_required: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DirectHciDefaultSelectionRisk {
    NotEvaluated,
    SafeLowerRank,
    UnsafeHigherEqualOrUnknownRank,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum RebindObservedState {
    NotObserved,
    WindowsOwned,
    DirectHciReady,
    UnexpectedDriver { service: Option<String> },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct CompatibleDriverObservation {
    pub list_index: u32,
    pub rank: Option<u32>,
    pub description: String,
    pub manufacturer: String,
    pub provider: String,
    pub version: String,
    pub raw_version: u64,
    pub driver_date_filetime: u64,
    pub inf_path: String,
    pub section: String,
    pub hardware_id: String,
    pub currently_installed: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum DirectHciPackageReadiness {
    NotInspected,
    Missing {
        expected: DirectHciWinUsbPackageSpec,
    },
    Ambiguous {
        expected: DirectHciWinUsbPackageSpec,
        candidates: Vec<CompatibleDriverObservation>,
    },
    Ready {
        expected: DirectHciWinUsbPackageSpec,
        candidate: CompatibleDriverObservation,
    },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(tag = "blocker", rename_all = "snake_case")]
pub enum RebindPlanBlocker {
    UnsupportedPlatform,
    ControllerNotFound,
    AmbiguousControllerId {
        observations: usize,
    },
    DeviceNotPresent,
    DeviceHasProblem {
        problem_code: u32,
    },
    NotWindowsOwned {
        service: Option<String>,
    },
    PackageTargetMismatch,
    DriverInspectionFailed {
        message: String,
    },
    DirectHciPackageMissing,
    DirectHciPackageAmbiguous,
    NoWindowsRecoveryDriver,
    AmbiguousWindowsRecoveryDriver,
    DirectHciPackageNotLowerRanked {
        directhci_rank: Option<u32>,
        windows_rank: Option<u32>,
    },
    DurableJournalNotImplemented,
    OfflineRecoveryNotImplemented,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct DriverInstallOutcome {
    pub selected: CompatibleDriverObservation,
    pub need_reboot: bool,
}

pub fn plan_temporary_winusb_rebind(
    controller_id: &str,
    package: DirectHciWinUsbPackageSpec,
    prerequisites: RebindSafetyPrerequisites,
) -> TemporaryRebindPlan {
    platform::plan(controller_id, package, prerequisites)
}

pub(crate) fn observe_controller_and_drivers(
    identity: &directhci_core::ControllerIdentity,
) -> Result<(ControllerObservation, Vec<CompatibleDriverObservation>), String> {
    platform::observe_controller_and_drivers(identity)
}

pub(crate) fn install_driver_for_controller(
    identity: &directhci_core::ControllerIdentity,
    selected: &CompatibleDriverObservation,
) -> Result<DriverInstallOutcome, String> {
    platform::install_driver_for_controller(identity, selected)
}

pub(crate) fn process_is_elevated() -> Result<bool, String> {
    platform::process_is_elevated()
}

pub(crate) fn process_is_running(process_id: u32) -> Result<bool, String> {
    platform::process_is_running(process_id)
}

pub(crate) fn same_physical_controller(
    expected: &directhci_core::ControllerIdentity,
    observed: &directhci_core::ControllerIdentity,
) -> bool {
    if expected.usb_vendor_id != observed.usb_vendor_id
        || expected.usb_product_id != observed.usb_product_id
    {
        return false;
    }
    match (
        expected.usb_serial.as_deref(),
        observed.usb_serial.as_deref(),
    ) {
        (Some(left), Some(right)) => return left.eq_ignore_ascii_case(right),
        (Some(_), None) | (None, Some(_)) => return false,
        (None, None) => {}
    }
    if !expected.location_paths.is_empty() && !observed.location_paths.is_empty() {
        return expected.location_paths.iter().any(|left| {
            observed
                .location_paths
                .iter()
                .any(|right| left.eq_ignore_ascii_case(right))
        });
    }
    expected
        .instance_id
        .eq_ignore_ascii_case(&observed.instance_id)
}

pub(crate) fn same_inf_name(left: &str, right: &str) -> bool {
    let basename = |value: &str| {
        value
            .rsplit(['\\', '/'])
            .next()
            .unwrap_or(value)
            .to_ascii_uppercase()
    };
    basename(left) == basename(right)
}

pub(crate) fn same_driver_candidate(
    left: &CompatibleDriverObservation,
    right: &CompatibleDriverObservation,
) -> bool {
    same_inf_name(&left.inf_path, &right.inf_path)
        && left.section.eq_ignore_ascii_case(&right.section)
        && left.provider.eq_ignore_ascii_case(&right.provider)
        && left.description.eq_ignore_ascii_case(&right.description)
        && left.version.eq_ignore_ascii_case(&right.version)
        && left.hardware_id.eq_ignore_ascii_case(&right.hardware_id)
}

#[cfg(any(windows, test))]
fn evaluate_observed_controller(
    requested_controller_id: &str,
    controller: ControllerObservation,
    applicable_drivers: Vec<CompatibleDriverObservation>,
    package: DirectHciWinUsbPackageSpec,
    prerequisites: RebindSafetyPrerequisites,
) -> TemporaryRebindPlan {
    let mut blockers = Vec::new();
    let current_state = classify_current_state(&controller, &package);
    if !controller.status.present {
        blockers.push(RebindPlanBlocker::DeviceNotPresent);
    }
    if let Some(problem_code) = controller.status.problem_code.filter(|code| *code != 0) {
        blockers.push(RebindPlanBlocker::DeviceHasProblem { problem_code });
    }
    if !matches!(current_state, RebindObservedState::WindowsOwned) {
        blockers.push(RebindPlanBlocker::NotWindowsOwned {
            service: controller.service.clone(),
        });
    }
    let target_matches = controller
        .identity
        .hardware_ids
        .iter()
        .any(|id| id.eq_ignore_ascii_case(&package.target_hardware_id));
    if !target_matches {
        blockers.push(RebindPlanBlocker::PackageTargetMismatch);
    }

    let package_candidates = matching_package_candidates(&applicable_drivers, &package);
    let package_readiness = match package_candidates.as_slice() {
        [] => {
            blockers.push(RebindPlanBlocker::DirectHciPackageMissing);
            DirectHciPackageReadiness::Missing {
                expected: package.clone(),
            }
        }
        [candidate] => DirectHciPackageReadiness::Ready {
            expected: package.clone(),
            candidate: (*candidate).clone(),
        },
        candidates => {
            blockers.push(RebindPlanBlocker::DirectHciPackageAmbiguous);
            DirectHciPackageReadiness::Ambiguous {
                expected: package.clone(),
                candidates: candidates
                    .iter()
                    .map(|candidate| (*candidate).clone())
                    .collect(),
            }
        }
    };

    let non_directhci: Vec<_> = applicable_drivers
        .iter()
        .filter(|candidate| !driver_matches_package(candidate, &package))
        .collect();
    let installed: Vec<_> = non_directhci
        .iter()
        .copied()
        .filter(|candidate| candidate.currently_installed)
        .collect();
    let windows_driver = match installed.as_slice() {
        [candidate] => Some((*candidate).clone()),
        candidates if candidates.len() > 1 => {
            blockers.push(RebindPlanBlocker::AmbiguousWindowsRecoveryDriver);
            None
        }
        _ => {
            let best_rank = non_directhci
                .iter()
                .filter_map(|candidate| candidate.rank)
                .min();
            let best: Vec<_> = non_directhci
                .iter()
                .copied()
                .filter(|candidate| candidate.rank == best_rank)
                .collect();
            match best.as_slice() {
                [candidate] if best_rank.is_some() => Some((*candidate).clone()),
                [] => {
                    blockers.push(RebindPlanBlocker::NoWindowsRecoveryDriver);
                    None
                }
                _ => {
                    blockers.push(RebindPlanBlocker::AmbiguousWindowsRecoveryDriver);
                    None
                }
            }
        }
    };

    let default_selection_risk =
        if let (DirectHciPackageReadiness::Ready { candidate, .. }, Some(windows_driver)) =
            (&package_readiness, &windows_driver)
        {
            let directhci_rank = candidate.rank;
            let windows_rank = windows_driver.rank;
            if directhci_rank.is_none() || windows_rank.is_none() || directhci_rank <= windows_rank
            {
                blockers.push(RebindPlanBlocker::DirectHciPackageNotLowerRanked {
                    directhci_rank,
                    windows_rank,
                });
                DirectHciDefaultSelectionRisk::UnsafeHigherEqualOrUnknownRank
            } else {
                DirectHciDefaultSelectionRisk::SafeLowerRank
            }
        } else {
            DirectHciDefaultSelectionRisk::NotEvaluated
        };

    let driver_switch_candidate_ready = blockers.is_empty();
    if !prerequisites.durable_journal_implemented {
        blockers.push(RebindPlanBlocker::DurableJournalNotImplemented);
    }
    if !prerequisites.offline_recovery_implemented {
        blockers.push(RebindPlanBlocker::OfflineRecoveryNotImplemented);
    }
    TemporaryRebindPlan {
        requested_controller_id: requested_controller_id.to_owned(),
        changes_applied: false,
        controller: Some(controller),
        current_state,
        applicable_drivers,
        package_accepted_by_driver_store: matches!(
            &package_readiness,
            DirectHciPackageReadiness::Ready { .. }
        ),
        directhci_package: package_readiness,
        default_selection_risk,
        best_windows_recovery_driver: windows_driver,
        driver_switch_candidate_ready,
        safe_to_execute: blockers.is_empty(),
        blockers,
        install_api: "DiInstallDevice",
        recovery_required: true,
    }
}

#[cfg(any(windows, test))]
fn classify_current_state(
    controller: &ControllerObservation,
    package: &DirectHciWinUsbPackageSpec,
) -> RebindObservedState {
    match controller.service.as_deref() {
        Some(service)
            if service.eq_ignore_ascii_case("BTHUSB") || service.eq_ignore_ascii_case("IBTUSB") =>
        {
            RebindObservedState::WindowsOwned
        }
        Some(service)
            if service.eq_ignore_ascii_case("WINUSB")
                && controller
                    .driver
                    .provider
                    .as_deref()
                    .is_some_and(|provider| provider.eq_ignore_ascii_case(&package.provider)) =>
        {
            RebindObservedState::DirectHciReady
        }
        _ => RebindObservedState::UnexpectedDriver {
            service: controller.service.clone(),
        },
    }
}

#[cfg(any(windows, test))]
fn matching_package_candidates<'a>(
    candidates: &'a [CompatibleDriverObservation],
    package: &DirectHciWinUsbPackageSpec,
) -> Vec<&'a CompatibleDriverObservation> {
    candidates
        .iter()
        .filter(|candidate| driver_matches_package(candidate, package))
        .collect()
}

#[cfg(any(windows, test))]
fn driver_matches_package(
    candidate: &CompatibleDriverObservation,
    package: &DirectHciWinUsbPackageSpec,
) -> bool {
    candidate.provider.eq_ignore_ascii_case(&package.provider)
        && candidate
            .description
            .eq_ignore_ascii_case(&package.description)
        && candidate
            .hardware_id
            .eq_ignore_ascii_case(&package.target_hardware_id)
}

fn empty_plan(
    controller_id: &str,
    _package: DirectHciWinUsbPackageSpec,
    blocker: RebindPlanBlocker,
) -> TemporaryRebindPlan {
    TemporaryRebindPlan {
        requested_controller_id: controller_id.to_owned(),
        changes_applied: false,
        controller: None,
        current_state: RebindObservedState::NotObserved,
        applicable_drivers: Vec::new(),
        directhci_package: DirectHciPackageReadiness::NotInspected,
        package_accepted_by_driver_store: false,
        default_selection_risk: DirectHciDefaultSelectionRisk::NotEvaluated,
        best_windows_recovery_driver: None,
        driver_switch_candidate_ready: false,
        safe_to_execute: false,
        blockers: vec![blocker],
        install_api: "DiInstallDevice",
        recovery_required: true,
    }
}

#[cfg(not(windows))]
mod platform {
    use super::*;

    pub(super) fn plan(
        controller_id: &str,
        package: DirectHciWinUsbPackageSpec,
        _prerequisites: RebindSafetyPrerequisites,
    ) -> TemporaryRebindPlan {
        empty_plan(
            controller_id,
            package,
            RebindPlanBlocker::UnsupportedPlatform,
        )
    }

    pub(super) fn observe_controller_and_drivers(
        _identity: &directhci_core::ControllerIdentity,
    ) -> Result<(ControllerObservation, Vec<CompatibleDriverObservation>), String> {
        Err("temporary driver rebind is only available on Windows".into())
    }

    pub(super) fn install_driver_for_controller(
        _identity: &directhci_core::ControllerIdentity,
        _selected: &CompatibleDriverObservation,
    ) -> Result<DriverInstallOutcome, String> {
        Err("temporary driver rebind is only available on Windows".into())
    }

    pub(super) fn process_is_elevated() -> Result<bool, String> {
        Err("temporary driver rebind is only available on Windows".into())
    }

    pub(super) fn process_is_running(_process_id: u32) -> Result<bool, String> {
        Err("process inspection is only available on Windows".into())
    }
}

#[cfg(windows)]
mod platform {
    use std::mem::{offset_of, size_of};

    use windows::Win32::Devices::DeviceAndDriverInstallation::{
        DI_FLAGSEX_ALLOWEXCLUDEDDRVS, DIGCF_ALLCLASSES, DIGCF_PRESENT, DIINSTALLDEVICE_FLAGS,
        DiInstallDevice, HDEVINFO, SP_DEVINFO_DATA, SP_DEVINSTALL_PARAMS_W, SP_DRVINFO_DATA_V2_W,
        SP_DRVINFO_DETAIL_DATA_W, SP_DRVINSTALL_PARAMS, SPDIT_COMPATDRIVER,
        SetupDiBuildDriverInfoList, SetupDiDestroyDeviceInfoList, SetupDiDestroyDriverInfoList,
        SetupDiEnumDriverInfoW, SetupDiGetClassDevsW, SetupDiGetDeviceInstallParamsW,
        SetupDiGetDriverInfoDetailW, SetupDiGetDriverInstallParamsW, SetupDiOpenDeviceInfoW,
        SetupDiSetDeviceInstallParamsW,
    };
    use windows::Win32::Foundation::{
        CloseHandle, ERROR_INSUFFICIENT_BUFFER, ERROR_INVALID_PARAMETER, ERROR_NO_MORE_ITEMS,
        GetLastError, HANDLE, STILL_ACTIVE,
    };
    use windows::Win32::Security::{
        GetTokenInformation, TOKEN_ELEVATION, TOKEN_QUERY, TokenElevation,
    };
    use windows::Win32::System::Threading::{
        GetCurrentProcess, GetExitCodeProcess, OpenProcess, OpenProcessToken,
        PROCESS_QUERY_LIMITED_INFORMATION,
    };
    use windows::core::{BOOL, PCWSTR};

    use super::*;

    pub(super) fn plan(
        controller_id: &str,
        package: DirectHciWinUsbPackageSpec,
        prerequisites: RebindSafetyPrerequisites,
    ) -> TemporaryRebindPlan {
        let controllers = match crate::enumerate_controllers() {
            Ok(controllers) => controllers,
            Err(error) => {
                return empty_plan(
                    controller_id,
                    package,
                    RebindPlanBlocker::DriverInspectionFailed {
                        message: error.to_string(),
                    },
                );
            }
        };
        let matches: Vec<_> = controllers
            .into_iter()
            .filter(|controller| controller.id.as_str().eq_ignore_ascii_case(controller_id))
            .collect();
        let controller = match matches.len() {
            0 => {
                return empty_plan(
                    controller_id,
                    package,
                    RebindPlanBlocker::ControllerNotFound,
                );
            }
            1 => matches.into_iter().next().unwrap(),
            observations => {
                return empty_plan(
                    controller_id,
                    package,
                    RebindPlanBlocker::AmbiguousControllerId { observations },
                );
            }
        };

        let drivers = match enumerate_compatible_drivers(
            &controller.identity.instance_id,
            &controller.driver,
        ) {
            Ok(drivers) => drivers,
            Err(message) => {
                let mut plan = empty_plan(
                    controller_id,
                    package,
                    RebindPlanBlocker::DriverInspectionFailed { message },
                );
                plan.controller = Some(controller);
                return plan;
            }
        };

        evaluate_observed_controller(controller_id, controller, drivers, package, prerequisites)
    }

    pub(super) fn observe_controller_and_drivers(
        identity: &directhci_core::ControllerIdentity,
    ) -> Result<(ControllerObservation, Vec<CompatibleDriverObservation>), String> {
        let observations = crate::enumerate_controllers().map_err(|error| error.to_string())?;
        let mut matches = observations
            .into_iter()
            .filter(|controller| same_physical_controller(identity, &controller.identity));
        let controller = matches
            .next()
            .ok_or_else(|| "controller is not present in the fresh observation".to_owned())?;
        if matches.next().is_some() {
            return Err("controller identity is ambiguous in the fresh observation".into());
        }
        let drivers =
            enumerate_compatible_drivers(&controller.identity.instance_id, &controller.driver)?;
        Ok((controller, drivers))
    }

    pub(super) fn install_driver_for_controller(
        identity: &directhci_core::ControllerIdentity,
        selected: &CompatibleDriverObservation,
    ) -> Result<DriverInstallOutcome, String> {
        // Resolve the controller and compatible list again immediately before
        // the destructive call. No cached devnode or SP_DRVINFO_DATA is used.
        let (controller, fresh_candidates) = observe_controller_and_drivers(identity)?;
        let matching: Vec<_> = fresh_candidates
            .iter()
            .filter(|candidate| same_driver_candidate(candidate, selected))
            .collect();
        if matching.len() != 1 {
            return Err(format!(
                "selected driver no longer resolves uniquely ({} matches)",
                matching.len()
            ));
        }

        with_compatible_driver_list(
            &controller.identity.instance_id,
            &controller.driver,
            |device_set, device_info, entries| {
                let matching: Vec<_> = entries
                    .iter()
                    .filter(|(_, candidate)| same_driver_candidate(candidate, selected))
                    .collect();
                let [(raw_driver, candidate)] = matching.as_slice() else {
                    return Err(format!(
                        "selected driver changed during final resolution ({} matches)",
                        matching.len()
                    ));
                };
                let mut need_reboot = BOOL::default();
                unsafe {
                    DiInstallDevice(
                        None,
                        device_set,
                        device_info,
                        Some(raw_driver),
                        DIINSTALLDEVICE_FLAGS::default(),
                        Some(&mut need_reboot),
                    )
                }
                .map_err(|error| format!("DiInstallDevice failed: {error}"))?;
                Ok(DriverInstallOutcome {
                    selected: (*candidate).clone(),
                    need_reboot: need_reboot.as_bool(),
                })
            },
        )
    }

    pub(super) fn process_is_elevated() -> Result<bool, String> {
        let mut token = HANDLE::default();
        unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) }
            .map_err(|error| format!("OpenProcessToken failed: {error}"))?;
        let token = OwnedHandle(token);
        let mut elevation = TOKEN_ELEVATION::default();
        let mut returned = 0u32;
        unsafe {
            GetTokenInformation(
                token.0,
                TokenElevation,
                Some((&mut elevation as *mut TOKEN_ELEVATION).cast()),
                size_of::<TOKEN_ELEVATION>() as u32,
                &mut returned,
            )
        }
        .map_err(|error| format!("GetTokenInformation(TokenElevation) failed: {error}"))?;
        if returned < size_of::<TOKEN_ELEVATION>() as u32 {
            return Err("GetTokenInformation returned a truncated TOKEN_ELEVATION".into());
        }
        Ok(elevation.TokenIsElevated != 0)
    }

    pub(super) fn process_is_running(process_id: u32) -> Result<bool, String> {
        let process =
            match unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, process_id) } {
                Ok(process) => OwnedHandle(process),
                Err(_) if unsafe { GetLastError() } == ERROR_INVALID_PARAMETER => return Ok(false),
                Err(error) => {
                    return Err(format!(
                        "OpenProcess({process_id}) failed while checking journal owner: {error}"
                    ));
                }
            };
        let mut exit_code = 0u32;
        unsafe { GetExitCodeProcess(process.0, &mut exit_code) }
            .map_err(|error| format!("GetExitCodeProcess({process_id}) failed: {error}"))?;
        Ok(exit_code == STILL_ACTIVE.0 as u32)
    }

    fn enumerate_compatible_drivers(
        instance_id: &str,
        current_driver: &DriverObservation,
    ) -> Result<Vec<CompatibleDriverObservation>, String> {
        with_compatible_driver_list(instance_id, current_driver, |_, _, entries| {
            Ok(entries
                .into_iter()
                .map(|(_, observation)| observation)
                .collect())
        })
    }

    fn with_compatible_driver_list<T>(
        instance_id: &str,
        current_driver: &DriverObservation,
        operation: impl FnOnce(
            HDEVINFO,
            &SP_DEVINFO_DATA,
            Vec<(SP_DRVINFO_DATA_V2_W, CompatibleDriverObservation)>,
        ) -> Result<T, String>,
    ) -> Result<T, String> {
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

        let mut install_params = SP_DEVINSTALL_PARAMS_W {
            cbSize: size_of::<SP_DEVINSTALL_PARAMS_W>() as u32,
            ..Default::default()
        };
        unsafe {
            SetupDiGetDeviceInstallParamsW(device_set.0, Some(&device_info), &mut install_params)
        }
        .map_err(|error| format!("SetupDiGetDeviceInstallParamsW failed: {error}"))?;
        install_params.FlagsEx |= DI_FLAGSEX_ALLOWEXCLUDEDDRVS;
        unsafe {
            SetupDiSetDeviceInstallParamsW(device_set.0, Some(&device_info), &install_params)
        }
        .map_err(|error| format!("SetupDiSetDeviceInstallParamsW failed: {error}"))?;

        unsafe {
            SetupDiBuildDriverInfoList(device_set.0, Some(&mut device_info), SPDIT_COMPATDRIVER)
        }
        .map_err(|error| format!("SetupDiBuildDriverInfoList failed: {error}"))?;
        let _driver_list = DriverInfoList {
            device_set: device_set.0,
            device_info,
        };

        let mut entries = Vec::new();
        let mut index = 0u32;
        loop {
            let mut driver = SP_DRVINFO_DATA_V2_W {
                cbSize: size_of::<SP_DRVINFO_DATA_V2_W>() as u32,
                ..Default::default()
            };
            let enumeration = unsafe {
                SetupDiEnumDriverInfoW(
                    device_set.0,
                    Some(&device_info),
                    SPDIT_COMPATDRIVER,
                    index,
                    &mut driver,
                )
            };
            if let Err(error) = enumeration {
                if unsafe { GetLastError() } == ERROR_NO_MORE_ITEMS {
                    break;
                }
                return Err(format!("SetupDiEnumDriverInfoW failed: {error}"));
            }

            let observation =
                driver_observation(device_set.0, &device_info, &driver, index, current_driver)?;
            entries.push((driver, observation));
            index += 1;
        }
        operation(device_set.0, &device_info, entries)
    }

    fn driver_observation(
        device_set: HDEVINFO,
        device_info: &SP_DEVINFO_DATA,
        driver: &SP_DRVINFO_DATA_V2_W,
        index: u32,
        current_driver: &DriverObservation,
    ) -> Result<CompatibleDriverObservation, String> {
        let detail = driver_detail(device_set, device_info, driver)?;
        let mut driver_params = SP_DRVINSTALL_PARAMS {
            cbSize: size_of::<SP_DRVINSTALL_PARAMS>() as u32,
            ..Default::default()
        };
        let rank = unsafe {
            SetupDiGetDriverInstallParamsW(
                device_set,
                Some(device_info),
                driver,
                &mut driver_params,
            )
        }
        .ok()
        .map(|_| driver_params.Rank);

        let inf_path = detail.inf_path;
        Ok(CompatibleDriverObservation {
            list_index: index,
            rank,
            description: decode_utf16_string(&driver.Description),
            manufacturer: decode_utf16_string(&driver.MfgName),
            provider: decode_utf16_string(&driver.ProviderName),
            version: format_driver_version(driver.DriverVersion),
            raw_version: driver.DriverVersion,
            driver_date_filetime: u64::from(driver.DriverDate.dwLowDateTime)
                | (u64::from(driver.DriverDate.dwHighDateTime) << 32),
            currently_installed: current_driver
                .inf_path
                .as_deref()
                .is_some_and(|current| same_inf_name(current, &inf_path)),
            inf_path,
            section: detail.section,
            hardware_id: detail.hardware_id,
        })
    }

    struct DriverDetail {
        inf_path: String,
        section: String,
        hardware_id: String,
    }

    fn driver_detail(
        device_set: HDEVINFO,
        device_info: &SP_DEVINFO_DATA,
        driver: &SP_DRVINFO_DATA_V2_W,
    ) -> Result<DriverDetail, String> {
        let mut required_size = 0u32;
        let first = unsafe {
            SetupDiGetDriverInfoDetailW(
                device_set,
                Some(device_info),
                driver,
                None,
                0,
                Some(&mut required_size),
            )
        };
        let first_error = unsafe { GetLastError() };
        if let Err(error) = first {
            if first_error != ERROR_INSUFFICIENT_BUFFER {
                return Err(format!(
                    "SetupDiGetDriverInfoDetailW size query failed: {error}"
                ));
            }
        }
        let required_size = required_size.max(size_of::<SP_DRVINFO_DETAIL_DATA_W>() as u32);
        let storage_words = (required_size as usize).div_ceil(size_of::<usize>());
        let mut storage = vec![0usize; storage_words];
        let detail = storage.as_mut_ptr().cast::<SP_DRVINFO_DETAIL_DATA_W>();
        unsafe {
            (*detail).cbSize = size_of::<SP_DRVINFO_DETAIL_DATA_W>() as u32;
            SetupDiGetDriverInfoDetailW(
                device_set,
                Some(device_info),
                driver,
                Some(detail),
                required_size,
                None,
            )
        }
        .map_err(|error| format!("SetupDiGetDriverInfoDetailW failed: {error}"))?;

        let detail_ref = unsafe { &*detail };
        let hardware_offset = offset_of!(SP_DRVINFO_DETAIL_DATA_W, HardwareID);
        let hardware_capacity =
            (required_size as usize).saturating_sub(hardware_offset) / size_of::<u16>();
        let hardware_ptr = unsafe {
            storage
                .as_ptr()
                .cast::<u8>()
                .add(hardware_offset)
                .cast::<u16>()
        };
        let hardware_units = unsafe { std::slice::from_raw_parts(hardware_ptr, hardware_capacity) };

        Ok(DriverDetail {
            inf_path: decode_utf16_string(&detail_ref.InfFileName),
            section: decode_utf16_string(&detail_ref.SectionName),
            hardware_id: decode_utf16_string(hardware_units),
        })
    }

    fn format_driver_version(version: u64) -> String {
        format!(
            "{}.{}.{}.{}",
            version >> 48,
            (version >> 32) & 0xffff,
            (version >> 16) & 0xffff,
            version & 0xffff
        )
    }

    fn wide_null(value: &str) -> Vec<u16> {
        value.encode_utf16().chain(std::iter::once(0)).collect()
    }

    fn decode_utf16_string(units: &[u16]) -> String {
        let length = units
            .iter()
            .position(|unit| *unit == 0)
            .unwrap_or(units.len());
        String::from_utf16_lossy(&units[..length])
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

    struct DriverInfoList {
        device_set: HDEVINFO,
        device_info: SP_DEVINFO_DATA,
    }

    impl Drop for DriverInfoList {
        fn drop(&mut self) {
            let _ = unsafe {
                SetupDiDestroyDriverInfoList(
                    self.device_set,
                    Some(&self.device_info),
                    SPDIT_COMPATDRIVER,
                )
            };
        }
    }

    struct OwnedHandle(HANDLE);

    impl Drop for OwnedHandle {
        fn drop(&mut self) {
            let _ = unsafe { CloseHandle(self.0) };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use directhci_core::{ControllerIdentity, DeviceStatus};

    fn driver(
        index: u32,
        rank: u32,
        provider: &str,
        description: &str,
        hardware_id: &str,
        current: bool,
    ) -> CompatibleDriverObservation {
        CompatibleDriverObservation {
            list_index: index,
            rank: Some(rank),
            description: description.into(),
            manufacturer: provider.into(),
            provider: provider.into(),
            version: "1.0.0.0".into(),
            raw_version: 1,
            driver_date_filetime: 1,
            inf_path: format!("oem{index}.inf"),
            section: "Install".into(),
            hardware_id: hardware_id.into(),
            currently_installed: current,
        }
    }

    fn controller() -> ControllerObservation {
        let identity = ControllerIdentity::new(
            Some(0x8087),
            Some(0x0026),
            None,
            None,
            "USB\\VID_8087&PID_0026\\INSTANCE".into(),
            vec!["USBROOT(0)#USB(1)".into()],
            None,
            vec!["USB\\VID_8087&PID_0026".into()],
            vec!["USB\\CLASS_E0&SUBCLASS_01&PROT_01".into()],
        );
        ControllerObservation::new(
            identity,
            Some("Bluetooth".into()),
            None,
            None,
            None,
            None,
            Some("USB".into()),
            Some("BTHUSB".into()),
            DriverObservation {
                inf_path: Some("oem1.inf".into()),
                provider: Some("Intel Corporation".into()),
                version: Some("1.0".into()),
                driver_key: None,
            },
            Vec::new(),
            DeviceStatus {
                present: true,
                status_flags: Some(1),
                problem_code: Some(0),
            },
        )
    }

    #[test]
    fn missing_package_and_recovery_infrastructure_block_plan() {
        let plan = evaluate_observed_controller(
            "controller",
            controller(),
            vec![driver(
                0,
                0,
                "Intel Corporation",
                "Intel Bluetooth",
                "USB\\VID_8087&PID_0026",
                true,
            )],
            DirectHciWinUsbPackageSpec::ax201_development(),
            RebindSafetyPrerequisites::default(),
        );

        assert!(!plan.driver_switch_candidate_ready);
        assert!(!plan.safe_to_execute);
        assert!(
            plan.blockers
                .contains(&RebindPlanBlocker::DirectHciPackageMissing)
        );
        assert!(
            plan.blockers
                .contains(&RebindPlanBlocker::DurableJournalNotImplemented)
        );
    }

    #[test]
    fn package_must_be_lower_ranked_than_windows_recovery_driver() {
        let package = DirectHciWinUsbPackageSpec::ax201_development();
        let plan = evaluate_observed_controller(
            "controller",
            controller(),
            vec![
                driver(
                    0,
                    100,
                    "Intel Corporation",
                    "Intel Bluetooth",
                    "USB\\VID_8087&PID_0026",
                    true,
                ),
                driver(
                    1,
                    50,
                    &package.provider,
                    &package.description,
                    &package.target_hardware_id,
                    false,
                ),
            ],
            package,
            RebindSafetyPrerequisites {
                durable_journal_implemented: true,
                offline_recovery_implemented: true,
            },
        );

        assert!(plan.blockers.iter().any(|blocker| matches!(
            blocker,
            RebindPlanBlocker::DirectHciPackageNotLowerRanked { .. }
        )));
        assert!(!plan.safe_to_execute);
    }

    #[test]
    fn inf_identity_ignores_driver_store_path_prefix() {
        assert!(same_inf_name(r"C:\Windows\INF\oem69.inf", "oem69.inf"));
        assert!(!same_inf_name(r"C:\Windows\INF\oem70.inf", "oem69.inf"));
    }

    #[test]
    fn complete_safe_inputs_produce_ready_plan_without_applying_changes() {
        let package = DirectHciWinUsbPackageSpec::ax201_development();
        let plan = evaluate_observed_controller(
            "controller",
            controller(),
            vec![
                driver(
                    0,
                    10,
                    "Intel Corporation",
                    "Intel Bluetooth",
                    "USB\\VID_8087&PID_0026",
                    true,
                ),
                driver(
                    1,
                    100,
                    &package.provider,
                    &package.description,
                    &package.target_hardware_id,
                    false,
                ),
            ],
            package,
            RebindSafetyPrerequisites {
                durable_journal_implemented: true,
                offline_recovery_implemented: true,
            },
        );

        assert!(plan.driver_switch_candidate_ready);
        assert!(plan.safe_to_execute);
        assert!(!plan.changes_applied);
    }
}
