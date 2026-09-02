#if DEBUG
import Foundation

/// Seeded Debug demo fleet: renders the #354 L2 READ-ONLY board — repo
/// groups with raw herdr state chips (working / idle / blocked / unknown),
/// blocked agents pinned on top, idle agents retained per repo — with no
/// daemon reachable. No actions exist; the one demo drive is read_tail.
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

        // Blocked agents: pinned top of the board, and first in their repo.
        agents["herdr:demo-garden-blocked"] = agent(
            "herdr:demo-garden-blocked", tool: "claude", state: .blocked,
            reason: "waiting on human review of the catalog push",
            displayName: "demo-garden-agent", repo: "demo-garden", branch: "demo-catalog",
            seq: 9, tsOffset: 30, dirty: true, prNumber: 9026, paneRef: "w21:p1")

        agents["herdr:demo-ledger-blocked"] = agent(
            "herdr:demo-ledger-blocked", tool: "codex", state: .blocked,
            reason: "awaiting a ship-or-hold decision",
            displayName: "demo-ledger-agent", repo: "demo-ledger", branch: "demo-migration",
            seq: 8, tsOffset: 90, dirty: true, prNumber: 9021, paneRef: "w22:p1")

        // Working agents.
        agents["herdr:demo-garden-working"] = agent(
            "herdr:demo-garden-working", tool: "claude", state: .working,
            reason: "running the test suite",
            displayName: "demo-garden-worker", repo: "demo-garden", branch: "demo-sweep",
            seq: 7, tsOffset: 12, paneRef: "w21:p2")

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

        // Idle agents: retained per repo (finished panes fall back to idle).
        agents["herdr:demo-ledger-idle"] = agent(
            "herdr:demo-ledger-idle", tool: "codex", state: .idle,
            reason: nil, displayName: "demo-ledger-idle", repo: "demo-ledger", branch: "demo-embed",
            seq: 5, tsOffset: 1800, paneRef: "w22:p2")

        agents["herdr:demo-orbit-idle"] = agent(
            "herdr:demo-orbit-idle", tool: "claude", state: .idle,
            reason: nil, displayName: "demo-orbit-idle", repo: "demo-orbit", branch: "demo-coverage",
            seq: 4, tsOffset: 3600, paneRef: "w23:p2")

        agents["herdr:demo-atlas-idle"] = agent(
            "herdr:demo-atlas-idle", tool: "opencode", state: .idle,
            reason: nil, displayName: "demo-atlas-idle", repo: "demo-atlas", branch: "demo-embed",
            seq: 3, tsOffset: 7200, paneRef: "w24:p2")

        // Unknown + orphan (no repo) agents exercise the tail buckets.
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

    /// Live-tail-only fixture: one stream of canonical blocks (user / agent
    /// / tool / system / unknown), unpartitioned — recents v1 renders the
    /// daemon's bounded tail as-is.
    static func recentBlocks(for agent: Agent) -> [RecentBlock] {
        // Divider + unprovenanced chrome blocks stay: the shared divider
        // scrub renders them as rules/unknown activity.
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
