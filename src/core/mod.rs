//! Canonical agent model + revisioned store. No adapter or transport concerns.

pub mod blocks;
pub mod diff;
pub mod events;
pub mod model;
pub mod redact;
pub mod store;
pub mod util;
pub mod workspace;

pub use events::{
    GhIssueRef, GhPrState, GhRepoState, GitEvent, GitStatus, Plane, PlaneEvent, PlaneSink,
    plane_channel,
};

pub use model::{
    Agent, AgentState, Attachment, CAPABILITIES, Change, CiStatus, Delta, Resume, SCHEMA_VERSION,
    Snapshot, WaitingOn, WaitingOnKind, Workspace,
};
pub use redact::{REDACTED, redact};
pub use store::Store;
pub use workspace::{RepoRoot, WorkspaceAttribution, WorkspaceFacts};
