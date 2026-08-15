//! Canonical agent model + revisioned store. No adapter or transport concerns.

pub mod events;
pub mod model;
pub mod store;
pub mod util;

pub use events::{
    GhIssueRef, GhPrState, GhRepoState, GitEvent, GitStatus, Plane, PlaneEvent, PlaneSink,
    plane_channel,
};

pub use model::{
    Agent, AgentState, Attachment, Change, CiStatus, Delta, Resume, SCHEMA_VERSION, Snapshot,
    WaitingOn, WaitingOnKind, Workspace, CAPABILITIES,
};
pub use store::Store;
