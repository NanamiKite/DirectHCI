//! Portable controller identity and observation types.
//!
//! This crate deliberately contains no Windows PnP calls, USB transport, or
//! vendor-specific controller initialization.

mod controller;
mod ownership;

pub use controller::{
    ControllerId, ControllerIdentity, ControllerIdentityConfidence, ControllerObservation,
    DeviceStatus, DriverObservation,
};
pub use ownership::{
    DesiredControllerState, DriverPackageIdentity, InvalidOwnershipTransition, LeaseId,
    LeaseOwnerMetadata, OWNERSHIP_JOURNAL_SCHEMA_VERSION, OwnershipBackend, OwnershipJournal,
    OwnershipPhase,
};
