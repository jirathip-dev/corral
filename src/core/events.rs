//! Cross-plane event contract (P2). The seam the three P2 workstreams agree
//! on before they diverge:
//!
//! - WS1 (`git_plane.rs`) implements [`Plane`] and emits [`PlaneEvent::Git`]
//!   facts about watched worktrees.
//! - WS2 (`gh_plane.rs`) implements [`Plane`] and emits [`PlaneEvent::Gh`]
//!   facts for the tracked repos.
//! - WS3 consumes the combined [`PlaneSink`] and folds every fact into the
//!   canonical agent records (via `workspace.worktree_path` / `workspace.repo`)
//!   through the existing snapshot/SSE pipeline.
//!
//! Contract rules:
//! - Additive only. New event kinds extend [`PlaneEvent`]; existing variants
//!   and field types never change shape.
//! - The primary signal for every plane is push-only (zero polling). A
//!   bounded, documented safety-net sweep is allowed (WS1's 10s `git status`
//!   catch-up); it must never be the primary source of events.
//! - Events are facts about a worktree/repo, never about an agent id — the
//!   agent linkage is the integrator's job, keyed on the path/repo strings
//!   P1's `Workspace` already carries.

use std::fmt::Debug;
use std::path::PathBuf;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

/// Capacity of the per-plane event channel. A backpressure-free burst (e.g.
/// a rebase touching many worktrees) must never deadlock a plane, so the
/// sink is lossless up to this many pending events; an overflowing plane
/// drops *itself* with a logged error rather than blocking forever.
pub const PLANE_CHANNEL_CAP: usize = 4096;

/// Git dirty/divergence summary for one worktree.
///
/// `dirty_index` / `dirty_worktree` mirror `git status` (index = staged,
/// worktree = unstaged modifications + untracked files); `ahead`/`behind`
/// are commits vs the upstream branch. The 10s safety-net sweep emits this
/// after any detected change so the `dirty` field can never go stale.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitStatus {
    pub dirty_index: bool,
    pub dirty_worktree: bool,
    pub ahead: u64,
    pub behind: u64,
}

impl GitStatus {
    pub fn is_dirty(&self) -> bool {
        self.dirty_index || self.dirty_worktree
    }
}

/// One worktree-level git fact (WS1).
///
/// `HeadMoved` carries both of the brief's "branch switch" and "HEAD move"
/// events: the consumer diffs `branch`/`commit` against the previously seen
/// head to tell them apart, so the wire surface stays one event.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum GitEvent {
    /// HEAD changed in a watched worktree: branch switch and/or commit move.
    HeadMoved {
        worktree: PathBuf,
        branch: String,
        commit: String,
    },
    /// Dirty state changed (or the 10s sweep refreshed it).
    DirtyChanged {
        worktree: PathBuf,
        status: GitStatus,
    },
    /// A worktree appeared (herdr `git worktree add`).
    WorktreeAdded { worktree: PathBuf },
    /// A worktree disappeared (`git worktree remove` / delete).
    WorktreeRemoved { worktree: PathBuf },
    /// A commit landed on the current branch.
    CommitOnBranch {
        worktree: PathBuf,
        branch: String,
        commit: String,
    },
}

/// PR state for one pull request (WS2). `state` is OPEN/CLOSED/MERGED,
/// `mergeable` MERGEABLE/CONFLICTING/UNKNOWN, `ci_status` SUCCESS/FAILURE/
/// PENDING/UNKNOWN — canonical strings, normalized from the GraphQL payload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GhPrState {
    pub repo: String,
    pub pr_number: u64,
    pub title: String,
    pub state: String,
    pub mergeable: String,
    pub ci_status: String,
    pub head_sha: String,
    /// Head branch name (`headRefName`), the (repo, branch) matching key
    /// (#22): an agent's committed-but-unpushed branch binds its PR after
    /// the head-SHA match misses. Empty when GitHub reports none (deleted
    /// head branch).
    #[serde(default)]
    pub head_branch: String,
    /// Issues this PR closes, from GitHub's authoritative
    /// `closingIssuesReferences` (#23). The `state` of each ref is enriched
    /// from the same poll's repo-level issues fetch when the issue is among
    /// the recent ones; otherwise `"UNKNOWN"` — the linkage itself always
    /// comes from the closing refs, never a heuristic.
    #[serde(default)]
    pub closing_issues: Vec<GhIssueRef>,
}

/// Issue reference for one repo (WS2) — "issue refs" leg of the aliased
/// query: number, current state, title.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GhIssueRef {
    pub repo: String,
    pub number: u64,
    pub state: String,
    pub title: String,
}

/// Repo-level gh facts for one poll round-trip (WS2).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct GhRepoState {
    pub repo: String,
    pub default_branch: String,
    /// Local tracking info, where the poller can observe it.
    pub ahead: u64,
    pub behind: u64,
    pub prs: Vec<GhPrState>,
    pub issues: Vec<GhIssueRef>,
}

/// Union of all non-agent facts. WS1/WS2 emit; WS3 integrates.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum PlaneEvent {
    Git(GitEvent),
    Gh(GhRepoState),
}

/// Push-only sink every plane emits into. Clones share the same channel;
/// WS3 owns the receiving half.
pub type PlaneSink = mpsc::Sender<PlaneEvent>;

/// Contract every P2 plane implements. Same shape as the P1 `Adapter` trait:
/// `start` must not block — it spawns background work and returns.
pub trait Plane: Debug + Send + Sync {
    /// Canonical source name, e.g. "git", "gh".
    fn source(&self) -> &'static str;

    /// Begin pushing [`PlaneEvent`]s into `sink`. The primary signal is
    /// push-only (no polling loop); a bounded safety-net sweep is allowed
    /// and must be documented in the implementing module.
    fn start(self: Arc<Self>, sink: PlaneSink);
}

/// Construct a fresh plane channel (sink + receiver).
pub fn plane_channel() -> (PlaneSink, mpsc::Receiver<PlaneEvent>) {
    mpsc::channel(PLANE_CHANNEL_CAP)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn git_status_dirty_semantics() {
        let clean = GitStatus::default();
        assert!(!clean.is_dirty());
        assert!(GitStatus {
            dirty_index: true,
            ..Default::default()
        }
        .is_dirty());
        assert!(GitStatus {
            dirty_worktree: true,
            ..Default::default()
        }
        .is_dirty());
        assert!(
            !GitStatus {
                ahead: 3,
                ..Default::default()
            }
            .is_dirty(),
            "divergence without changes is not dirty"
        );
    }

    #[test]
    fn plane_event_round_trips_through_json() {
        let events = [
            PlaneEvent::Git(GitEvent::HeadMoved {
                worktree: PathBuf::from("/wt/a"),
                branch: "feat/x".to_string(),
                commit: "abc123".to_string(),
            }),
            PlaneEvent::Git(GitEvent::DirtyChanged {
                worktree: PathBuf::from("/wt/a"),
                status: GitStatus {
                    dirty_worktree: true,
                    ahead: 1,
                    behind: 2,
                    ..Default::default()
                },
            }),
            PlaneEvent::Git(GitEvent::CommitOnBranch {
                worktree: PathBuf::from("/wt/b"),
                branch: "main".to_string(),
                commit: "def456".to_string(),
            }),
            PlaneEvent::Gh(GhRepoState {
                repo: "herdr-board".to_string(),
                default_branch: "main".to_string(),
                prs: vec![GhPrState {
                    repo: "herdr-board".to_string(),
                    pr_number: 7,
                    title: "P2 three planes".to_string(),
                    state: "OPEN".to_string(),
                    mergeable: "MERGEABLE".to_string(),
                    ci_status: "SUCCESS".to_string(),
                    head_sha: "abc123".to_string(),
                    head_branch: "ws2/gh-plane".to_string(),
                    closing_issues: vec![GhIssueRef {
                        repo: "herdr-board".to_string(),
                        number: 4,
                        state: "OPEN".to_string(),
                        title: "P2 planes".to_string(),
                    }],
                }],
                ..Default::default()
            }),
        ];
        for event in events {
            let wire = serde_json::to_string(&event).expect("serialize");
            let back: PlaneEvent = serde_json::from_str(&wire).expect("deserialize");
            assert_eq!(back, event, "round trip must preserve the event");
        }
    }
}
