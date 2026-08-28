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
    let mut data: DemoData = serde_json::from_str(FIXTURE).expect("embedded demo fixture is valid");
    // #210: the fixture's heartbeat anchors are static; the demo board
    // must not read as a stale lane, so re-anchor them to "a few seconds
    // ago" from the wall clock (demo-only sugar, never production data).
    let now = demo_now_epoch_ms();
    for (i, row) in data.snapshot.fleet_health.iter_mut().enumerate() {
        if row.last_heartbeat.is_some() {
            row.last_heartbeat = Some(now.saturating_sub((4 + i as u64 * 8) * 1000));
        }
    }
    data
}

/// Glyph-rich transcript blocks for the static board evidence. The same
/// daemon scrubber is applied here so demo output exercises the shared tofu
/// contract without changing live read-tail behavior.
pub fn recent_tail() -> Vec<String> {
    [
        "› make the transcript readable",
        "I grouped the latest work by speaker and kept the live tail bounded.",
        "$ python deploy.py --dry-run\ndef deploy():\n    print(\"ready ✅\")\n    return True",
        "git diff -- src/ui/board.rs\n@@ -1,2 +1,3 @@\n-old label\n+speaker rail\n+tool output",
        "tool status: ok \u{e000} ⚠️",
    ]
    .into_iter()
    .map(scrub_demo_icons)
    .collect()
}

fn scrub_demo_icons(line: &str) -> String {
    let mut output = String::with_capacity(line.len());
    let mut replaced = false;
    for character in line.chars() {
        let private_use = ('\u{e000}'..='\u{f8ff}').contains(&character)
            || ('\u{f0000}'..='\u{ffffd}').contains(&character)
            || ('\u{100000}'..='\u{10fffd}').contains(&character);
        if private_use {
            if !replaced {
                output.push_str("[icon]");
                replaced = true;
            }
        } else {
            replaced = false;
            output.push(character);
        }
    }
    output
}
/// Wall-clock epoch millis that also works on wasm32-unknown-unknown —
/// `std::time::SystemTime` panics there ("time not implemented"), so the
/// web build uses `js_sys::Date::now()` (epoch), never the monotonic
/// `performance.now()` (page-relative, useless for epoch math).
fn demo_now_epoch_ms() -> u64 {
    #[cfg(target_arch = "wasm32")]
    {
        js_sys::Date::now() as u64
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_millis() as u64)
            .unwrap_or(0)
    }
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
            "fixture snapshot carries a believable fleet"
        );
        // #210: the demo fixture carries pre-aggregated fleet-health rows so
        // the wasm demo renders the strip with no daemon anywhere.
        assert_eq!(
            demo.snapshot.fleet_health.len(),
            3,
            "fixture carries the fleet-health strip (healthy/degraded/paused)"
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
        assert!(
            demo.issues
                .values()
                .flatten()
                .any(|issue| issue.body.is_some()),
            "fixture carries an expandable issue body"
        );

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
