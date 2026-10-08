use std::process::ExitCode;

use directhci_client::DirectHciClient;
use directhci_core::{
    ControllerObservation, HCI_READ_LOCAL_VERSION_INFORMATION, HCI_RESET,
    parse_local_version_information, parse_reset_response,
};
use directhci_windows::{
    DedicatedWinUsbProbeReport, DedicatedWinUsbProbeStatus, DedicatedWinUsbReadinessStatus,
    DirectHciPackageReadiness, HciInformationReport, OfflineRecoveryReport, OfflineRecoveryStatus,
    RoundTripStatus, TakeoverPreflightReport, TakeoverRoundTripReport, TemporaryRebindPlan,
    UsbDkApiStatus, UsbDkControllerCorrelationStatus, UsbDkEnumerationStatus, UsbDkHelperStatus,
    UsbDkProbeReport, UsbDkServiceStatus,
};

fn main() -> ExitCode {
    match run(std::env::args().skip(1).collect()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("error: {message}");
            ExitCode::FAILURE
        }
    }
}

fn run(arguments: Vec<String>) -> Result<(), String> {
    #[cfg(windows)]
    let _presence = directhci_windows::process_lifecycle::enter_application()?;
    match arguments.as_slice() {
        [command] if command == "controllers" => list_controllers(false, false),
        [command, flag] if command == "controllers" && flag == "--json" => {
            list_controllers(true, false)
        }
        [command, flag] if command == "controllers" && flag == "--direct" => {
            list_controllers(false, true)
        }
        [command, first, second]
            if command == "controllers"
                && ((first == "--direct" && second == "--json")
                    || (first == "--json" && second == "--direct")) =>
        {
            list_controllers(true, true)
        }
        [command] if command == "status" => runtime_status(false),
        [command, flag] if command == "status" && flag == "--json" => runtime_status(true),
        [group, command, id] if group == "controller" && command == "show" => {
            show_controller(id, false)
        }
        [group, command, id, flag]
            if group == "controller" && command == "show" && flag == "--json" =>
        {
            show_controller(id, true)
        }
        [command] if command == "doctor" => doctor(false),
        [command, flag] if command == "doctor" && flag == "--json" => doctor(true),
        [group, command, id] if group == "takeover" && command == "plan" => {
            takeover_plan(id, false)
        }
        [group, command, id, flag]
            if group == "takeover" && command == "plan" && flag == "--json" =>
        {
            takeover_plan(id, true)
        }
        [group, command, id, flag]
            if group == "takeover" && command == "roundtrip" && flag == "--execute" =>
        {
            takeover_roundtrip(id, false)
        }
        [group, command, id, execute, json]
            if group == "takeover"
                && command == "roundtrip"
                && ((execute == "--execute" && json == "--json")
                    || (execute == "--json" && json == "--execute")) =>
        {
            takeover_roundtrip(id, true)
        }
        [group, command, id, flag]
            if group == "takeover" && command == "hci-info" && flag == "--execute" =>
        {
            takeover_hci_info(id, false)
        }
        [group, command, id, execute, json]
            if group == "takeover"
                && command == "hci-info"
                && ((execute == "--execute" && json == "--json")
                    || (execute == "--json" && json == "--execute")) =>
        {
            takeover_hci_info(id, true)
        }
        [command, id, flag] if command == "hci-info" && flag == "--execute" => {
            runtime_hci_info(id, false)
        }
        [command, id, execute, json]
            if command == "hci-info"
                && ((execute == "--execute" && json == "--json")
                    || (execute == "--json" && json == "--execute")) =>
        {
            runtime_hci_info(id, true)
        }
        [command, flag] if command == "recover" && flag == "--offline" => recover_offline(false),
        [command, offline, json]
            if command == "recover" && offline == "--offline" && json == "--json" =>
        {
            recover_offline(true)
        }
        [] => {
            print_help();
            Ok(())
        }
        [flag] if flag == "-h" || flag == "--help" => {
            print_help();
            Ok(())
        }
        [flag] if flag == "-V" || flag == "--version" => {
            println!("directhci {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        _ => Err("invalid command; run `directhci --help`".into()),
    }
}

fn list_controllers(json: bool, direct: bool) -> Result<(), String> {
    let controllers = if direct {
        enumerate()?
    } else {
        runtime_client()?
            .list_controllers()
            .map_err(|error| error.to_string())?
    };
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&controllers).map_err(|error| error.to_string())?
        );
        return Ok(());
    }

    if controllers.is_empty() {
        println!("No present USB Bluetooth controller candidates were found.");
        return Ok(());
    }

    println!("ID                         USB       DRIVER             DESCRIPTION");
    for controller in controllers {
        println!(
            "{:<26} {:<9} {:<18} {}",
            controller.id,
            format_vid_pid(&controller),
            controller
                .driver
                .inf_path
                .as_deref()
                .or(controller.service.as_deref())
                .unwrap_or("unknown"),
            description(&controller),
        );
    }
    Ok(())
}

fn runtime_client() -> Result<DirectHciClient, String> {
    DirectHciClient::connect("directhci-cli", Some(env!("CARGO_PKG_VERSION").into()))
        .map_err(|error| format!("connect to directhcid: {error}"))
}

fn runtime_status(json: bool) -> Result<(), String> {
    let status = runtime_client()?
        .runtime_status()
        .map_err(|error| error.to_string())?;
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&status).map_err(|error| error.to_string())?
        );
    } else {
        println!("DirectHCI Runtime {}", status.runtime_version);
        println!("  recovery required: {}", yes_no(status.recovery_required));
        if let Some(message) = &status.recovery_message {
            println!("  recovery message:  {message}");
        }
        match &status.active_session {
            Some(session) => println!(
                "  active session:    {} client={} controller={}",
                session.session_id, session.client_name, session.controller_id
            ),
            None => println!("  active session:    none"),
        }
        for controller in &status.controllers {
            println!("  {}: {:?}", controller.controller.id, controller.state);
        }
    }
    Ok(())
}

fn runtime_hci_info(id: &str, json: bool) -> Result<(), String> {
    let client = runtime_client()?;
    let mut session = client
        .acquire_raw_hci(id)
        .map_err(|error| error.to_string())?;
    let reset = session
        .send_command(HCI_RESET, &[])
        .map_err(|error| error.to_string())?;
    let reset_status = parse_reset_response(&reset).map_err(|error| error.to_string())?;
    let version_response = session
        .send_command(HCI_READ_LOCAL_VERSION_INFORMATION, &[])
        .map_err(|error| error.to_string())?;
    let version =
        parse_local_version_information(&version_response).map_err(|error| error.to_string())?;
    let session_id = session.session_id();
    let controller = session.controller().clone();
    let transport = session.transport().clone();
    let restored = session.release().map_err(|error| error.to_string())?;
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "session_id": session_id,
                "controller": controller,
                "transport": transport,
                "hci": { "reset_status": reset_status, "local_version": version },
                "restore": { "windows_owned": true, "controller": restored },
            }))
            .map_err(|error| error.to_string())?
        );
    } else {
        println!("DirectHCI runtime HCI information");
        println!("  session:        {session_id}");
        println!("  controller:     {}", controller.id);
        println!("  reset status:   {reset_status:#04x}");
        println!("  HCI version:    {:#04x}", version.hci_version);
        println!("  HCI revision:   {:#06x}", version.hci_revision);
        println!("  manufacturer:   {:#06x}", version.manufacturer_name);
        println!("  LMP version:    {:#04x}", version.lmp_pal_version);
        println!("  LMP subversion: {:#06x}", version.lmp_pal_subversion);
        println!(
            "  transport:      event=0x{:02X} ACL_IN=0x{:02X} ACL_OUT=0x{:02X}",
            transport.event_pipe, transport.acl_in_pipe, transport.acl_out_pipe
        );
        println!("  restore:        WindowsOwned");
    }
    Ok(())
}

fn show_controller(id: &str, json: bool) -> Result<(), String> {
    let controller = enumerate()?
        .into_iter()
        .find(|controller| controller.id.as_str().eq_ignore_ascii_case(id))
        .ok_or_else(|| format!("controller `{id}` was not found in the current observation"))?;

    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&controller).map_err(|error| error.to_string())?
        );
        return Ok(());
    }

    println!("Controller:       {}", controller.id);
    println!("Description:      {}", description(&controller));
    println!("USB:              {}", format_vid_pid(&controller));
    println!("Identity:         {:?}", controller.identity.confidence);
    println!("Instance ID:      {}", controller.identity.instance_id);
    print_optional("Container ID", controller.identity.container_id.as_deref());
    print_optional(
        "Parent instance",
        controller.identity.parent_instance_id.as_deref(),
    );
    print_optional("Service", controller.service.as_deref());
    print_optional("INF", controller.driver.inf_path.as_deref());
    print_optional("Driver provider", controller.driver.provider.as_deref());
    print_optional("Driver version", controller.driver.version.as_deref());
    println!("Problem code:     {:?}", controller.status.problem_code);
    print_values("Location paths", &controller.identity.location_paths);
    print_values("Hardware IDs", &controller.identity.hardware_ids);
    print_values("Compatible IDs", &controller.identity.compatible_ids);
    print_values("Interface paths", &controller.interface_paths);
    Ok(())
}

fn enumerate() -> Result<Vec<ControllerObservation>, String> {
    directhci_windows::enumerate_controllers().map_err(|error| error.to_string())
}

fn doctor(json: bool) -> Result<(), String> {
    let (controllers, controller_error) = match directhci_windows::enumerate_controllers() {
        Ok(controllers) => (controllers, None),
        Err(error) => (Vec::new(), Some(error.to_string())),
    };
    let usbdk = directhci_windows::probe_usbdk(&controllers);
    let dedicated_winusb = directhci_windows::probe_dedicated_winusb();

    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "directhci_version": env!("CARGO_PKG_VERSION"),
                "platform": std::env::consts::OS,
                "controller_count": controllers.len(),
                "controller_error": controller_error,
                "controllers": controllers,
                "usbdk": usbdk,
                "dedicated_winusb": dedicated_winusb,
            }))
            .map_err(|error| error.to_string())?
        );
        return Ok(());
    }

    println!("DirectHCI");
    println!("  version:     {}", env!("CARGO_PKG_VERSION"));
    println!("  platform:    {}", std::env::consts::OS);
    if let Some(error) = controller_error {
        println!("  controllers: error ({error})");
    } else {
        println!("  controllers: {}", controllers.len());
    }
    print_usbdk_report(&usbdk);
    print_dedicated_winusb_report(&dedicated_winusb);
    Ok(())
}

fn takeover_plan(id: &str, json: bool) -> Result<(), String> {
    let plan = directhci_windows::plan_takeover(id);

    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&plan).map_err(|error| error.to_string())?
        );
        return Ok(());
    }

    print_takeover_preflight(&plan);
    Ok(())
}

fn takeover_roundtrip(id: &str, json: bool) -> Result<(), String> {
    let report = directhci_windows::execute_takeover_roundtrip(id);
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&report).map_err(|error| error.to_string())?
        );
    } else {
        print_roundtrip_report(&report);
    }
    match report.status {
        RoundTripStatus::Completed => Ok(()),
        RoundTripStatus::FailedButWindowsRestored => {
            Err("takeover failed, but Windows Bluetooth was restored".into())
        }
        RoundTripStatus::Refused => Err("takeover was refused before any driver mutation".into()),
        RoundTripStatus::RecoveryRequired => Err(
            "takeover recovery is required; run `directhci recover --offline` as administrator"
                .into(),
        ),
    }
}

fn takeover_hci_info(id: &str, json: bool) -> Result<(), String> {
    let report = directhci_windows::execute_takeover_hci_info(id);
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&report).map_err(|error| error.to_string())?
        );
    } else {
        print_hci_information_report(&report);
    }
    match report.takeover.status {
        RoundTripStatus::Completed => Ok(()),
        RoundTripStatus::FailedButWindowsRestored => {
            Err("Raw HCI bring-up failed, but Windows Bluetooth was restored".into())
        }
        RoundTripStatus::Refused => {
            Err("HCI takeover was refused before any driver mutation".into())
        }
        RoundTripStatus::RecoveryRequired => Err(
            "HCI takeover recovery is required; run `directhci recover --offline` as administrator"
                .into(),
        ),
    }
}

fn recover_offline(json: bool) -> Result<(), String> {
    let report = directhci_windows::recover_offline();
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&report).map_err(|error| error.to_string())?
        );
    } else {
        print_offline_recovery_report(&report);
    }
    match report.status {
        OfflineRecoveryStatus::NoJournal
        | OfflineRecoveryStatus::AlreadyWindowsOwned
        | OfflineRecoveryStatus::RestoredWindows => Ok(()),
        OfflineRecoveryStatus::DeviceMissing => {
            Err("journal controller is missing; journal retained".into())
        }
        OfflineRecoveryStatus::Refused | OfflineRecoveryStatus::RecoveryRequired => {
            Err("offline recovery did not converge to WindowsOwned; journal retained".into())
        }
    }
}

fn print_takeover_preflight(preflight: &TakeoverPreflightReport) {
    println!("Temporary WinUSB takeover preflight");
    println!("  elevated:          {:?}", preflight.elevated);
    println!("  journal:           {:?}", preflight.journal_status);
    println!("  journal path:      {}", preflight.journal_path);
    println!(
        "  package accepted:  {}",
        yes_no(preflight.package_accepted_by_driver_store)
    );
    print_takeover_plan(&preflight.rebind);
    println!(
        "  complete preflight: {}",
        yes_no(preflight.safe_to_execute)
    );
    if !preflight.blockers.is_empty() {
        println!("  complete blockers:");
        for blocker in &preflight.blockers {
            println!("    - {blocker}");
        }
    }
}

fn print_takeover_plan(plan: &TemporaryRebindPlan) {
    println!("Temporary WinUSB rebind plan");
    println!("  controller:      {}", plan.requested_controller_id);
    println!("  current state:   {:?}", plan.current_state);
    if let Some(controller) = &plan.controller {
        println!("  instance:        {}", controller.identity.instance_id);
        println!(
            "  current driver:  service={} INF={} provider={} version={}",
            controller.service.as_deref().unwrap_or("unknown"),
            controller.driver.inf_path.as_deref().unwrap_or("unknown"),
            controller.driver.provider.as_deref().unwrap_or("unknown"),
            controller.driver.version.as_deref().unwrap_or("unknown"),
        );
    }

    println!("  applicable drivers: {}", plan.applicable_drivers.len());
    for driver in &plan.applicable_drivers {
        println!(
            "    rank={} INF={} provider={} description={}{}",
            driver
                .rank
                .map_or_else(|| "unknown".into(), |rank| rank.to_string()),
            driver.inf_path,
            driver.provider,
            driver.description,
            if driver.currently_installed {
                " [current]"
            } else {
                ""
            },
        );
    }

    match &plan.directhci_package {
        DirectHciPackageReadiness::NotInspected => {
            println!("  DirectHCI package: not inspected")
        }
        DirectHciPackageReadiness::Missing { expected } => println!(
            "  DirectHCI package: missing (expected {} / {})",
            expected.provider, expected.description
        ),
        DirectHciPackageReadiness::Ambiguous { candidates, .. } => println!(
            "  DirectHCI package: ambiguous ({} candidates)",
            candidates.len()
        ),
        DirectHciPackageReadiness::Ready { candidate, .. } => println!(
            "  DirectHCI package: ready ({} rank={})",
            candidate.inf_path,
            candidate
                .rank
                .map_or_else(|| "unknown".into(), |rank| rank.to_string())
        ),
    }
    println!(
        "  package in Driver Store: {}",
        yes_no(plan.package_accepted_by_driver_store)
    );
    println!(
        "  default selection risk:  {:?}",
        plan.default_selection_risk
    );

    if let Some(driver) = &plan.best_windows_recovery_driver {
        println!(
            "  recovery driver: {} / {} (rank={})",
            driver.provider,
            driver.inf_path,
            driver
                .rank
                .map_or_else(|| "unknown".into(), |rank| rank.to_string())
        );
    } else {
        println!("  recovery driver: none identified");
    }
    println!(
        "  switch candidate: {}",
        yes_no(plan.driver_switch_candidate_ready)
    );
    println!("  safe to execute:  {}", yes_no(plan.safe_to_execute));
    println!("  install API:      {}", plan.install_api);
    if plan.blockers.is_empty() {
        println!("  blockers:         none");
    } else {
        println!("  blockers:");
        for blocker in &plan.blockers {
            println!("    - {blocker:?}");
        }
    }
    println!();
    println!("NO CHANGES APPLIED");
}

fn print_roundtrip_report(report: &TakeoverRoundTripReport) {
    println!("DirectHCI temporary WinUSB round trip");
    println!("  status: {:?}", report.status);
    if let Some(pre) = &report.pre_state {
        println!("  controller: {}", pre.id);
        println!(
            "  pre-state: service={} INF={} version={}",
            pre.service.as_deref().unwrap_or("unknown"),
            pre.driver.inf_path.as_deref().unwrap_or("unknown"),
            pre.driver.version.as_deref().unwrap_or("unknown")
        );
    }
    println!(
        "  journal: created={} cleared={} retained={}",
        yes_no(report.journal_created),
        yes_no(report.journal_cleared),
        yes_no(report.journal_retained)
    );
    print_driver_step("Rebind", report.rebind.as_ref());
    if let Some(state) = &report.directhci_state {
        println!(
            "  DirectHCI: service={} INF={} problem={:?}",
            state.service.as_deref().unwrap_or("unknown"),
            state.driver.inf_path.as_deref().unwrap_or("unknown"),
            state.status.problem_code
        );
    }
    if let Some(readiness) = &report.winusb_readiness {
        println!("  WinUSB readiness: {:?}", readiness.status);
        for interface in &readiness.application_interfaces {
            println!("    application interface: {}", interface.path);
        }
    }
    print_driver_step("Restore", report.restore.as_ref());
    if let Some(final_state) = &report.final_state {
        println!(
            "  final: service={} present={} problem={:?}",
            final_state.service.as_deref().unwrap_or("unknown"),
            final_state.status.present,
            final_state.status.problem_code
        );
    }
    if let Some(error) = &report.primary_error {
        println!("  primary error: {error}");
    }
    if let Some(error) = &report.recovery_error {
        println!("  recovery error: {error}");
    }
}

fn print_hci_information_report(report: &HciInformationReport) {
    println!("DirectHCI Raw HCI information session");
    print_roundtrip_report(&report.takeover);
    let Some(hci) = &report.hci else {
        println!("HCI: not started");
        return;
    };
    println!("HCI");
    if let Some(transport) = &hci.transport {
        println!(
            "  transport: interface={} alt={} event=0x{:02X} ACL_IN=0x{:02X} ACL_OUT=0x{:02X}",
            transport.interface_number,
            transport.alternate_setting,
            transport.event_pipe,
            transport.acl_in_pipe,
            transport.acl_out_pipe,
        );
    }
    println!(
        "  receive workers: event={} ACL={}",
        yes_no(hci.event_rx_started),
        yes_no(hci.acl_rx_started)
    );
    match hci.reset_status {
        Some(status) => println!("  Reset: status=0x{status:02X}"),
        None => println!("  Reset: not completed"),
    }
    if let Some(version) = &hci.local_version {
        println!(
            "  Local Version: HCI=0x{:02X} revision=0x{:04X} LMP/PAL=0x{:02X} manufacturer=0x{:04X} subversion=0x{:04X}",
            version.hci_version,
            version.hci_revision,
            version.lmp_pal_version,
            version.manufacturer_name,
            version.lmp_pal_subversion,
        );
    }
    if let Some(commands) = &hci.local_supported_commands {
        println!("  Supported Commands: {} bytes", commands.commands.len());
    }
    if let Some(features) = &hci.local_supported_features {
        println!("  Supported Features: {}", hex_bytes(&features.features));
    }
    if let Some(buffer) = &hci.buffer_size {
        println!(
            "  Buffer Size: ACL length={} packets={} synchronous length={} packets={}",
            buffer.acl_data_packet_length,
            buffer.total_num_acl_data_packets,
            buffer.synchronous_data_packet_length,
            buffer.total_num_synchronous_data_packets,
        );
    }
    if let Some(shutdown) = &hci.shutdown {
        println!(
            "  shutdown: event_joined={} ACL_joined={} handles_released={} cancellation_errors={}",
            yes_no(shutdown.event_rx_joined),
            yes_no(shutdown.acl_rx_joined),
            yes_no(shutdown.handles_released),
            shutdown.cancellation_errors.len(),
        );
        for error in &shutdown.cancellation_errors {
            println!("    - {error}");
        }
    }
    if let Some(error) = &hci.error {
        println!("  HCI error: {error}");
    }
}

fn hex_bytes(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|byte| format!("{byte:02X}"))
        .collect::<Vec<_>>()
        .join(" ")
}

fn print_offline_recovery_report(report: &OfflineRecoveryReport) {
    println!("DirectHCI offline recovery");
    println!("  status:          {:?}", report.status);
    println!("  journal:         {}", report.journal_path);
    println!("  stale:           {:?}", report.stale_journal);
    println!("  phase:           {:?}", report.journal_phase);
    print_driver_step("Restore", report.restore.as_ref());
    if let Some(final_state) = &report.final_state {
        println!(
            "  final: service={} present={} problem={:?}",
            final_state.service.as_deref().unwrap_or("unknown"),
            final_state.status.present,
            final_state.status.problem_code
        );
    }
    println!(
        "  journal result: cleared={} retained={}",
        yes_no(report.journal_cleared),
        yes_no(report.journal_retained)
    );
    if let Some(error) = &report.error {
        println!("  error: {error}");
    }
}

fn print_driver_step(label: &str, step: Option<&directhci_windows::DriverInstallStep>) {
    match step {
        Some(step) => println!(
            "  {label}: INF={} provider={} success={} reboot={:?} error={}",
            step.selected.inf_path,
            step.selected.provider,
            yes_no(step.api_succeeded),
            step.need_reboot,
            step.error.as_deref().unwrap_or("none")
        ),
        None => println!("  {label}: not attempted"),
    }
}

fn print_usbdk_report(report: &UsbDkProbeReport) {
    println!();
    println!("UsbDk");
    match &report.helper {
        UsbDkHelperStatus::UnsupportedPlatform => println!("  helper:      unsupported platform"),
        UsbDkHelperStatus::Missing { expected_path } => {
            println!("  helper:      missing ({expected_path})")
        }
        UsbDkHelperStatus::LoadFailed { path, message } => {
            println!("  helper:      load failed ({path}: {message})")
        }
        UsbDkHelperStatus::Loaded { path } => println!("  helper:      loaded ({path})"),
    }
    match &report.api {
        UsbDkApiStatus::NotInspected => println!("  API:         not inspected"),
        UsbDkApiStatus::RequiredExportMissing { missing, .. } => {
            println!(
                "  API:         incompatible; missing {}",
                missing.join(", ")
            )
        }
        UsbDkApiStatus::Compatible {
            configuration_descriptor_exports,
            redirect_exports,
        } => println!(
            "  API:         compatible (config descriptors: {}; redirect exports: {}, not invoked)",
            yes_no(*configuration_descriptor_exports),
            yes_no(*redirect_exports)
        ),
    }
    match &report.service {
        UsbDkServiceStatus::UnsupportedPlatform => println!("  service:     unsupported platform"),
        UsbDkServiceStatus::Missing => println!("  service:     missing"),
        UsbDkServiceStatus::Stopped => println!("  service:     stopped"),
        UsbDkServiceStatus::Running => println!("  service:     running"),
        UsbDkServiceStatus::Other { raw_state } => {
            println!("  service:     state {raw_state}")
        }
        UsbDkServiceStatus::QueryFailed { message } => {
            println!("  service:     query failed ({message})")
        }
    }
    match &report.enumeration {
        UsbDkEnumerationStatus::NotAttempted => println!("  enumeration: not attempted"),
        UsbDkEnumerationStatus::DriverOrServiceUnavailable { message } => {
            println!("  enumeration: driver/service unavailable ({message})")
        }
        UsbDkEnumerationStatus::Failed { message } => {
            println!("  enumeration: failed ({message})")
        }
        UsbDkEnumerationStatus::InvalidUpstreamData { message } => {
            println!("  enumeration: invalid data ({message})")
        }
        UsbDkEnumerationStatus::Succeeded => println!("  enumeration: success"),
    }
    println!("  devices:     {}", report.devices.len());

    for device in &report.devices {
        println!(
            "    {:04X}:{:04X} class {:02X}/{:02X}/{:02X} speed {:?} filter {} port {}",
            device.vendor_id,
            device.product_id,
            device.device_class,
            device.device_subclass,
            device.device_protocol,
            device.speed,
            device.filter_id,
            device.port,
        );
        println!(
            "      DeviceID={} InstanceID={}",
            device.key.device_id, device.key.instance_id
        );
    }

    for correlation in &report.correlations {
        let summary = match &correlation.status {
            UsbDkControllerCorrelationStatus::NotEvaluated => "not evaluated".into(),
            UsbDkControllerCorrelationStatus::NoMatch { reason } => {
                format!("no match ({reason:?})")
            }
            UsbDkControllerCorrelationStatus::Unique { device } => format!(
                "unique (DeviceID={} InstanceID={})",
                device.device_id, device.instance_id
            ),
            UsbDkControllerCorrelationStatus::Ambiguous { reason, candidates } => {
                format!("ambiguous ({reason:?}, {} candidate(s))", candidates.len())
            }
        };
        println!("  controller {}: {summary}", correlation.controller_id);
    }
}
fn print_dedicated_winusb_report(report: &DedicatedWinUsbProbeReport) {
    println!();
    println!("Dedicated WinUSB");
    match &report.status {
        DedicatedWinUsbProbeStatus::UnsupportedPlatform => {
            println!("  discovery:  unsupported platform")
        }
        DedicatedWinUsbProbeStatus::EnumerationFailed { message } => {
            println!("  discovery:  failed ({message})")
        }
        DedicatedWinUsbProbeStatus::Succeeded => println!("  discovery:  success"),
    }
    println!("  candidates: {}", report.candidate_count());

    for controller in &report.controllers {
        if matches!(
            controller.status,
            DedicatedWinUsbReadinessStatus::NotWinUsbBound
        ) {
            continue;
        }

        println!("  controller {}", controller.controller_id);
        println!("    instance:  {}", controller.instance_id);
        println!(
            "    driver:    service={} INF={} provider={} version={}",
            controller.service.as_deref().unwrap_or("unknown"),
            controller.driver_inf.as_deref().unwrap_or("unknown"),
            controller.driver_provider.as_deref().unwrap_or("unknown"),
            controller.driver_version.as_deref().unwrap_or("unknown"),
        );
        if controller.registered_interface_guids.is_empty() {
            println!("    interface GUIDs: none");
        } else {
            println!(
                "    interface GUIDs: {}",
                controller.registered_interface_guids.join(", ")
            );
        }
        for application_interface in &controller.application_interfaces {
            println!(
                "    application interface: {} ({})",
                application_interface.path, application_interface.class_guid
            );
        }
        println!("    readiness: {:?}", controller.status);

        if let Some(interface) = controller.interface {
            println!(
                "    interface: number={} alt={} class={:02X}/{:02X}/{:02X} endpoints={}",
                interface.number,
                interface.alternate_setting,
                interface.class,
                interface.subclass,
                interface.protocol,
                interface.endpoint_count,
            );
        }
        for pipe in &controller.pipes {
            println!(
                "    pipe:       0x{:02X} {:?} {:?} max_packet={} interval={}",
                pipe.pipe_id,
                pipe.pipe_type,
                pipe.direction,
                pipe.maximum_packet_size,
                pipe.interval,
            );
        }
        if matches!(controller.status, DedicatedWinUsbReadinessStatus::Ready) {
            println!(
                "    HCI pipes:  event=0x{:02X} ACL_IN=0x{:02X} ACL_OUT=0x{:02X}",
                controller.event_pipe.unwrap_or_default(),
                controller.acl_in_pipe.unwrap_or_default(),
                controller.acl_out_pipe.unwrap_or_default(),
            );
        }
    }
}

fn yes_no(value: bool) -> &'static str {
    if value { "yes" } else { "no" }
}

fn description(controller: &ControllerObservation) -> &str {
    controller
        .bus_reported_description
        .as_deref()
        .or(controller.device_description.as_deref())
        .unwrap_or("unknown")
}

fn format_vid_pid(controller: &ControllerObservation) -> String {
    match (
        controller.identity.usb_vendor_id,
        controller.identity.usb_product_id,
    ) {
        (Some(vendor), Some(product)) => format!("{vendor:04X}:{product:04X}"),
        _ => "unknown".into(),
    }
}

fn print_optional(label: &str, value: Option<&str>) {
    println!("{label:<17}{}", value.unwrap_or("unknown"));
}

fn print_values(label: &str, values: &[String]) {
    if values.is_empty() {
        println!("{label:<17}none");
        return;
    }
    println!("{label}:");
    for value in values {
        println!("  {value}");
    }
}

fn print_help() {
    println!(
        "DirectHCI read-only controller diagnostics\n\n\
         Usage:\n  \
           directhci controllers [--json] [--direct]\n  \
           directhci status [--json]\n  \
           directhci controller show <id> [--json]\n  \
           directhci doctor [--json]\n  \
           directhci takeover plan <id> [--json]\n  \
           directhci takeover roundtrip <id> --execute [--json]\n  \
           directhci takeover hci-info <id> --execute [--json]\n  \
           directhci hci-info <id> --execute [--json]\n  \
           directhci recover --offline [--json]\n\n\
         Normal controller/HCI commands use directhcid. `--direct`, takeover, and\n  \
         offline recovery are explicit development/recovery paths."
    );
}
