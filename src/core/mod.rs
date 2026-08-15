//! Canonical agent model + revisioned store. No adapter or transport concerns.

pub mod model;
pub mod store;
pub mod util;

pub use model::{
    Agent, AgentState, Attachment, Change, Delta, Resume, SCHEMA_VERSION, Snapshot, WaitingOn,
    WaitingOnKind, Workspace, CAPABILITIES,
};
pub use store::Store;
