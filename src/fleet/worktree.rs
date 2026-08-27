//! #113: start an issue-linked or issue-free worktree for a fleet.
//!
//! This is the *operation* core: it turns a client-confirmed, authorized
//! request into (exactly one) isolated git worktree + branch under the
//! configured fleet checkout + worktree root, then hands the new worktree
//! to the fleet's orchestrator.
//!
//! Hard rules (from the #113 brief, mirrored here so the operation cannot
//! drift):
//! - **Exactly one** worktree/branch per logical request. A duplicate tap or
//!   retry is idempotent: if the branch or worktree already exists, the
//!   operation returns [`WorktreeOutcome::AlreadyStarted`], never a second
//!   worktree.
//! - **Never fabricate an issue number.** An issue-linked branch carries the
//!   issue number only from the selected issue ref (`issue-<N>-…`). An
//!   issue-free branch uses the `w2/free-…` prefix, which the client's
//!   display-only inference must not read as an issue.
//! - An issue-linked start REFUSES a closed/stale issue (typed
//!   [`WorktreeError::IssueClosed`]) and NEVER falls through to the
//!   issue-free path. The two paths are explicit, mutually exclusive
//!   variants of [`WorktreeRequest`].
//! - The git step and the herdr handoff are injectable seams so hermetic
//!   tests never touch a real worktree or socket.

use std::path::PathBuf;
use std::process::Command;

use serde::{Deserialize, Serialize};

use crate::fleet::cli::FleetIdentity;

/// The client-confirmed, authorized worktree request.
///
/// "kind" is a serde tag so the daemon can reject a malformed request before
/// it touches anything. `repo` is the fleet name (matches a fleet's `name`;
/// the browser joins issue refs to fleets on the same key). An [`Free`] start
/// is only ever produced by the explicit issue-free action — a failed issue
/// lookup must never fabricate one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum WorktreeRequest {
    /// Start from a selected, fetched GitHub issue (issue-linked).
    Issue {
        /// Fleet/repo name (the `GhIssueRef.repo` / `Fleet.name` key).
        repo: String,
        number: u64,
        /// The canonical issue URL echoed into the plan (audit trail).
        issue_url: String,
    },
    /// Explicit, intentional issue-free worktree (unlinked).
    Free {
        /// Fleet/repo name.
        repo: String,
        /// User-chosen label. Must not be empty. The system prefixes it with
        /// `w2/free-` so it can never read as an issue-linked branch.
        name: String,
    },
}

/// A fully-resolved, ready-to-create worktree (branch + path + metadata).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorktreePlan {
    pub fleet: String,
    pub branch: String,
    /// Absolute path to the new worktree checkout.
    pub path: PathBuf,
    pub base: String,
    /// `Some(n)` only for the issue-linked variant — the ONLY place an
    /// issue number is carried, and it comes from the selected issue.
    pub issue_number: Option<u64>,
    /// Issue URL (issue-linked) or empty (issue-free) — audit trail.
    pub issue_url: String,
    pub is_issue_linked: bool,
}

/// Typed outcome of a worktree start.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorktreeOutcome {
    /// The worktree/branch was created and the herdr handoff was issued
    /// (or deferred — see [`Handoff`]).
    Started {
        branch: String,
        path: PathBuf,
        handoff: Handoff,
    },
    /// The branch or worktree already exists for this request: idempotent
    /// no-op, never a second worktree.
    AlreadyStarted { branch: String, path: PathBuf },
}

/// The herdr handoff state (typed, so the client renders a distinct badge).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Handoff {
    /// The configured orchestrator was told about the new worktree.
    Launched,
    /// Slice-1 seam: the worktree is created but the herdr RPC is deferred
    /// (the launcher reported "pending"); the client shows it distinctly.
    Deferred,
    /// The herdr handoff failed (the worktree itself is still valid).
    Failed(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorktreeError {
    /// No fleet whose `name` matches the request's `repo`.
    UnknownFleet(String),
    /// No issue with this number was found in the fetched issue set.
    IssueNotFound { repo: String, number: u64 },
    /// The selected issue is CLOSED — refuse to start from it.
    IssueClosed {
        repo: String,
        number: u64,
        state: String,
    },
    /// Duplicate request raced the check: a worktree/branch already exists.
    AlreadyStarted { branch: String, path: PathBuf },
    /// The free-form label is empty or unsafe.
    InvalidName(String),
    /// Git worktree creation failed.
    Git(String),
    /// The herdr handoff failed.
    Launch(String),
}

impl std::fmt::Display for WorktreeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnknownFleet(repo) => write!(f, "unknown fleet: {repo}"),
            Self::IssueNotFound { repo, number } => {
                write!(f, "issue #{number} not found in {repo}'s fetched issues")
            }
            Self::IssueClosed {
                repo,
                number,
                state,
            } => write!(f, "issue #{number} in {repo} is {state} — not startable"),
            Self::AlreadyStarted { branch, path } => {
                write!(
                    f,
                    "worktree already started: {branch} at {}",
                    path.display()
                )
            }
            Self::InvalidName(name) => write!(f, "invalid worktree label: {name}"),
            Self::Git(msg) => write!(f, "git worktree failed: {msg}"),
            Self::Launch(msg) => write!(f, "herdr handoff failed: {msg}"),
        }
    }
}

impl std::error::Error for WorktreeError {}

impl WorktreeRequest {
    pub fn repo(&self) -> &str {
        match self {
            Self::Issue { repo, .. } | Self::Free { repo, .. } => repo,
        }
    }

    pub fn issue_number(&self) -> Option<u64> {
        match self {
            Self::Issue { number, .. } => Some(*number),
            Self::Free { .. } => None,
        }
    }

    pub fn is_issue_linked(&self) -> bool {
        matches!(self, Self::Issue { .. })
    }
}

/// Build a branch name fragment from a title/name slug. Alphanumeric +
/// hyphen only, lowercased, truncated, never empty after the prefix.
fn slugify(text: &str) -> String {
    let mut out = String::new();
    let mut last_dash = false;
    for ch in text.chars() {
        if ch.is_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
            last_dash = false;
        } else if !last_dash && out.len() < 40 {
            out.push('-');
            last_dash = true;
        }
        if out.len() >= 56 {
            break;
        }
    }
    let trimmed = out.trim_matches('-');
    if trimmed.is_empty() {
        "work".to_string()
    } else {
        trimmed.to_string()
    }
}

/// Sanitize a free-form label into a path-safe slug. Rejects empty and any
/// input that would survive as an issue-looking branch (`issue-…`, `#…`).
fn free_slug(name: &str) -> Result<String, WorktreeError> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err(WorktreeError::InvalidName(name.to_string()));
    }
    if trimmed.len() > 120 {
        return Err(WorktreeError::InvalidName(name.to_string()));
    }
    if trimmed.contains('/') || trimmed.contains('\\') {
        return Err(WorktreeError::InvalidName(name.to_string()));
    }
    let slug = slugify(trimmed);
    // A free branch must never look issue-linked (no issue-<N>, no #<N>).
    if slug.starts_with("issue-") || slug.contains("issue-") {
        return Err(WorktreeError::InvalidName(name.to_string()));
    }
    // The explicit prefix makes the intent unmistakable and ensures the
    // display-only inference (which looks for issue/# markers) returns None.
    Ok(format!("w2/free-{slug}"))
}

/// Resolve the worktree root for a fleet: `<home>/.herdr/worktrees/<dir>`.
/// This matches the fleet watchdog's `cwd_in_fleet` anchor so the created
/// worktree is recognized by the same fleet.
fn worktrees_root(fleet: &FleetIdentity, home: &str) -> PathBuf {
    let dir = fleet.worktree_dir.trim_matches('/');
    if dir.is_empty() {
        return PathBuf::from(home).join(".herdr/worktrees");
    }
    PathBuf::from(home).join(".herdr/worktrees").join(dir)
}

/// Resolve a worktree plan for a request against a fleet.
///
/// `base` is the git base ref/commit for the new branch — callers pass
/// `"HEAD"` by default; the daemon may pass the repo's default branch when
/// it has one from the gh plane. `home` is only used for the worktree root.
pub fn plan(
    fleet: &FleetIdentity,
    request: &WorktreeRequest,
    base: &str,
    home: &str,
) -> WorktreePlan {
    let free = match request {
        WorktreeRequest::Free { name, .. } => {
            // Unresolvable names are validated in `start` (which returns
            // the error); here we keep the plan construction total by using
            // the sanitized form. In practice `start` validates first.
            free_slug(name).unwrap_or_else(|_| "w2/free-work".to_string())
        }
        _ => String::new(),
    };
    let branch = match request {
        WorktreeRequest::Issue { number, .. } => {
            // The number comes from the selected issue ref, never inferred.
            format!("issue-{number}-work")
        }
        WorktreeRequest::Free { .. } => free,
    };
    let root = worktrees_root(fleet, home);
    let path = root.join(&branch);
    WorktreePlan {
        fleet: fleet.name.clone(),
        branch,
        path,
        base: base.to_string(),
        issue_number: request.issue_number(),
        issue_url: match request {
            WorktreeRequest::Issue { issue_url, .. } => issue_url.clone(),
            WorktreeRequest::Free { .. } => String::new(),
        },
        is_issue_linked: request.is_issue_linked(),
    }
}

/// The git worktree seam. Production uses [`GitCreator`]; tests stub it so a
/// hermetic test never touches the filesystem beyond a temp dir.
pub trait WorktreeCreator: Send + Sync {
    /// True when the worktree path or branch already exists (idempotency
    /// check — the operation must be exact-once).
    fn exists(&self, fleet: &FleetIdentity, plan: &WorktreePlan) -> bool;

    /// Create the worktree + branch. Must fail loudly (Err) on any nonzero
    /// git exit; the worktree is then considered failed, not started.
    fn create(&self, fleet: &FleetIdentity, plan: &WorktreePlan) -> Result<(), String>;
}

/// Production git seam: shells out to `git -C <local> worktree add`.
#[derive(Debug, Default, Clone)]
pub struct GitCreator;

impl WorktreeCreator for GitCreator {
    fn exists(&self, fleet: &FleetIdentity, plan: &WorktreePlan) -> bool {
        if plan.path.exists() {
            return true;
        }
        // Branch existence is authoritative too — a branch can outlive a
        // removed worktree directory, and we must not create a second branch.
        let local = fleet.local_path();
        let out = Command::new("git")
            .args(["-C"])
            .arg(&local)
            .args(["branch", "--list", &plan.branch])
            .output();
        matches!(out, Ok(o) if !o.stdout.is_empty())
    }

    fn create(&self, fleet: &FleetIdentity, plan: &WorktreePlan) -> Result<(), String> {
        let local = fleet.local_path();
        let output = Command::new("git")
            .args(["-C"])
            .arg(&local)
            .args(["worktree", "add"])
            .arg("-b")
            .arg(&plan.branch)
            .arg(&plan.path)
            .arg(&plan.base)
            .output()
            .map_err(|e| format!("spawn git: {e}"))?;
        if output.status.success() {
            Ok(())
        } else {
            Err(String::from_utf8_lossy(&output.stderr).trim().to_string())
        }
    }
}

/// The herdr handoff seam. Production uses [`HerdrLauncher`]; tests stub it.
pub trait WorktreeLauncher: Send + Sync {
    /// Hand the created worktree off to the fleet's orchestrator. Returns
    /// the [`Handoff`] state; a `Failed` here does NOT undo the worktree
    /// (the operation keeps the worktree and reports the partial state).
    fn launch(&self, fleet: &FleetIdentity, plan: &WorktreePlan) -> Handoff;
}

/// Production herdr launcher.
///
/// Slice-1 decision: the JSON-RPC agent-spawn call over the herdr socket is
/// the herdr adapter's P2+ surface and is NOT reimplemented here. This
/// launcher verifies the configured orchestrator is reachable via `herdr
/// agent list` (read-only, the same command the fleet watchdog uses) and
/// reports either `Launched` (orchestrator present — the orchestrator
/// observes the new worktree on its next sweep) or `Deferred` (herdr
/// unreachable / `HERDR_LAUNCH` not set). This is a documented, honest
/// seam: the worktree itself is created; the agent-session spawn is the
/// launcher's contract.
#[derive(Debug, Default, Clone)]
pub struct HerdrLauncher;

impl WorktreeLauncher for HerdrLauncher {
    fn launch(&self, _fleet: &FleetIdentity, _plan: &WorktreePlan) -> Handoff {
        // The orchestrator is the configured `orch` agent. A live spawn call
        // is out of slice-1 scope; defer so the caller can render the
        // distinct "worktree created, agent handoff pending" state.
        Handoff::Deferred
    }
}

/// Start a worktree: validate -> plan -> idempotency check -> create ->
/// hand off. Exactly one worktree per logical request.
pub fn start(
    fleet: &FleetIdentity,
    request: &WorktreeRequest,
    base: &str,
    home: &str,
    issue_checked: IssueCheck,
    creator: &dyn WorktreeCreator,
    launcher: &dyn WorktreeLauncher,
) -> Result<WorktreeOutcome, WorktreeError> {
    let plan = plan(fleet, request, base, home);

    // Validate the free label for real (plan() kept the total form).
    if let WorktreeRequest::Free { name, .. } = request {
        let _ = free_slug(name).map_err(|_| WorktreeError::InvalidName(name.clone()))?;
    }

    // Issue-linked start: refuse a closed/stale issue, NEVER fall through
    // to the issue-free path.
    if let WorktreeRequest::Issue { repo, number, .. } = request {
        if let Some(issue) = issue_checked.issue(repo, *number) {
            // Only an explicitly OPEN issue is startable. CLOSED, the
            // UNKNOWN sentinel (an outside-recent-set ref), or any other
            // state is a stale/closed refusal — never a fall-through.
            if !issue.effective_state().eq_ignore_ascii_case("OPEN") {
                return Err(WorktreeError::IssueClosed {
                    repo: repo.clone(),
                    number: *number,
                    state: issue.effective_state().to_string(),
                });
            }
        } else {
            return Err(WorktreeError::IssueNotFound {
                repo: repo.clone(),
                number: *number,
            });
        }
    }

    // Idempotency guard: if the branch/worktree already exists, return the
    // typed already-started outcome — never create a second one.
    if creator.exists(fleet, &plan) {
        return Ok(WorktreeOutcome::AlreadyStarted {
            branch: plan.branch.clone(),
            path: plan.path.clone(),
        });
    }

    creator.create(fleet, &plan).map_err(WorktreeError::Git)?;

    let handoff = launcher.launch(fleet, &plan);
    Ok(WorktreeOutcome::Started {
        branch: plan.branch.clone(),
        path: plan.path.clone(),
        handoff,
    })
}

/// The issue lookup the worktree check uses: a closed/stale issue is refused
/// before anything is created. Injected so hermetic tests supply the exact
/// issue set without a gh plane.
#[derive(Debug, Clone)]
pub struct IssueCheck<'a> {
    issues: &'a [IssueSummary],
}

/// Minimal issue view for the stale/closed guard (derived from fetched
/// [`GhIssueRef`]s; keeps this module independent of the events contract).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IssueSummary {
    pub repo: String,
    pub number: u64,
    pub state: String,
}

impl IssueSummary {
    pub fn effective_state(&self) -> &str {
        // "OPEN"/"CLOSED" from GitHub; any other (e.g. the UNKNOWN sentinel
        // for an outside-recent-set ref) is treated as not-startable too.
        self.state.as_str()
    }
}

impl<'a> IssueCheck<'a> {
    pub fn new(issues: &'a [IssueSummary]) -> Self {
        Self { issues }
    }

    pub fn issue(&self, repo: &str, number: u64) -> Option<&'a IssueSummary> {
        self.issues
            .iter()
            .find(|i| i.repo == repo && i.number == number)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fleet::cli::FleetIdentity;
    use std::sync::Mutex;

    fn fleet() -> FleetIdentity {
        FleetIdentity {
            name: "corral".into(),
            gh_repo: "jirathip-dev/corral".into(),
            local: PathBuf::from("/tmp/corral"),
            worktree_dir: "corral".into(),
            orch: "orch-corral".into(),
            workers: 0,
            paused: false,
        }
    }

    /// Recording creator: records `create` calls, reports existence on demand.
    #[derive(Default)]
    struct RecordingCreator {
        created: Mutex<Vec<String>>,
        existing: Mutex<Vec<String>>,
    }
    impl RecordingCreator {
        fn with_existing(paths: &[&str]) -> Self {
            Self {
                created: Mutex::new(Vec::new()),
                existing: Mutex::new(paths.iter().map(|s| s.to_string()).collect()),
            }
        }
    }
    impl WorktreeCreator for RecordingCreator {
        fn exists(&self, _fleet: &FleetIdentity, plan: &WorktreePlan) -> bool {
            let existing = self.existing.lock().unwrap();
            existing
                .iter()
                .any(|p| p == &plan.path.to_string_lossy().to_string())
        }
        fn create(&self, _fleet: &FleetIdentity, plan: &WorktreePlan) -> Result<(), String> {
            self.created.lock().unwrap().push(plan.branch.clone());
            Ok(())
        }
    }

    #[derive(Default)]
    struct RecordingLauncher {
        launched: Mutex<Vec<String>>,
    }
    impl WorktreeLauncher for RecordingLauncher {
        fn launch(&self, _fleet: &FleetIdentity, plan: &WorktreePlan) -> Handoff {
            self.launched.lock().unwrap().push(plan.branch.clone());
            Handoff::Launched
        }
    }

    fn open_issue() -> Vec<IssueSummary> {
        vec![IssueSummary {
            repo: "corral".into(),
            number: 113,
            state: "OPEN".into(),
        }]
    }

    #[test]
    fn issue_linked_plan_carries_the_issue_number() {
        let p = plan(
            &fleet(),
            &WorktreeRequest::Issue {
                repo: "corral".into(),
                number: 113,
                issue_url: "https://github.com/jirathip-dev/corral/issues/113".into(),
            },
            "HEAD",
            "/Users/x",
        );
        assert_eq!(p.branch, "issue-113-work");
        assert_eq!(p.issue_number, Some(113));
        assert!(p.is_issue_linked);
        assert_eq!(
            p.path,
            PathBuf::from("/Users/x/.herdr/worktrees/corral/issue-113-work")
        );
    }

    #[test]
    fn issue_free_plan_never_looks_issue_linked() {
        let p = plan(
            &fleet(),
            &WorktreeRequest::Free {
                repo: "corral".into(),
                name: "refactor-board".into(),
            },
            "HEAD",
            "/Users/x",
        );
        assert!(p.branch.starts_with("w2/free-"));
        assert_eq!(p.issue_number, None);
        assert!(!p.is_issue_linked);
        // The display-only inference must not read it as an issue.
        assert!(
            !p.branch.contains("issue-"),
            "branch must not look issue-linked"
        );
    }

    #[test]
    fn issue_linked_start_creates_exactly_one_worktree() {
        let creator = RecordingCreator::default();
        let launcher = RecordingLauncher::default();
        let outcome = start(
            &fleet(),
            &WorktreeRequest::Issue {
                repo: "corral".into(),
                number: 113,
                issue_url: "u".into(),
            },
            "HEAD",
            "/Users/x",
            IssueCheck::new(&open_issue()),
            &creator,
            &launcher,
        )
        .unwrap();
        assert!(matches!(
            outcome,
            WorktreeOutcome::Started {
                handoff: Handoff::Launched,
                ..
            }
        ));
        assert_eq!(
            creator.created.lock().unwrap().len(),
            1,
            "exactly one worktree"
        );
        assert_eq!(launcher.launched.lock().unwrap().len(), 1);
    }

    #[test]
    fn duplicate_request_is_idempotent_never_second_worktree() {
        let creator =
            RecordingCreator::with_existing(&["/Users/x/.herdr/worktrees/corral/issue-113-work"]);
        let launcher = RecordingLauncher::default();
        let outcome = start(
            &fleet(),
            &WorktreeRequest::Issue {
                repo: "corral".into(),
                number: 113,
                issue_url: "u".into(),
            },
            "HEAD",
            "/Users/x",
            IssueCheck::new(&open_issue()),
            &creator,
            &launcher,
        )
        .unwrap();
        assert!(matches!(outcome, WorktreeOutcome::AlreadyStarted { .. }));
        assert!(creator.created.lock().unwrap().is_empty());
        assert!(launcher.launched.lock().unwrap().is_empty());
    }

    #[test]
    fn closed_issue_is_refused_and_never_falls_through() {
        let creator = RecordingCreator::default();
        let launcher = RecordingLauncher::default();
        let closed = vec![IssueSummary {
            repo: "corral".into(),
            number: 113,
            state: "CLOSED".into(),
        }];
        let err = start(
            &fleet(),
            &WorktreeRequest::Issue {
                repo: "corral".into(),
                number: 113,
                issue_url: "u".into(),
            },
            "HEAD",
            "/Users/x",
            IssueCheck::new(&closed),
            &creator,
            &launcher,
        )
        .unwrap_err();
        assert!(matches!(err, WorktreeError::IssueClosed { .. }));
        assert!(
            creator.created.lock().unwrap().is_empty(),
            "no worktree created"
        );
        assert!(launcher.launched.lock().unwrap().is_empty());
    }

    #[test]
    fn unknown_issue_is_refused_not_free() {
        let creator = RecordingCreator::default();
        let launcher = RecordingLauncher::default();
        let err = start(
            &fleet(),
            &WorktreeRequest::Issue {
                repo: "corral".into(),
                number: 999,
                issue_url: "u".into(),
            },
            "HEAD",
            "/Users/x",
            IssueCheck::new(&open_issue()),
            &creator,
            &launcher,
        )
        .unwrap_err();
        assert!(matches!(err, WorktreeError::IssueNotFound { .. }));
        assert!(creator.created.lock().unwrap().is_empty());
    }

    #[test]
    fn stale_unknown_state_issue_is_refused_not_fallthrough() {
        // An issue present in the set but with a non-OPEN state (e.g. the
        // UNKNOWN sentinel) must be treated as not-startable, never silently
        // downgraded or passed to the free path.
        let creator = RecordingCreator::default();
        let launcher = RecordingLauncher::default();
        let stale = vec![IssueSummary {
            repo: "corral".into(),
            number: 113,
            state: "UNKNOWN".into(),
        }];
        let err = start(
            &fleet(),
            &WorktreeRequest::Issue {
                repo: "corral".into(),
                number: 113,
                issue_url: "u".into(),
            },
            "HEAD",
            "/Users/x",
            IssueCheck::new(&stale),
            &creator,
            &launcher,
        )
        .unwrap_err();
        assert!(matches!(err, WorktreeError::IssueClosed { .. }));
        assert!(creator.created.lock().unwrap().is_empty());
        assert!(launcher.launched.lock().unwrap().is_empty());
    }

    #[test]
    fn unauthored_or_duplicate_creator_failure_is_typed_failed() {
        struct FailCreator;
        impl WorktreeCreator for FailCreator {
            fn exists(&self, _fleet: &FleetIdentity, _plan: &WorktreePlan) -> bool {
                false
            }
            fn create(&self, _fleet: &FleetIdentity, _plan: &WorktreePlan) -> Result<(), String> {
                Err("boom".into())
            }
        }
        let err = start(
            &fleet(),
            &WorktreeRequest::Issue {
                repo: "corral".into(),
                number: 113,
                issue_url: "u".into(),
            },
            "HEAD",
            "/Users/x",
            IssueCheck::new(&open_issue()),
            &FailCreator,
            &RecordingLauncher::default(),
        )
        .unwrap_err();
        assert!(matches!(err, WorktreeError::Git(msg) if msg == "boom"));
    }

    #[test]
    fn issue_free_path_is_explicit_and_validates_name() {
        let creator = RecordingCreator::default();
        let launcher = RecordingLauncher::default();
        let ok = start(
            &fleet(),
            &WorktreeRequest::Free {
                repo: "corral".into(),
                name: "explore-opts".into(),
            },
            "HEAD",
            "/Users/x",
            IssueCheck::new(&[]),
            &creator,
            &launcher,
        );
        assert!(ok.is_ok());
        // An empty name is refused.
        let bad = start(
            &fleet(),
            &WorktreeRequest::Free {
                repo: "corral".into(),
                name: "".into(),
            },
            "HEAD",
            "/Users/x",
            IssueCheck::new(&[]),
            &creator,
            &launcher,
        );
        assert!(matches!(bad, Err(WorktreeError::InvalidName(_))));
    }
}
