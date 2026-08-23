//! #135: read-only fleet registry view (`GET /fleet-registry`).
//!
//! The desktop board and the `/issues` fleet grouping must agree on the
//! configured fleets. This endpoint projects the SAME registry loader used
//! by `/issues` ([`crate::fleet::config::default_path`] + [`load`]) into the
//! board's read view. A parse, validation, or IO failure is still HTTP 200
//! with `status="error"` — a broken registry is visible in the UI, never
//! silently replaced by an empty list.
//!
//! ## Auth scope
//!
//! Like `/issues`, this is a deliberate NON-AUTH read surface for loopback
//! or private/tailnet use. It exposes only the local fleet configuration and
//! never mutates the registry or GitHub. Do NOT expose the daemon on a
//! public interface.

use axum::response::Json;
use serde::Serialize;
use serde_json::Value;

use crate::fleet::config::{self, Fleet};

/// `GET /fleet-registry` response: either a full projection of the loaded
/// registry or a prominent error with an empty fleet list.
#[derive(Debug, Serialize)]
pub struct FleetRegistryResponse {
    pub status: &'static str,
    pub path: String,
    pub error: Option<String>,
    pub fleets: Vec<FleetRegistryEntry>,
}

#[derive(Debug, Serialize)]
pub struct FleetRegistryEntry {
    pub name: String,
    pub gh_repo: String,
    pub local: String,
    pub worktree_dir: String,
    pub orch: String,
    pub workers: Vec<String>,
    pub paused: bool,
    pub models: FleetModels,
}

#[derive(Debug, Serialize)]
pub struct FleetModels {
    pub orch: String,
    #[serde(rename = "impl")]
    pub impl_: String,
    pub review: String,
    pub impl_alt: Option<String>,
    pub impl_alt2: Option<String>,
    pub reasoning_effort: Option<Value>,
}

/// `GET /fleet-registry`: the configured fleet registry projected for the
/// board. Always HTTP 200; `status` distinguishes a loaded registry from a
/// parse/IO/validation failure.
pub(crate) async fn fleet_registry() -> Json<FleetRegistryResponse> {
    let path = config::default_path();
    let response = match config::load(&path) {
        Ok(registry) => FleetRegistryResponse {
            status: "ok",
            path: path.display().to_string(),
            error: None,
            fleets: registry
                .fleets
                .iter()
                .map(|fleet| project_fleet(fleet, registry.reasoning_effort(&fleet.name)))
                .collect(),
        },
        Err(error) => FleetRegistryResponse {
            status: "error",
            path: path.display().to_string(),
            error: Some(error.to_string()),
            fleets: Vec::new(),
        },
    };
    Json(response)
}

fn project_fleet(fleet: &Fleet, reasoning_effort: Option<&Value>) -> FleetRegistryEntry {
    FleetRegistryEntry {
        name: fleet.name.clone(),
        gh_repo: fleet.gh_repo.clone(),
        local: fleet.local.clone(),
        worktree_dir: fleet.worktree_dir.clone(),
        orch: fleet.orch.clone(),
        workers: fleet.workers.clone(),
        paused: fleet.paused,
        models: FleetModels {
            orch: fleet.models.orch.clone(),
            impl_: fleet.models.impl_.clone(),
            review: fleet.models.review.clone(),
            impl_alt: fleet.models.impl_alt.clone(),
            impl_alt2: fleet.models.impl_alt2.clone(),
            reasoning_effort: reasoning_effort.cloned(),
        },
    }
}
