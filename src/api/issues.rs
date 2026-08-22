//! #113: read-only repo-level issue view (`GET /issues`).
//!
//! The gh poller fetches issues per repo but the snapshot only carries
//! per-agent facts. This endpoint exposes the *repo-level* issue set the
//! poller most recently emitted (the same source the browser's issue action
//! validates a selected issue against), so the desktop client can render an
//! issue browser without a GitHub token and without touching the frozen
//! snapshot contract.
//!
//! Read-only by construction: the cache is written ONLY by the integrator's
//! [`PlaneEvent::Gh`](crate::core::events::PlaneEvent::Gh) handler; there is
//! no mutation surface here.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use axum::extract::State;
use axum::response::Json;

use crate::core::events::GhIssueRef;

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
pub async fn issues(State(state): State<Arc<SuperState>>) -> Json<serde_json::Value> {
    let map = state.issues.snapshot();
    Json(serde_json::json!({ "repos": map }))
}

/// Thin alias so the handler signature stays readable (the full state type
/// lives in the parent module).
pub type SuperState = crate::api::AppState;
