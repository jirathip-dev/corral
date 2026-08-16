//! Plane integrator (WS3): drains the combined [`PlaneSink`] and folds every
//! git/gh fact onto the canonical agent records in the [`Store`].
//!
//! ## Mapping (facts are keyed by path/repo, never by agent id)
//!
//! - **Git facts** (keyed by `workspace.worktree_path`): a fact for a
//!   worktree updates EVERY agent whose `worktree_path` matches that path.
//!   `HeadMoved`/`CommitOnBranch` set `branch` (+ `repo` derived from the
//!   worktree path pattern `<worktrees_root>/<repo>/<label>`, or the repo
//!   root directory name for the main checkout), `head_sha` + `head_subject`
//!   (G21: the head commit the PR matcher resolves, carried onto the
//!   snapshot — `null` for unborn/empty checkouts); `DirtyChanged` sets
//!   `dirty` (index OR worktree dirty) and `ahead`/`behind`. The path's
//!   `<label>` component is a herdr worktree label, NOT branch identity —
//!   `branch` always comes from the git head fact, never from the path.
//! - **gh facts** (keyed by `workspace.repo`): a fact for a repo updates
//!   every agent whose `repo` matches. The agent's PR is the OPEN PR whose
//!   `head_sha` equals the agent's current HEAD commit (primary; commit
//!   identity is branch identity here — branch and commit come from the
//!   same git probe); after a head-SHA miss the agent's branch matches the
//!   PR's `headRefName` (the #22 fallback that binds committed-but-unpushed
//!   work); last, a still-open previously-bound PR survives head-SHA lag.
//!   `pr_number` + `ci_status` are set from that PR, `pr_match_source`
//!   records which match path won (debug-only), and `issues` carries the
//!   PR's authoritative closing-issue refs (#23).
//!   [`WorktreeAdded`](GitEvent::WorktreeAdded)/[`WorktreeRemoved`](GitEvent::WorktreeRemoved)
//!   are topology facts: no synthetic agents; a removal RE-APPLIES EMPTY
//!   facts (git-derived fields reset, PR binding dropped).
//!
//! ## Convergence (WS3 F1: not event-gated)
//!
//! Facts are CACHED by path/repo. Every event re-applies the FULL cached
//! fact set for that path/repo onto the currently-matching agents — and the
//! integrator also re-applies whenever the [`Store`] version signal advances
//! (any herdr upsert, removal, or the integrator's own merges). An agent
//! that appears after its worktree's facts were cached converges IMMEDIATELY,
//! with zero subsequent plane events. The signal is a `watch` channel, not a
//! broadcast receiver, so `subscriber_count` stays a true SSE measure and
//! the daemon adds no polling anywhere.
//!
//! ## Guards (WS3 hard gates)
//!
//! - **No broadcast receiver**: the integrator never calls
//!   [`Store::subscribe`]. Only [`Store::matching`]/[`Store::update_where`]/
//!   [`Store::changes`]/[`Store::get`] are used.
//! - **No second tick**: [`Store::matching`]/[`Store::update_where`] are
//!   non-flushing, so the existing 250ms/2s coalescer alone owns the rev.
//!   One monotonic rev covers all planes.
//! - **Atomic read-compare-apply** (WS3 F3): merges run through
//!   [`Store::update_where`] — predicate, merge, change-check and pending
//!   insert under one lock, so the integrator can never overwrite a fresher
//!   record (`ts`/`seq` never regress).
//! - **ci_status fidelity (WS2 policy, G4)**: `ci_status` is WS2's collapse
//!   verdict, mapped VERBATIM (`SUCCESS`→[`CiStatus::Success`] etc.). WS2
//!   policy lets recognized check items decide over the GitHub aggregate, so
//!   a PR can legitimately read SUCCESS while GitHub's aggregate shows
//!   FAILURE. The integrator must not distort that: the string maps 1:1.
//!
//! ## Application rules
//!
//! - A record is upserted only when the merged candidate differs from the
//!   current one, so unchanged facts cannot fabricate deltas.
//! - `pr_number`/`ci_status`/`issues` reset to `None`/empty only when the
//!   agent's bound PR leaves the repo's OPEN set; a still-open bound PR
//!   survives head-SHA lag (unpushed local commits, GitHub propagation)
//!   instead of flashing to `None` for a poll cycle.
//! - PR matching precedence (#22): head-SHA match first (unchanged
//!   semantics — pushed agents keep binding by commit identity), then the
//!   (repo, branch) fallback via the PR's `headRefName`, then the
//!   still-open bound PR. The winning path is noted on the record as
//!   `workspace.pr_match_source` (debug-only).
//! - `workspace.issues` is populated ONLY from the bound PR's authoritative
//!   `closingIssuesReferences` (#23) — no heuristic, no branch-name
//!   inference; empty when the PR links none or no PR is bound.
//! - Detached HEAD (the git plane reports the literal `"HEAD"`) normalizes
//!   to `branch: None` (F7).
//! - Paths are compared in canonical form when the raw spellings differ
//!   (F5), so a symlinked `HOME` cannot split the plane's canonical event
//!   paths from herdr's raw cwd paths.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use tokio::sync::mpsc;
use tracing::{debug, warn};

use crate::core::events::{GhRepoState, GitEvent, GitStatus, PlaneEvent};
use crate::core::model::{CiStatus, Workspace};
use crate::core::redact::redact;
use crate::core::store::Store;

/// Last-known git facts for one worktree path (the re-apply unit).
#[derive(Debug, Clone, Default)]
struct GitFacts {
    branch: Option<String>,
    commit: Option<String>,
    /// First line of the commit message, carried by the same probe that
    /// resolved `commit` (G21) — `None` when the commit has no message.
    subject: Option<String>,
    status: Option<GitStatus>,
}

impl GitFacts {
    fn head(&mut self, branch: String, commit: String, subject: Option<String>) {
        self.branch = Some(branch);
        self.commit = Some(commit);
        self.subject = subject;
    }
}

/// Consumes [`PlaneEvent`]s and folds them onto agent records in the store.
///
/// One task per daemon generation, draining the combined plane channel and
/// the store's change-version signal. Event-driven only: no timers, no
/// polling, no broadcast subscription.
pub struct Integrator {
    store: Store,
    /// Main checkout (repo root) — the path pattern's anchor.
    repo_root: PathBuf,
    /// Root of the herdr-managed worktrees — `<root>/<repo>/<label>`.
    worktrees_root: PathBuf,
    /// worktree path -> last-known git facts (path-keyed re-apply).
    git: Mutex<HashMap<PathBuf, GitFacts>>,
    /// repo name -> last-known gh state (repo-keyed re-apply).
    gh: Mutex<HashMap<String, GhRepoState>>,
}

impl Integrator {
    pub fn new(store: Store, repo_root: PathBuf, worktrees_root: PathBuf) -> Self {
        Self {
            store,
            repo_root,
            worktrees_root,
            git: Mutex::new(HashMap::new()),
            gh: Mutex::new(HashMap::new()),
        }
    }

    /// Drain the plane channel AND the store change signal until the channel
    /// closes. The daemon keeps the channel alive for the process's
    /// lifetime; the function returns when every plane drops its sink
    /// (shutdown) — the supervisor restarts it on any exit (F4).
    pub async fn run(self, mut rx: mpsc::Receiver<PlaneEvent>) {
        let mut changes = self.store.changes();
        loop {
            tokio::select! {
                event = rx.recv() => {
                    match event {
                        Some(event) => self.handle(event).await,
                        None => {
                            warn!("integrator: plane channel closed — no more facts will merge");
                            break;
                        }
                    }
                }
                // F1: any store change (e.g. a herdr agent upsert) triggers a
                // re-apply, so a new agent converges even with zero plane
                // events after its appearance.
                _ = changes.changed() => {
                    self.converge().await;
                }
            }
        }
    }

    async fn handle(&self, event: PlaneEvent) {
        match event {
            PlaneEvent::Git(event) => self.handle_git(event).await,
            PlaneEvent::Gh(state) => self.handle_gh(state).await,
        }
    }

    async fn handle_git(&self, event: GitEvent) {
        match event {
            GitEvent::HeadMoved { worktree, branch, commit, subject } => {
                self.git
                    .lock()
                    .unwrap()
                    .entry(worktree.clone())
                    .or_default()
                    .head(branch, commit, subject);
                self.converge().await;
            }
            GitEvent::CommitOnBranch { worktree, branch, commit, subject } => {
                // Wire-level distinction only: the read-model impact equals
                // HeadMoved's (branch + head commit facts).
                self.git
                    .lock()
                    .unwrap()
                    .entry(worktree.clone())
                    .or_default()
                    .head(branch, commit, subject);
                self.converge().await;
            }
            GitEvent::DirtyChanged { worktree, status } => {
                self.git
                    .lock()
                    .unwrap()
                    .entry(worktree.clone())
                    .or_default()
                    .status = Some(status);
                self.converge().await;
            }
            GitEvent::WorktreeAdded { worktree } => {
                // Topology fact, no read-model payload. Ensure the path is
                // cached so later partial facts have a landing spot; no agent
                // mapping and no synthetic agents.
                self.git.lock().unwrap().entry(worktree.clone()).or_default();
                debug!(worktree = %worktree.display(), "integrator: worktree added (topology only)");
            }
            GitEvent::WorktreeRemoved { worktree } => {
                // Drop the cached facts AND reset the matching agents' git-
                // derived fields + PR binding (F6): a removed worktree must
                // not leave records claiming a branch/pr of a nonexistent
                // worktree.
                self.git.lock().unwrap().remove(&worktree);
                self.reset_worktree(&worktree).await;
                debug!(worktree = %worktree.display(), "integrator: worktree removed (facts dropped, agents reset)");
            }
        }
    }

    async fn handle_gh(&self, state: GhRepoState) {
        let repo = state.repo.clone();
        {
            let mut gh = self.gh.lock().unwrap();
            gh.insert(repo.clone(), state.clone());
        }
        self.converge().await;
    }

    /// Re-apply every cached fact (git per path, gh per repo) plus the
    /// path-derived repo fallback onto the current agent set. Idempotent:
    /// records change only when the merged candidate differs.
    async fn converge(&self) {
        let (git, gh) = {
            let git = self.git.lock().unwrap();
            let gh = self.gh.lock().unwrap();
            (git.clone(), gh.clone())
        };
        for (path, facts) in &git {
            self.reapply_path(path, facts).await;
        }
        for (repo, state) in &gh {
            self.reapply_repo(repo, state).await;
        }
        // F2 fallback: derive the repo from the path pattern even when the
        // git plane has no facts for the path yet, so gh facts can still
        // bind fleet-wide by repo.
        let repo_root = self.repo_root.clone();
        let worktrees_root = self.worktrees_root.clone();
        self.store
            .update_where(|_| true, move |agent| {
                if let Some(path) = agent.workspace.worktree_path.as_deref()
                    && let Some(repo) = derive_repo(Path::new(path), &repo_root, &worktrees_root)
                {
                    agent.workspace.repo = Some(repo);
                }
            })
            .await;
    }

    /// Merge the FULL cached fact set for one worktree path into every agent
    /// whose `worktree_path` matches: branch/repo from the head, dirty/ahead/
    /// behind from the status, then the gh PR facts for the derived repo.
    async fn reapply_path(&self, path: &Path, facts: &GitFacts) {
        let repo = derive_repo(path, &self.repo_root, &self.worktrees_root);
        let gh_state = repo
            .as_ref()
            .and_then(|r| self.gh.lock().unwrap().get(r).cloned());
        let (path, facts, repo, gh_state) = (path.to_path_buf(), facts.clone(), repo, gh_state);
        let match_path = path.clone();
        self.store
            .update_where(
                |a| {
                    a.workspace.worktree_path.as_deref().is_some_and(|p| {
                        paths_match(Path::new(p), &match_path)
                    })
                },
                move |agent| {
                    let ws = &mut agent.workspace;
                    if let Some(repo) = &repo {
                        ws.repo = Some(repo.clone());
                    }
                    if let Some(branch) = &facts.branch {
                        // F7: detached HEAD reports the literal "HEAD" —
                        // never a branch; normalize to None.
                        //
                        // TODO(F2, W4 review follow-up): branch is a git-plane
                        // fact VALUE that serializes to /snapshot and SSE and
                        // currently bypasses the D9 redactor (a branch label
                        // like `test-ghp_…` would egress unredacted). Redact
                        // here at the integrate boundary — facts are keyed by
                        // path/repo, so matching is unaffected.
                        ws.branch = (branch != "HEAD").then(|| branch.clone());
                    }
                    // G21: the head commit the PR matcher already resolves is
                    // carried onto the snapshot (sha + first-line subject) —
                    // same probe, no extra git calls. The workspace head
                    // mirrors the cached facts exactly: `None` (null on the
                    // wire) for unborn/empty checkouts, whose probe never
                    // produces a head fact.
                    ws.head_sha = facts.commit.clone();
                    // D9: `head_subject` is DISPLAY text (arbitrary commit
                    // prose), so it goes through the same redaction pass as
                    // herdr pane text before it lands on the wire — a
                    // subject like `fix: rotate ghp_…` must not egress raw.
                    // `head_sha` stays raw: identity, not display (a sha
                    // fails rule 4 by design). Unlike `branch` (F2 TODO
                    // above), the subject is already redacted here, so the
                    // snapshot/SSE egress is covered at this boundary.
                    ws.head_subject = facts.subject.as_deref().map(|s| redact(s).into_owned());
                    if let Some(status) = &facts.status {
                        ws.dirty = status.is_dirty();
                        ws.ahead = status.ahead;
                        ws.behind = status.behind;
                    }
                    apply_pr_facts(ws, gh_state.as_ref(), &facts);
                },
            )
            .await;
    }

    /// Merge the gh PR facts for one repo into every agent whose `repo`
    /// matches, using the agent's current commit from the cached git facts.
    async fn reapply_repo(&self, repo: &str, state: &GhRepoState) {
        let git = self.git.lock().unwrap().clone();
        let (repo, state) = (repo.to_string(), state.clone());
        let match_repo = repo.clone();
        self.store
            .update_where(
                |a| a.workspace.repo.as_deref() == Some(match_repo.as_str()),
                move |agent| {
                    let facts = agent
                        .workspace
                        .worktree_path
                        .as_deref()
                        .and_then(|p| git.get(&canon_best_effort(Path::new(p))));
                    // No git facts for this agent's worktree yet: a PR cannot
                    // be matched by commit. Leave pr/ci untouched.
                    if let Some(facts) = facts {
                        apply_pr_facts(&mut agent.workspace, Some(&state), facts);
                    }
                },
            )
            .await;
    }

    /// F6: a removed worktree's agents lose their git-derived read-model
    /// fields and their PR binding (branch/dirty/ahead/behind/pr/ci/head/
    /// issues reset; `repo` stays — it is path-derived and re-derived by the
    /// fallback).
    async fn reset_worktree(&self, worktree: &Path) {
        let path = worktree.to_path_buf();
        self.store
            .update_where(
                |a| {
                    a.workspace
                        .worktree_path
                        .as_deref()
                        .is_some_and(|p| paths_match(Path::new(p), &path))
                },
                |agent| {
                    let ws = &mut agent.workspace;
                    ws.branch = None;
                    ws.dirty = false;
                    ws.ahead = 0;
                    ws.behind = 0;
                    ws.pr_number = None;
                    ws.ci_status = None;
                    ws.head_sha = None;
                    ws.head_subject = None;
                    ws.pr_match_source = None;
                    ws.issues = Vec::new();
                },
            )
            .await;
    }
}

/// Canonical repo name for a worktree path: the main checkout derives from
/// the repo root directory name; a herdr worktree from the first component
/// under the worktrees root (`<root>/<repo>/<label>`). Paths outside both
/// roots yield `None`.
fn derive_repo(worktree: &Path, repo_root: &Path, worktrees_root: &Path) -> Option<String> {
    if worktree == repo_root {
        return repo_root
            .file_name()
            .map(|name| name.to_string_lossy().into_owned());
    }
    let rel = worktree.strip_prefix(worktrees_root).ok()?;
    let first = rel.components().next()?;
    Some(first.as_os_str().to_string_lossy().into_owned())
}

/// Best-effort canonical form: real paths resolve symlinks (F5), synthetic
/// test paths fall back to their raw spelling.
fn canon_best_effort(p: &Path) -> PathBuf {
    std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf())
}

/// F5 path comparison: raw equality first (cheap, and identity on
/// non-symlinked installs), then canonical forms of both sides so a
/// symlinked `HOME` cannot split the plane's canonical event paths from
/// herdr's raw cwd paths.
fn paths_match(a: &Path, b: &Path) -> bool {
    a == b || canon_best_effort(a) == canon_best_effort(b)
}

/// Map the gh repo state onto the agent's PR fields.
///
/// Match precedence (#22):
/// 1. **head-SHA** — the OPEN PR whose `head_sha` equals the agent's current
///    HEAD commit (highest PR number wins ties). Unchanged semantics: pushed
///    agents keep binding by commit identity.
/// 2. **(repo, branch)** — after a head-SHA miss, the OPEN PR whose
///    `headRefName` equals the agent's branch. Binds committed-but-unpushed
///    work: the branch is pushed to GitHub, so its PR exists even though
///    the local HEAD commit is not the PR's head.
/// 3. **bound-PR survival** — the agent is already bound to a PR that is
///    still in the repo's OPEN set (head-SHA lag from GitHub propagation).
///
/// When none hold, the fields reset (the PR left the open set). The winning
/// path is recorded on `workspace.pr_match_source` (debug-only), and
/// `workspace.issues` mirrors the bound PR's authoritative
/// `closingIssuesReferences` (#23) — empty when the PR links none.
fn apply_pr_facts(ws: &mut Workspace, state: Option<&GhRepoState>, facts: &GitFacts) {
    let Some(state) = state else {
        return; // No gh facts for this repo yet: leave pr/ci/issues alone.
    };
    let commit = facts.commit.as_deref();
    // F7 normalization: the git plane reports detached HEAD as "HEAD" —
    // never a branch, so it must not match a PR's headRefName.
    let branch = facts.branch.as_deref().filter(|b| *b != "HEAD");
    let by_head = commit.and_then(|c| {
        state
            .prs
            .iter()
            .filter(|pr| pr.head_sha == c)
            .max_by_key(|pr| pr.pr_number)
    });
    let by_branch = branch.and_then(|b| {
        state
            .prs
            .iter()
            .filter(|pr| pr.head_branch == b)
            .max_by_key(|pr| pr.pr_number)
    });
    let bound = ws
        .pr_number
        .and_then(|n| state.prs.iter().find(|pr| pr.pr_number == n));
    let (pr, source) = by_head
        .map(|pr| (Some(pr), Some("head_sha")))
        .or_else(|| by_branch.map(|pr| (Some(pr), Some("branch"))))
        .or_else(|| bound.map(|pr| (Some(pr), Some("bound_pr"))))
        .unwrap_or((None, None));
    let Some(pr) = pr else {
        ws.pr_number = None;
        ws.ci_status = None;
        ws.pr_match_source = None;
        ws.issues = Vec::new();
        return;
    };
    ws.pr_number = Some(pr.pr_number);
    ws.ci_status = Some(map_ci(&pr.ci_status));
    ws.pr_match_source = source.map(str::to_string);
    ws.issues = pr.closing_issues.clone();
}

/// 1:1 mapping of the contract's canonical CI strings (WS2's collapse
/// verdict — see the module docs on G4). Anything unrecognized is
/// [`CiStatus::Unknown`], never a distortion.
fn map_ci(s: &str) -> CiStatus {
    match s {
        "SUCCESS" => CiStatus::Success,
        "FAILURE" => CiStatus::Failure,
        "PENDING" => CiStatus::Pending,
        _ => CiStatus::Unknown,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::events::GhIssueRef;
    use crate::core::events::GhPrState;
    use crate::core::model::{Agent, Change};

    fn agent_on(path: &str) -> Agent {
        Agent {
            agent_id: "herdr:a".to_string(),
            source: "herdr".to_string(),
            tool: "claude".to_string(),
            state: crate::core::model::AgentState::Idle,
            reason: None,
            seq: 1,
            ts: 1,
            capabilities: Vec::new(),
            waiting_on: None,
            cost: None,
            parent_id: None,
            host: None,
            workspace: Workspace {
                worktree_path: Some(path.to_string()),
                ..Default::default()
            },
            attachment: None,
            display_name: None,
            title: None,
        }
    }

    #[tokio::test]
    async fn reapply_path_merges_head_sha_and_subject_from_cached_facts() {
        // G21 acceptance 1+2: the head fields ride the cached git facts the
        // PR matcher already resolves — merged here with zero extra git.
        let store = Store::new();
        store.apply(Change::upsert(agent_on("/wt/a"))).await;
        let integrator = Integrator::new(store.clone(), PathBuf::from("/repo"), PathBuf::from("/wts"));

        let facts = GitFacts {
            branch: Some("feat/x".to_string()),
            commit: Some("abc123".to_string()),
            subject: Some("add head fields".to_string()),
            status: Some(GitStatus {
                dirty_index: true,
                ..Default::default()
            }),
        };
        integrator.reapply_path(Path::new("/wt/a"), &facts).await;

        let agents = store.matching(|_| true).await;
        let ws = &agents[0].workspace;
        assert_eq!(ws.head_sha.as_deref(), Some("abc123"));
        assert_eq!(ws.head_subject.as_deref(), Some("add head fields"));
        assert_eq!(ws.branch.as_deref(), Some("feat/x"));
        assert!(ws.dirty);

        // No head fact (unborn/empty checkout): fields stay null.
        integrator
            .reapply_path(
                Path::new("/wt/a"),
                &GitFacts {
                    branch: Some("feat/x".to_string()),
                    ..Default::default()
                },
            )
            .await;
        let agents = store.matching(|_| true).await;
        assert_eq!(agents[0].workspace.head_sha, None);
        assert_eq!(agents[0].workspace.head_subject, None);
    }

    #[tokio::test]
    async fn head_subject_is_redacted_before_landing_in_the_read_model() {
        // D9 (G21 re-review F1): a commit subject is display text — a
        // seeded secret in it must reach the snapshot redacted, while
        // head_sha (identity) stays raw.
        const GHP: &str = "ghp_AbCdEfGhIjKlMnOpQrStUvWxYz1234567890";
        let store = Store::new();
        store.apply(Change::upsert(agent_on("/wt/a"))).await;
        let integrator = Integrator::new(store.clone(), PathBuf::from("/repo"), PathBuf::from("/wts"));

        let facts = GitFacts {
            branch: Some("feat/x".to_string()),
            commit: Some("abc123".to_string()),
            subject: Some(format!("rotate the {GHP} token now")),
            ..Default::default()
        };
        integrator.reapply_path(Path::new("/wt/a"), &facts).await;

        let agents = store.matching(|_| true).await;
        let ws = &agents[0].workspace;
        assert_eq!(
            ws.head_subject.as_deref(),
            Some("rotate the [REDACTED] token now"),
            "the subject must egress redacted (F1)"
        );
        assert!(
            !ws.head_subject.as_deref().is_some_and(|s| s.contains(GHP)),
            "no raw PAT may reach the read model"
        );
        assert_eq!(ws.head_sha.as_deref(), Some("abc123"), "the sha is identity: stays raw");

        // Idempotent under re-apply: the second pass must not double-redact.
        integrator.reapply_path(Path::new("/wt/a"), &facts).await;
        let agents = store.matching(|_| true).await;
        assert_eq!(agents[0].workspace.head_subject.as_deref(), Some("rotate the [REDACTED] token now"));
    }

    #[tokio::test]
    async fn reset_worktree_clears_head_fields() {
        let store = Store::new();
        store.apply(Change::upsert(agent_on("/wt/a"))).await;
        let integrator = Integrator::new(store.clone(), PathBuf::from("/repo"), PathBuf::from("/wts"));
        let facts = GitFacts {
            commit: Some("abc123".to_string()),
            subject: Some("add head fields".to_string()),
            ..Default::default()
        };
        integrator.reapply_path(Path::new("/wt/a"), &facts).await;

        integrator.reset_worktree(Path::new("/wt/a")).await;
        let agents = store.matching(|_| true).await;
        let ws = &agents[0].workspace;
        assert_eq!(ws.head_sha, None, "removed worktree must drop its head facts");
        assert_eq!(ws.head_subject, None);
        assert_eq!(ws.branch, None);
    }

    #[test]
    fn derives_repo_from_both_path_forms() {
        let root = PathBuf::from("/Users/jirathip/Projects/herdr-board");
        let wts = PathBuf::from("/Users/jirathip/.herdr/worktrees");
        assert_eq!(
            derive_repo(&root, &root, &wts).as_deref(),
            Some("herdr-board"),
            "main checkout -> repo root directory name"
        );
        assert_eq!(
            derive_repo(&wts.join("project-hearthwild/feat-x"), &root, &wts).as_deref(),
            Some("project-hearthwild"),
            "herdr worktree -> first component under the worktrees root"
        );
        assert_eq!(
            derive_repo(&wts.join("synergy-costing/ws2/gh-plane"), &root, &wts).as_deref(),
            Some("synergy-costing"),
            "label components after the repo component are ignored"
        );
        assert_eq!(
            derive_repo(&wts.join("herdr-board/corral-p2-ws2"), &root, &wts).as_deref(),
            Some("herdr-board"),
            "herdr-board worktrees carry the repo as their first component"
        );
        assert_eq!(derive_repo(Path::new("/elsewhere/x"), &root, &wts), None);
    }

    #[test]
    fn maps_ci_strings_verbatim() {
        assert_eq!(map_ci("SUCCESS"), CiStatus::Success);
        assert_eq!(map_ci("FAILURE"), CiStatus::Failure);
        assert_eq!(map_ci("PENDING"), CiStatus::Pending);
        assert_eq!(map_ci("UNKNOWN"), CiStatus::Unknown);
        assert_eq!(map_ci("WEIRD"), CiStatus::Unknown);
    }

    fn pr(number: u64, sha: &str, head_branch: &str, ci: &str) -> GhPrState {
        GhPrState {
            repo: "herdr-board".to_string(),
            pr_number: number,
            title: String::new(),
            state: "OPEN".to_string(),
            mergeable: "MERGEABLE".to_string(),
            ci_status: ci.to_string(),
            head_sha: sha.to_string(),
            head_branch: head_branch.to_string(),
            closing_issues: Vec::new(),
        }
    }

    fn issue(number: u64, state: &str, title: &str) -> GhIssueRef {
        GhIssueRef {
            repo: "herdr-board".to_string(),
            number,
            state: state.to_string(),
            title: title.to_string(),
        }
    }

    #[test]
    fn pr_matching_prefers_head_sha_then_branch_then_bound_then_clears() {
        // --- Primary: head-SHA match (unchanged semantics, highest number
        // wins ties) — and it wins even when the branch ALSO matches a
        // different PR (pushed agents never regress to the fallback).
        let state = GhRepoState {
            repo: "herdr-board".to_string(),
            default_branch: "main".to_string(),
            prs: vec![
                pr(7, "abc123", "other/branch", "PENDING"),
                pr(42, "abc123", "ws2/gh-plane", "SUCCESS"),
                pr(9, "zzzzzz", "ws2/gh-plane", "FAILURE"),
            ],
            ..Default::default()
        };
        let facts = GitFacts {
            commit: Some("abc123".to_string()),
            branch: Some("ws2/gh-plane".to_string()),
            ..Default::default()
        };
        let mut ws = Workspace::default();
        apply_pr_facts(&mut ws, Some(&state), &facts);
        assert_eq!(ws.pr_number, Some(42), "highest open PR for the head commit wins");
        assert_eq!(ws.ci_status, Some(CiStatus::Success));
        assert_eq!(
            ws.pr_match_source.as_deref(),
            Some("head_sha"),
            "pushed agents bind by head-SHA first, never the fallback"
        );

        // --- #22 fallback: head moves locally (committed-but-unpushed), so
        // the head-SHA misses; the (repo, branch) match re-binds the SAME
        // PR whose headRefName equals the agent's branch — the badge never
        // goes blank, and the source note flips to "branch".
        let unpushed = GitFacts {
            commit: Some("def456".to_string()),
            branch: Some("ws2/gh-plane".to_string()),
            ..Default::default()
        };
        let mut ws = Workspace {
            pr_number: Some(42),
            ci_status: Some(CiStatus::Success),
            pr_match_source: Some("head_sha".to_string()),
            ..Default::default()
        };
        apply_pr_facts(&mut ws, Some(&state), &unpushed);
        assert_eq!(ws.pr_number, Some(42), "branch fallback keeps the committed-but-unpushed PR bound");
        assert_eq!(ws.pr_match_source.as_deref(), Some("branch"));

        // The branch fallback also beats a stale binding to a DIFFERENT PR.
        let mut ws = Workspace {
            pr_number: Some(7),
            ..Default::default()
        };
        apply_pr_facts(&mut ws, Some(&state), &unpushed);
        assert_eq!(ws.pr_number, Some(42), "a live branch match outranks the stale bound PR");
        assert_eq!(ws.pr_match_source.as_deref(), Some("branch"));

        // --- Bound-PR survival: neither SHA nor branch matches (e.g. the
        // branch was renamed locally); the still-open bound PR keeps the
        // binding instead of flashing to None.
        let renamed = GitFacts {
            commit: Some("def456".to_string()),
            branch: Some("renamed/branch".to_string()),
            ..Default::default()
        };
        let mut ws = Workspace {
            pr_number: Some(42),
            ci_status: Some(CiStatus::Success),
            pr_match_source: Some("head_sha".to_string()),
            ..Default::default()
        };
        apply_pr_facts(&mut ws, Some(&state), &renamed);
        assert_eq!(ws.pr_number, Some(42), "still-open bound PR survives head-SHA lag");
        assert_eq!(ws.ci_status, Some(CiStatus::Success));
        assert_eq!(ws.pr_match_source.as_deref(), Some("bound_pr"));

        // Detached HEAD ("HEAD") never matches a PR's headRefName literally.
        let detached = GitFacts {
            commit: Some("def456".to_string()),
            branch: Some("HEAD".to_string()),
            ..Default::default()
        };
        let mut ws = Workspace::default();
        apply_pr_facts(&mut ws, Some(&state), &detached);
        assert_eq!(ws.pr_number, None, "detached HEAD must not bind by branch");

        // --- The PR leaves the open set entirely -> fields reset.
        let empty = GhRepoState {
            repo: "herdr-board".to_string(),
            default_branch: "main".to_string(),
            prs: Vec::new(),
            ..Default::default()
        };
        let mut ws = Workspace {
            pr_number: Some(42),
            ci_status: Some(CiStatus::Success),
            pr_match_source: Some("bound_pr".to_string()),
            issues: vec![issue(4, "OPEN", "P2 planes")],
            ..Default::default()
        };
        apply_pr_facts(&mut ws, Some(&empty), &unpushed);
        assert_eq!(ws.pr_number, None);
        assert_eq!(ws.ci_status, None);
        assert_eq!(ws.pr_match_source, None);
        assert!(ws.issues.is_empty(), "issues reset with the binding");

        // --- No gh facts at all -> pr/ci/issues untouched.
        let mut ws = Workspace {
            pr_number: Some(42),
            ci_status: Some(CiStatus::Success),
            pr_match_source: Some("head_sha".to_string()),
            issues: vec![issue(4, "OPEN", "P2 planes")],
            ..Default::default()
        };
        apply_pr_facts(&mut ws, None, &unpushed);
        assert_eq!(ws.pr_number, Some(42));
        assert_eq!(ws.pr_match_source.as_deref(), Some("head_sha"));
        assert_eq!(ws.issues.len(), 1, "no gh facts -> the last-known binding is untouched");
    }

    #[test]
    fn issues_flow_from_the_bound_prs_closing_refs_only() {
        let mut with_issues = pr(42, "zzzzzz", "ws2/gh-plane", "SUCCESS");
        with_issues.closing_issues = vec![
            issue(4, "OPEN", "P2 planes"),
            issue(3, "UNKNOWN", "long-closed"),
        ];
        let state = GhRepoState {
            repo: "herdr-board".to_string(),
            default_branch: "main".to_string(),
            prs: vec![
                with_issues,
                pr(9, "zzzzzz", "other/branch", "PENDING"),
                pr(11, "abc123", "other/branch", "SUCCESS"),
            ],
            ..Default::default()
        };
        let facts = GitFacts {
            commit: Some("abc123".to_string()),
            branch: Some("ws2/gh-plane".to_string()),
            ..Default::default()
        };
        // #23: an agent bound by ANY path carries the bound PR's closing
        // refs — never the repo's recent issues (that would be a heuristic).
        let mut ws = Workspace::default();
        apply_pr_facts(&mut ws, Some(&state), &facts);
        assert_eq!(ws.pr_number, Some(11), "head-SHA primary wins (highest number)");
        assert_eq!(
            ws.issues,
            vec![],
            "PR 11 links no issues -> empty, even though the repo HAS issues"
        );

        let mut ws = Workspace::default();
        let unpushed = GitFacts {
            commit: Some("def456".to_string()),
            branch: Some("ws2/gh-plane".to_string()),
            ..Default::default()
        };
        apply_pr_facts(&mut ws, Some(&state), &unpushed);
        assert_eq!(ws.pr_number, Some(42), "branch fallback binds PR 42");
        assert_eq!(
            ws.issues,
            vec![issue(4, "OPEN", "P2 planes"), issue(3, "UNKNOWN", "long-closed")],
            "issues mirror the bound PR's authoritative closing refs"
        );
    }
}
