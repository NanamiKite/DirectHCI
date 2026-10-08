//! Executable temporary-WinUSB ownership round trip and offline recovery.
//!
//! This module owns orchestration only. Driver selection/rebind, durable
//! journal storage, and WinUSB readiness remain reusable backend components.

use std::thread;
use std::time::{Duration, Instant};

use directhci_core::{
    ControllerBufferSize, ControllerIdentity, ControllerObservation, ControllerSecurityBaseline,
    DriverObservation, DriverPackageIdentity, HCI_READ_BUFFER_SIZE,
    HCI_READ_LOCAL_SUPPORTED_COMMANDS, HCI_READ_LOCAL_SUPPORTED_FEATURES,
    HCI_READ_LOCAL_VERSION_INFORMATION, HCI_RESET, LeaseId, LeaseOwnerMetadata,
    LocalSupportedCommands, LocalSupportedFeatures, LocalVersionInformation,
    OWNERSHIP_JOURNAL_SCHEMA_VERSION, OwnershipJournal, OwnershipPhase,
    parse_controller_buffer_size, parse_local_supported_commands, parse_local_supported_features,
    parse_local_version_information, parse_reset_response,
};
use serde::Serialize;

use crate::device_security;
use crate::journal::unix_time_ms;
use crate::raw_hci::{
    RawHciSession, RawHciSessionOptions, RawHciShutdownReport, RawHciTransportSummary,
};
use crate::rebind::{
    CompatibleDriverObservation, DIRECTHCI_WINUSB_INTERFACE_GUID, DirectHciPackageReadiness,
    DriverInstallOutcome, RebindSafetyPrerequisites, TemporaryRebindPlan,
    controller_restart_pending, install_driver_for_controller, observe_controller_and_drivers,
    plan_temporary_device_specific_rebind, plan_temporary_winusb_rebind, process_is_elevated,
    process_is_running, restart_windows_controller, same_driver_candidate, same_inf_name,
    same_physical_controller,
};
use crate::winusb::{
    DedicatedWinUsbControllerReadiness, DedicatedWinUsbReadinessStatus,
    application_interface_is_active, probe_winusb_controller,
};
use crate::{JournalLoad, JournalStore};

const REENUMERATION_TIMEOUT: Duration = Duration::from_secs(20);
const REENUMERATION_POLL: Duration = Duration::from_millis(250);
// cfg.h devnode flags. Keep the decision portable for regression tests.
const DN_STARTED: u32 = 0x00000008;
const DN_NEED_RESTART: u32 = 0x00000100;
const DN_HAS_PROBLEM: u32 = 0x00000400;

#[derive(Clone, Debug, Serialize)]
pub struct TakeoverPreflightReport {
    pub changes_applied: bool,
    pub elevated: Option<bool>,
    pub journal_path: String,
    pub journal_status: PreflightJournalStatus,
    pub package_accepted_by_driver_store: bool,
    pub offline_recovery_available: bool,
    pub rebind: TemporaryRebindPlan,
    pub safe_to_execute: bool,
    pub blockers: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum PreflightJournalStatus {
    Missing,
    Present { stale: bool, age_ms: u64 },
    Unavailable { message: String },
}

#[derive(Clone, Debug, Serialize)]
pub struct DriverInstallStep {
    pub selected: CompatibleDriverObservation,
    pub api_succeeded: bool,
    pub need_reboot: Option<bool>,
    pub error: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RoundTripStatus {
    Completed,
    Refused,
    FailedButWindowsRestored,
    RecoveryRequired,
}

#[derive(Clone, Debug, Serialize)]
pub struct TakeoverRoundTripReport {
    pub status: RoundTripStatus,
    pub preflight: TakeoverPreflightReport,
    pub pre_state: Option<ControllerObservation>,
    pub journal_created: bool,
    pub journal_cleared: bool,
    pub journal_retained: bool,
    pub rebind: Option<DriverInstallStep>,
    pub directhci_state: Option<ControllerObservation>,
    pub winusb_readiness: Option<DedicatedWinUsbControllerReadiness>,
    pub restore: Option<DriverInstallStep>,
    pub final_state: Option<ControllerObservation>,
    pub primary_error: Option<String>,
    pub recovery_error: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct HciInformationReport {
    pub takeover: TakeoverRoundTripReport,
    pub hci: Option<HciBringUpReport>,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct HciBringUpReport {
    pub transport: Option<RawHciTransportSummary>,
    pub event_rx_started: bool,
    pub acl_rx_started: bool,
    pub reset_status: Option<u8>,
    pub local_version: Option<LocalVersionInformation>,
    pub local_supported_commands: Option<LocalSupportedCommands>,
    pub local_supported_features: Option<LocalSupportedFeatures>,
    pub buffer_size: Option<ControllerBufferSize>,
    pub shutdown: Option<RawHciShutdownReport>,
    pub error: Option<String>,
}

/// A live temporary-takeover session intended to be owned by `directhcid`.
/// Dropping it is a recovery fallback; callers should use `release` and check
/// the resulting report.
pub struct RuntimeControllerSession {
    report: TakeoverRoundTripReport,
    store: JournalStore,
    journal: OwnershipJournal,
    raw_hci: Option<RawHciSession>,
}

impl RuntimeControllerSession {
    pub fn controller(&self) -> &ControllerObservation {
        self.report
            .directhci_state
            .as_ref()
            .expect("a live runtime session has a DirectHCI observation")
    }

    pub fn raw_hci(&self) -> &RawHciSession {
        self.raw_hci
            .as_ref()
            .expect("a live runtime session has RawHciSession")
    }

    pub fn release(mut self) -> TakeoverRoundTripReport {
        self.release_inner()
    }

    fn release_inner(&mut self) -> TakeoverRoundTripReport {
        let Some(mut raw_hci) = self.raw_hci.take() else {
            return self.report.clone();
        };
        let shutdown = raw_hci.shutdown();
        drop(raw_hci);
        if !shutdown.event_rx_joined
            || !shutdown.acl_rx_joined
            || !shutdown.handles_released
            || !shutdown.cancellation_errors.is_empty()
        {
            self.report.primary_error = Some(format!(
                "RawHciSession shutdown was not clean: {:?}",
                shutdown.cancellation_errors
            ));
        }
        match restore_windows(&self.store, &mut self.journal) {
            Ok(restored) => {
                self.report.restore = Some(restored.install);
                self.report.final_state = Some(restored.final_state);
                self.report.journal_cleared = true;
                self.report.journal_retained = false;
                self.report.status = if self.report.primary_error.is_some() {
                    RoundTripStatus::FailedButWindowsRestored
                } else {
                    RoundTripStatus::Completed
                };
            }
            Err(failure) => {
                self.report.restore = failure.install;
                self.report.final_state = failure.final_state;
                self.report.recovery_error = Some(failure.message);
                self.report.status = RoundTripStatus::RecoveryRequired;
            }
        }
        self.report.clone()
    }
}

impl Drop for RuntimeControllerSession {
    fn drop(&mut self) {
        if self.raw_hci.is_some() {
            let _ = self.release_inner();
        }
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OfflineRecoveryStatus {
    NoJournal,
    AlreadyWindowsOwned,
    RestoredWindows,
    DeviceMissing,
    Refused,
    RecoveryRequired,
}

#[derive(Clone, Debug, Serialize)]
pub struct OfflineRecoveryReport {
    pub status: OfflineRecoveryStatus,
    pub journal_path: String,
    pub stale_journal: Option<bool>,
    pub journal_phase: Option<OwnershipPhase>,
    pub observed: Option<ControllerObservation>,
    pub restore: Option<DriverInstallStep>,
    pub final_state: Option<ControllerObservation>,
    pub journal_cleared: bool,
    pub journal_retained: bool,
    pub error: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum RestoreCandidateDecision {
    Selected(CompatibleDriverObservation),
    Missing,
    Ambiguous,
}

pub fn plan_takeover(controller_id: &str) -> TakeoverPreflightReport {
    let mut rebind = plan_temporary_device_specific_rebind(
        controller_id,
        RebindSafetyPrerequisites::m1_implemented(),
    );
    // Migration compatibility only: an already-staged multi-HWID development
    // package may still be used for this freshly observed exact Hardware ID.
    // No static device list is consulted and no package is staged here.
    if let (Some(original), DirectHciPackageReadiness::Missing { expected }) =
        (&rebind.controller, &rebind.directhci_package)
    {
        if expected.supported_hardware_ids.len() == 1 {
            let mut legacy = expected.clone();
            legacy.description = "DirectHCI WinUSB Bluetooth Controller (Development)".into();
            let legacy_plan = plan_temporary_winusb_rebind(
                controller_id,
                legacy,
                RebindSafetyPrerequisites::m1_implemented(),
            );
            if legacy_plan.package_accepted_by_driver_store
                && legacy_plan.controller.as_ref().is_some_and(|current| {
                    same_physical_controller(&original.identity, &current.identity)
                })
            {
                rebind = legacy_plan;
            }
        }
    }
    let mut blockers: Vec<String> = rebind
        .blockers
        .iter()
        .map(|blocker| format!("{blocker:?}"))
        .collect();

    let elevated = match process_is_elevated() {
        Ok(value) => {
            if !value {
                blockers.push("AdministratorPrivilegesRequired".into());
            }
            Some(value)
        }
        Err(message) => {
            blockers.push(format!("PrivilegeInspectionFailed: {message}"));
            None
        }
    };

    let (journal_path, journal_status) = match JournalStore::program_data() {
        Ok(store) => {
            let path = store.path().display().to_string();
            match store.load() {
                Ok(JournalLoad::Missing) => match store.probe_writable() {
                    Ok(()) => (path, PreflightJournalStatus::Missing),
                    Err(error) => {
                        blockers.push(format!("JournalUnavailable: {error}"));
                        (
                            path,
                            PreflightJournalStatus::Unavailable {
                                message: error.to_string(),
                            },
                        )
                    }
                },
                Ok(JournalLoad::Present { stale, age_ms, .. }) => {
                    blockers.push("UnresolvedOwnershipJournal".into());
                    (path, PreflightJournalStatus::Present { stale, age_ms })
                }
                Err(error) => {
                    blockers.push(format!("JournalUnavailable: {error}"));
                    (
                        path,
                        PreflightJournalStatus::Unavailable {
                            message: error.to_string(),
                        },
                    )
                }
            }
        }
        Err(error) => {
            blockers.push(format!("JournalUnavailable: {error}"));
            (
                "%ProgramData%\\DirectHCI\\ownership-journal-v1.json".into(),
                PreflightJournalStatus::Unavailable {
                    message: error.to_string(),
                },
            )
        }
    };

    if elevated == Some(true) && blockers.is_empty() {
        if let Some(controller) = rebind.controller.as_ref() {
            if let Err(error) = device_security::capture(controller) {
                blockers.push(format!("ControllerSecurityBaselineUnavailable: {error}"));
            }
        }
    }
    let package_accepted_by_driver_store = rebind.package_accepted_by_driver_store;
    TakeoverPreflightReport {
        changes_applied: false,
        elevated,
        journal_path,
        journal_status,
        package_accepted_by_driver_store,
        offline_recovery_available: true,
        safe_to_execute: rebind.safe_to_execute && blockers.is_empty(),
        rebind,
        blockers,
    }
}

pub fn execute_takeover_roundtrip(controller_id: &str) -> TakeoverRoundTripReport {
    execute_takeover_operation(controller_id, "directhci takeover roundtrip", |_| {
        ((), Ok(()))
    })
    .0
}

pub fn execute_takeover_hci_info(controller_id: &str) -> HciInformationReport {
    let (takeover, hci) = execute_takeover_operation(
        controller_id,
        "directhci takeover hci-info",
        run_hci_bring_up,
    );
    HciInformationReport { takeover, hci }
}

pub fn acquire_runtime_controller_session(
    controller_id: &str,
    client_label: &str,
) -> Result<RuntimeControllerSession, TakeoverRoundTripReport> {
    let preflight = plan_takeover(controller_id);
    let mut report = TakeoverRoundTripReport {
        status: RoundTripStatus::Refused,
        pre_state: preflight.rebind.controller.clone(),
        preflight,
        journal_created: false,
        journal_cleared: false,
        journal_retained: false,
        rebind: None,
        directhci_state: None,
        winusb_readiness: None,
        restore: None,
        final_state: None,
        primary_error: None,
        recovery_error: None,
    };
    if !report.preflight.safe_to_execute {
        report.primary_error = Some("takeover preflight refused execution".into());
        return Err(report);
    }
    let Some(pre_state) = report.pre_state.clone() else {
        report.primary_error = Some("preflight did not produce a controller observation".into());
        return Err(report);
    };
    let direct_candidate = match &report.preflight.rebind.directhci_package {
        DirectHciPackageReadiness::Ready { candidate, .. } => candidate.clone(),
        _ => {
            report.primary_error = Some("DirectHCI package is not uniquely ready".into());
            return Err(report);
        }
    };
    let store = match JournalStore::program_data() {
        Ok(store) => store,
        Err(error) => {
            report.primary_error = Some(error.to_string());
            return Err(report);
        }
    };
    let now = match unix_time_ms() {
        Ok(now) => now,
        Err(error) => {
            report.primary_error = Some(error.to_string());
            return Err(report);
        }
    };
    let lease_id = LeaseId::new(format!(
        "m3-{}-{}-{}",
        now,
        std::process::id(),
        pre_state.id
    ))
    .expect("generated lease ID is non-empty");
    let mut journal = OwnershipJournal::new(
        lease_id,
        pre_state.identity.clone(),
        pre_state.clone(),
        DriverPackageIdentity {
            provider: direct_candidate.provider.clone(),
            description: direct_candidate.description.clone(),
            published_inf: Some(direct_candidate.inf_path.clone()),
            version: Some(direct_candidate.version.clone()),
            device_interface_guid: DIRECTHCI_WINUSB_INTERFACE_GUID.into(),
        },
        now,
        now,
        LeaseOwnerMetadata {
            process_id: Some(std::process::id()),
            session_id: None,
            client_label: Some(client_label.into()),
        },
    );
    journal.pre_acquire_security = match device_security::capture(&pre_state) {
        Ok(baseline) => Some(baseline),
        Err(error) => {
            report.primary_error = Some(format!(
                "capture pre-acquire controller permissions: {error}"
            ));
            return Err(report);
        }
    };
    if let Err(error) = store.create(&journal) {
        report.primary_error = Some(format!("persist AcquirePrepared journal: {error}"));
        return Err(report);
    }
    report.journal_created = true;
    report.journal_retained = true;

    if let Err(error) = validate_pre_rebind(&journal, &direct_candidate) {
        report.primary_error = Some(format!("final pre-rebind validation failed: {error}"));
        return Err(cancel_before_rebind(report, journal, &store));
    }
    if let Err(error) = persist_phase(&store, &mut journal, OwnershipPhase::RebindingToDirectHci) {
        report.primary_error = Some(error);
        return Err(cancel_before_rebind(report, journal, &store));
    }
    match install_driver_for_controller(&journal.controller_identity, &direct_candidate) {
        Ok(outcome) => {
            report.rebind = Some(successful_step(&outcome));
            if outcome.need_reboot {
                report.primary_error =
                    Some("DirectHCI rebind requires reboot; runtime session was not opened".into());
                mark_recovery_required(&store, &mut journal, &mut report.recovery_error);
                report.status = RoundTripStatus::RecoveryRequired;
                return Err(report);
            }
        }
        Err(error) => {
            report.rebind = Some(failed_step(direct_candidate, &error));
            report.primary_error = Some(error);
            return Err(recover_after_primary_failure(report, journal, &store));
        }
    }
    let direct_state = match wait_for_controller(&journal.controller_identity, |controller| {
        is_directhci_ready(controller, &journal.directhci_driver_package)
    }) {
        Ok(controller) => controller,
        Err(error) => {
            report.primary_error = Some(format!("post-rebind observation failed: {error}"));
            return Err(recover_after_primary_failure(report, journal, &store));
        }
    };
    report.directhci_state = Some(direct_state.clone());
    if let Err(error) = persist_phase(&store, &mut journal, OwnershipPhase::DirectHciReady) {
        report.primary_error = Some(error);
        return Err(recover_after_primary_failure(report, journal, &store));
    }
    let readiness = match probe_winusb_controller(direct_state.id.as_str()) {
        Ok(readiness) if matches!(readiness.status, DedicatedWinUsbReadinessStatus::Ready) => {
            readiness
        }
        Ok(readiness) => {
            report.winusb_readiness = Some(readiness);
            report.primary_error = Some("WinUSB readiness did not reach Ready".into());
            return Err(recover_after_primary_failure(report, journal, &store));
        }
        Err(error) => {
            report.primary_error = Some(format!("WinUSB readiness probe failed: {error}"));
            return Err(recover_after_primary_failure(report, journal, &store));
        }
    };
    report.winusb_readiness = Some(readiness.clone());
    if let Err(error) = persist_phase(&store, &mut journal, OwnershipPhase::DirectHciOwned) {
        report.primary_error = Some(error);
        return Err(recover_after_primary_failure(report, journal, &store));
    }
    let raw_hci = match RawHciSession::open(&readiness, RawHciSessionOptions::default()) {
        Ok(session) => session,
        Err(error) => {
            report.primary_error = Some(format!("open RawHciSession: {error}"));
            return Err(recover_after_primary_failure(report, journal, &store));
        }
    };
    Ok(RuntimeControllerSession {
        report,
        store,
        journal,
        raw_hci: Some(raw_hci),
    })
}

fn execute_takeover_operation<T, F>(
    controller_id: &str,
    client_label: &str,
    action: F,
) -> (TakeoverRoundTripReport, Option<T>)
where
    F: FnOnce(&DedicatedWinUsbControllerReadiness) -> (T, Result<(), String>),
{
    let preflight = plan_takeover(controller_id);
    let mut report = TakeoverRoundTripReport {
        status: RoundTripStatus::Refused,
        pre_state: preflight.rebind.controller.clone(),
        preflight,
        journal_created: false,
        journal_cleared: false,
        journal_retained: false,
        rebind: None,
        directhci_state: None,
        winusb_readiness: None,
        restore: None,
        final_state: None,
        primary_error: None,
        recovery_error: None,
    };
    if !report.preflight.safe_to_execute {
        report.primary_error = Some("takeover preflight refused execution".into());
        return (report, None);
    }

    let Some(pre_state) = report.pre_state.clone() else {
        report.primary_error = Some("preflight did not produce a controller observation".into());
        return (report, None);
    };
    let direct_candidate = match &report.preflight.rebind.directhci_package {
        DirectHciPackageReadiness::Ready { candidate, .. } => candidate.clone(),
        _ => {
            report.primary_error = Some("DirectHCI package is not uniquely ready".into());
            return (report, None);
        }
    };
    let store = match JournalStore::program_data() {
        Ok(store) => store,
        Err(error) => {
            report.primary_error = Some(error.to_string());
            return (report, None);
        }
    };
    let now = match unix_time_ms() {
        Ok(now) => now,
        Err(error) => {
            report.primary_error = Some(error.to_string());
            return (report, None);
        }
    };
    let lease_id = LeaseId::new(format!(
        "m1-{}-{}-{}",
        now,
        std::process::id(),
        pre_state.id
    ))
    .expect("generated lease ID is non-empty");
    let mut journal = OwnershipJournal::new(
        lease_id,
        pre_state.identity.clone(),
        pre_state.clone(),
        DriverPackageIdentity {
            provider: direct_candidate.provider.clone(),
            description: direct_candidate.description.clone(),
            published_inf: Some(direct_candidate.inf_path.clone()),
            version: Some(direct_candidate.version.clone()),
            device_interface_guid: DIRECTHCI_WINUSB_INTERFACE_GUID.into(),
        },
        now,
        now,
        LeaseOwnerMetadata {
            process_id: Some(std::process::id()),
            session_id: None,
            client_label: Some(client_label.into()),
        },
    );
    journal.pre_acquire_security = match device_security::capture(&pre_state) {
        Ok(baseline) => Some(baseline),
        Err(error) => {
            report.primary_error = Some(format!(
                "capture pre-acquire controller permissions: {error}"
            ));
            return (report, None);
        }
    };
    if let Err(error) = store.create(&journal) {
        report.primary_error = Some(format!("persist AcquirePrepared journal: {error}"));
        return (report, None);
    }
    report.journal_created = true;
    report.journal_retained = true;

    if let Err(error) = validate_pre_rebind(&journal, &direct_candidate) {
        report.primary_error = Some(format!("final pre-rebind validation failed: {error}"));
        return (cancel_before_rebind(report, journal, &store), None);
    }

    if let Err(error) = persist_phase(&store, &mut journal, OwnershipPhase::RebindingToDirectHci) {
        report.primary_error = Some(error);
        return (cancel_before_rebind(report, journal, &store), None);
    }

    match install_driver_for_controller(&journal.controller_identity, &direct_candidate) {
        Ok(outcome) => {
            report.rebind = Some(successful_step(&outcome));
            if outcome.need_reboot {
                report.primary_error = Some(
                    "DiInstallDevice selected DirectHCI WinUSB but requires a reboot; no reboot was initiated"
                        .into(),
                );
                mark_recovery_required(&store, &mut journal, &mut report.recovery_error);
                report.status = RoundTripStatus::RecoveryRequired;
                return (report, None);
            }
        }
        Err(error) => {
            report.rebind = Some(failed_step(direct_candidate, &error));
            report.primary_error = Some(error);
            return (recover_after_primary_failure(report, journal, &store), None);
        }
    }

    let direct_state = match wait_for_controller(&journal.controller_identity, |controller| {
        is_directhci_ready(controller, &journal.directhci_driver_package)
    }) {
        Ok(controller) => controller,
        Err(error) => {
            report.primary_error = Some(format!("post-rebind observation failed: {error}"));
            return (recover_after_primary_failure(report, journal, &store), None);
        }
    };
    report.directhci_state = Some(direct_state.clone());
    if let Err(error) = persist_phase(&store, &mut journal, OwnershipPhase::DirectHciReady) {
        report.primary_error = Some(error);
        return (recover_after_primary_failure(report, journal, &store), None);
    }

    let readiness = match probe_winusb_controller(direct_state.id.as_str()) {
        Ok(readiness) => {
            let ready = matches!(readiness.status, DedicatedWinUsbReadinessStatus::Ready);
            report.winusb_readiness = Some(readiness.clone());
            if !ready {
                report.primary_error = Some("WinUSB readiness did not reach Ready".into());
                return (recover_after_primary_failure(report, journal, &store), None);
            }
            readiness
        }
        Err(error) => {
            report.primary_error = Some(format!("WinUSB readiness probe failed: {error}"));
            return (recover_after_primary_failure(report, journal, &store), None);
        }
    };

    if let Err(error) = persist_phase(&store, &mut journal, OwnershipPhase::DirectHciOwned) {
        report.primary_error = Some(error);
        return (recover_after_primary_failure(report, journal, &store), None);
    }

    let (action_output, action_result) = action(&readiness);
    if let Err(error) = action_result {
        report.primary_error = Some(error);
        return (
            recover_after_primary_failure(report, journal, &store),
            Some(action_output),
        );
    }

    match restore_windows(&store, &mut journal) {
        Ok(restored) => {
            report.restore = Some(restored.install);
            report.final_state = Some(restored.final_state);
            report.journal_cleared = true;
            report.journal_retained = false;
            report.status = RoundTripStatus::Completed;
        }
        Err(failure) => {
            report.restore = failure.install;
            report.final_state = failure.final_state;
            report.recovery_error = Some(failure.message);
            report.status = RoundTripStatus::RecoveryRequired;
        }
    }
    (report, Some(action_output))
}

fn run_hci_bring_up(
    readiness: &DedicatedWinUsbControllerReadiness,
) -> (HciBringUpReport, Result<(), String>) {
    let mut report = HciBringUpReport::default();
    let mut session = match RawHciSession::open(readiness, RawHciSessionOptions::default()) {
        Ok(session) => session,
        Err(error) => {
            let message = format!("open RawHciSession: {error}");
            report.error = Some(message.clone());
            return (report, Err(message));
        }
    };
    report.transport = Some(session.transport().clone());
    report.event_rx_started = true;
    report.acl_rx_started = true;

    let result = (|| -> Result<(), String> {
        let reset = session
            .send_command(HCI_RESET, &[])
            .map_err(|error| format!("HCI Reset transaction: {error}"))?;
        report.reset_status = Some(
            parse_reset_response(&reset).map_err(|error| format!("HCI Reset response: {error}"))?,
        );

        let version = session
            .send_command(HCI_READ_LOCAL_VERSION_INFORMATION, &[])
            .map_err(|error| format!("Read Local Version transaction: {error}"))?;
        report.local_version = Some(
            parse_local_version_information(&version)
                .map_err(|error| format!("Read Local Version response: {error}"))?,
        );

        let commands = session
            .send_command(HCI_READ_LOCAL_SUPPORTED_COMMANDS, &[])
            .map_err(|error| format!("Read Local Supported Commands transaction: {error}"))?;
        report.local_supported_commands = Some(
            parse_local_supported_commands(&commands)
                .map_err(|error| format!("Read Local Supported Commands response: {error}"))?,
        );

        let features = session
            .send_command(HCI_READ_LOCAL_SUPPORTED_FEATURES, &[])
            .map_err(|error| format!("Read Local Supported Features transaction: {error}"))?;
        report.local_supported_features = Some(
            parse_local_supported_features(&features)
                .map_err(|error| format!("Read Local Supported Features response: {error}"))?,
        );

        let buffer_size = session
            .send_command(HCI_READ_BUFFER_SIZE, &[])
            .map_err(|error| format!("Read Buffer Size transaction: {error}"))?;
        report.buffer_size = Some(
            parse_controller_buffer_size(&buffer_size)
                .map_err(|error| format!("Read Buffer Size response: {error}"))?,
        );
        Ok(())
    })();

    let shutdown = session.shutdown();
    let clean_shutdown = shutdown.event_rx_joined
        && shutdown.acl_rx_joined
        && shutdown.handles_released
        && shutdown.cancellation_errors.is_empty();
    report.shutdown = Some(shutdown);

    let result = match (result, clean_shutdown) {
        (Ok(()), true) => Ok(()),
        (Ok(()), false) => Err("RawHciSession shutdown did not complete cleanly".into()),
        (Err(primary), true) => Err(primary),
        (Err(primary), false) => Err(format!(
            "{primary}; RawHciSession shutdown also did not complete cleanly"
        )),
    };
    if let Err(error) = &result {
        report.error = Some(error.clone());
    }
    (report, result)
}

pub fn recover_offline() -> OfflineRecoveryReport {
    recover_with_owner_policy(false)
}

/// Retry a journal left by this process after its controller session has ended.
/// The privileged runtime must first stop and wait for its active session.
/// Journals owned by any other live process remain protected.
pub fn recover_owning_process_after_session_closed() -> OfflineRecoveryReport {
    recover_with_owner_policy(true)
}

fn recover_with_owner_policy(allow_current_owner: bool) -> OfflineRecoveryReport {
    let store = match JournalStore::program_data() {
        Ok(store) => store,
        Err(error) => {
            return OfflineRecoveryReport {
                status: OfflineRecoveryStatus::Refused,
                journal_path: "%ProgramData%\\DirectHCI\\ownership-journal-v1.json".into(),
                stale_journal: None,
                journal_phase: None,
                observed: None,
                restore: None,
                final_state: None,
                journal_cleared: false,
                journal_retained: false,
                error: Some(error.to_string()),
            };
        }
    };
    let mut report = OfflineRecoveryReport {
        status: OfflineRecoveryStatus::Refused,
        journal_path: store.path().display().to_string(),
        stale_journal: None,
        journal_phase: None,
        observed: None,
        restore: None,
        final_state: None,
        journal_cleared: false,
        journal_retained: true,
        error: None,
    };
    let (mut journal, stale) = match store.load() {
        Ok(JournalLoad::Missing) => {
            report.status = OfflineRecoveryStatus::NoJournal;
            report.journal_retained = false;
            return report;
        }
        Ok(JournalLoad::Present { journal, stale, .. }) => (*journal, stale),
        Err(error) => {
            report.status = OfflineRecoveryStatus::Refused;
            report.error = Some(error.to_string());
            return report;
        }
    };
    report.stale_journal = Some(stale);
    report.journal_phase = Some(journal.phase);
    if journal.schema_version == 1 {
        // v1 never captured permissions. A coincidental extra JSON field is
        // not a trusted baseline; retain the legacy recovery limitation.
        journal.pre_acquire_security = None;
    }
    if journal.schema_version < OWNERSHIP_JOURNAL_SCHEMA_VERSION {
        // v2 can have replayed permissions without reinitializing Windows
        // clients. Do not mistake matching ACLs for a finished stack restart.
        journal.windows_stack_restart_pending = journal.schema_version == 2
            && journal.pre_acquire_security.is_some()
            && matches!(
                journal.phase,
                OwnershipPhase::RestoringWindows
                    | OwnershipPhase::RecoveryRequired
                    | OwnershipPhase::WindowsOwned
            );
        journal.schema_version = OWNERSHIP_JOURNAL_SCHEMA_VERSION;
    }

    if !matches!(process_is_elevated(), Ok(true)) {
        report.error = Some("offline recovery requires an elevated administrator process".into());
        return report;
    }

    if let Some(owner_process_id) = journal.owner.process_id
        && !(allow_current_owner && owner_process_id == std::process::id())
    {
        match process_is_running(owner_process_id) {
            Ok(true) => {
                report.error = Some(format!(
                    "journal owner process {owner_process_id} is still active; offline recovery refused"
                ));
                return report;
            }
            Ok(false) => {}
            Err(error) => {
                report.error = Some(error);
                return report;
            }
        }
    }

    let observed = match fresh_controller(&journal.controller_identity) {
        Ok(controller) => controller,
        Err(LocateError::Missing) => {
            report.status = OfflineRecoveryStatus::DeviceMissing;
            report.error = Some("journal controller is not currently present".into());
            return report;
        }
        Err(LocateError::Ambiguous) => {
            report.error =
                Some("journal controller identity is ambiguous; refusing mutation".into());
            return report;
        }
        Err(LocateError::Enumeration(message)) => {
            report.error = Some(message);
            return report;
        }
    };
    report.observed = Some(observed.clone());

    if has_windows_bluetooth_service(&observed) {
        // Windows may have finished the restore at boot. Re-installing the
        // same INF here can trigger its Reboot directive AGAIN. Reconcile
        // fresh state instead; a still-pending transition keeps the journal
        // unchanged and must never trigger a speculative reinstall.
        let completed = (|| {
            verify_windows_recovery(&journal, &observed)?;
            // Persist recovery intent before any permission replay, including
            // recovery after a reboot has already restored the driver binding.
            security_baseline(&journal)?;
            persist_phase(&store, &mut journal, OwnershipPhase::RestoringWindows)?;
            let refreshed = restore_windows_permissions(
                &store,
                &mut journal,
                &observed,
                Instant::now() + REENUMERATION_TIMEOUT,
            )?;
            verify_windows_recovery(&journal, &refreshed)?;
            Ok::<_, String>(refreshed)
        })();
        let completed = match completed {
            Ok(controller) => controller,
            Err(error) => {
                report.status = OfflineRecoveryStatus::RecoveryRequired;
                report.error = Some(error);
                return report;
            }
        };
        if let Err(error) = journal_to_windows_owned_and_clear(&store, &mut journal) {
            report.status = OfflineRecoveryStatus::RecoveryRequired;
            report.error = Some(error);
            return report;
        }
        report.status = OfflineRecoveryStatus::AlreadyWindowsOwned;
        report.final_state = Some(completed);
        report.journal_cleared = true;
        report.journal_retained = false;
        return report;
    }

    if !is_directhci_ready(&observed, &journal.directhci_driver_package) {
        report.error = Some(format!(
            "unexpected current driver/service ({:?}); refusing recovery mutation",
            observed.service
        ));
        return report;
    }

    match restore_windows(&store, &mut journal) {
        Ok(restored) => {
            report.status = OfflineRecoveryStatus::RestoredWindows;
            report.restore = Some(restored.install);
            report.final_state = Some(restored.final_state);
            report.journal_cleared = true;
            report.journal_retained = false;
        }
        Err(failure) => {
            report.status = OfflineRecoveryStatus::RecoveryRequired;
            report.restore = failure.install;
            report.final_state = failure.final_state;
            report.error = Some(failure.message);
        }
    }
    report
}

fn verify_windows_recovery(
    journal: &OwnershipJournal,
    controller: &ControllerObservation,
) -> Result<(), String> {
    let pending_restart = controller_restart_pending(&controller.identity)?;
    let interface_active = application_interface_is_active(
        &controller.identity.instance_id,
        &journal.directhci_driver_package.device_interface_guid,
    )?;
    windows_recovery_evidence(journal, controller, pending_restart, interface_active)
}

/// This predicate never installs a driver or rewrites a failed journal.
fn windows_recovery_evidence(
    journal: &OwnershipJournal,
    controller: &ControllerObservation,
    pending_restart: bool,
    interface_active: bool,
) -> Result<(), String> {
    if !same_physical_controller(&journal.controller_identity, &controller.identity) {
        return Err("recovery controller identity changed; journal retained".into());
    }
    if pending_restart
        || controller
            .status
            .status_flags
            .is_some_and(|flags| flags & DN_NEED_RESTART != 0)
    {
        return Err("Windows still reports a pending device restart; journal retained without repeating driver binding. Choose Windows Restart (not shutdown/power-on); repeated recovery/uninstall does not complete this transition".into());
    }
    if !is_healthy_windows_owned(controller) {
        return Err(format!(
            "Windows recovery is not yet healthy (service={:?}, flags={:?}, problem={:?}); journal retained without repeating driver binding",
            controller.service, controller.status.status_flags, controller.status.problem_code
        ));
    }
    if interface_active {
        return Err("DirectHCI application interface remains active while Windows driver is observed; journal retained without repeating driver binding".into());
    }
    if controller
        .driver
        .provider
        .as_deref()
        .is_some_and(|provider| {
            provider.eq_ignore_ascii_case(&journal.directhci_driver_package.provider)
        })
        || journal
            .directhci_driver_package
            .published_inf
            .as_deref()
            .is_some_and(|inf| {
                controller
                    .driver
                    .inf_path
                    .as_deref()
                    .is_some_and(|current| same_inf_name(current, inf))
            })
        || !observation_matches_historical_driver(
            controller,
            &journal.pre_acquire_observation.driver,
        )
    {
        return Err("observed Windows driver does not match the pre-acquire package; journal retained without repeating driver binding".into());
    }
    Ok(())
}

fn validate_pre_rebind(
    journal: &OwnershipJournal,
    direct_candidate: &CompatibleDriverObservation,
) -> Result<(), String> {
    let (controller, candidates) = observe_controller_and_drivers(&journal.controller_identity)?;
    if !is_healthy_windows_owned(&controller) {
        return Err(format!(
            "controller is no longer healthy WindowsOwned (service={:?}, present={}, problem={:?})",
            controller.service, controller.status.present, controller.status.problem_code
        ));
    }
    if !observation_matches_historical_driver(&controller, &journal.pre_acquire_observation.driver)
    {
        return Err("current Windows driver changed after preflight".into());
    }
    device_security::verify(&controller, security_baseline(journal)?)?;

    let fresh_direct: Vec<_> = candidates
        .iter()
        .filter(|candidate| same_driver_candidate(candidate, direct_candidate))
        .collect();
    let [fresh_direct] = fresh_direct.as_slice() else {
        return Err(format!(
            "DirectHCI candidate no longer resolves uniquely ({} matches)",
            fresh_direct.len()
        ));
    };
    let recovery = match select_restore_candidate(
        &journal.pre_acquire_observation.driver,
        &journal.directhci_driver_package,
        &candidates,
    ) {
        RestoreCandidateDecision::Selected(candidate) => candidate,
        RestoreCandidateDecision::Missing => {
            return Err("no safe Windows recovery candidate remains".into());
        }
        RestoreCandidateDecision::Ambiguous => {
            return Err("Windows recovery candidate became ambiguous".into());
        }
    };
    match (fresh_direct.rank, recovery.rank) {
        (Some(direct_rank), Some(windows_rank)) if direct_rank > windows_rank => Ok(()),
        (direct_rank, windows_rank) => Err(format!(
            "DirectHCI candidate is no longer strictly lower-ranked than recovery candidate (DirectHCI={direct_rank:?}, Windows={windows_rank:?})"
        )),
    }
}

fn cancel_before_rebind(
    mut report: TakeoverRoundTripReport,
    mut journal: OwnershipJournal,
    store: &JournalStore,
) -> TakeoverRoundTripReport {
    match fresh_controller(&journal.controller_identity) {
        Ok(controller)
            if is_healthy_windows_owned(&controller)
                && matches!(
                    application_interface_is_active(
                        &controller.identity.instance_id,
                        &journal.directhci_driver_package.device_interface_guid,
                    ),
                    Ok(false)
                ) =>
        {
            match journal_to_windows_owned_and_clear(store, &mut journal) {
                Ok(()) => {
                    report.final_state = Some(controller);
                    report.journal_cleared = true;
                    report.journal_retained = false;
                    report.status = RoundTripStatus::Refused;
                }
                Err(error) => {
                    report.recovery_error = Some(error);
                    report.status = RoundTripStatus::RecoveryRequired;
                }
            }
        }
        Ok(_) | Err(_) => {
            mark_recovery_required(store, &mut journal, &mut report.recovery_error);
            report.status = RoundTripStatus::RecoveryRequired;
        }
    }
    report
}

fn recover_after_primary_failure(
    mut report: TakeoverRoundTripReport,
    mut journal: OwnershipJournal,
    store: &JournalStore,
) -> TakeoverRoundTripReport {
    match restore_windows(store, &mut journal) {
        Ok(restored) => {
            report.restore = Some(restored.install);
            report.final_state = Some(restored.final_state);
            report.journal_cleared = true;
            report.journal_retained = false;
            report.status = RoundTripStatus::FailedButWindowsRestored;
        }
        Err(failure) => {
            report.restore = failure.install;
            report.final_state = failure.final_state;
            report.recovery_error = Some(failure.message);
            report.status = RoundTripStatus::RecoveryRequired;
        }
    }
    report
}

struct RestoreSuccess {
    install: DriverInstallStep,
    final_state: ControllerObservation,
}

struct RestoreFailure {
    install: Option<DriverInstallStep>,
    final_state: Option<ControllerObservation>,
    message: String,
}

fn restore_windows(
    store: &JournalStore,
    journal: &mut OwnershipJournal,
) -> Result<RestoreSuccess, Box<RestoreFailure>> {
    if let Err(message) = persist_phase(store, journal, OwnershipPhase::RestoringWindows) {
        return Err(Box::new(RestoreFailure {
            install: None,
            final_state: None,
            message,
        }));
    }
    let (_, candidates) = match observe_controller_and_drivers(&journal.controller_identity) {
        Ok(value) => value,
        Err(message) => {
            let message = retain_recovery_message(
                store,
                journal,
                &format!("fresh recovery driver enumeration failed: {message}"),
            );
            return Err(Box::new(RestoreFailure {
                install: None,
                final_state: None,
                message,
            }));
        }
    };
    let selected = match select_restore_candidate(
        &journal.pre_acquire_observation.driver,
        &journal.directhci_driver_package,
        &candidates,
    ) {
        RestoreCandidateDecision::Selected(candidate) => candidate,
        RestoreCandidateDecision::Missing => {
            let message = retain_recovery_message(
                store,
                journal,
                "no safe non-DirectHCI Windows recovery driver is available",
            );
            return Err(Box::new(RestoreFailure {
                install: None,
                final_state: None,
                message,
            }));
        }
        RestoreCandidateDecision::Ambiguous => {
            let message = retain_recovery_message(
                store,
                journal,
                "Windows recovery driver selection is ambiguous",
            );
            return Err(Box::new(RestoreFailure {
                install: None,
                final_state: None,
                message,
            }));
        }
    };

    let outcome = match install_driver_for_controller(&journal.controller_identity, &selected) {
        Ok(outcome) => outcome,
        Err(message) => {
            let retained_message = retain_recovery_message(store, journal, &message);
            return Err(Box::new(RestoreFailure {
                install: Some(failed_step(selected, &message)),
                final_state: None,
                message: retained_message,
            }));
        }
    };
    let install = successful_step(&outcome);
    if outcome.need_reboot {
        let message = retain_recovery_message(
            store,
            journal,
            "Windows driver restore requires a reboot; journal retained",
        );
        return Err(Box::new(RestoreFailure {
            install: Some(install),
            final_state: None,
            message,
        }));
    }

    // Driver binding and BTHPORT publication share one readiness budget.
    // BTHUSB/INF can be visible before its radio interface is enabled.
    let readiness_deadline = Instant::now() + REENUMERATION_TIMEOUT;
    let final_state = wait_for_controller_until(
        &journal.controller_identity,
        |controller| {
            is_healthy_windows_owned(controller)
                && observation_matches_driver_candidate(controller, &selected)
        },
        readiness_deadline,
    )
    .map_err(|message| {
        let message = retain_recovery_message(
            store,
            journal,
            &format!("post-restore WindowsOwned verification failed: {message}"),
        );
        Box::new(RestoreFailure {
            install: Some(install.clone()),
            final_state: None,
            message,
        })
    })?;
    match application_interface_is_active(
        &final_state.identity.instance_id,
        &journal.directhci_driver_package.device_interface_guid,
    ) {
        Ok(true) => {
            let message = retain_recovery_message(
                store,
                journal,
                "DirectHCI WinUSB application interface remains active after restore",
            );
            return Err(Box::new(RestoreFailure {
                install: Some(install),
                final_state: Some(final_state),
                message,
            }));
        }
        Err(message) => {
            let message = retain_recovery_message(
                store,
                journal,
                &format!("post-restore WinUSB absence check failed: {message}"),
            );
            return Err(Box::new(RestoreFailure {
                install: Some(install),
                final_state: Some(final_state),
                message,
            }));
        }
        Ok(false) => {}
    }

    let final_state =
        match restore_windows_permissions(store, journal, &final_state, readiness_deadline) {
            Ok(refreshed) => refreshed,
            Err(message) => {
                let message = retain_recovery_message(store, journal, &message);
                return Err(Box::new(RestoreFailure {
                    install: Some(install),
                    final_state: Some(final_state),
                    message,
                }));
            }
        };
    if let Err(message) = journal_to_windows_owned_and_clear(store, journal) {
        return Err(Box::new(RestoreFailure {
            install: Some(install),
            final_state: Some(final_state),
            message,
        }));
    }
    Ok(RestoreSuccess {
        install,
        final_state,
    })
}

fn select_restore_candidate(
    pre_driver: &DriverObservation,
    directhci_package: &DriverPackageIdentity,
    candidates: &[CompatibleDriverObservation],
) -> RestoreCandidateDecision {
    if let Some(original_inf) = pre_driver.inf_path.as_deref() {
        let originals: Vec<_> = candidates
            .iter()
            .filter(|candidate| same_inf_name(&candidate.inf_path, original_inf))
            .filter(|candidate| {
                pre_driver
                    .provider
                    .as_deref()
                    .is_none_or(|provider| candidate.provider.eq_ignore_ascii_case(provider))
            })
            .filter(|candidate| {
                pre_driver
                    .version
                    .as_deref()
                    .is_none_or(|version| candidate.version.eq_ignore_ascii_case(version))
            })
            .cloned()
            .collect();
        match originals.as_slice() {
            [candidate] => return RestoreCandidateDecision::Selected(candidate.clone()),
            candidates if candidates.len() > 1 => return RestoreCandidateDecision::Ambiguous,
            _ => {}
        }
    }

    let non_directhci: Vec<_> = candidates
        .iter()
        // The journal names the active DirectHCI package. A second staged
        // DirectHCI package is still not a Windows recovery driver.
        .filter(|candidate| {
            !candidate
                .provider
                .eq_ignore_ascii_case(&directhci_package.provider)
        })
        .filter_map(|candidate| candidate.rank.map(|rank| (rank, candidate)))
        .collect();
    let Some(best_rank) = non_directhci.iter().map(|(rank, _)| *rank).min() else {
        return RestoreCandidateDecision::Missing;
    };
    let best: Vec<_> = non_directhci
        .into_iter()
        .filter(|(rank, _)| *rank == best_rank)
        .map(|(_, candidate)| candidate.clone())
        .collect();
    match best.as_slice() {
        [candidate] => RestoreCandidateDecision::Selected(candidate.clone()),
        [] => RestoreCandidateDecision::Missing,
        _ => RestoreCandidateDecision::Ambiguous,
    }
}

fn is_directhci_ready(controller: &ControllerObservation, package: &DriverPackageIdentity) -> bool {
    controller.status.present
        && controller.status.problem_code == Some(0)
        && controller
            .service
            .as_deref()
            .is_some_and(|service| service.eq_ignore_ascii_case("WINUSB"))
        && controller
            .driver
            .provider
            .as_deref()
            .is_some_and(|provider| provider.eq_ignore_ascii_case(&package.provider))
        && package.published_inf.as_deref().is_none_or(|expected| {
            controller
                .driver
                .inf_path
                .as_deref()
                .is_some_and(|actual| same_inf_name(actual, expected))
        })
        && package.version.as_deref().is_none_or(|expected| {
            controller
                .driver
                .version
                .as_deref()
                .is_some_and(|actual| actual.eq_ignore_ascii_case(expected))
        })
}

fn observation_matches_driver_candidate(
    controller: &ControllerObservation,
    candidate: &CompatibleDriverObservation,
) -> bool {
    controller
        .driver
        .inf_path
        .as_deref()
        .is_some_and(|actual| same_inf_name(actual, &candidate.inf_path))
        && controller
            .driver
            .provider
            .as_deref()
            .is_some_and(|actual| actual.eq_ignore_ascii_case(&candidate.provider))
        && controller
            .driver
            .version
            .as_deref()
            .is_some_and(|actual| actual.eq_ignore_ascii_case(&candidate.version))
}

fn observation_matches_historical_driver(
    controller: &ControllerObservation,
    historical: &DriverObservation,
) -> bool {
    let Some(expected_inf) = historical.inf_path.as_deref() else {
        return false;
    };
    controller
        .driver
        .inf_path
        .as_deref()
        .is_some_and(|actual| same_inf_name(actual, expected_inf))
        && historical.provider.as_deref().is_none_or(|expected| {
            controller
                .driver
                .provider
                .as_deref()
                .is_some_and(|actual| actual.eq_ignore_ascii_case(expected))
        })
        && historical.version.as_deref().is_none_or(|expected| {
            controller
                .driver
                .version
                .as_deref()
                .is_some_and(|actual| actual.eq_ignore_ascii_case(expected))
        })
}

fn has_windows_bluetooth_service(controller: &ControllerObservation) -> bool {
    controller.service.as_deref().is_some_and(|service| {
        service.eq_ignore_ascii_case("BTHUSB") || service.eq_ignore_ascii_case("IBTUSB")
    })
}

fn is_healthy_windows_owned(controller: &ControllerObservation) -> bool {
    controller.status.present
        && controller.status.problem_code == Some(0)
        && controller.status.status_flags.is_some_and(|flags| {
            flags & DN_STARTED != 0 && flags & (DN_NEED_RESTART | DN_HAS_PROBLEM) == 0
        })
        && controller
            .driver
            .inf_path
            .as_deref()
            .is_some_and(|inf| !inf.is_empty())
        && controller
            .driver
            .provider
            .as_deref()
            .is_some_and(|provider| {
                !provider.is_empty() && !provider.eq_ignore_ascii_case("DirectHCI Project")
            })
        && has_windows_bluetooth_service(controller)
}

fn wait_for_controller(
    identity: &ControllerIdentity,
    predicate: impl Fn(&ControllerObservation) -> bool,
) -> Result<ControllerObservation, String> {
    wait_for_controller_until(identity, predicate, Instant::now() + REENUMERATION_TIMEOUT)
}

fn wait_for_controller_until(
    identity: &ControllerIdentity,
    predicate: impl Fn(&ControllerObservation) -> bool,
    deadline: Instant,
) -> Result<ControllerObservation, String> {
    let mut last_observation: String;
    loop {
        match fresh_controller(identity) {
            Ok(controller) => {
                if predicate(&controller) {
                    return Ok(controller);
                }
                last_observation = format!(
                    "service={:?}, INF={:?}, present={}, problem={:?}",
                    controller.service,
                    controller.driver.inf_path,
                    controller.status.present,
                    controller.status.problem_code
                );
            }
            Err(error) => last_observation = error.to_string(),
        }
        if Instant::now() >= deadline {
            return Err(format!(
                "timed out after {} seconds; last observation: {last_observation}",
                REENUMERATION_TIMEOUT.as_secs()
            ));
        }
        thread::sleep(REENUMERATION_POLL);
    }
}

enum LocateError {
    Missing,
    Ambiguous,
    Enumeration(String),
}

impl std::fmt::Display for LocateError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Missing => formatter.write_str("controller missing"),
            Self::Ambiguous => formatter.write_str("controller identity ambiguous"),
            Self::Enumeration(message) => formatter.write_str(message),
        }
    }
}

fn fresh_controller(identity: &ControllerIdentity) -> Result<ControllerObservation, LocateError> {
    let controllers = crate::enumerate_controllers()
        .map_err(|error| LocateError::Enumeration(error.to_string()))?;
    let mut matches = controllers
        .into_iter()
        .filter(|controller| same_physical_controller(identity, &controller.identity));
    let controller = matches.next().ok_or(LocateError::Missing)?;
    if matches.next().is_some() {
        return Err(LocateError::Ambiguous);
    }
    Ok(controller)
}

fn persist_phase(
    store: &JournalStore,
    journal: &mut OwnershipJournal,
    phase: OwnershipPhase,
) -> Result<(), String> {
    let now = unix_time_ms().map_err(|error| error.to_string())?;
    if journal.phase == phase {
        journal.updated_unix_ms = now;
    } else {
        journal
            .transition_to(phase, now)
            .map_err(|error| error.to_string())?;
    }
    store
        .write(journal)
        .map_err(|error| format!("persist {phase:?} journal: {error}"))
}

fn journal_to_windows_owned_and_clear(
    store: &JournalStore,
    journal: &mut OwnershipJournal,
) -> Result<(), String> {
    if journal.windows_stack_restart_pending {
        return Err("WindowsStackRestartPending: permissions may match, but post-permission Windows device restart has not been verified; journal retained".into());
    }
    // Every success/cleanup route (release, failed acquire, offline recovery,
    // startup reconciliation) must include permissions, not just BTHUSB/INF.
    let controller = fresh_controller(&journal.controller_identity)
        .map_err(|e| format!("verify controller before clearing recovery journal: {e}"))?;
    verify_permission_restore_target(journal, &controller)?;
    device_security::verify(&controller, security_baseline(journal)?)?;
    if journal.phase != OwnershipPhase::WindowsOwned
        && journal.phase != OwnershipPhase::RestoringWindows
    {
        persist_phase(store, journal, OwnershipPhase::RestoringWindows)?;
    }
    if journal.phase != OwnershipPhase::WindowsOwned {
        persist_phase(store, journal, OwnershipPhase::WindowsOwned)?;
    }
    store
        .clear()
        .map_err(|error| format!("clear satisfied ownership journal: {error}"))
}

fn security_baseline(journal: &OwnershipJournal) -> Result<&ControllerSecurityBaseline, String> {
    journal.pre_acquire_security.as_ref().ok_or_else(|| {
        "ControllerSecurityBaselineMissing: this legacy journal has no pre-acquire device permissions. Windows driver binding alone cannot confirm permission recovery; journal retained. Do not invent a default ACL or repeatedly rebind/reboot to clear this error".into()
    })
}

fn restore_windows_permissions(
    store: &JournalStore,
    journal: &mut OwnershipJournal,
    controller: &ControllerObservation,
    readiness_deadline: Instant,
) -> Result<ControllerObservation, String> {
    let baseline = security_baseline(journal)?.clone();
    // No Windows ACL is applied while the controller is still WinUSB-bound.
    // That would briefly expose the raw transport to Windows radio clients.
    let (controller, permissions_match) =
        wait_for_radio_permissions(journal, controller, &baseline, readiness_deadline)?;
    if permissions_match && !journal.windows_stack_restart_pending {
        return Ok(controller);
    }
    // Persist BEFORE either ACL write. A crash after readback but before the
    // restart must not let recovery clear the journal on matching ACLs alone.
    journal.windows_stack_restart_pending = true;
    persist_phase(store, journal, OwnershipPhase::RestoringWindows)?;
    device_security::restore_override(&controller, &baseline)?;
    device_security::restore_radio(&controller, &baseline)?;
    device_security::verify(&controller, &baseline)?;

    // All radio handles above have closed. The first Windows start may have
    // occurred with DirectHCI's restrictive DACL. Restart ONLY this Windows
    // devnode after replaying the baseline, never an active WinUSB devnode.
    // One attempt per recovery call; polling below never repeats the mutation.
    verify_permission_restore_target(journal, &controller)?;
    if restart_windows_controller(&controller)? {
        return Err("WindowsStackRestartRequired: Windows requested a system restart while reinitializing the Bluetooth device after permission restoration; journal retained. Choose Windows Restart; no automatic reboot or repeated driver installation was performed".into());
    }
    let refreshed = wait_for_permission_restart(journal, &controller)?;
    journal.windows_stack_restart_pending = false;
    if let Err(error) = persist_phase(store, journal, OwnershipPhase::RestoringWindows) {
        // Keep in-memory intent conservative if a subsequent error handler
        // writes the journal again after this failed commit.
        journal.windows_stack_restart_pending = true;
        return Err(error);
    }
    Ok(refreshed)
}

fn wait_for_radio_permissions(
    journal: &OwnershipJournal,
    previous: &ControllerObservation,
    baseline: &ControllerSecurityBaseline,
    deadline: Instant,
) -> Result<(ControllerObservation, bool), String> {
    loop {
        let controller = fresh_controller(&journal.controller_identity)
            .map_err(|e| format!("before permission replay: re-enumerate controller: {e}"))?;
        if !controller
            .identity
            .instance_id
            .eq_ignore_ascii_case(&previous.identity.instance_id)
            || controller.driver != previous.driver
            || controller.service != previous.service
        {
            return Err("before permission replay: controller identity or Windows binding changed while waiting for BTHPORT; journal retained".into());
        }
        verify_permission_restore_target(journal, &controller)
            .map_err(|e| format!("before permission replay: {e}"))?;
        // Only a successful enumeration with zero present interfaces is
        // retryable here. API/ACL errors and multiple interfaces fail closed.
        // No write, restart or driver installation is performed in this loop.
        if let Some(matches) = device_security::try_matches_baseline(&controller, baseline)
            .map_err(|e| format!("before permission replay: {e}"))?
        {
            return Ok((controller, matches));
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(format!(
                "BthportInterfaceNotReady: 0 present interfaces for controller {} within the {}-second Windows/radio readiness budget; permission replay was not started in this attempt; journal retained",
                controller.identity.instance_id,
                REENUMERATION_TIMEOUT.as_secs()
            ));
        }
        thread::sleep(REENUMERATION_POLL.min(remaining));
    }
}

fn wait_for_permission_restart(
    journal: &OwnershipJournal,
    previous: &ControllerObservation,
) -> Result<ControllerObservation, String> {
    let deadline = Instant::now() + REENUMERATION_TIMEOUT;
    loop {
        let observation: Result<ControllerObservation, String> = (|| {
            let controller = fresh_controller(&journal.controller_identity)
                .map_err(|e| format!("re-enumerate permission restart: {e}"))?;
            if !controller
                .identity
                .instance_id
                .eq_ignore_ascii_case(&previous.identity.instance_id)
                || !observation_matches_historical_driver(&controller, &previous.driver)
            {
                return Err(
                    "controller identity or Windows driver changed after permission restart".into(),
                );
            }
            verify_permission_restore_target(journal, &controller)?;
            // This opens the exact new BTHPORT interface, not a retained handle
            // from before the restart. A successful SetupAPI call is not enough.
            device_security::verify(&controller, security_baseline(journal)?)?;
            Ok(controller)
        })();
        match observation {
            Ok(controller) => return Ok(controller),
            Err(error) if Instant::now() >= deadline => {
                return Err(format!(
                    "WindowsStackRestartUnverified: {error}; journal retained; device restart will not be repeated in this operation"
                ));
            }
            Err(_) => thread::sleep(REENUMERATION_POLL),
        }
    }
}

fn verify_permission_restore_target(
    journal: &OwnershipJournal,
    controller: &ControllerObservation,
) -> Result<(), String> {
    // Normal restore has already matched its freshly selected Windows
    // candidate (which may be a legitimate fallback). Do not replace that
    // selection policy with a demand for the historical INF here.
    if !same_physical_controller(&journal.controller_identity, &controller.identity)
        || !is_healthy_windows_owned(controller)
        || controller
            .driver
            .provider
            .as_deref()
            .is_some_and(|p| p.eq_ignore_ascii_case(&journal.directhci_driver_package.provider))
        || controller_restart_pending(&controller.identity)?
        || application_interface_is_active(
            &controller.identity.instance_id,
            &journal.directhci_driver_package.device_interface_guid,
        )?
    {
        return Err("controller permissions cannot be restored/confirmed until this same controller is healthy Windows-owned with no active DirectHCI interface or pending restart".into());
    }
    Ok(())
}

fn mark_recovery_required(
    store: &JournalStore,
    journal: &mut OwnershipJournal,
    report_error: &mut Option<String>,
) {
    if let Err(error) = mark_journal_recovery_required(store, journal) {
        *report_error = Some(error);
    }
}

fn mark_journal_recovery_required(
    store: &JournalStore,
    journal: &mut OwnershipJournal,
) -> Result<(), String> {
    if journal.phase == OwnershipPhase::RecoveryRequired {
        journal.updated_unix_ms = unix_time_ms().map_err(|error| error.to_string())?;
        return store
            .write(journal)
            .map_err(|error| format!("refresh RecoveryRequired journal: {error}"));
    }
    persist_phase(store, journal, OwnershipPhase::RecoveryRequired)
}

fn retain_recovery_message(
    store: &JournalStore,
    journal: &mut OwnershipJournal,
    primary: &str,
) -> String {
    match mark_journal_recovery_required(store, journal) {
        Ok(()) => primary.to_owned(),
        Err(journal_error) => {
            format!("{primary}; additionally failed to persist RecoveryRequired: {journal_error}")
        }
    }
}

fn successful_step(outcome: &DriverInstallOutcome) -> DriverInstallStep {
    DriverInstallStep {
        selected: outcome.selected.clone(),
        api_succeeded: true,
        need_reboot: Some(outcome.need_reboot),
        error: None,
    }
}

fn failed_step(selected: CompatibleDriverObservation, error: &str) -> DriverInstallStep {
    DriverInstallStep {
        selected,
        api_succeeded: false,
        need_reboot: None,
        error: Some(error.to_owned()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use directhci_core::DeviceStatus;

    fn recovery_fixture() -> (OwnershipJournal, ControllerObservation) {
        let identity = ControllerIdentity::new(
            Some(0x8087),
            Some(0x0026),
            None,
            None,
            r"USB\VID_8087&PID_0026\INSTANCE".into(),
            vec!["USBROOT(0)#USB(14)".into()],
            None,
            vec![r"USB\VID_8087&PID_0026".into()],
            Vec::new(),
        );
        let controller = ControllerObservation::new(
            identity.clone(),
            Some("Intel Bluetooth".into()),
            None,
            None,
            Some("Bluetooth".into()),
            None,
            Some("USB".into()),
            Some("BTHUSB".into()),
            DriverObservation {
                inf_path: Some("oem184.inf".into()),
                provider: Some("Intel Corporation".into()),
                version: Some("24.70.0.4".into()),
                driver_key: None,
            },
            Vec::new(),
            DeviceStatus {
                present: true,
                status_flags: Some(DN_STARTED),
                problem_code: Some(0),
            },
        );
        let mut journal = OwnershipJournal::new(
            LeaseId::new("recovery-test").unwrap(),
            identity,
            controller.clone(),
            direct_package(),
            1000,
            2000,
            LeaseOwnerMetadata::default(),
        );
        journal.phase = OwnershipPhase::RecoveryRequired;
        (journal, controller)
    }

    #[test]
    fn recovery_accepts_verified_windows_state_regardless_of_old_phase() {
        let (mut journal, controller) = recovery_fixture();
        for phase in [
            OwnershipPhase::WindowsOwned,
            OwnershipPhase::AcquirePrepared,
            OwnershipPhase::RebindingToDirectHci,
            OwnershipPhase::DirectHciReady,
            OwnershipPhase::DirectHciOwned,
            OwnershipPhase::RestoringWindows,
            OwnershipPhase::RecoveryRequired,
        ] {
            journal.phase = phase;
            assert_eq!(
                windows_recovery_evidence(&journal, &controller, false, false),
                Ok(())
            );
        }
    }

    #[test]
    fn recovery_does_not_treat_zero_problem_code_as_completed_restart() {
        let (journal, mut controller) = recovery_fixture();
        // Actual incident: DN_STARTED and DN_NEED_RESTART, problem_code=0.
        controller.status.status_flags = Some(25174282);
        let error = windows_recovery_evidence(&journal, &controller, false, false).unwrap_err();
        assert!(error.contains("pending device restart"));
        assert_eq!(journal.phase, OwnershipPhase::RecoveryRequired);
        assert_eq!(journal.updated_unix_ms, 2000);
    }

    #[test]
    fn recovery_retains_journal_for_setupapi_pending_restart_or_enabled_interface() {
        let (journal, controller) = recovery_fixture();
        assert!(windows_recovery_evidence(&journal, &controller, true, false).is_err());
        assert!(windows_recovery_evidence(&journal, &controller, false, true).is_err());
    }

    #[test]
    fn recovery_requires_present_started_known_healthy_devnode() {
        let (journal, controller) = recovery_fixture();
        for status in [
            DeviceStatus {
                present: false,
                ..controller.status.clone()
            },
            DeviceStatus {
                status_flags: None,
                ..controller.status.clone()
            },
            DeviceStatus {
                status_flags: Some(0),
                ..controller.status.clone()
            },
            DeviceStatus {
                status_flags: Some(DN_STARTED | DN_HAS_PROBLEM),
                ..controller.status.clone()
            },
            DeviceStatus {
                problem_code: None,
                ..controller.status.clone()
            },
            DeviceStatus {
                problem_code: Some(43),
                ..controller.status.clone()
            },
        ] {
            let mut observed = controller.clone();
            observed.status = status;
            assert!(windows_recovery_evidence(&journal, &observed, false, false).is_err());
        }
    }

    #[test]
    fn recovery_requires_same_controller_and_original_windows_package() {
        let (journal, controller) = recovery_fixture();
        let mut changed = controller.clone();
        changed.identity.location_paths = vec!["USBROOT(0)#USB(2)".into()];
        assert!(windows_recovery_evidence(&journal, &changed, false, false).is_err());
        let mut changed = controller.clone();
        changed.driver.inf_path = Some("oem200.inf".into());
        assert!(windows_recovery_evidence(&journal, &changed, false, false).is_err());
        let mut changed = controller.clone();
        changed.driver.version = Some("different".into());
        assert!(windows_recovery_evidence(&journal, &changed, false, false).is_err());
        let mut changed = controller;
        changed.service = Some("WinUSB".into());
        assert!(windows_recovery_evidence(&journal, &changed, false, false).is_err());
    }

    #[test]
    fn recovery_rejects_directhci_driver_even_with_windows_service() {
        let (mut journal, mut controller) = recovery_fixture();
        controller.driver.provider = Some("DirectHCI Project".into());
        journal.pre_acquire_observation.driver = controller.driver.clone();
        assert!(windows_recovery_evidence(&journal, &controller, false, false).is_err());
        controller.driver.provider = Some("Intel Corporation".into());
        controller.driver.inf_path = journal.directhci_driver_package.published_inf.clone();
        journal.pre_acquire_observation.driver = controller.driver.clone();
        assert!(windows_recovery_evidence(&journal, &controller, false, false).is_err());
    }

    fn driver(
        index: u32,
        rank: u32,
        inf: &str,
        provider: &str,
        version: &str,
    ) -> CompatibleDriverObservation {
        CompatibleDriverObservation {
            list_index: index,
            rank: Some(rank),
            description: format!("{provider} driver"),
            manufacturer: provider.into(),
            provider: provider.into(),
            version: version.into(),
            raw_version: 1,
            driver_date_filetime: 1,
            inf_path: inf.into(),
            section: "Install".into(),
            hardware_id: "USB\\VID_8087&PID_0026".into(),
            currently_installed: false,
        }
    }

    fn direct_package() -> DriverPackageIdentity {
        DriverPackageIdentity {
            provider: "DirectHCI Project".into(),
            description: "DirectHCI Project driver".into(),
            published_inf: Some("oem99.inf".into()),
            version: Some("0.1.0.0".into()),
            device_interface_guid: "{guid}".into(),
        }
    }

    #[test]
    fn restore_prefers_exact_pre_acquire_package() {
        let pre = DriverObservation {
            inf_path: Some("oem69.inf".into()),
            provider: Some("Intel".into()),
            version: Some("23.90.0.8".into()),
            driver_key: None,
        };
        let candidates = vec![
            driver(0, 10, "oem70.inf", "Intel", "24.0.0.0"),
            driver(1, 20, r"C:\Windows\INF\oem69.inf", "Intel", "23.90.0.8"),
        ];
        assert!(matches!(
            select_restore_candidate(&pre, &direct_package(), &candidates),
            RestoreCandidateDecision::Selected(candidate)
                if candidate.inf_path == r"C:\Windows\INF\oem69.inf"
        ));
    }

    #[test]
    fn restore_falls_back_to_unique_best_non_directhci_rank() {
        let pre = DriverObservation::default();
        let candidates = vec![
            driver(0, 10, "oem70.inf", "Intel", "24.0.0.0"),
            driver(1, 20, "oem71.inf", "Intel", "23.0.0.0"),
            driver(2, 100, "oem99.inf", "DirectHCI Project", "0.1.0.0"),
        ];
        assert!(matches!(
            select_restore_candidate(&pre, &direct_package(), &candidates),
            RestoreCandidateDecision::Selected(candidate)
                if candidate.inf_path == "oem70.inf"
        ));
    }

    #[test]
    fn restore_refuses_equal_rank_ambiguity() {
        let pre = DriverObservation::default();
        let candidates = vec![
            driver(0, 10, "oem70.inf", "Vendor A", "1.0"),
            driver(1, 10, "oem71.inf", "Vendor B", "1.0"),
        ];
        assert_eq!(
            select_restore_candidate(&pre, &direct_package(), &candidates),
            RestoreCandidateDecision::Ambiguous
        );
    }

    #[test]
    fn restore_refuses_when_only_directhci_candidate_exists() {
        let pre = DriverObservation::default();
        let candidates = vec![driver(0, 10, "oem99.inf", "DirectHCI Project", "0.1.0.0")];
        assert_eq!(
            select_restore_candidate(&pre, &direct_package(), &candidates),
            RestoreCandidateDecision::Missing
        );
    }
}
