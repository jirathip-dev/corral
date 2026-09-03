#if DEBUG
import Foundation

/// Seeded Debug demo fleet: renders the #371 READ-ONLY board v2 — raw herdr
/// status sections (blocked / working / idle / done / unknown), each split
/// into always-open repo subgroups (alphabetical, Other last) with working
/// rows carrying the breathing-motion chips — with no daemon reachable. No
/// actions exist; the one demo drive is read_tail.
///
/// Shape (evidence-fit): blocked spans two repos, working spans two repos
/// PLUS the orphan (Other subgroup, gray), and one done row proves the Done
/// section renders when present. The demo exercises every status + subgroup
/// kind in ONE seed: the first 844 pt viewport shows the chips row, the
/// blocked (2) section's two subgroup bands, and the working (3) section's
/// bands with the breathing-motion chips; idle/done/unknown + the Other
/// band sit below the fold (the board scrolls; simctl cannot inject scroll,
/// so the lower sections are unit- + source-wiring-covered, never claimed
/// as viewport evidence).
enum DemoFleet {

    /// The opt-in detail route used by the reproducible evidence gate.
    /// Keeping the target in the fixture makes the route deterministic
    /// without changing normal fleet selection behavior.
    static let featuredAgentID = "herdr:demo-output"

    /// Deterministic seed so the demo is stable across launches.
    static func seed(rev: UInt64 = 1) -> [String: Agent] {
        let now = UInt64(Date().timeIntervalSince1970 * 1000)
        var agents: [String: Agent] = [:]

        func agent(_ id: String, tool: String, state: AgentState,
                   reason: String?, displayName: String, repo: String?, branch: String?,
                   seq: UInt64, tsOffset: UInt64, capabilities: [String] = ["read_tail"],
                   dirty: Bool = false, prNumber: UInt64? = nil,
                   paneRef: String) -> Agent {
            Agent(agentId: id, tool: tool, state: state, reason: reason,
                  seq: seq, ts: now - tsOffset, capabilities: capabilities,
                  workspace: Workspace(repo: repo, branch: branch,
                                       worktreePath: nil, prNumber: prNumber,
                                       ciStatus: .success, dirty: dirty,
                                       ahead: 0, behind: 0),
                  attachment: Attachment(kind: "herdr", reference: paneRef),
                  displayName: displayName, title: nil)
        }

        // Blocked agents: the blocked section is the board's first section.
        // TWO blocked rows across different repos prove the subgroup bands
        // render alphabetically (demo-garden < demo-orbit) INSIDE Blocked —
        // #371 groups every section uniformly, incl. Blocked.
        agents["herdr:demo-garden-blocked"] = agent(
            "herdr:demo-garden-blocked", tool: "claude", state: .blocked,
            reason: "waiting on human review of the catalog push",
            displayName: "demo-garden-agent", repo: "demo-garden", branch: "demo-catalog",
            seq: 9, tsOffset: 30, dirty: true, prNumber: 9026, paneRef: "w21:p1")

        agents["herdr:demo-orbit-blocked"] = agent(
            "herdr:demo-orbit-blocked", tool: "codex", state: .blocked,
            reason: "waiting on a payload review decision",
            displayName: "demo-orbit-blocked", repo: "demo-orbit", branch: "demo-payload",
            seq: 8, tsOffset: 90, paneRef: "w23:p2")

        // Working agents across DIFFERENT repos (plus the orphan below):
        // the working section shows demo-atlas + demo-orbit subgroup bands
        // then the gray Other band — and every working row carries the
        // #371 breathing-motion chip.
        agents["herdr:demo-orbit-working"] = agent(
            "herdr:demo-orbit-working", tool: "opencode", state: .working,
            reason: "streaming a segmented recent output",
            displayName: "demo-orbit-worker", repo: "demo-orbit", branch: "demo-ios",
            seq: 6, tsOffset: 6, dirty: true, prNumber: 9025, paneRef: "w23:p1")

        agents[featuredAgentID] = agent(
            featuredAgentID, tool: "claude", state: .working,
            reason: "streaming a segmented recent output",
            displayName: "demo-output", repo: "demo-atlas", branch: "demo-recent",
            seq: 10, tsOffset: 12, dirty: true, prNumber: 9005, paneRef: "w24:p1")

        // Idle agent: the idle section (finished panes fall back to idle;
        // a row stays visible until the daemon replaces/deletes it).
        agents["herdr:demo-ledger-idle"] = agent(
            "herdr:demo-ledger-idle", tool: "codex", state: .idle,
            reason: nil, displayName: "demo-ledger-idle", repo: "demo-ledger", branch: "demo-embed",
            seq: 5, tsOffset: 1800, paneRef: "w22:p2")

        // Done row: proves the Done section renders when herdr reports done
        // (the live-board norm has none; BoardModel emits it only when a
        // done agent exists).
        agents["herdr:demo-garden-done"] = agent(
            "herdr:demo-garden-done", tool: "claude", state: .done,
            reason: "catalog push merged", displayName: "demo-garden-done",
            repo: "demo-garden", branch: "demo-catalog",
            seq: 7, tsOffset: 600, paneRef: "w21:p2")

        // Unknown + orphan (no repo) agents exercise the tail statuses: the
        // orphan carries repo = nil and lands in the working section's
        // Other subgroup (gray, after the alphabetical repos — #371).
        agents["herdr:demo-atlas-unknown"] = agent(
            "herdr:demo-atlas-unknown", tool: "claude", state: .unknown,
            reason: nil, displayName: "demo-atlas-unknown", repo: "demo-atlas", branch: nil,
            seq: 2, tsOffset: 240, paneRef: "w24:p3")

        agents["herdr:demo-orphan"] = agent(
            "herdr:demo-orphan", tool: "codex", state: .working,
            reason: "no workspace repo attached",
            displayName: "demo-orphan", repo: nil, branch: nil,
            seq: 1, tsOffset: 60, paneRef: "w25:p1")

        return agents
    }

    struct RecentBlock: Equatable, Sendable {
        let kind: TranscriptBlockKind
        let text: String
        let truncatedBefore: UInt32?
    }

    /// Demo read_tail responder: canned blocks, no daemon.
    static func respond(to capability: Capability, agent: Agent,
                        rev: UInt64) -> DriveResult {
        switch capability {
        case .readTail:
            let blocks = recentBlocks(for: agent)
            let lines = recentLines(from: blocks)
            let result: CodableValue = .object([
                "agent_id": .string(agent.agentId),
                "lines": .array(lines.map { .string($0) }),
                // Demo serves the exact same segmented blocks used to
                // derive `lines`; the fixture cannot drift into two scripts.
                "blocks": .array(blocks.map { block in
                    var value: [String: CodableValue] = [
                        "kind": .string(block.kind.rawValue),
                        "text": .string(block.text)
                    ]
                    if let truncatedBefore = block.truncatedBefore {
                        value["truncated_before"] = .int(Int64(truncatedBefore))
                    }
                    return .object(value)
                })
            ])
            return .dispatched(DriveResponse(requestId: "demo", ok: true, error: nil, errorKind: nil,
                                             rev: rev, result: result))
        case .readDiff:
            return .dispatched(DriveResponse(requestId: "demo", ok: true, error: nil, errorKind: nil,
                                             rev: rev, result: nil))
        }
    }

    /// Live-tail fixture: one stream of canonical blocks (user / agent /
    /// tool / system / unknown) in daemon order — recents renders the
    /// bounded tail as ONE continuous chronological rail (#361).
    static func recentBlocks(for agent: Agent) -> [RecentBlock] {
        // The divider-only system block below stays ON PURPOSE: the rail
        // row model drops divider-only rows, so simulator evidence proves
        // ZERO divider rows render.
        let file = "src/board_view.rs"
        let omitted = agent.seq > 0 ? UInt32(min(agent.seq * 10, 2_000)) : nil
        return [
            RecentBlock(
                kind: .agent,
                text: "(demo) Snapshot read model is consistent.",
                truncatedBefore: omitted),
            RecentBlock(kind: .user,
                        text: "Please verify the diff too.",
                        truncatedBefore: nil),
            RecentBlock(
                kind: .agent,
                text: "def deploy():\n    print(\"ready ✅\")\n    return True",
                truncatedBefore: nil),
            RecentBlock(
                kind: .system,
                text: "read_tail page truncated to the newest 200 lines.",
                truncatedBefore: nil),
            RecentBlock(
                kind: .system,
                text: "──────────────────────────────────────",
                truncatedBefore: nil),
            RecentBlock(kind: .unknown,
                        text: "raw pane line without provenance",
                        truncatedBefore: nil),
            RecentBlock(
                kind: .tool,
                text: "git diff -- \(file)\n@@ -18,2 +18,4 @@\n-const OLD: &str = \"plain\";\n+pub fn recent_output() -> Bool {\n+    true\n+}",
                truncatedBefore: nil)
        ]
    }

    static func recentLines(from blocks: [RecentBlock]) -> [String] {
        blocks.flatMap { $0.text.components(separatedBy: .newlines) }
    }
}
#endif
