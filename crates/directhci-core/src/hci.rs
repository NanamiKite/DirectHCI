//! Minimal, transport-neutral Bluetooth HCI packet framing.
//!
//! These types intentionally stop at the Raw HCI boundary. They do not model
//! L2CAP, ATT, GATT, vendor initialization, or controller ownership.

use serde::Serialize;

pub const HCI_EVENT_COMMAND_COMPLETE: u8 = 0x0e;
pub const HCI_EVENT_COMMAND_STATUS: u8 = 0x0f;

pub const fn opcode(ogf: u16, ocf: u16) -> u16 {
    ((ogf & 0x003f) << 10) | (ocf & 0x03ff)
}

pub const HCI_RESET: u16 = opcode(0x03, 0x0003);
pub const HCI_READ_LOCAL_VERSION_INFORMATION: u16 = opcode(0x04, 0x0001);
pub const HCI_READ_LOCAL_SUPPORTED_COMMANDS: u16 = opcode(0x04, 0x0002);
pub const HCI_READ_LOCAL_SUPPORTED_FEATURES: u16 = opcode(0x04, 0x0003);
pub const HCI_READ_BUFFER_SIZE: u16 = opcode(0x04, 0x0005);

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum HciPacketError {
    CommandParametersTooLong(usize),
    EventTooShort(usize),
    EventLengthMismatch {
        declared: usize,
        actual: usize,
    },
    CommandCompleteTooShort(usize),
    CommandStatusLengthMismatch(usize),
    AclPayloadTooLong(usize),
    AclTooShort(usize),
    AclLengthMismatch {
        declared: usize,
        actual: usize,
    },
    InvalidAclHandle(u16),
    InvalidPacketBoundaryFlag(u8),
    InvalidBroadcastFlag(u8),
    UnexpectedCommandResponse {
        opcode: u16,
        response: &'static str,
    },
    InvalidReturnParameters {
        opcode: u16,
        expected: usize,
        actual: usize,
    },
    CommandFailed {
        opcode: u16,
        status: u8,
    },
}

impl std::fmt::Display for HciPacketError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::CommandParametersTooLong(length) => write!(
                formatter,
                "HCI command parameters exceed 255 bytes ({length})"
            ),
            Self::EventTooShort(length) => write!(formatter, "HCI event is too short ({length})"),
            Self::EventLengthMismatch { declared, actual } => write!(
                formatter,
                "HCI event length mismatch (declared {declared}, actual {actual})"
            ),
            Self::CommandCompleteTooShort(length) => write!(
                formatter,
                "Command Complete parameters are too short ({length})"
            ),
            Self::CommandStatusLengthMismatch(length) => write!(
                formatter,
                "Command Status must contain 4 bytes, got {length}"
            ),
            Self::AclPayloadTooLong(length) => {
                write!(formatter, "HCI ACL payload exceeds 65535 bytes ({length})")
            }
            Self::AclTooShort(length) => {
                write!(formatter, "HCI ACL packet is too short ({length})")
            }
            Self::AclLengthMismatch { declared, actual } => write!(
                formatter,
                "HCI ACL length mismatch (declared {declared}, actual {actual})"
            ),
            Self::InvalidAclHandle(handle) => {
                write!(formatter, "invalid 12-bit ACL handle {handle:#x}")
            }
            Self::InvalidPacketBoundaryFlag(flag) => {
                write!(formatter, "invalid ACL packet-boundary flag {flag}")
            }
            Self::InvalidBroadcastFlag(flag) => {
                write!(formatter, "invalid ACL broadcast flag {flag}")
            }
            Self::UnexpectedCommandResponse { opcode, response } => write!(
                formatter,
                "opcode {opcode:#06x} returned unexpected {response} response"
            ),
            Self::InvalidReturnParameters {
                opcode,
                expected,
                actual,
            } => write!(
                formatter,
                "opcode {opcode:#06x} returned {actual} bytes; expected {expected}"
            ),
            Self::CommandFailed { opcode, status } => write!(
                formatter,
                "opcode {opcode:#06x} failed with HCI status {status:#04x}"
            ),
        }
    }
}

impl std::error::Error for HciPacketError {}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct HciCommandPacket {
    pub opcode: u16,
    pub parameters: Vec<u8>,
}

impl HciCommandPacket {
    pub fn new(opcode: u16, parameters: impl Into<Vec<u8>>) -> Self {
        Self {
            opcode,
            parameters: parameters.into(),
        }
    }

    pub fn encode(&self) -> Result<Vec<u8>, HciPacketError> {
        let parameter_length = u8::try_from(self.parameters.len())
            .map_err(|_| HciPacketError::CommandParametersTooLong(self.parameters.len()))?;
        let mut packet = Vec::with_capacity(3 + self.parameters.len());
        packet.extend_from_slice(&self.opcode.to_le_bytes());
        packet.push(parameter_length);
        packet.extend_from_slice(&self.parameters);
        Ok(packet)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct HciEventPacket {
    pub event_code: u8,
    pub parameters: Vec<u8>,
}

impl HciEventPacket {
    pub fn parse(packet: &[u8]) -> Result<Self, HciPacketError> {
        if packet.len() < 2 {
            return Err(HciPacketError::EventTooShort(packet.len()));
        }
        let declared = packet[1] as usize;
        let actual = packet.len() - 2;
        if declared != actual {
            return Err(HciPacketError::EventLengthMismatch { declared, actual });
        }
        Ok(Self {
            event_code: packet[0],
            parameters: packet[2..].to_vec(),
        })
    }

    pub fn encode(&self) -> Result<Vec<u8>, HciPacketError> {
        let parameter_length = u8::try_from(self.parameters.len()).map_err(|_| {
            HciPacketError::EventLengthMismatch {
                declared: u8::MAX as usize,
                actual: self.parameters.len(),
            }
        })?;
        let mut packet = Vec::with_capacity(2 + self.parameters.len());
        packet.push(self.event_code);
        packet.push(parameter_length);
        packet.extend_from_slice(&self.parameters);
        Ok(packet)
    }

    pub fn command_response(&self) -> Result<Option<HciCommandResponse>, HciPacketError> {
        match self.event_code {
            HCI_EVENT_COMMAND_COMPLETE => {
                if self.parameters.len() < 3 {
                    return Err(HciPacketError::CommandCompleteTooShort(
                        self.parameters.len(),
                    ));
                }
                Ok(Some(HciCommandResponse::Complete {
                    num_hci_command_packets: self.parameters[0],
                    command_opcode: u16::from_le_bytes([self.parameters[1], self.parameters[2]]),
                    return_parameters: self.parameters[3..].to_vec(),
                }))
            }
            HCI_EVENT_COMMAND_STATUS => {
                if self.parameters.len() != 4 {
                    return Err(HciPacketError::CommandStatusLengthMismatch(
                        self.parameters.len(),
                    ));
                }
                Ok(Some(HciCommandResponse::Status {
                    status: self.parameters[0],
                    num_hci_command_packets: self.parameters[1],
                    command_opcode: u16::from_le_bytes([self.parameters[2], self.parameters[3]]),
                }))
            }
            _ => Ok(None),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum HciCommandResponse {
    Complete {
        num_hci_command_packets: u8,
        command_opcode: u16,
        return_parameters: Vec<u8>,
    },
    Status {
        status: u8,
        num_hci_command_packets: u8,
        command_opcode: u16,
    },
}

impl HciCommandResponse {
    pub fn opcode(&self) -> u16 {
        match self {
            Self::Complete { command_opcode, .. } | Self::Status { command_opcode, .. } => {
                *command_opcode
            }
        }
    }

    pub fn command_credits(&self) -> u8 {
        match self {
            Self::Complete {
                num_hci_command_packets,
                ..
            }
            | Self::Status {
                num_hci_command_packets,
                ..
            } => *num_hci_command_packets,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct HciAclPacket {
    pub handle: u16,
    pub packet_boundary_flag: u8,
    pub broadcast_flag: u8,
    pub payload: Vec<u8>,
}

impl HciAclPacket {
    pub fn parse(packet: &[u8]) -> Result<Self, HciPacketError> {
        if packet.len() < 4 {
            return Err(HciPacketError::AclTooShort(packet.len()));
        }
        let handle_and_flags = u16::from_le_bytes([packet[0], packet[1]]);
        let declared = u16::from_le_bytes([packet[2], packet[3]]) as usize;
        let actual = packet.len() - 4;
        if declared != actual {
            return Err(HciPacketError::AclLengthMismatch { declared, actual });
        }
        Ok(Self {
            handle: handle_and_flags & 0x0fff,
            packet_boundary_flag: ((handle_and_flags >> 12) & 0x03) as u8,
            broadcast_flag: ((handle_and_flags >> 14) & 0x03) as u8,
            payload: packet[4..].to_vec(),
        })
    }

    pub fn encode(&self) -> Result<Vec<u8>, HciPacketError> {
        if self.handle > 0x0fff {
            return Err(HciPacketError::InvalidAclHandle(self.handle));
        }
        if self.packet_boundary_flag > 0x03 {
            return Err(HciPacketError::InvalidPacketBoundaryFlag(
                self.packet_boundary_flag,
            ));
        }
        if self.broadcast_flag > 0x03 {
            return Err(HciPacketError::InvalidBroadcastFlag(self.broadcast_flag));
        }
        let payload_length = u16::try_from(self.payload.len())
            .map_err(|_| HciPacketError::AclPayloadTooLong(self.payload.len()))?;
        let handle_and_flags = self.handle
            | ((self.packet_boundary_flag as u16) << 12)
            | ((self.broadcast_flag as u16) << 14);
        let mut packet = Vec::with_capacity(4 + self.payload.len());
        packet.extend_from_slice(&handle_and_flags.to_le_bytes());
        packet.extend_from_slice(&payload_length.to_le_bytes());
        packet.extend_from_slice(&self.payload);
        Ok(packet)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct LocalVersionInformation {
    pub hci_version: u8,
    pub hci_revision: u16,
    pub lmp_pal_version: u8,
    pub manufacturer_name: u16,
    pub lmp_pal_subversion: u16,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct LocalSupportedCommands {
    pub commands: Vec<u8>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct LocalSupportedFeatures {
    pub features: Vec<u8>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ControllerBufferSize {
    pub acl_data_packet_length: u16,
    pub synchronous_data_packet_length: u8,
    pub total_num_acl_data_packets: u16,
    pub total_num_synchronous_data_packets: u16,
}

fn completed_parameters(
    expected_opcode: u16,
    response: &HciCommandResponse,
) -> Result<&[u8], HciPacketError> {
    match response {
        HciCommandResponse::Complete {
            command_opcode,
            return_parameters,
            ..
        } if *command_opcode == expected_opcode => Ok(return_parameters),
        HciCommandResponse::Complete { command_opcode, .. } => {
            Err(HciPacketError::UnexpectedCommandResponse {
                opcode: *command_opcode,
                response: "Command Complete for another opcode",
            })
        }
        HciCommandResponse::Status { .. } => Err(HciPacketError::UnexpectedCommandResponse {
            opcode: expected_opcode,
            response: "Command Status",
        }),
    }
}

fn successful_fixed_return(
    opcode: u16,
    response: &HciCommandResponse,
    expected_length: usize,
) -> Result<&[u8], HciPacketError> {
    let parameters = completed_parameters(opcode, response)?;
    if parameters.len() != expected_length {
        return Err(HciPacketError::InvalidReturnParameters {
            opcode,
            expected: expected_length,
            actual: parameters.len(),
        });
    }
    if parameters[0] != 0 {
        return Err(HciPacketError::CommandFailed {
            opcode,
            status: parameters[0],
        });
    }
    Ok(parameters)
}

pub fn parse_reset_response(response: &HciCommandResponse) -> Result<u8, HciPacketError> {
    Ok(successful_fixed_return(HCI_RESET, response, 1)?[0])
}

pub fn parse_local_version_information(
    response: &HciCommandResponse,
) -> Result<LocalVersionInformation, HciPacketError> {
    let value = successful_fixed_return(HCI_READ_LOCAL_VERSION_INFORMATION, response, 9)?;
    Ok(LocalVersionInformation {
        hci_version: value[1],
        hci_revision: u16::from_le_bytes([value[2], value[3]]),
        lmp_pal_version: value[4],
        manufacturer_name: u16::from_le_bytes([value[5], value[6]]),
        lmp_pal_subversion: u16::from_le_bytes([value[7], value[8]]),
    })
}

pub fn parse_local_supported_commands(
    response: &HciCommandResponse,
) -> Result<LocalSupportedCommands, HciPacketError> {
    let value = successful_fixed_return(HCI_READ_LOCAL_SUPPORTED_COMMANDS, response, 65)?;
    Ok(LocalSupportedCommands {
        commands: value[1..].to_vec(),
    })
}

pub fn parse_local_supported_features(
    response: &HciCommandResponse,
) -> Result<LocalSupportedFeatures, HciPacketError> {
    let value = successful_fixed_return(HCI_READ_LOCAL_SUPPORTED_FEATURES, response, 9)?;
    Ok(LocalSupportedFeatures {
        features: value[1..].to_vec(),
    })
}

pub fn parse_controller_buffer_size(
    response: &HciCommandResponse,
) -> Result<ControllerBufferSize, HciPacketError> {
    let value = successful_fixed_return(HCI_READ_BUFFER_SIZE, response, 8)?;
    Ok(ControllerBufferSize {
        acl_data_packet_length: u16::from_le_bytes([value[1], value[2]]),
        synchronous_data_packet_length: value[3],
        total_num_acl_data_packets: u16::from_le_bytes([value[4], value[5]]),
        total_num_synchronous_data_packets: u16::from_le_bytes([value[6], value[7]]),
    })
}
