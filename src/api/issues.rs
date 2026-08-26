//! #113: read-only repo-level issue view (`GET /issues`).
//!
//! The gh poller fetches issues per repo but the snapshot only carries
//! per-agent facts. This endpoint exposes the *repo-level* issue set the
//! poller most recently emitted (the same source the browser's issue action
//! validates a selected issue against), plus empty category entries for the
//! live agent `workspace.repo` union and validated registry `gh_repo`
//! basenames. The desktop client can therefore render repo categories even
//! when the registry is absent, without touching the frozen snapshot
//! contract.
//!
//! ## Auth scope (review 2)
//!
//! `GET /issues` is a deliberate NON-AUTH read surface, like the existing
//! `/snapshot`, `/events`, and `/history` GETs: it exposes only the public
//! repo-level issue metadata the gh poller already fetches (number, state,
//! title, labels, url) and carries no per-agent transcript/tail content. It
//! is safe to serve only from a loopback or private/tailnet interface — do
//! NOT expose the daemon on a public interface. The write path
//! (`POST /drive <start_worktree>`) is separately capability-gated and
//! authorized; this GET never mutates GitHub.
//!
//! Read-only by construction: the cache is written ONLY by the integrator's
//! [`PlaneEvent::Gh`](crate::core::events::PlaneEvent::Gh) handler; there is
//! no mutation surface here.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use axum::extract::State;
use axum::response::Json;

use crate::core::events::GhIssueRef;
use crate::fleet::config;

use super::repo::live_workspace_repos;
use super::repo::union_with_registry;

/// The shared, last-known repo -> issues map. Cloned inside `Integrator` so
/// the handler can read whatever the gh plane most recently emitted.
#[derive(Debug, Default)]
pub struct IssuesCache(Mutex<BTreeMap<String, Vec<GhIssueRef>>>);

impl IssuesCache {
    pub fn update(&self, repo: &str, issues: Vec<GhIssueRef>) {
        self.0.lock().unwrap().insert(repo.to_string(), issues);
    }

    pub fn snapshot(&self) -> BTreeMap<String, Vec<GhIssueRef>> {
        self.0.lock().unwrap().clone()
    }

    /// The issue for `repo`/`number`, if currently tracked (used by the
    /// worktree action's stale/closed guard).
    pub fn get(&self, repo: &str, number: u64) -> Option<GhIssueRef> {
        self.0
            .lock()
            .unwrap()
            .get(repo)
            .and_then(|issues| issues.iter().find(|i| i.number == number).cloned())
    }
}

/// `GET /issues`: the last-known repo-level issues, grouped by repo.
///
/// Existing configured-fleet keys remain intact because the fleet-level
/// `start_worktree` guard addresses them by fleet name. Category-only keys
/// have empty issue arrays and are informational; they do not authorize a
/// write or bypass the registry-backed worktree operation.
pub async fn issues(State(state): State<Arc<SuperState>>) -> Json<serde_json::Value> {
    let mut map = state.issues.snapshot();
    let live_repos = live_workspace_repos(&state.store).await;
    let registry = config::load(&config::default_path());

    // #113 review 5: include EVERY configured fleet (even with zero fetched
    // issues) so the issue browser's explicit issue-free path is reachable
    // for a fleet whose issues have not been fetched yet or whose poll has
    // not produced a result. Keep those fleet-name keys for the write guard,
    // then add the canonical/live category union for the board read model.
    if let Ok(registry) = &registry {
        for fleet in &registry.fleets {
            map.entry(fleet.name.clone()).or_default();
        }
    } else if let Err(error) = &registry {
        tracing::warn!(error = %error, "fleet registry unavailable for /issues fleet list");
    }
    for repo in union_with_registry(live_repos, registry.as_ref().ok()) {
        map.entry(repo).or_default();
    }
    Json(serde_json::json!({ "repos": map }))
}

/// Thin alias so the handler signature stays readable (the full state type
/// lives in the parent module).
pub type SuperState = crate::api::AppState;
