//! Plane integrator (WS3): drains the combined [`PlaneSink`] and folds every
//! git/gh fact onto the canonical agent records in the [`Store`].
//!
//! ## Mapping (facts are keyed by path/repo, never by agent id)
//!
//! - **Git facts** (keyed by `workspace.worktree_path`): a fact for a
//!   worktree updates EVERY agent whose `worktree_path` matches that path.
//!   `HeadMoved`/`CommitOnBranch` set `branch` (+ `repo` derived from the
//!   worktree path pattern `<worktrees_root>/<repo>/<label>`, or the repo
//!   root directory name for the main checkout); `DirtyChanged` sets
//!   `dirty` (index OR worktree dirty) and `ahead`/`behind`. The path's
//!   `<label>` component is a herdr worktree label, NOT branch identity —
//!   `branch` always comes from the git head fact, never from the path.
//! - **gh facts** (keyed by `workspace.repo`): a fact for a repo updates
//!   every agent whose `repo` matches. The agent's PR is the OPEN PR whose
//!   `head_sha` equals the agent's current HEAD commit (the contract's
//!   [`GhPrState`] carries no branch name — only the head oid — so commit
//!   identity is the branch identity here: branch and commit come from the
//!   same git probe). `pr_number` + `ci_status` are set from that PR.
//!   [`WorktreeAdded`](GitEvent::WorktreeAdded)/[`WorktreeRemoved`](GitEvent::WorktreeRemoved)
//!   are topology facts: no agent mapping, no synthetic agents.
//!
//! ## Convergence ("unknown-path events")
//!
//! Facts are CACHED by path/repo. Every event re-applies the FULL cached
//! fact set for that path/repo onto the currently-matching agents, so an
//! agent that appears after its worktree's facts were cached converges on
//! the next event touching that path/repo. The daemon never polls for this:
//! the git plane's 10s sweep and the gh plane's polls are the only re-read
//! sources, and the integrator itself is pure push (`channel.recv`).
//!
//! ## Guards (WS3 hard gates)
//!
//! - **No broadcast receiver**: the integrator never calls
//!   [`Store::subscribe`]. [`Store::subscriber_count`] must stay a true
//!   measure of live SSE connections or the gh plane's cadence rule breaks;
//!   only `Store::matching`/`get`/`apply` are used.
//! - **No second tick**: [`Store::matching`] is deliberately non-flushing,
//!   so the existing 250ms/2s coalescer alone owns the rev. One monotonic
//!   rev covers all planes.
//! - **ci_status fidelity (WS2 policy, G4)**: `ci_status` is WS2's collapse
//!   verdict, mapped VERBATIM (`SUCCESS`→[`CiStatus::Success`] etc.). WS2
//!   policy lets recognized check items decide over the GitHub aggregate, so
//!   a PR can legitimately read SUCCESS while GitHub's aggregate shows
//!   FAILURE. The integrator must not distort that: the string maps 1:1.
//!
//! ## Application rules
//!
//! - A record is upserted only when the merged candidate differs from the
//!   current one (`ts`/`seq` are herdr-owned and left untouched), so
//!   unchanged facts cannot fabricate deltas.
//! - `pr_number`/`ci_status` reset to `None` only when the agent's bound PR
//!   leaves the repo's OPEN set; a still-open bound PR survives head-SHA lag
//!   (unpushed local commits, GitHub propagation) instead of flashing to
//!   `None` for a poll cycle.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use tokio::sync::mpsc;
use tracing::{debug, warn};

use crate::core::events::{GhRepoState, GitEvent, GitStatus, PlaneEvent};
use crate::core::model::{Change, CiStatus, Workspace};
use crate::core::store::Store;

/// Last-known git facts for one worktree path (the re-apply unit).
#[derive(Debug, Clone, Default)]
struct GitFacts {
    branch: Option<String>,
    commit: Option<String>,
    status: Option<GitStatus>,
}

impl GitFacts {
    fn head(&mut self, branch: String, commit: String) {
        self.branch = Some(branch);
        self.commit = Some(commit);
    }
}

/// Consumes [`PlaneEvent`]s and folds them onto agent records in the store.
///
/// One task per daemon, draining the combined plane channel. Event-driven
/// only: no timers, no polling, no broadcast subscription.
pub struct Integrator {
    store: Store,
    /// Main checkout (repo root) — the path pattern's anchor.
    repo_root: PathBuf,
    /// Root of the herdr-managed worktrees — `<root>/<repo>/<branch>`.
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

    /// Drain the plane channel until it closes. The daemon keeps it alive for
    /// the process's lifetime; the function returns when every plane drops
    /// its sink (shutdown).
    pub async fn run(self, mut rx: mpsc::Receiver<PlaneEvent>) {
        while let Some(event) = rx.recv().await {
            self.handle(event).await;
        }
        warn!("integrator: plane channel closed — no more facts will merge");
    }

    async fn handle(&self, event: PlaneEvent) {
        match event {
            PlaneEvent::Git(event) => self.handle_git(event).await,
            PlaneEvent::Gh(state) => self.handle_gh(state).await,
        }
    }

    async fn handle_git(&self, event: GitEvent) {
        match event {
            GitEvent::HeadMoved { worktree, branch, commit } => {
                self.git
                    .lock()
                    .unwrap()
                    .entry(worktree.clone())
                    .or_default()
                    .head(branch, commit);
                self.reapply_worktree(&worktree).await;
            }
            GitEvent::CommitOnBranch { worktree, branch, commit } => {
                // Wire-level distinction only: the read-model impact equals
                // HeadMoved's (branch + head commit facts).
                self.git
                    .lock()
                    .unwrap()
                    .entry(worktree.clone())
                    .or_default()
                    .head(branch, commit);
                self.reapply_worktree(&worktree).await;
            }
            GitEvent::DirtyChanged { worktree, status } => {
                self.git
                    .lock()
                    .unwrap()
                    .entry(worktree.clone())
                    .or_default()
                    .status = Some(status);
                self.reapply_worktree(&worktree).await;
            }
            GitEvent::WorktreeAdded { worktree } => {
                // Topology fact, no read-model payload. Ensure the path is
                // cached so later partial facts have a landing spot; no agent
                // mapping and no synthetic agents.
                self.git.lock().unwrap().entry(worktree.clone()).or_default();
                debug!(worktree = %worktree.display(), "integrator: worktree added (topology only)");
            }
            GitEvent::WorktreeRemoved { worktree } => {
                // Drop the cached facts so a removed worktree can never feed
                // stale PR matches; the agent records themselves are the
                // herdr adapter's to clean up.
                self.git.lock().unwrap().remove(&worktree);
                debug!(worktree = %worktree.display(), "integrator: worktree removed (facts dropped)");
            }
        }
    }

    async fn handle_gh(&self, state: GhRepoState) {
        let repo = state.repo.clone();
        {
            let mut gh = self.gh.lock().unwrap();
            gh.insert(repo.clone(), state.clone());
        }
        let agents = self
            .store
            .matching(|a| a.workspace.repo.as_deref() == Some(repo.as_str()))
            .await;
        for agent in agents {
            let mut candidate = agent.clone();
            let facts = agent
                .workspace
                .worktree_path
                .as_deref()
                .and_then(|p| self.git.lock().unwrap().get(Path::new(p)).cloned());
            // No git facts for this agent's worktree yet: a PR cannot be
            // matched by commit. Leave pr/ci untouched — the path-keyed
            // re-apply on the next git event settles them.
            if let Some(facts) = facts {
                apply_pr_facts(&mut candidate.workspace, Some(&state), &facts);
            }
            if candidate != agent {
                self.store.apply(Change::upsert(candidate)).await;
            }
        }
    }

    /// Re-apply the FULL cached fact set for one worktree path onto every
    /// agent whose `worktree_path` matches: branch/repo from the head,
    /// dirty/ahead/behind from the status, then the gh PR facts for the
    /// derived repo. This is the convergence path — an agent that appeared
    /// after the facts were cached picks them up on the next event for the
    /// path.
    async fn reapply_worktree(&self, worktree: &Path) {
        let repo = derive_repo(worktree, &self.repo_root, &self.worktrees_root);
        let (facts, gh_state) = {
            let git = self.git.lock().unwrap();
            let facts = git.get(worktree).cloned();
            let gh_state = repo
                .as_ref()
                .and_then(|r| self.gh.lock().unwrap().get(r).cloned());
            (facts, gh_state)
        };
        let target = worktree.to_string_lossy().into_owned();
        let agents = self
            .store
            .matching(|a| a.workspace.worktree_path.as_deref() == Some(target.as_str()))
            .await;
        for agent in agents {
            let mut candidate = agent.clone();
            let ws = &mut candidate.workspace;
            if let Some(repo) = &repo {
                ws.repo = Some(repo.clone());
            }
            if let Some(facts) = &facts {
                if let Some(branch) = &facts.branch {
                    ws.branch = Some(branch.clone());
                }
                if let Some(status) = &facts.status {
                    ws.dirty = status.is_dirty();
                    ws.ahead = status.ahead;
                    ws.behind = status.behind;
                }
                apply_pr_facts(ws, gh_state.as_ref(), facts);
            }
            if candidate != agent {
                self.store.apply(Change::upsert(candidate)).await;
            }
        }
    }
}

/// Canonical repo name for a worktree path: the main checkout derives from
/// the repo root directory name; a herdr worktree from the first component
/// under the worktrees root (`<root>/<repo>/<label>`). Paths outside both
/// roots yield `None` (the git plane never watches them).
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

/// Map the gh repo state onto the agent's PR fields.
///
/// Primary match: the OPEN PR whose `head_sha` equals the agent's current
/// HEAD commit (highest PR number wins ties). Fallback: the agent is already
/// bound to a PR that is still in the repo's OPEN set — head-SHA lag (a
/// local commit not yet pushed, or GitHub propagation) must not flash
/// `pr_number`/`ci_status` to `None` for a poll cycle. When neither holds,
/// the fields reset to `None` (the PR left the open set).
fn apply_pr_facts(ws: &mut Workspace, state: Option<&GhRepoState>, facts: &GitFacts) {
    let Some(state) = state else {
        return; // No gh facts for this repo yet: leave pr/ci alone.
    };
    let commit = facts.commit.as_deref();
    let pr = state
        .prs
        .iter()
        .filter(|pr| commit.is_some_and(|c| pr.head_sha == c))
        .max_by_key(|pr| pr.pr_number)
        .or_else(|| {
            ws.pr_number.and_then(|n| state.prs.iter().find(|pr| pr.pr_number == n))
        });
    let Some(pr) = pr else {
        ws.pr_number = None;
        ws.ci_status = None;
        return;
    };
    ws.pr_number = Some(pr.pr_number);
    ws.ci_status = Some(map_ci(&pr.ci_status));
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
    use crate::core::events::GhPrState;

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

    #[test]
    fn pr_matching_prefers_head_sha_then_survives_lag_then_clears() {
        let pr = |number: u64, sha: &str, ci: &str| GhPrState {
            repo: "herdr-board".to_string(),
            pr_number: number,
            title: String::new(),
            state: "OPEN".to_string(),
            mergeable: "MERGEABLE".to_string(),
            ci_status: ci.to_string(),
            head_sha: sha.to_string(),
        };
        let state = GhRepoState {
            repo: "herdr-board".to_string(),
            default_branch: "main".to_string(),
            prs: vec![pr(7, "abc123", "PENDING"), pr(42, "abc123", "SUCCESS")],
            ..Default::default()
        };
        let facts = GitFacts {
            commit: Some("abc123".to_string()),
            ..Default::default()
        };
        let mut ws = Workspace::default();
        apply_pr_facts(&mut ws, Some(&state), &facts);
        assert_eq!(ws.pr_number, Some(42), "highest open PR for the head commit wins");
        assert_eq!(ws.ci_status, Some(CiStatus::Success));

        // Head moves locally (unpushed): PR 42 is still open, so the bound
        // PR survives the SHA mismatch instead of flashing to None.
        let moved = GitFacts {
            commit: Some("def456".to_string()),
            ..Default::default()
        };
        let mut ws = Workspace {
            pr_number: Some(42),
            ci_status: Some(CiStatus::Success),
            ..Default::default()
        };
        apply_pr_facts(&mut ws, Some(&state), &moved);
        assert_eq!(ws.pr_number, Some(42), "still-open bound PR survives head-SHA lag");
        assert_eq!(ws.ci_status, Some(CiStatus::Success));

        // The PR leaves the open set entirely -> fields reset.
        let empty = GhRepoState {
            repo: "herdr-board".to_string(),
            default_branch: "main".to_string(),
            prs: Vec::new(),
            ..Default::default()
        };
        apply_pr_facts(&mut ws, Some(&empty), &moved);
        assert_eq!(ws.pr_number, None);
        assert_eq!(ws.ci_status, None);

        // No gh facts at all -> pr/ci untouched.
        apply_pr_facts(&mut ws, None, &moved);
        assert_eq!(ws.pr_number, None);
    }
}
