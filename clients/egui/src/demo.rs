//! Bundled demo fixture (#215): a representative corral snapshot, a short
//! canned SSE delta sequence, and the issue/fleet projections, so the
//! read-only WASM build renders a believable fleet board out of the box —
//! no daemon, no network.
//!
//! The fixture is embedded at compile time (`include_str!`), so the demo
//! works on a plain GitHub Pages static deployment. Bump the JSON when the
//! local daemon's wire shapes change (it mirrors `crate::model`).

use std::collections::BTreeMap;

use crate::model::{Delta, FleetIdentities, GhIssueRef, Snapshot};

/// Everything the demo mode needs to render + animate the board.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct DemoData {
    pub snapshot: Snapshot,
    /// Canned SSE delta frames applied one every few seconds, wrapped
    /// (revs strictly increase before the wrap so `Fleet::apply_delta`
    /// accepts every frame).
    pub deltas: Vec<Delta>,
    /// Demo repo-level issue view (`GET /issues` shape).
    pub issues: BTreeMap<String, Vec<GhIssueRef>>,
    /// Demo fleet identity catalog (`GET /fleets` shape).
    pub fleets: FleetIdentities,
}

const FIXTURE: &str = include_str!("../assets/demo-fixture.json");

/// Parse the embedded fixture. A failing parse is a build-time defect in
/// this crate, so this may panic (embedded data can never change at
/// runtime).
pub fn load() -> DemoData {
    serde_json::from_str(FIXTURE).expect("embedded demo fixture is valid")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixture_decodes_and_delta_revs_follow_the_snapshot() {
        let demo = load();
        assert_eq!(demo.snapshot.schema_version, crate::model::SCHEMA_VERSION);
        assert!(
            demo.snapshot.agents.len() >= 5,
            "the demo board should show a representative fleet"
        );
        assert!(!demo.deltas.is_empty(), "canned SSE frames are required");
        let mut rev = demo.snapshot.rev;
        for delta in &demo.deltas {
            assert!(
                delta.rev > rev,
                "delta revs must strictly increase from the snapshot"
            );
            rev = delta.rev;
        }
        assert_eq!(demo.fleets.status, "ok");
        assert!(!demo.issues.is_empty());

        // The fixture must actually fold into the view model (the demo
        // board is fed through the same path as live SSE).
        let mut fleet = crate::state::Fleet::default();
        fleet.apply_snapshot(&demo.snapshot);
        fleet.apply_delta(&demo.deltas[0]);
        assert!(fleet.rev.unwrap() > demo.snapshot.rev);
        assert_eq!(fleet.agents.len(), demo.snapshot.agents.len());
        fleet.set_issues(Ok(demo.issues.clone()));
        fleet.set_fleets(Ok(demo.fleets.clone()));
        assert!(fleet.issues_loaded);
        assert!(fleet.fleets_ready());
    }
}
