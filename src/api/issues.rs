//! #113 + #237: read-only repo-level issue view (`GET /issues`) — configless.
//!
//! The gh poller fetches issues per repo but the snapshot only carries
//! per-agent facts. This endpoint exposes the *repo-level* issue set the
//! poller most recently emitted (the same source the browser's issue action
//! validates a selected issue against), plus empty category entries for the
//! live agent `workspace.repo` union. Since #237 the category union is the
//! LIVE HDRD SNAPSHOT ONLY (no registry-derived basenames) and the fleet
//! categories come only from the native issue and workspace caches.
//!
//! ## Auth scope (review 2)
//!
//! `GET /issues` is a deliberate NON-AUTH read surface, like the existing
//! `/snapshot`, `/events`, and `/history` GETs: it exposes only the public
//! repo-level issue metadata the gh poller already fetches (number, state,
//! title, labels, url) and carries no per-agent tail content. It
//! is safe to serve only from a loopback or private/tailnet interface — do
//! NOT expose the daemon on a public interface. The write path
//! (`POST /drive <start_worktree>`) is separately capability-gated and
//! authorized; this GET never mutates GitHub.
//!
//! Read-only by construction: the cache is written ONLY by the integrator's
//! [`PlaneEvent::Gh`](crate::core::events::PlaneEvent::Gh) handler. Since #237
//! the category set is the LIVE HERDR SNAPSHOT ONLY (no registry-derived
//! basenames); the integrator prunes categories when that topology changes and
//! the projection filters independently for a race-safe read.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};

use axum::extract::State;
use axum::response::Json;

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

    /// Remove issue categories that no longer belong to a live Herdr
    /// workspace. The integrator calls this when the store topology changes;
    /// the API view also filters independently so a stale read cannot leak.
    pub fn prune_to(&self, live_repos: &BTreeSet<String>) {
        self.0
            .lock()
            .unwrap()
            .retain(|repo, _| live_repos.contains(repo));
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
/// - live issue-cache keys (the native `workspace.repo` identity under which
///   the gh plane grouped them), and
/// - the live Herdr `workspace.repo` category set.
///
/// Empty arrays are informational placeholders. Neither kind authorizes a
/// write by itself: write paths perform their own capability and identity
/// validation.
pub async fn issues(State(state): State<Arc<SuperState>>) -> Json<serde_json::Value> {
    Json(issues_view(&state).await)
}

/// The shared read-only issues view (#267): built once and served by BOTH
/// the unauthenticated `GET /issues` (egui board parity) and the
/// grant-gated `/drive read_issues` arm (iOS browser) — one builder so the
/// two surfaces can never diverge on what the iOS client sees.
pub async fn issues_view(state: &SuperState) -> serde_json::Value {
    let mut map = state.issues.snapshot();
    let live_repos = normalize_categories(live_workspace_repos(&state.store).await);
    map.retain(|repo, _| live_repos.contains(repo));

    for repo in live_repos {
        map.entry(repo).or_default();
    }
    serde_json::json!({ "repos": map })
}

/// Thin alias so the handler signature stays readable (the full state type
/// lives in the parent module).
pub type SuperState = crate::api::AppState;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::model::{Agent, AgentState, Workspace};

    #[tokio::test]
    async fn issues_view_excludes_cached_categories_without_live_herdr_workspace() {
        let state = SuperState::default();
        state
            .store
            .apply(crate::core::model::Change::upsert(Agent {
                agent_id: "herdr-live".to_string(),
                source: "herdr".to_string(),
                tool: "fixture".to_string(),
                state: AgentState::Idle,
                reason: None,
                seq: 1,
                ts: 0,
                capabilities: Vec::new(),
                waiting_on: None,
                parent_id: None,
                host: None,
                workspace: Workspace {
                    repo: Some("in-scope".to_string()),
                    worktree_path: Some("/herdr/in-scope".to_string()),
                    ..Default::default()
                },
                attachment: None,
                display_name: None,
                title: None,
            }))
            .await;
        state.issues.update("in-scope", Vec::new());
        state.issues.update("unrelated", Vec::new());

        let view = issues_view(&state).await;
        let repos = view["repos"].as_object().expect("repos object");
        assert_eq!(repos.keys().collect::<Vec<_>>(), vec!["in-scope"]);
    }
}
