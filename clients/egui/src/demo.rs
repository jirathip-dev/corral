//! Bundled demo fixture (#215): a representative read-only corral snapshot,
//! a short canned SSE delta sequence, and the recents tail for the featured
//! agent, so the read-only WASM build renders a believable v2 fleet board
//! out of the box — no daemon, no network.
//!
//! #354 L3: the fixture is BOARD-ONLY — fictional repos over the raw herdr
//! states (working / idle / blocked / unknown; no claims, no issues, no
//! `done`), with a canonical recents tail for the featured agent.
//!
//! The fixture is embedded at compile time (`include_str!`), so the demo
//! works on a plain GitHub Pages static deployment. Bump the JSON when the
//! local daemon's wire shapes change (it mirrors `crate::model`).

use crate::model::{Delta, Snapshot};

/// Everything the demo mode needs to render + animate the board.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct DemoData {
    /// Explicitly synthetic fixture data; never used by the live read path.
    pub fixture_name: String,
    pub snapshot: Snapshot,
    /// Canned SSE delta frames applied one every few seconds, wrapped
    /// (revs strictly increase before the wrap so `Fleet::apply_delta`
    /// accepts every frame).
    pub deltas: Vec<Delta>,
    /// Sanitized transcript lines served through the normal read-tail cache.
    pub recent_output: Vec<String>,
    /// #316: canonical semantic blocks (same wire shape as the daemon's
    /// read_tail `blocks`). Present = the demo recents drill-in renders the
    /// canonical stream; absent = the legacy lines-only fallback. Stored as
    /// raw wire entries and parsed by the SAME tolerant `parse_tail_blocks`
    /// path as live results.
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
/// `Fleet::remember_tail_full`, exactly as the live read-tail result is
/// stored.
pub fn recent_tail() -> Vec<String> {
    load()
        .recent_output
        .into_iter()
        .map(|line| scrub_demo_icons(&line))
        .collect()
}

/// Fixture-backed canonical blocks, parsed by the same tolerant wire path
/// as live read_tail results (missing/malformed entries skipped).
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
        assert!(demo.fixture_name.starts_with("Synthetic"));
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

        // #354 L3 fixture contract: raw states only (no `done`, no claims),
        // read_tail advertised on the recents-capable agents, and a recents
        // tail exists for at least the featured agent.
        let mut read_tail_agents = 0;
        for agent in demo.snapshot.agents.values() {
            assert_ne!(
                agent.state.label(),
                "done",
                "the read-only fixture never shows a wire done"
            );
            assert!(
                agent.state.label() == "working"
                    || agent.state.label() == "idle"
                    || agent.state.label() == "blocked"
                    || agent.state.label() == "unknown",
                "fixture states are the raw herdr tokens"
            );
            if agent.can_read_tail() {
                read_tail_agents += 1;
            }
        }
        assert!(read_tail_agents >= 1, "fixture advertises read_tail");

        // The fixture must actually fold into the view model (the demo
        // board is fed through the same path as live SSE).
        let mut fleet = crate::state::Fleet::default();
        fleet.apply_snapshot(&demo.snapshot);
        fleet.apply_delta(&demo.deltas[0]);
        assert!(fleet.rev.unwrap() > demo.snapshot.rev);
        assert_eq!(fleet.agents.len(), demo.snapshot.agents.len());
    }

    #[test]
    fn fixture_recent_output_round_trips_through_the_tail_parsers() {
        let demo = load();
        assert!(!demo.recent_output.is_empty());
        let lines = recent_tail();
        let blocks = recent_tail_blocks();
        assert!(!lines.is_empty());
        let rows = crate::ui::board::tail_rows(&lines, &blocks);
        assert!(!rows.is_empty(), "recents v1 rows derive from the fixture");
        for line in &lines {
            assert!(
                !line.contains('\u{e000}'),
                "private-use demo icons are scrubbed"
            );
        }
    }
}
