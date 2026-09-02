//! Bundled demo fixture (#215): a representative corral snapshot, a short
//! canned SSE delta sequence, and the issue/fleet projections, so the
//! read-only WASM build renders a believable fleet board out of the box —
//! no daemon, no network.
//!
//! The fixture is embedded at compile time (`include_str!`), so the demo
//! works on a plain GitHub Pages static deployment. Bump the JSON when the
//! local daemon's wire shapes change (it mirrors `crate::model`).

use std::collections::BTreeMap;

use crate::model::{Delta, GhIssueRef, Snapshot};

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
    /// Sanitized transcript lines served through the normal read-tail cache.
    pub recent_output: Vec<String>,
    /// #316 V3: canonical semantic blocks (same wire shape as the daemon's
    /// read_tail `blocks`). Present = the demo Recent-output surface renders
    /// the real Conversation / Harness activity context split; absent = the
    /// legacy lines-only fallback. Stored as raw wire entries and parsed by
    /// the SAME tolerant `parse_tail_blocks` path as live results.
    #[serde(default)]
    pub recent_output_blocks: Vec<serde_json::Value>,
}

const FIXTURE: &str = include_str!("../assets/demo-fixture.json");

/// Parse the embedded fixture. A failing parse is a build-time defect in
/// this crate, so this may panic (embedded data can never change at
/// runtime).
pub fn load() -> DemoData {
    let data: DemoData = serde_json::from_str(FIXTURE).expect("embedded demo fixture is valid");
    data
}

/// Fixture-backed transcript lines. The browser demo stores these through
/// `Fleet::remember_tail`, exactly as the live read-tail result is stored.
pub fn recent_tail() -> Vec<String> {
    load()
        .recent_output
        .into_iter()
        .map(|line| scrub_demo_icons(&line))
        .collect()
}

/// #316 V3: fixture-backed canonical blocks, parsed by the same tolerant
/// wire path as live read_tail results (missing/malformed entries skipped).
pub fn recent_tail_blocks() -> Vec<crate::drive::CanonicalBlock> {
    let mut blocks = {
        crate::drive::parse_tail_blocks(
            &serde_json::json!({ "blocks": load().recent_output_blocks }),
        )
    };
    for block in &mut blocks {
        block.text = scrub_demo_icons(&block.text);
    }
    blocks
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
        assert!(!demo.deltas.is_empty(), "canned SSE frames are required");
        let mut rev = demo.snapshot.rev;
        for delta in &demo.deltas {
            assert!(
                delta.rev > rev,
                "delta revs must strictly increase from the snapshot"
            );
            rev = delta.rev;
        }
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
        assert!(fleet.issues_loaded);
    }
}
