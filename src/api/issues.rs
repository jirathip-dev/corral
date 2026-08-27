//! #113 + #237: read-only repo-level issue view (`GET /issues`) — configless.
//!
//! The gh poller fetches issues per repo but the snapshot only carries
//! per-agent facts. This endpoint exposes the *repo-level* issue set the
//! poller most recently emitted (the same source the browser's issue action
//! validates a selected issue against), plus empty category entries for the
//! live agent `workspace.repo` union. Since #237 the category union is the
//! LIVE HDRD SNAPSHOT ONLY (no registry-derived basenames) and the fleet
//! identity keys are the fleet-ops CLI validated fleet names
//! ([`crate::fleet::cli::FleetOpsProvider`]) — corral never reads
//! `fleets.json`. Display repo categories are never actionable identities.
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
use tracing::warn;

use crate::core::events::GhIssueRef;

use super::repo::live_workspace_repos;
use super::repo::normalize_categories;

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
/// The map contains:
/// - live issue-cache keys (`issues_key`, the validated fleet name under
///   which the gh plane grouped them), and
/// - the live `workspace.repo` category union.
///
/// Empty arrays are informational placeholders. Neither kind authorizes a
/// write by itself: what matters is the capability grant and the worktree
/// dispatch's own fleet-ops CLI identity validation.
pub async fn issues(State(state): State<Arc<SuperState>>) -> Json<serde_json::Value> {
    let mut map = state.issues.snapshot();
    let live_repos = live_workspace_repos(&state.store).await;

    // Fleet identity keys from the fleet-ops CLI validated identity path
    // ONLY. Absent CLI (or an empty registry) leaves the map live-only; the
    // daemon still starts and renders without any fleets.json.
    match state.fleets.list() {
        Ok(identities) => {
            for identity in identities {
                map.entry(identity.name).or_default();
            }
        }
        Err(error) => {
            warn!(error = %error, "fleet-ops CLI identity path unavailable for /issues fleet keys");
        }
    }
    for repo in normalize_categories(live_repos) {
        map.entry(repo).or_default();
    }
    Json(serde_json::json!({ "repos": map }))
}

/// Thin alias so the handler signature stays readable (the full state type
/// lives in the parent module).
pub type SuperState = crate::api::AppState;
