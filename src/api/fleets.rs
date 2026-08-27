//! #237: fleet-ops CLI validated fleet identities (`GET /fleets`).
//!
//! Configless Corral never reads `fleets.json`. The board's actionable
//! fleet choices — the names a start-worktree / issue-free action may
//! target — come exclusively from the fleet-ops CLI validated identity path
//! ([`crate::fleet::cli::FleetOpsProvider`], production: `herdr-fleet
//! list`).
//!
//! This is deliberately NOT the old `GET /fleet-registry` projection: it
//! carries no corral-owned registry fields (no `local`, `worktree_dir`,
//! `models`, `reasoning_effort`, or an on-disk path). It is the validated
//! identity catalog only, and it disappears when the fleet-ops CLI is
//! unavailable — the board then renders live snapshot categories with no
//! actionable fleet choices, never a guessed identity. Display repo
//! categories (live `workspace.repo` values) are NEVER actionable; the
//! client must resolve an action target through this catalog and the
//! daemon re-validates every target.
//!
//! ## Auth scope
//!
//! Like `/issues`, this is a deliberate NON-AUTH read surface for loopback
//! or private/tailnet use: it exposes only validated fleet names and repo
//! slugs and never mutates anything. Do NOT expose the daemon on a public
//! interface.

use std::sync::Arc;

use axum::extract::State;
use axum::response::Json;
use serde::Serialize;

use super::AppState;

/// `GET /fleets` response: the fleet-ops CLI validated identity catalog.
#[derive(Debug, Serialize)]
pub struct FleetIdentitiesResponse {
    /// "ok" when the fleet-ops CLI identity path answered; "error" when it
    /// was unavailable (the CLI went away, a herdr-free machine, or a
    /// registry-less host). `fleets` is empty on error, never guessed.
    pub status: &'static str,
    pub error: Option<String>,
    pub fleets: Vec<FleetIdentityEntry>,
}

#[derive(Debug, Serialize)]
pub struct FleetIdentityEntry {
    pub name: String,
    pub gh_repo: String,
    pub orch: String,
    pub workers: usize,
    pub paused: bool,
}

/// `GET /fleets`: always HTTP 200; `status` distinguishes a live catalog
/// from a CLI-unavailable one.
pub(crate) async fn fleets(State(state): State<Arc<AppState>>) -> Json<FleetIdentitiesResponse> {
    match state.fleets.list() {
        Ok(identities) => Json(FleetIdentitiesResponse {
            status: "ok",
            error: None,
            fleets: identities
                .into_iter()
                .map(|identity| FleetIdentityEntry {
                    name: identity.name,
                    gh_repo: identity.gh_repo,
                    orch: identity.orch,
                    workers: identity.workers,
                    paused: identity.paused,
                })
                .collect(),
        }),
        Err(error) => Json(FleetIdentitiesResponse {
            status: "error",
            error: Some(error.to_string()),
            fleets: Vec::new(),
        }),
    }
}
