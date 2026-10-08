//! Small, versioned local IPC wire protocol shared by the runtime and SDK.

use std::io::{Read, Write};

use serde::{Deserialize, Serialize, de::DeserializeOwned};

use crate::{
    ControllerId, ControllerObservation, HciAclPacket, HciCommandResponse, HciEventPacket,
};

pub const IPC_MAGIC: [u8; 4] = *b"DHCI";
pub const IPC_PROTOCOL_VERSION: u16 = 1;
pub const IPC_MAX_PAYLOAD: usize = 1024 * 1024;
pub const IPC_PIPE_NAME: &str = r"\\.\pipe\DirectHCI\v1";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u16)]
pub enum IpcMessageKind {
    ClientHello = 1,
    ServerHello = 2,
    ControlRequest = 3,
    ControlResponse = 4,
    HciCommand = 5,
    HciCommandResponse = 6,
    HciEvent = 7,
    AclTx = 8,
    AclRx = 9,
    Error = 10,
}

impl TryFrom<u16> for IpcMessageKind {
    type Error = IpcFrameError;

    fn try_from(value: u16) -> Result<Self, IpcFrameError> {
        match value {
            1 => Ok(Self::ClientHello),
            2 => Ok(Self::ServerHello),
            3 => Ok(Self::ControlRequest),
            4 => Ok(Self::ControlResponse),
            5 => Ok(Self::HciCommand),
            6 => Ok(Self::HciCommandResponse),
            7 => Ok(Self::HciEvent),
            8 => Ok(Self::AclTx),
            9 => Ok(Self::AclRx),
            10 => Ok(Self::Error),
            other => Err(IpcFrameError::UnknownMessageKind(other)),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IpcFrame {
    pub version: u16,
    pub kind: IpcMessageKind,
    pub request_id: u32,
    pub payload: Vec<u8>,
}

impl IpcFrame {
    pub fn v1(
        kind: IpcMessageKind,
        request_id: u32,
        payload: Vec<u8>,
    ) -> Result<Self, IpcFrameError> {
        if payload.len() > IPC_MAX_PAYLOAD {
            return Err(IpcFrameError::PayloadTooLarge(payload.len()));
        }
        Ok(Self {
            version: IPC_PROTOCOL_VERSION,
            kind,
            request_id,
            payload,
        })
    }
}

#[derive(Debug)]
pub enum IpcFrameError {
    Io(std::io::Error),
    InvalidMagic([u8; 4]),
    UnsupportedVersion(u16),
    UnknownMessageKind(u16),
    PayloadTooLarge(usize),
    InvalidPayload(String),
}

impl std::fmt::Display for IpcFrameError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "IPC I/O: {error}"),
            Self::InvalidMagic(magic) => write!(formatter, "invalid IPC magic {magic:?}"),
            Self::UnsupportedVersion(version) => {
                write!(formatter, "unsupported IPC version {version}")
            }
            Self::UnknownMessageKind(kind) => write!(formatter, "unknown IPC message kind {kind}"),
            Self::PayloadTooLarge(length) => {
                write!(formatter, "IPC payload is too large ({length} bytes)")
            }
            Self::InvalidPayload(message) => write!(formatter, "invalid IPC payload: {message}"),
        }
    }
}

impl std::error::Error for IpcFrameError {}
impl From<std::io::Error> for IpcFrameError {
    fn from(value: std::io::Error) -> Self {
        Self::Io(value)
    }
}

pub fn write_ipc_frame(writer: &mut impl Write, frame: &IpcFrame) -> Result<(), IpcFrameError> {
    if frame.version != IPC_PROTOCOL_VERSION {
        return Err(IpcFrameError::UnsupportedVersion(frame.version));
    }
    if frame.payload.len() > IPC_MAX_PAYLOAD {
        return Err(IpcFrameError::PayloadTooLarge(frame.payload.len()));
    }
    writer.write_all(&IPC_MAGIC)?;
    writer.write_all(&frame.version.to_le_bytes())?;
    writer.write_all(&(frame.kind as u16).to_le_bytes())?;
    writer.write_all(&frame.request_id.to_le_bytes())?;
    writer.write_all(&(frame.payload.len() as u32).to_le_bytes())?;
    writer.write_all(&frame.payload)?;
    writer.flush()?;
    Ok(())
}

pub fn read_ipc_frame(reader: &mut impl Read) -> Result<Option<IpcFrame>, IpcFrameError> {
    let mut header = [0u8; 16];
    match reader.read(&mut header[..1]) {
        Ok(0) => return Ok(None),
        Ok(1) => reader.read_exact(&mut header[1..])?,
        Ok(_) => unreachable!(),
        Err(error) => return Err(IpcFrameError::Io(error)),
    }
    let magic = [header[0], header[1], header[2], header[3]];
    if magic != IPC_MAGIC {
        return Err(IpcFrameError::InvalidMagic(magic));
    }
    let version = u16::from_le_bytes([header[4], header[5]]);
    if version != IPC_PROTOCOL_VERSION {
        return Err(IpcFrameError::UnsupportedVersion(version));
    }
    let kind = IpcMessageKind::try_from(u16::from_le_bytes([header[6], header[7]]))?;
    let request_id = u32::from_le_bytes([header[8], header[9], header[10], header[11]]);
    let length = u32::from_le_bytes([header[12], header[13], header[14], header[15]]) as usize;
    if length > IPC_MAX_PAYLOAD {
        return Err(IpcFrameError::PayloadTooLarge(length));
    }
    let mut payload = vec![0u8; length];
    reader.read_exact(&mut payload)?;
    Ok(Some(IpcFrame {
        version,
        kind,
        request_id,
        payload,
    }))
}

pub fn encode_ipc_json<T: Serialize>(value: &T) -> Result<Vec<u8>, IpcFrameError> {
    serde_json::to_vec(value).map_err(|error| IpcFrameError::InvalidPayload(error.to_string()))
}

pub fn decode_ipc_json<T: DeserializeOwned>(payload: &[u8]) -> Result<T, IpcFrameError> {
    serde_json::from_slice(payload)
        .map_err(|error| IpcFrameError::InvalidPayload(error.to_string()))
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ClientHello {
    pub protocol_version: u16,
    pub client_name: String,
    pub client_version: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ServerHello {
    pub protocol_version: u16,
    pub directhci_version: String,
    pub capabilities: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case")]
pub enum ControlRequest {
    ListControllers,
    RuntimeStatus,
    GetPreferences,
    SetPreferredController {
        controller_id: String,
    },
    ControllerPreparationStatus {
        controller_id: String,
    },
    PrepareController {
        controller_id: String,
        trust_acknowledged: bool,
    },
    RestoreWindows,
    AcquireRawHci {
        controller_id: String,
    },
    ReleaseSession {
        session_id: u64,
    },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "result", rename_all = "snake_case")]
pub enum ControlResponse {
    Controllers {
        controllers: Vec<ControllerObservation>,
    },
    RuntimeStatus {
        status: RuntimeStatus,
    },
    Preferences {
        preferences: RuntimePreferences,
    },
    ControllerPreparationStatus {
        status: ControllerPreparationStatus,
    },
    ControllerPrepared {
        preparation: PreparedController,
    },
    SessionReady {
        session_id: u64,
        controller: ControllerObservation,
        transport: IpcTransportInfo,
    },
    Released {
        session_id: u64,
        controller: Option<ControllerObservation>,
    },
    Accepted,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct RuntimePreferences {
    pub preferred_controller_id: Option<ControllerId>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ControllerPreparationStatus {
    pub hardware_id: String,
    pub ready: bool,
    pub takeover_safe: bool,
    pub blockers: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct PreparedController {
    pub status: ControllerPreparationStatus,
    pub already_prepared: bool,
    pub staged_inf: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct IpcTransportInfo {
    pub interface_number: u8,
    pub alternate_setting: u8,
    pub event_pipe: u8,
    pub acl_in_pipe: u8,
    pub acl_out_pipe: u8,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeControllerState {
    WindowsOwned,
    Acquiring,
    DirectHciOwned,
    Restoring,
    RecoveryRequired,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct RuntimeControllerStatus {
    pub controller: ControllerObservation,
    pub state: RuntimeControllerState,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ActiveSessionInfo {
    pub session_id: u64,
    pub controller_id: ControllerId,
    pub client_name: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct RuntimeStatus {
    pub runtime_version: String,
    pub recovery_required: bool,
    pub recovery_message: Option<String>,
    pub controllers: Vec<RuntimeControllerStatus>,
    pub active_session: Option<ActiveSessionInfo>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IpcErrorCode {
    Protocol,
    Unauthorized,
    ControllerNotFound,
    ControllerBusy,
    RecoveryRequired,
    InvalidSession,
    Backpressure,
    Ownership,
    RawHci,
    Runtime,
    Provisioning,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct IpcErrorResponse {
    pub code: IpcErrorCode,
    pub message: String,
}

fn session_prefix(session_id: u64) -> Vec<u8> {
    session_id.to_le_bytes().to_vec()
}
fn decode_session_prefix(payload: &[u8]) -> Result<(u64, &[u8]), IpcFrameError> {
    if payload.len() < 8 {
        return Err(IpcFrameError::InvalidPayload("missing session ID".into()));
    }
    Ok((
        u64::from_le_bytes(payload[..8].try_into().expect("8-byte slice")),
        &payload[8..],
    ))
}

pub fn encode_hci_command(
    session_id: u64,
    opcode: u16,
    parameters: &[u8],
) -> Result<Vec<u8>, IpcFrameError> {
    if parameters.len() > u8::MAX as usize {
        return Err(IpcFrameError::InvalidPayload(
            "HCI command parameters exceed 255 bytes".into(),
        ));
    }
    let mut payload = session_prefix(session_id);
    payload.extend_from_slice(&opcode.to_le_bytes());
    payload.push(parameters.len() as u8);
    payload.extend_from_slice(parameters);
    Ok(payload)
}

pub fn decode_hci_command(payload: &[u8]) -> Result<(u64, u16, Vec<u8>), IpcFrameError> {
    let (session_id, packet) = decode_session_prefix(payload)?;
    if packet.len() < 3 {
        return Err(IpcFrameError::InvalidPayload(
            "HCI command header is truncated".into(),
        ));
    }
    let opcode = u16::from_le_bytes([packet[0], packet[1]]);
    let length = packet[2] as usize;
    if packet.len() != 3 + length {
        return Err(IpcFrameError::InvalidPayload(
            "HCI command length mismatch".into(),
        ));
    }
    Ok((session_id, opcode, packet[3..].to_vec()))
}

pub fn encode_hci_command_response(session_id: u64, response: &HciCommandResponse) -> Vec<u8> {
    let mut payload = session_prefix(session_id);
    match response {
        HciCommandResponse::Complete {
            num_hci_command_packets,
            command_opcode,
            return_parameters,
        } => {
            payload.push(0);
            payload.push(*num_hci_command_packets);
            payload.extend_from_slice(&command_opcode.to_le_bytes());
            payload.extend_from_slice(return_parameters);
        }
        HciCommandResponse::Status {
            status,
            num_hci_command_packets,
            command_opcode,
        } => {
            payload.push(1);
            payload.push(*status);
            payload.push(*num_hci_command_packets);
            payload.extend_from_slice(&command_opcode.to_le_bytes());
        }
    }
    payload
}

pub fn decode_hci_command_response(
    payload: &[u8],
) -> Result<(u64, HciCommandResponse), IpcFrameError> {
    let (session_id, value) = decode_session_prefix(payload)?;
    let Some(tag) = value.first() else {
        return Err(IpcFrameError::InvalidPayload(
            "missing command response tag".into(),
        ));
    };
    match *tag {
        0 if value.len() >= 4 => Ok((
            session_id,
            HciCommandResponse::Complete {
                num_hci_command_packets: value[1],
                command_opcode: u16::from_le_bytes([value[2], value[3]]),
                return_parameters: value[4..].to_vec(),
            },
        )),
        1 if value.len() == 5 => Ok((
            session_id,
            HciCommandResponse::Status {
                status: value[1],
                num_hci_command_packets: value[2],
                command_opcode: u16::from_le_bytes([value[3], value[4]]),
            },
        )),
        _ => Err(IpcFrameError::InvalidPayload(
            "invalid command response".into(),
        )),
    }
}

pub fn encode_hci_event(session_id: u64, event: &HciEventPacket) -> Result<Vec<u8>, IpcFrameError> {
    let mut payload = session_prefix(session_id);
    payload.extend_from_slice(
        &event
            .encode()
            .map_err(|error| IpcFrameError::InvalidPayload(error.to_string()))?,
    );
    Ok(payload)
}
pub fn decode_hci_event(payload: &[u8]) -> Result<(u64, HciEventPacket), IpcFrameError> {
    let (session_id, packet) = decode_session_prefix(payload)?;
    Ok((
        session_id,
        HciEventPacket::parse(packet)
            .map_err(|error| IpcFrameError::InvalidPayload(error.to_string()))?,
    ))
}
pub fn encode_acl_packet(session_id: u64, packet: &HciAclPacket) -> Result<Vec<u8>, IpcFrameError> {
    let mut payload = session_prefix(session_id);
    payload.extend_from_slice(
        &packet
            .encode()
            .map_err(|error| IpcFrameError::InvalidPayload(error.to_string()))?,
    );
    Ok(payload)
}
pub fn decode_acl_packet(payload: &[u8]) -> Result<(u64, HciAclPacket), IpcFrameError> {
    let (session_id, packet) = decode_session_prefix(payload)?;
    Ok((
        session_id,
        HciAclPacket::parse(packet)
            .map_err(|error| IpcFrameError::InvalidPayload(error.to_string()))?,
    ))
}
