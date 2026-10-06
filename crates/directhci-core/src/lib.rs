//! Portable controller identity and observation types.
//!
//! This crate deliberately contains no Windows PnP calls, USB transport, or
//! vendor-specific controller initialization.

mod controller;
mod hci;
mod ipc;
mod ownership;

pub use controller::{
    ControllerId, ControllerIdentity, ControllerIdentityConfidence, ControllerObservation,
    DeviceStatus, DriverObservation,
};
pub use hci::{
    ControllerBufferSize, HCI_EVENT_COMMAND_COMPLETE, HCI_EVENT_COMMAND_STATUS,
    HCI_READ_BUFFER_SIZE, HCI_READ_LOCAL_SUPPORTED_COMMANDS, HCI_READ_LOCAL_SUPPORTED_FEATURES,
    HCI_READ_LOCAL_VERSION_INFORMATION, HCI_RESET, HciAclPacket, HciCommandPacket,
    HciCommandResponse, HciEventPacket, HciIncomingPacket, HciPacketError, LocalSupportedCommands,
    LocalSupportedFeatures, LocalVersionInformation, opcode, parse_controller_buffer_size,
    parse_local_supported_commands, parse_local_supported_features,
    parse_local_version_information, parse_reset_response,
};
pub use ipc::*;
pub use ownership::{
    DesiredControllerState, DeviceSecurityBaseline, DriverPackageIdentity,
    InvalidOwnershipTransition, LeaseId, LeaseOwnerMetadata, OWNERSHIP_JOURNAL_SCHEMA_VERSION,
    OwnershipBackend, OwnershipJournal, OwnershipPhase,
};
