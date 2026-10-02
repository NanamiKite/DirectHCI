use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak, mpsc};
use std::thread;
use std::time::Duration;

use directhci_core::{
    ActiveSessionInfo, ControlResponse, ControllerObservation, IpcErrorCode, IpcFrame,
    IpcMessageKind, IpcTransportInfo, RuntimeControllerState, RuntimeControllerStatus,
    RuntimePreferences, RuntimeStatus, encode_acl_packet, encode_hci_command_response,
    encode_hci_event,
};
use directhci_windows::{
    OfflineRecoveryStatus, PreferencesStore, RoundTripStatus, RuntimeControllerSession,
    acquire_runtime_controller_session,
};

const CONTROLLER_REQUEST_DEPTH: usize = 32;
const RX_POLL: Duration = Duration::from_millis(10);
const RX_BATCH: usize = 64;
const SESSION_STOP_TIMEOUT: Duration = Duration::from_secs(45);

pub type Outbound = mpsc::SyncSender<IpcFrame>;
pub type DisconnectOwner = Arc<dyn Fn() + Send + Sync>;

enum ControllerRequest {
    Command {
        request_id: u32,
        opcode: u16,
        parameters: Vec<u8>,
    },
    Acl {
        request_id: u32,
        packet: directhci_core::HciAclPacket,
    },
    Release {
        request_id: u32,
    },
    Stop,
}

struct ActiveSession {
    info: ActiveSessionInfo,
    owner_connection: u64,
    phase: RuntimeControllerState,
    requests: mpsc::SyncSender<ControllerRequest>,
    cancel: Arc<AtomicBool>,
}

struct State {
    active: Option<ActiveSession>,
    maintenance: bool,
    preferences: Result<RuntimePreferences, String>,
    recovery_required: bool,
    recovery_message: Option<String>,
}

pub struct DirectHciRuntime {
    state: Mutex<State>,
    preferences_store: Result<PreferencesStore, String>,
    next_session: AtomicU64,
    shutting_down: AtomicBool,
}

impl DirectHciRuntime {
    pub fn start() -> Arc<Self> {
        let directory_repair = directhci_windows::repair_program_data_directory();
        if let Err(error) = &directory_repair {
            eprintln!("directhcid: state directory is unsafe: {error}");
        }
        let preferences_store = PreferencesStore::program_data();
        let mut preferences = directory_repair.and_then(|_| {
            preferences_store
                .as_ref()
                .map_err(Clone::clone)
                .and_then(PreferencesStore::load)
        });
        let recovery = directhci_windows::recover_offline();
        // An empty preference may be initialized once during privileged daemon
        // startup. A read-only IPC request must never persist configuration.
        if matches!(&preferences, Ok(value) if value.preferred_controller_id.is_none()) {
            if let Ok(controllers) = directhci_windows::enumerate_controllers() {
                if controllers.len() == 1 {
                    let selected = RuntimePreferences {
                        preferred_controller_id: Some(controllers[0].id.clone()),
                    };
                    if let Ok(store) = &preferences_store {
                        preferences = store.save(&selected).map(|()| selected);
                    }
                }
            }
        }
        let recovery_required = matches!(
            recovery.status,
            OfflineRecoveryStatus::DeviceMissing
                | OfflineRecoveryStatus::Refused
                | OfflineRecoveryStatus::RecoveryRequired
        );
        let recovery_message = recovery_required.then(|| {
            recovery
                .error
                .unwrap_or_else(|| format!("startup recovery ended in {:?}", recovery.status))
        });
        if !recovery_required && !matches!(recovery.status, OfflineRecoveryStatus::NoJournal) {
            eprintln!(
                "directhcid: startup ownership recovery completed: {:?}",
                recovery.status
            );
        }
        Arc::new(Self {
            state: Mutex::new(State {
                active: None,
                maintenance: false,
                preferences,
                recovery_required,
                recovery_message,
            }),
            preferences_store,
            next_session: AtomicU64::new(1),
            shutting_down: AtomicBool::new(false),
        })
    }

    pub fn list_controllers(&self) -> Result<Vec<ControllerObservation>, String> {
        directhci_windows::enumerate_controllers().map_err(|error| error.to_string())
    }

    pub fn owns_connection(&self, connection_id: u64) -> bool {
        self.state.lock().ok().is_some_and(|state| {
            state
                .active
                .as_ref()
                .is_some_and(|active| active.owner_connection == connection_id)
        })
    }

    pub fn preferences(&self) -> Result<RuntimePreferences, String> {
        let state = self
            .state
            .lock()
            .map_err(|_| "runtime state lock poisoned")?;
        state.preferences.clone()
    }

    pub fn set_preferred_controller(
        &self,
        controller_id: &str,
    ) -> Result<RuntimePreferences, (IpcErrorCode, String)> {
        let controllers = self
            .list_controllers()
            .map_err(|error| (IpcErrorCode::Runtime, error))?;
        let mut matching = controllers
            .iter()
            .filter(|controller| controller.id.as_str().eq_ignore_ascii_case(controller_id));
        let controller = matching.next().ok_or_else(|| {
            (
                IpcErrorCode::ControllerNotFound,
                "preferred controller is not present".into(),
            )
        })?;
        if matching.next().is_some() {
            return Err((
                IpcErrorCode::ControllerBusy,
                "controller identity is ambiguous".into(),
            ));
        }
        let selected = RuntimePreferences {
            preferred_controller_id: Some(controller.id.clone()),
        };
        let mut state = self.state.lock().map_err(|_| runtime_lock_error())?;
        if state.active.is_some() || state.maintenance {
            return Err((
                IpcErrorCode::ControllerBusy,
                "controller is currently in use".into(),
            ));
        }
        state
            .preferences
            .as_ref()
            .map_err(|error| (IpcErrorCode::Runtime, error.clone()))?;
        self.preferences_store
            .as_ref()
            .map_err(|error| (IpcErrorCode::Runtime, error.clone()))?
            .save(&selected)
            .map_err(|error| (IpcErrorCode::Runtime, error))?;
        state.preferences = Ok(selected.clone());
        Ok(selected)
    }

    pub fn controller_preparation_status(
        &self,
        controller_id: &str,
    ) -> Result<directhci_core::ControllerPreparationStatus, (IpcErrorCode, String)> {
        let status = directhci_windows::controller_preparation_status(controller_id)
            .map_err(|error| (IpcErrorCode::Provisioning, error))?;
        Ok(directhci_core::ControllerPreparationStatus {
            hardware_id: status.hardware_id,
            ready: status.ready,
            takeover_safe: status.takeover_safe,
            blockers: status.blockers,
        })
    }

    pub fn prepare_controller(
        &self,
        controller_id: &str,
    ) -> Result<directhci_core::PreparedController, (IpcErrorCode, String)> {
        if self.shutting_down.load(Ordering::Acquire) {
            return Err((IpcErrorCode::Runtime, "runtime is shutting down".into()));
        }
        {
            let mut state = self.state.lock().map_err(|_| runtime_lock_error())?;
            if state.recovery_required {
                return Err((
                    IpcErrorCode::RecoveryRequired,
                    state
                        .recovery_message
                        .clone()
                        .unwrap_or_else(|| "offline recovery is required".into()),
                ));
            }
            if state.maintenance || state.active.is_some() {
                return Err((
                    IpcErrorCode::ControllerBusy,
                    "controller is currently in use".into(),
                ));
            }
            state.maintenance = true;
        }
        let result = directhci_windows::prepare_controller(controller_id);
        self.state
            .lock()
            .map_err(|_| runtime_lock_error())?
            .maintenance = false;
        result
            .map(|result| directhci_core::PreparedController {
                status: directhci_core::ControllerPreparationStatus {
                    hardware_id: result.status.hardware_id,
                    ready: result.status.ready,
                    takeover_safe: result.status.takeover_safe,
                    blockers: result.status.blockers,
                },
                already_prepared: result.already_prepared,
                staged_inf: result.staged_inf,
            })
            .map_err(|error| (IpcErrorCode::Provisioning, error))
    }

    pub fn restore_windows(&self) -> Result<(), (IpcErrorCode, String)> {
        let active = {
            let mut state = self.state.lock().map_err(|_| runtime_lock_error())?;
            if state.maintenance {
                return Err((
                    IpcErrorCode::ControllerBusy,
                    "recovery is already running".into(),
                ));
            }
            state.maintenance = true;
            state.active.as_mut().map(|active| {
                active.phase = RuntimeControllerState::Restoring;
                active.cancel.store(true, Ordering::Release);
                (active.requests.clone(), active.info.session_id)
            })
        };
        if let Some((sender, session_id)) = active {
            let _ = sender.try_send(ControllerRequest::Stop);
            let deadline = std::time::Instant::now() + SESSION_STOP_TIMEOUT;
            while std::time::Instant::now() < deadline {
                if self
                    .state
                    .lock()
                    .map_err(|_| runtime_lock_error())?
                    .active
                    .as_ref()
                    .is_none_or(|active| active.info.session_id != session_id)
                {
                    break;
                }
                thread::sleep(Duration::from_millis(50));
            }
            if self
                .state
                .lock()
                .map_err(|_| runtime_lock_error())?
                .active
                .is_some()
            {
                let mut state = self.state.lock().map_err(|_| runtime_lock_error())?;
                state.recovery_required = true;
                state.recovery_message =
                    Some("active session did not stop within the recovery deadline".into());
                state.maintenance = false;
                return Err((
                    IpcErrorCode::RecoveryRequired,
                    state.recovery_message.clone().unwrap(),
                ));
            }
        }
        // The active session has finished above. Offline recovery deliberately
        // rejects this service's still-live PID, so use the narrowly scoped
        // same-process retry without changing the offline safety policy.
        let report = directhci_windows::recover_owning_process_after_session_closed();
        let success = matches!(
            report.status,
            OfflineRecoveryStatus::NoJournal
                | OfflineRecoveryStatus::AlreadyWindowsOwned
                | OfflineRecoveryStatus::RestoredWindows
        );
        let mut state = self.state.lock().map_err(|_| runtime_lock_error())?;
        state.maintenance = false;
        state.recovery_required = !success;
        state.recovery_message = (!success).then(|| {
            report
                .error
                .unwrap_or_else(|| format!("Windows recovery ended in {:?}", report.status))
        });
        if success {
            Ok(())
        } else {
            Err((
                IpcErrorCode::RecoveryRequired,
                state.recovery_message.clone().unwrap(),
            ))
        }
    }

    pub fn status(&self) -> Result<RuntimeStatus, String> {
        let controllers = self.list_controllers()?;
        let state = self
            .state
            .lock()
            .map_err(|_| "runtime state lock poisoned")?;
        let controllers = controllers
            .into_iter()
            .map(|controller| {
                let controller_state = if state.recovery_required {
                    RuntimeControllerState::RecoveryRequired
                } else if let Some(active) = &state.active {
                    if active.info.controller_id == controller.id {
                        active.phase
                    } else {
                        RuntimeControllerState::WindowsOwned
                    }
                } else {
                    RuntimeControllerState::WindowsOwned
                };
                RuntimeControllerStatus {
                    controller,
                    state: controller_state,
                }
            })
            .collect();
        Ok(RuntimeStatus {
            runtime_version: env!("CARGO_PKG_VERSION").into(),
            recovery_required: state.recovery_required,
            recovery_message: state.recovery_message.clone(),
            controllers,
            active_session: state.active.as_ref().map(|active| active.info.clone()),
        })
    }

    pub fn acquire(
        self: &Arc<Self>,
        connection_id: u64,
        client_name: String,
        controller_id: &str,
        outbound: Outbound,
        disconnect_owner: DisconnectOwner,
    ) -> Result<ControlResponse, (IpcErrorCode, String)> {
        if self.shutting_down.load(Ordering::Acquire) {
            return Err((IpcErrorCode::Runtime, "runtime is shutting down".into()));
        }
        let (requests, receiver) = mpsc::sync_channel(CONTROLLER_REQUEST_DEPTH);
        let cancel = Arc::new(AtomicBool::new(false));
        let session_id = self.next_session.fetch_add(1, Ordering::Relaxed);
        {
            let mut state = self.state.lock().map_err(|_| runtime_lock_error())?;
            if state.recovery_required {
                return Err((
                    IpcErrorCode::RecoveryRequired,
                    state
                        .recovery_message
                        .clone()
                        .unwrap_or_else(|| "offline recovery is required".into()),
                ));
            }
            if state.maintenance {
                return Err((
                    IpcErrorCode::ControllerBusy,
                    "Windows recovery is in progress".into(),
                ));
            }
            if state.active.is_some() {
                return Err((
                    IpcErrorCode::ControllerBusy,
                    "a controller session is already active".into(),
                ));
            }
            let controller = self
                .list_controllers()
                .map_err(|error| (IpcErrorCode::Runtime, error))?
                .into_iter()
                .find(|value| value.id.as_str().eq_ignore_ascii_case(controller_id))
                .ok_or_else(|| {
                    (
                        IpcErrorCode::ControllerNotFound,
                        format!("controller `{controller_id}` was not found"),
                    )
                })?;
            state.active = Some(ActiveSession {
                info: ActiveSessionInfo {
                    session_id,
                    controller_id: controller.id,
                    client_name: client_name.clone(),
                },
                owner_connection: connection_id,
                phase: RuntimeControllerState::Acquiring,
                requests: requests.clone(),
                cancel: Arc::clone(&cancel),
            });
        }

        eprintln!("directhcid: client `{client_name}` acquiring {controller_id}");
        let live = match acquire_runtime_controller_session(controller_id, &client_name) {
            Ok(live) => live,
            Err(report) => {
                self.finish_failed_acquire(
                    connection_id,
                    report.status,
                    report.recovery_error.or(report.primary_error),
                );
                return Err((
                    IpcErrorCode::Ownership,
                    "temporary takeover failed; inspect runtime status and M1 journal".into(),
                ));
            }
        };
        let controller = live.controller().clone();
        let transport = live.raw_hci().transport();
        let transport = IpcTransportInfo {
            interface_number: transport.interface_number,
            alternate_setting: transport.alternate_setting,
            event_pipe: transport.event_pipe,
            acl_in_pipe: transport.acl_in_pipe,
            acl_out_pipe: transport.acl_out_pipe,
        };
        if let Ok(mut state) = self.state.lock() {
            if let Some(active) = &mut state.active {
                active.phase = RuntimeControllerState::DirectHciOwned;
            }
        }
        let runtime = Arc::downgrade(self);
        if let Err(error) = thread::Builder::new()
            .name(format!("directhci-controller-{session_id}"))
            .spawn(move || {
                controller_worker(
                    runtime,
                    session_id,
                    live,
                    receiver,
                    outbound,
                    cancel,
                    disconnect_owner,
                )
            })
        {
            self.worker_finished(
                session_id,
                false,
                Some(format!("start controller worker: {error}")),
            );
            return Err((
                IpcErrorCode::Runtime,
                format!("start controller worker: {error}"),
            ));
        }
        eprintln!("directhcid: session {session_id} owns {controller_id}");
        Ok(ControlResponse::SessionReady {
            session_id,
            controller,
            transport,
        })
    }

    pub fn send_command(
        &self,
        connection_id: u64,
        session_id: u64,
        request_id: u32,
        opcode: u16,
        parameters: Vec<u8>,
    ) -> Result<(), (IpcErrorCode, String)> {
        self.session_sender(connection_id, session_id)?
            .try_send(ControllerRequest::Command {
                request_id,
                opcode,
                parameters,
            })
            .map_err(map_request_send)
    }

    pub fn send_acl(
        &self,
        connection_id: u64,
        session_id: u64,
        request_id: u32,
        packet: directhci_core::HciAclPacket,
    ) -> Result<(), (IpcErrorCode, String)> {
        self.session_sender(connection_id, session_id)?
            .try_send(ControllerRequest::Acl { request_id, packet })
            .map_err(map_request_send)
    }

    pub fn release(
        &self,
        connection_id: u64,
        session_id: u64,
        request_id: u32,
    ) -> Result<(), (IpcErrorCode, String)> {
        let sender = self.session_sender(connection_id, session_id)?;
        self.mark_restoring(session_id);
        sender
            .try_send(ControllerRequest::Release { request_id })
            .map_err(map_request_send)
    }

    pub fn disconnect(&self, connection_id: u64) {
        let sender = self.state.lock().ok().and_then(|mut state| {
            let active = state.active.as_mut()?;
            if active.owner_connection != connection_id {
                return None;
            }
            active.phase = RuntimeControllerState::Restoring;
            active.cancel.store(true, Ordering::Release);
            Some(active.requests.clone())
        });
        if let Some(sender) = sender {
            eprintln!("directhcid: owning client disconnected; restoring controller");
            let _ = sender.try_send(ControllerRequest::Stop);
        }
    }

    pub fn shutdown(&self) {
        self.shutting_down.store(true, Ordering::Release);
        let active = self.state.lock().ok().and_then(|mut state| {
            let active = state.active.as_mut()?;
            active.phase = RuntimeControllerState::Restoring;
            active.cancel.store(true, Ordering::Release);
            Some((active.requests.clone(), active.info.session_id))
        });
        let Some((sender, session_id)) = active else {
            return;
        };
        let _ = sender.try_send(ControllerRequest::Stop);
        let deadline = std::time::Instant::now() + SESSION_STOP_TIMEOUT;
        while std::time::Instant::now() < deadline {
            let active = self
                .state
                .lock()
                .ok()
                .and_then(|state| state.active.as_ref().map(|value| value.info.session_id));
            if active != Some(session_id) {
                return;
            }
            thread::sleep(Duration::from_millis(50));
        }
        eprintln!(
            "directhcid: shutdown recovery exceeded 45 seconds; durable M1 journal is retained"
        );
    }

    fn session_sender(
        &self,
        connection_id: u64,
        session_id: u64,
    ) -> Result<mpsc::SyncSender<ControllerRequest>, (IpcErrorCode, String)> {
        let state = self.state.lock().map_err(|_| runtime_lock_error())?;
        let active = state
            .active
            .as_ref()
            .ok_or_else(|| (IpcErrorCode::InvalidSession, "no active session".into()))?;
        if active.owner_connection != connection_id || active.info.session_id != session_id {
            return Err((
                IpcErrorCode::InvalidSession,
                "session is not owned by this connection".into(),
            ));
        }
        Ok(active.requests.clone())
    }

    fn mark_restoring(&self, session_id: u64) {
        if let Ok(mut state) = self.state.lock() {
            if let Some(active) = &mut state.active {
                if active.info.session_id == session_id {
                    active.phase = RuntimeControllerState::Restoring;
                }
            }
        }
    }

    fn finish_failed_acquire(
        &self,
        connection_id: u64,
        status: RoundTripStatus,
        message: Option<String>,
    ) {
        if let Ok(mut state) = self.state.lock() {
            if state
                .active
                .as_ref()
                .is_some_and(|active| active.owner_connection == connection_id)
            {
                state.active = None;
            }
            if matches!(status, RoundTripStatus::RecoveryRequired) {
                state.recovery_required = true;
                state.recovery_message = message;
            }
        }
    }

    fn worker_finished(&self, session_id: u64, recovery_required: bool, message: Option<String>) {
        if let Ok(mut state) = self.state.lock() {
            if state
                .active
                .as_ref()
                .is_some_and(|active| active.info.session_id == session_id)
            {
                state.active = None;
            }
            if recovery_required {
                state.recovery_required = true;
                state.recovery_message = message;
            }
        }
    }
}

fn controller_worker(
    runtime: Weak<DirectHciRuntime>,
    session_id: u64,
    live: RuntimeControllerSession,
    receiver: mpsc::Receiver<ControllerRequest>,
    outbound: Outbound,
    cancel: Arc<AtomicBool>,
    disconnect_owner: DisconnectOwner,
) {
    let live = live;
    let mut release_request = None;
    let mut fatal = None;
    'session: loop {
        if cancel.load(Ordering::Acquire) {
            break;
        }
        match receiver.recv_timeout(RX_POLL) {
            Ok(ControllerRequest::Command {
                request_id,
                opcode,
                parameters,
            }) => match live.raw_hci().send_command(opcode, &parameters) {
                Ok(response) => {
                    let frame = IpcFrame::v1(
                        IpcMessageKind::HciCommandResponse,
                        request_id,
                        encode_hci_command_response(session_id, &response),
                    );
                    if send_frame(&outbound, frame).is_err() {
                        fatal = Some("client command response queue overflow".into());
                        break;
                    }
                }
                Err(error)
                    if send_error(
                        &outbound,
                        request_id,
                        IpcErrorCode::RawHci,
                        error.to_string(),
                    )
                    .is_err() =>
                {
                    fatal = Some("client response queue overflow".into());
                    break;
                }
                Err(_) => {}
            },
            Ok(ControllerRequest::Acl { request_id, packet }) => {
                match live.raw_hci().send_acl(&packet) {
                    Ok(()) => {
                        if send_json(
                            &outbound,
                            IpcMessageKind::ControlResponse,
                            request_id,
                            &ControlResponse::Accepted,
                        )
                        .is_err()
                        {
                            fatal = Some("client response queue overflow".into());
                            break;
                        }
                    }
                    Err(error)
                        if send_error(
                            &outbound,
                            request_id,
                            IpcErrorCode::RawHci,
                            error.to_string(),
                        )
                        .is_err() =>
                    {
                        fatal = Some("client response queue overflow".into());
                        break;
                    }
                    Err(_) => {}
                }
            }
            Ok(ControllerRequest::Release { request_id }) => {
                release_request = Some(request_id);
                break;
            }
            Ok(ControllerRequest::Stop) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }

        for _ in 0..RX_BATCH {
            let event = match live.raw_hci().receive_event(Duration::ZERO) {
                Ok(Some(event)) => event,
                Ok(None) => break,
                Err(error) => {
                    fatal = Some(format!("event RX failed: {error}"));
                    break 'session;
                }
            };
            let payload = match encode_hci_event(session_id, &event) {
                Ok(payload) => payload,
                Err(error) => {
                    fatal = Some(format!("encode HCI event: {error}"));
                    break 'session;
                }
            };
            let frame =
                IpcFrame::v1(IpcMessageKind::HciEvent, 0, payload).expect("bounded HCI event");
            if outbound.try_send(frame).is_err() {
                fatal = Some("client event queue overflow".into());
                break 'session;
            }
        }
        for _ in 0..RX_BATCH {
            let packet = match live.raw_hci().receive_acl(Duration::ZERO) {
                Ok(Some(packet)) => packet,
                Ok(None) => break,
                Err(error) => {
                    fatal = Some(format!("ACL RX failed: {error}"));
                    break 'session;
                }
            };
            let payload = match encode_acl_packet(session_id, &packet) {
                Ok(payload) => payload,
                Err(error) => {
                    fatal = Some(format!("encode ACL packet: {error}"));
                    break 'session;
                }
            };
            let frame =
                IpcFrame::v1(IpcMessageKind::AclRx, 0, payload).expect("bounded ACL packet");
            if outbound.try_send(frame).is_err() {
                fatal = Some("client ACL queue overflow".into());
                break 'session;
            }
        }
        if let Some(error) = live.raw_hci().terminal_error() {
            fatal = Some(error.to_string());
            break;
        }
    }

    if let Some(runtime) = runtime.upgrade() {
        runtime.mark_restoring(session_id);
    }
    let report = live.release();
    let recovery_required = matches!(report.status, RoundTripStatus::RecoveryRequired);
    let message = report
        .recovery_error
        .clone()
        .or(fatal)
        .or(report.primary_error.clone());
    if let Some(request_id) = release_request {
        if recovery_required {
            let _ = send_error(
                &outbound,
                request_id,
                IpcErrorCode::RecoveryRequired,
                message
                    .clone()
                    .unwrap_or_else(|| "Windows restore failed".into()),
            );
        } else {
            let _ = send_json(
                &outbound,
                IpcMessageKind::ControlResponse,
                request_id,
                &ControlResponse::Released {
                    session_id,
                    controller: report.final_state.clone(),
                },
            );
        }
    }
    if let Some(runtime) = runtime.upgrade() {
        runtime.worker_finished(session_id, recovery_required, message);
    }
    // An unsolicited worker exit has no ReleaseSession response. Wake the
    // owning pipe so event/ACL receivers observe disconnection, not timeouts.
    if release_request.is_none() {
        disconnect_owner();
    }
    eprintln!(
        "directhcid: session {session_id} ended; Windows restore status: {:?}",
        report.status
    );
}

fn send_frame(
    outbound: &Outbound,
    frame: Result<IpcFrame, directhci_core::IpcFrameError>,
) -> Result<(), ()> {
    outbound.try_send(frame.map_err(|_| ())?).map_err(|_| ())
}
fn send_json<T: serde::Serialize>(
    outbound: &Outbound,
    kind: IpcMessageKind,
    request_id: u32,
    value: &T,
) -> Result<(), ()> {
    let payload = directhci_core::encode_ipc_json(value).map_err(|_| ())?;
    send_frame(outbound, IpcFrame::v1(kind, request_id, payload))
}
fn send_error(
    outbound: &Outbound,
    request_id: u32,
    code: IpcErrorCode,
    message: String,
) -> Result<(), ()> {
    send_json(
        outbound,
        IpcMessageKind::Error,
        request_id,
        &directhci_core::IpcErrorResponse { code, message },
    )
}
fn runtime_lock_error() -> (IpcErrorCode, String) {
    (IpcErrorCode::Runtime, "runtime state lock poisoned".into())
}
fn map_request_send(error: mpsc::TrySendError<ControllerRequest>) -> (IpcErrorCode, String) {
    match error {
        mpsc::TrySendError::Full(_) => (
            IpcErrorCode::Backpressure,
            "controller request queue is full".into(),
        ),
        mpsc::TrySendError::Disconnected(_) => (
            IpcErrorCode::InvalidSession,
            "controller session has ended".into(),
        ),
    }
}
