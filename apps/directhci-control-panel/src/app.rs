use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use directhci_client::DirectHciClient;
use directhci_core::{
    ControllerObservation, RuntimeControllerState, RuntimePreferences, RuntimeStatus,
};
use native_windows_gui as nwg;

use crate::service::{self, ServiceState};

const REFRESH_INTERVAL: Duration = Duration::from_secs(2);

enum Action {
    Refresh,
    StartService,
    StopService,
    SetPreferred(String),
    RestoreWindows,
}

struct Snapshot {
    service: Result<ServiceState, String>,
    runtime: Option<RuntimeStatus>,
    preferences: Option<RuntimePreferences>,
    runtime_error: Option<String>,
    preferences_error: Option<String>,
}

struct WorkerResult {
    snapshot: Snapshot,
    operation_error: Option<String>,
    user_action: bool,
    stopping_service: bool,
}

struct Panel {
    window: nwg::Window,
    _layout: nwg::GridLayout,
    _title: nwg::Label,
    _subtitle: nwg::Label,
    _runtime_frame: nwg::Frame,
    _runtime_layout: nwg::GridLayout,
    _runtime_title: nwg::Label,
    _service_caption: nwg::Label,
    service_value: nwg::Label,
    _version_caption: nwg::Label,
    version_value: nwg::Label,
    start_button: nwg::Button,
    stop_button: nwg::Button,
    _controller_frame: nwg::Frame,
    _controller_layout: nwg::GridLayout,
    _controller_title: nwg::Label,
    _preferred_caption: nwg::Label,
    preferred_combo: nwg::ComboBox<String>,
    _state_caption: nwg::Label,
    state_value: nwg::Label,
    _usb_caption: nwg::Label,
    usb_value: nwg::Label,
    _driver_caption: nwg::Label,
    driver_value: nwg::Label,
    _active_frame: nwg::Frame,
    _active_layout: nwg::GridLayout,
    _active_title: nwg::Label,
    active_value: nwg::Label,
    _recovery_frame: nwg::Frame,
    _recovery_layout: nwg::GridLayout,
    _recovery_title: nwg::Label,
    recovery_value: nwg::Label,
    note_value: nwg::Label,
    refresh_button: nwg::Button,
    restore_button: nwg::Button,
    diagnostics_button: nwg::Button,
    notice: nwg::Notice,
    diagnostics_window: nwg::Window,
    _diagnostics_layout: nwg::GridLayout,
    diagnostics_text: nwg::TextBox,
    copy_button: nwg::Button,
    close_diagnostics_button: nwg::Button,
    requests: mpsc::Sender<Action>,
    results: RefCell<mpsc::Receiver<WorkerResult>>,
    snapshot: RefCell<Option<Snapshot>>,
    diagnostics_plain_text: RefCell<String>,
    busy: Cell<bool>,
    close_requested: Cell<bool>,
    updating_combo: Cell<bool>,
    _icon: nwg::Icon,
}

pub fn run() -> Result<(), String> {
    // NWG scales native controls using the system DPI; GridLayout continues to
    // fit controls when the user resizes either window.
    #[allow(deprecated)]
    unsafe {
        nwg::set_dpi_awareness()
    };
    nwg::init().map_err(|error| error.to_string())?;
    nwg::Font::set_global_family("Segoe UI").map_err(|error| error.to_string())?;
    let (requests, actions) = mpsc::channel();
    let (completed, results) = mpsc::channel();
    let panel = Rc::new(build_panel(requests, results)?);
    let notice = panel.notice.sender();
    thread::Builder::new()
        .name("directhci-control-panel-io".into())
        .spawn(move || {
            loop {
                let (action, user_action) = match actions.recv_timeout(REFRESH_INTERVAL) {
                    Ok(action) => (action, true),
                    Err(mpsc::RecvTimeoutError::Timeout) => (Action::Refresh, false),
                    Err(mpsc::RecvTimeoutError::Disconnected) => break,
                };
                let stopping_service = user_action && matches!(&action, Action::StopService);
                let operation_error = perform(action).err();
                let snapshot = collect_snapshot();
                if completed
                    .send(WorkerResult {
                        snapshot,
                        operation_error,
                        user_action,
                        stopping_service,
                    })
                    .is_err()
                {
                    break;
                }
                notice.notice();
            }
        })
        .map_err(|error| format!("start Control Panel worker: {error}"))?;

    let main_panel = Rc::clone(&panel);
    let main_handler = nwg::full_bind_event_handler(
        &panel.window.handle,
        move |event, data, handle| {
            use nwg::Event as E;
            if event == E::OnWindowClose && handle == main_panel.window.handle {
                match service::query() {
                    Ok(ServiceState::Stopped | ServiceState::NotInstalled) => {
                        nwg::stop_thread_dispatch();
                    }
                    Ok(
                        ServiceState::Running
                        | ServiceState::StartPending
                        | ServiceState::StopPending,
                    ) => {
                        if let nwg::EventData::OnWindowClose(close) = data {
                            close.close(false);
                        }
                        if !main_panel.close_requested.get()
                            && main_panel.confirm_active_client(
                                "Closing the Control Panel will stop DirectHCI, disconnect the active client, and restore Windows Bluetooth. Continue?",
                            )
                        {
                            main_panel.close_requested.set(true);
                            if !main_panel.busy.get() {
                                if main_panel.request(Action::StopService) {
                                    main_panel.note_value.set_text("Stopping service before closing...");
                                } else {
                                    main_panel.close_requested.set(false);
                                }
                            } else {
                                main_panel.note_value.set_text("Waiting for the current operation before stopping...");
                            }
                        }
                    }
                    Ok(ServiceState::Unknown) => {
                        if let nwg::EventData::OnWindowClose(close) = data {
                            close.close(false);
                        }
                        nwg::modal_error_message(
                            &main_panel.window,
                            "Cannot confirm service is stopped",
                            "DirectHCI service state is unknown; the panel will remain open.",
                        );
                    }
                    Err(error) => {
                        if let nwg::EventData::OnWindowClose(close) = data {
                            close.close(false);
                        }
                        nwg::modal_error_message(
                            &main_panel.window,
                            "Cannot confirm service is stopped",
                            &error,
                        );
                    }
                }
            } else if event == E::OnNotice && handle == main_panel.notice.handle {
                main_panel.apply_results();
            } else if event == E::OnButtonClick {
                if handle == main_panel.refresh_button.handle {
                    main_panel.request(Action::Refresh);
                } else if handle == main_panel.start_button.handle {
                    main_panel.request(Action::StartService);
                } else if handle == main_panel.stop_button.handle {
                    if main_panel.confirm_active_client("Stop DirectHCI service? The active client will be disconnected and Windows Bluetooth restored.") {
                        main_panel.request(Action::StopService);
                    }
                } else if handle == main_panel.restore_button.handle {
                    if main_panel.confirm_active_client(
                        "Restore Windows Bluetooth? The active client will be disconnected.",
                    ) {
                        main_panel.request(Action::RestoreWindows);
                    }
                } else if handle == main_panel.diagnostics_button.handle {
                    main_panel.show_diagnostics();
                }
            } else if event == E::OnComboxBoxSelection
                && handle == main_panel.preferred_combo.handle
                && !main_panel.updating_combo.get()
                && !main_panel.busy.get()
            {
                main_panel.select_preferred();
            }
        },
    );
    let dialog_panel = Rc::clone(&panel);
    let dialog_handler =
        nwg::full_bind_event_handler(&panel.diagnostics_window.handle, move |event, _, handle| {
            use nwg::Event as E;
            if event == E::OnWindowClose && handle == dialog_panel.diagnostics_window.handle {
                dialog_panel.diagnostics_window.set_visible(false);
            } else if event == E::OnButtonClick {
                if handle == dialog_panel.copy_button.handle {
                    nwg::Clipboard::set_data_text(
                        &dialog_panel.diagnostics_window,
                        &dialog_panel.diagnostics_plain_text.borrow(),
                    );
                } else if handle == dialog_panel.close_diagnostics_button.handle {
                    dialog_panel.diagnostics_window.set_visible(false);
                }
            }
        });

    panel.request(Action::Refresh);
    nwg::dispatch_thread_events();
    nwg::unbind_event_handler(&dialog_handler);
    nwg::unbind_event_handler(&main_handler);
    Ok(())
}

fn perform(action: Action) -> Result<(), String> {
    match action {
        Action::Refresh => Ok(()),
        Action::StartService => {
            service::start()?;
            if wait_for_service_attempts(ServiceState::Running, 150) {
                Ok(())
            } else {
                Err("DirectHCI service did not become running within 30 seconds".into())
            }
        }
        Action::StopService => stop_service_and_wait(),
        Action::SetPreferred(id) => DirectHciClient::connect(
            "directhci-control-panel",
            Some(env!("CARGO_PKG_VERSION").into()),
        )
        .map_err(|error| error.to_string())?
        .set_preferred_controller(&id)
        .map(|_| ())
        .map_err(|error| error.to_string()),
        Action::RestoreWindows => DirectHciClient::connect(
            "directhci-control-panel",
            Some(env!("CARGO_PKG_VERSION").into()),
        )
        .map_err(|error| error.to_string())?
        .restore_windows()
        .map_err(|error| error.to_string()),
    }
}

fn stop_service_and_wait() -> Result<(), String> {
    let mut stop_requested = false;
    for _ in 0..450 {
        match service::query()? {
            ServiceState::NotInstalled | ServiceState::Stopped => return Ok(()),
            ServiceState::Running if !stop_requested => {
                service::stop()?;
                stop_requested = true;
            }
            ServiceState::Running | ServiceState::StartPending | ServiceState::StopPending => {}
            ServiceState::Unknown => return Err("DirectHCI service state is unknown".into()),
        }
        thread::sleep(Duration::from_millis(200));
    }
    Err("DirectHCI service did not stop within 90 seconds".into())
}

fn wait_for_service_attempts(target: ServiceState, attempts: usize) -> bool {
    for _ in 0..attempts {
        if service::query().ok() == Some(target) {
            return true;
        }
        thread::sleep(Duration::from_millis(200));
    }
    false
}

fn collect_snapshot() -> Snapshot {
    let service = service::query();
    if matches!(
        &service,
        Ok(ServiceState::Stopped | ServiceState::NotInstalled)
    ) {
        return Snapshot {
            service,
            runtime: None,
            preferences: None,
            runtime_error: None,
            preferences_error: None,
        };
    }
    match DirectHciClient::connect(
        "directhci-control-panel",
        Some(env!("CARGO_PKG_VERSION").into()),
    ) {
        Ok(client) => {
            let status = client.runtime_status();
            let preferences = client.preferences();
            Snapshot {
                service,
                runtime: status.as_ref().ok().cloned(),
                preferences: preferences.as_ref().ok().cloned(),
                runtime_error: status.err().map(|error| error.to_string()),
                preferences_error: preferences.err().map(|error| error.to_string()),
            }
        }
        Err(error) => Snapshot {
            service,
            runtime: None,
            preferences: None,
            runtime_error: Some(error.to_string()),
            preferences_error: None,
        },
    }
}

impl Panel {
    fn request(&self, action: Action) -> bool {
        if self.busy.replace(true) {
            return false;
        }
        self.set_busy_controls();
        if self.requests.send(action).is_err() {
            self.busy.set(false);
            self.note_value.set_text("Control Panel worker stopped");
            self.set_busy_controls();
            return false;
        }
        true
    }

    fn apply_results(&self) {
        loop {
            // Drop the RefCell borrow before updating controls or showing a modal
            // dialog; those calls may dispatch another window message.
            let result = self.results.borrow_mut().try_recv();
            let Ok(result) = result else { break };
            let stopped = matches!(
                &result.snapshot.service,
                Ok(ServiceState::Stopped | ServiceState::NotInstalled)
            );
            let had_error = result.operation_error.is_some();
            if result.user_action {
                self.busy.set(false);
            }
            self.apply_snapshot(result.snapshot);
            if let Some(error) = result.operation_error {
                nwg::modal_error_message(&self.window, "DirectHCI operation failed", &error);
            }
            if self.close_requested.get() {
                if stopped && !self.busy.get() {
                    nwg::stop_thread_dispatch();
                    return;
                }
                if result.stopping_service {
                    self.close_requested.set(false);
                    if !had_error {
                        nwg::modal_error_message(
                            &self.window,
                            "Cannot close DirectHCI Control Panel",
                            "DirectHCI service is not stopped. Try Stop Service again.",
                        );
                    }
                } else if !self.busy.get() {
                    if self.request(Action::StopService) {
                        self.note_value
                            .set_text("Stopping service before closing...");
                    } else {
                        self.close_requested.set(false);
                    }
                }
            }
        }
    }

    fn apply_snapshot(&self, snapshot: Snapshot) {
        self.service_value.set_text(match &snapshot.service {
            Ok(state) => state.label(),
            Err(_) => "Unknown / Error",
        });
        self.version_value.set_text(
            snapshot
                .runtime
                .as_ref()
                .map_or("Unavailable", |runtime| &runtime.runtime_version),
        );
        self.updating_combo.set(true);
        if let Some(runtime) = &snapshot.runtime {
            let labels = runtime
                .controllers
                .iter()
                .map(|status| {
                    let controller = &status.controller;
                    format!(
                        "{}  ({})  {}",
                        controller_name(controller),
                        usb_id(controller),
                        controller.id
                    )
                })
                .collect();
            self.preferred_combo.set_collection(labels);
            let preferred = snapshot
                .preferences
                .as_ref()
                .and_then(|p| p.preferred_controller_id.as_ref());
            let selected = preferred.and_then(|id| {
                runtime
                    .controllers
                    .iter()
                    .position(|status| &status.controller.id == id)
            });
            self.preferred_combo.set_selection(selected);
            if let Some(index) = selected {
                let status = &runtime.controllers[index];
                self.state_value.set_text(ownership_label(status.state));
                self.usb_value.set_text(&usb_id(&status.controller));
                self.driver_value.set_text(&format!(
                    "{} / {}",
                    status.controller.service.as_deref().unwrap_or("-"),
                    status.controller.driver.inf_path.as_deref().unwrap_or("-")
                ));
            } else {
                self.state_value
                    .set_text(if runtime.controllers.is_empty() {
                        "Unavailable"
                    } else {
                        "Not selected"
                    });
                self.usb_value.set_text("-");
                self.driver_value.set_text("-");
            }
            self.active_value
                .set_text(&runtime.active_session.as_ref().map_or_else(
                    || "None".into(),
                    |active| format!("{}  ·  Session {}", active.client_name, active.session_id),
                ));
            self.recovery_value.set_text(if runtime.recovery_required {
                "Recovery Required"
            } else {
                "OK · No action required"
            });
            let note = if runtime.active_session.is_some() {
                "Controller is currently in use. Disconnect the active client before changing it."
                    .into()
            } else if runtime.controllers.is_empty() {
                "No compatible Bluetooth controller".into()
            } else if preferred.is_some() && selected.is_none() {
                "Preferred controller unavailable; no replacement selected.".into()
            } else if preferred.is_none() && runtime.controllers.len() > 1 {
                "Please select a controller".into()
            } else if let Some(error) = &snapshot.preferences_error {
                format!("Preferences unavailable: {error}")
            } else {
                "".into()
            };
            self.note_value.set_text(&note);
        } else {
            self.preferred_combo.set_collection(Vec::new());
            self.preferred_combo.set_selection(None);
            self.state_value.set_text("Unavailable");
            self.usb_value.set_text("-");
            self.driver_value.set_text("-");
            self.active_value.set_text("None");
            self.recovery_value.set_text("Unknown");
            self.note_value.set_text(match snapshot.service {
                Ok(ServiceState::NotInstalled) => "DirectHCI service is not installed.",
                Ok(ServiceState::Stopped) => {
                    "DirectHCI service is stopped. Start it to view controllers."
                }
                _ => "Runtime IPC connection unavailable; see Diagnostics.",
            });
        }
        self.updating_combo.set(false);
        *self.snapshot.borrow_mut() = Some(snapshot);
        self.set_busy_controls();
    }

    fn set_busy_controls(&self) {
        let busy = self.busy.get();
        let snapshot = self.snapshot.borrow();
        let service = snapshot
            .as_ref()
            .and_then(|value| value.service.as_ref().ok())
            .copied();
        self.start_button
            .set_enabled(!busy && service == Some(ServiceState::Stopped));
        self.stop_button
            .set_enabled(!busy && service == Some(ServiceState::Running));
        self.refresh_button.set_enabled(!busy);
        let runtime = snapshot.as_ref().and_then(|value| value.runtime.as_ref());
        self.restore_button.set_enabled(!busy && runtime.is_some());
        self.preferred_combo.set_enabled(
            !busy
                && runtime.is_some_and(|value| {
                    value.active_session.is_none() && !value.controllers.is_empty()
                }),
        );
    }

    fn select_preferred(&self) {
        let Some(index) = self.preferred_combo.selection() else {
            return;
        };
        let selected = self
            .snapshot
            .borrow()
            .as_ref()
            .and_then(|snapshot| snapshot.runtime.as_ref())
            .and_then(|runtime| runtime.controllers.get(index))
            .map(|status| status.controller.id.as_str().to_owned());
        if let Some(id) = selected {
            self.request(Action::SetPreferred(id));
        }
    }

    fn confirm_active_client(&self, question: &str) -> bool {
        if self
            .snapshot
            .borrow()
            .as_ref()
            .and_then(|snapshot| snapshot.runtime.as_ref())
            .and_then(|runtime| runtime.active_session.as_ref())
            .is_none()
        {
            return true;
        }
        nwg::modal_message(
            &self.window,
            &nwg::MessageParams {
                title: "DirectHCI active session",
                content: question,
                buttons: nwg::MessageButtons::YesNo,
                icons: nwg::MessageIcons::Warning,
            },
        ) == nwg::MessageChoice::Yes
    }

    fn show_diagnostics(&self) {
        let text = self
            .snapshot
            .borrow()
            .as_ref()
            .map_or_else(|| "Waiting for first refresh".into(), format_diagnostics);
        self.diagnostics_text.set_text_unix2dos(&text);
        *self.diagnostics_plain_text.borrow_mut() = text;
        self.diagnostics_window.set_visible(true);
    }
}

fn controller_name(controller: &ControllerObservation) -> &str {
    controller
        .bus_reported_description
        .as_deref()
        .or(controller.device_description.as_deref())
        .unwrap_or("USB Bluetooth Controller")
}

fn usb_id(controller: &ControllerObservation) -> String {
    match (
        controller.identity.usb_vendor_id,
        controller.identity.usb_product_id,
    ) {
        (Some(vid), Some(pid)) => format!("{vid:04X}:{pid:04X}"),
        _ => "unknown USB ID".into(),
    }
}

fn ownership_label(state: RuntimeControllerState) -> &'static str {
    match state {
        RuntimeControllerState::WindowsOwned => "Windows Owned",
        RuntimeControllerState::Acquiring => "Acquiring",
        RuntimeControllerState::DirectHciOwned => "DirectHCI Owned",
        RuntimeControllerState::Restoring => "Restoring",
        RuntimeControllerState::RecoveryRequired => "Recovery Required",
    }
}

fn format_diagnostics(snapshot: &Snapshot) -> String {
    let mut lines = vec![
        format!("DirectHCI Control Panel {}", env!("CARGO_PKG_VERSION")),
        format!(
            "Service: {}",
            snapshot
                .service
                .as_ref()
                .map_or("Unknown / Error", |state| state.label())
        ),
        format!(
            "Runtime: {}",
            snapshot
                .runtime
                .as_ref()
                .map_or("Unavailable", |status| &status.runtime_version)
        ),
        format!(
            "Preferred ControllerId: {}",
            snapshot
                .preferences
                .as_ref()
                .and_then(|p| p.preferred_controller_id.as_ref())
                .map_or("None", |id| id.as_str())
        ),
    ];
    if let Err(error) = &snapshot.service {
        lines.push(format!("Service error: {error}"));
    }
    if let Some(error) = &snapshot.runtime_error {
        lines.push(format!("Runtime error: {error}"));
    }
    if let Some(error) = &snapshot.preferences_error {
        lines.push(format!("Preferences error: {error}"));
    }
    if let Some(runtime) = &snapshot.runtime {
        lines.push(format!("Recovery required: {}", runtime.recovery_required));
        if let Some(message) = &runtime.recovery_message {
            lines.push(format!("Recovery detail: {message}"));
        }
        lines.push(format!(
            "Active session: {}",
            runtime.active_session.as_ref().map_or_else(
                || "None".into(),
                |active| format!(
                    "{} / {} / Session {}",
                    active.client_name, active.controller_id, active.session_id
                )
            )
        ));
        for status in &runtime.controllers {
            let controller = &status.controller;
            lines.extend([
                String::new(),
                format!("Controller: {}", controller.id),
                format!("Description: {}", controller_name(controller)),
                format!("USB: {}", usb_id(controller)),
                format!("PnP Instance ID: {}", controller.identity.instance_id),
                format!("Service: {}", controller.service.as_deref().unwrap_or("-")),
                format!(
                    "INF: {}",
                    controller.driver.inf_path.as_deref().unwrap_or("-")
                ),
                format!(
                    "Provider: {}",
                    controller.driver.provider.as_deref().unwrap_or("-")
                ),
                format!(
                    "Driver version: {}",
                    controller.driver.version.as_deref().unwrap_or("-")
                ),
                format!("Ownership: {}", ownership_label(status.state)),
            ]);
        }
    }
    lines.join("\n")
}

fn build_label(parent: impl Into<nwg::ControlHandle>, text: &str) -> Result<nwg::Label, String> {
    let mut label = nwg::Label::default();
    nwg::Label::builder()
        .text(text)
        .parent(parent)
        .build(&mut label)
        .map_err(|error| error.to_string())?;
    Ok(label)
}

fn build_button(parent: impl Into<nwg::ControlHandle>, text: &str) -> Result<nwg::Button, String> {
    let mut button = nwg::Button::default();
    nwg::Button::builder()
        .text(text)
        .parent(parent)
        .build(&mut button)
        .map_err(|error| error.to_string())?;
    Ok(button)
}

fn item<C: Into<nwg::ControlHandle>>(
    control: C,
    column: u32,
    row: u32,
    columns: u32,
    rows: u32,
) -> nwg::GridLayoutItem {
    nwg::GridLayoutItem::new(control, column, row, columns, rows)
}

fn build_section(window: &nwg::Window) -> Result<nwg::Frame, String> {
    let mut frame = nwg::Frame::default();
    nwg::Frame::builder()
        .parent(window)
        .flags(nwg::FrameFlags::VISIBLE | nwg::FrameFlags::BORDER)
        .build(&mut frame)
        .map_err(|error| error.to_string())?;
    Ok(frame)
}

fn build_panel(
    requests: mpsc::Sender<Action>,
    results: mpsc::Receiver<WorkerResult>,
) -> Result<Panel, String> {
    let scale = nwg::scale_factor();
    let resources = nwg::EmbedResource::load(None).map_err(|error| error.to_string())?;
    let icon = resources
        .icon(1, None)
        .ok_or("DirectHCI icon resource is missing")?;
    let mut window = nwg::Window::default();
    nwg::Window::builder()
        .title("DirectHCI Control Panel")
        .size(((800.0 * scale) as i32, (610.0 * scale) as i32))
        .flags(nwg::WindowFlags::MAIN_WINDOW | nwg::WindowFlags::VISIBLE)
        .icon(Some(&icon))
        .build(&mut window)
        .map_err(|error| error.to_string())?;

    let title = build_label(&window, "DirectHCI")?;
    let subtitle = build_label(&window, "Windows Bluetooth HCI compatibility layer")?;
    let runtime_frame = build_section(&window)?;
    let runtime_title = build_label(&runtime_frame, "Runtime")?;
    let service_caption = build_label(&runtime_frame, "Service")?;
    let service_value = build_label(&runtime_frame, "Checking…")?;
    let version_caption = build_label(&runtime_frame, "Version")?;
    let version_value = build_label(&runtime_frame, "-")?;
    let start_button = build_button(&runtime_frame, "Start Service")?;
    let stop_button = build_button(&runtime_frame, "Stop Service")?;
    let controller_frame = build_section(&window)?;
    let controller_title = build_label(&controller_frame, "Bluetooth Controller")?;
    let preferred_caption = build_label(&controller_frame, "Preferred")?;
    let mut preferred_combo = nwg::ComboBox::<String>::default();
    nwg::ComboBox::builder()
        .parent(&controller_frame)
        .build(&mut preferred_combo)
        .map_err(|error| error.to_string())?;
    let state_caption = build_label(&controller_frame, "State")?;
    let state_value = build_label(&controller_frame, "Unavailable")?;
    let usb_caption = build_label(&controller_frame, "USB")?;
    let usb_value = build_label(&controller_frame, "-")?;
    let driver_caption = build_label(&controller_frame, "Driver")?;
    let driver_value = build_label(&controller_frame, "-")?;
    let active_frame = build_section(&window)?;
    let active_title = build_label(&active_frame, "Active Client")?;
    let active_value = build_label(&active_frame, "None")?;
    let recovery_frame = build_section(&window)?;
    let recovery_title = build_label(&recovery_frame, "Recovery")?;
    let recovery_value = build_label(&recovery_frame, "Unknown")?;
    let note_value = build_label(&window, "")?;
    let refresh_button = build_button(&window, "Refresh")?;
    let restore_button = build_button(&window, "Restore Windows")?;
    let diagnostics_button = build_button(&window, "Diagnostics")?;
    let runtime_layout = nwg::GridLayout::default();
    nwg::GridLayout::builder()
        .parent(&runtime_frame)
        .max_column(Some(4))
        .max_row(Some(3))
        .margin([10, 8, 10, 8])
        .spacing(5)
        .child_item(item(&runtime_title, 0, 0, 4, 1))
        .child(0, 1, &service_caption)
        .child(1, 1, &service_value)
        .child(2, 1, &version_caption)
        .child(3, 1, &version_value)
        .child(2, 2, &start_button)
        .child(3, 2, &stop_button)
        .build(&runtime_layout)
        .map_err(|error| error.to_string())?;
    let controller_layout = nwg::GridLayout::default();
    nwg::GridLayout::builder()
        .parent(&controller_frame)
        .max_column(Some(4))
        .max_row(Some(4))
        .margin([10, 8, 10, 8])
        .spacing(5)
        .child_item(item(&controller_title, 0, 0, 4, 1))
        .child(0, 1, &preferred_caption)
        .child_item(item(&preferred_combo, 1, 1, 3, 1))
        .child(0, 2, &state_caption)
        .child(1, 2, &state_value)
        .child(2, 2, &usb_caption)
        .child(3, 2, &usb_value)
        .child(0, 3, &driver_caption)
        .child_item(item(&driver_value, 1, 3, 3, 1))
        .build(&controller_layout)
        .map_err(|error| error.to_string())?;
    let active_layout = nwg::GridLayout::default();
    nwg::GridLayout::builder()
        .parent(&active_frame)
        .max_column(Some(1))
        .max_row(Some(2))
        .margin([10, 8, 10, 8])
        .spacing(5)
        .child(0, 0, &active_title)
        .child(0, 1, &active_value)
        .build(&active_layout)
        .map_err(|error| error.to_string())?;
    let recovery_layout = nwg::GridLayout::default();
    nwg::GridLayout::builder()
        .parent(&recovery_frame)
        .max_column(Some(1))
        .max_row(Some(2))
        .margin([10, 8, 10, 8])
        .spacing(5)
        .child(0, 0, &recovery_title)
        .child(0, 1, &recovery_value)
        .build(&recovery_layout)
        .map_err(|error| error.to_string())?;
    let layout = nwg::GridLayout::default();
    nwg::GridLayout::builder()
        .parent(&window)
        .max_column(Some(4))
        .max_row(Some(14))
        .margin([10, 10, 10, 10])
        .spacing(5)
        .child_item(item(&title, 0, 0, 4, 1))
        .child_item(item(&subtitle, 0, 1, 4, 1))
        .child_item(item(&runtime_frame, 0, 2, 4, 3))
        .child_item(item(&controller_frame, 0, 5, 4, 4))
        .child_item(item(&active_frame, 0, 9, 2, 3))
        .child_item(item(&recovery_frame, 2, 9, 2, 3))
        .child_item(item(&note_value, 0, 12, 4, 1))
        .child(0, 13, &refresh_button)
        .child_item(item(&restore_button, 1, 13, 2, 1))
        .child(3, 13, &diagnostics_button)
        .build(&layout)
        .map_err(|error| error.to_string())?;
    let mut notice = nwg::Notice::default();
    nwg::Notice::builder()
        .parent(&window)
        .build(&mut notice)
        .map_err(|error| error.to_string())?;

    let mut diagnostics_window = nwg::Window::default();
    nwg::Window::builder()
        .title("DirectHCI Diagnostics")
        .size(((760.0 * scale) as i32, (520.0 * scale) as i32))
        .flags(nwg::WindowFlags::WINDOW | nwg::WindowFlags::RESIZABLE)
        .icon(Some(&icon))
        .build(&mut diagnostics_window)
        .map_err(|error| error.to_string())?;
    let mut diagnostics_text = nwg::TextBox::default();
    nwg::TextBox::builder()
        .readonly(true)
        .parent(&diagnostics_window)
        .build(&mut diagnostics_text)
        .map_err(|error| error.to_string())?;
    let copy_button = build_button(&diagnostics_window, "Copy")?;
    let close_diagnostics_button = build_button(&diagnostics_window, "Close")?;
    let diagnostics_layout = nwg::GridLayout::default();
    nwg::GridLayout::builder()
        .parent(&diagnostics_window)
        .max_column(Some(2))
        .max_row(Some(12))
        .margin([8, 8, 8, 8])
        .spacing(3)
        .child_item(item(&diagnostics_text, 0, 0, 2, 11))
        .child(0, 11, &copy_button)
        .child(1, 11, &close_diagnostics_button)
        .build(&diagnostics_layout)
        .map_err(|error| error.to_string())?;

    let panel = Panel {
        _icon: icon,
        window,
        _layout: layout,
        _title: title,
        _subtitle: subtitle,
        _runtime_frame: runtime_frame,
        _runtime_layout: runtime_layout,
        _runtime_title: runtime_title,
        _service_caption: service_caption,
        service_value,
        _version_caption: version_caption,
        version_value,
        start_button,
        stop_button,
        _controller_frame: controller_frame,
        _controller_layout: controller_layout,
        _controller_title: controller_title,
        _preferred_caption: preferred_caption,
        preferred_combo,
        _state_caption: state_caption,
        state_value,
        _usb_caption: usb_caption,
        usb_value,
        _driver_caption: driver_caption,
        driver_value,
        _active_frame: active_frame,
        _active_layout: active_layout,
        _active_title: active_title,
        active_value,
        _recovery_frame: recovery_frame,
        _recovery_layout: recovery_layout,
        _recovery_title: recovery_title,
        recovery_value,
        note_value,
        refresh_button,
        restore_button,
        diagnostics_button,
        notice,
        diagnostics_window,
        _diagnostics_layout: diagnostics_layout,
        diagnostics_text,
        copy_button,
        close_diagnostics_button,
        requests,
        results: RefCell::new(results),
        snapshot: RefCell::new(None),
        diagnostics_plain_text: RefCell::new(String::new()),
        busy: Cell::new(false),
        close_requested: Cell::new(false),
        updating_combo: Cell::new(false),
    };
    panel.set_busy_controls();
    Ok(panel)
}
