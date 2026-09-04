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

    /// Live-tail fixture (#373 block-per-run): one canonical block stream
    /// whose role-run shape exercises every block treatment the recents
    /// sheet must prove — quiet Status material first, user/assistant
    /// prose runs, a >20-line tool run (per-block 20-line cap + inline
    /// "Show all"), a BARE tool call (doc icon — no shell echo), a diff
    /// run (ANSI-remap proof: `+` / `-` / `@@` syntax marks resolve
    /// through the ACTIVE flavor's ANSI slots), and a FINAL call-only tool
    /// run (the muted inline "waiting for output…" line — never its own
    /// block). Same-tool invocations ride inside ONE tool run (compact
    /// line per call). Content is fully fictional.
    static func recentBlocks(for agent: Agent) -> [RecentBlock] {
        let omitted = agent.seq > 0 ? UInt32(min(agent.seq * 10, 2_000)) : nil
        return [
            RecentBlock(
                kind: .system,
                text: "read_tail page truncated to the newest 200 lines.",
                truncatedBefore: omitted),
            RecentBlock(
                kind: .user,
                text: "The retry wrapper double-applies on the demo endpoint — find where it wraps twice and add a regression test that bites.",
                truncatedBefore: nil),
            RecentBlock(
                kind: .agent,
                text: "Starting from the request path: the wrapper probably wraps both the call site and its caller. Running the baseline first.",
                truncatedBefore: nil),
            RecentBlock(
                kind: .tool,
                text: "read_file src/retry.ts  lines 1-18\n"
                    + "  1  export function withRetry(fn, attempts = 3) {\n"
                    + "  2    return async (...args) => {\n"
                    + "  3      for (let attempt = 1; attempt <= attempts; attempt++) {\n"
                    + "  4        try { return await fn(...args) }\n"
                    + "  5        catch (error) {\n"
                    + "  6          if (attempt === attempts) throw error\n"
                    + "  7        }\n"
                    + "  8      }\n"
                    + "  9    }\n"
                    + " 10  }",
                truncatedBefore: nil),
            RecentBlock(
                kind: .agent,
                text: "Found it — the caller wraps the post a second time, so 409s retry twice. Removing the outer wrapper and the redundant predicate range.",
                truncatedBefore: nil),
            RecentBlock(
                kind: .tool,
                text: "$ pnpm vitest run src/retry.test.ts\n"
                    + " RUN  v2.1.4  demo-atlas\n"
                    + "\n"
                    + " ✓ src/retry.test.ts (9 tests) 142ms\n"
                    + "   ✓ wraps the post exactly once\n"
                    + "   ✓ retries 503 up to the attempt budget\n"
                    + "   ✓ retries 429 and honours Retry-After\n"
                    + "   ✓ does not retry 409 Conflict\n"
                    + "   ✓ does not retry 400 Bad Request\n"
                    + "   ✓ does not retry 404 Not Found\n"
                    + "   ✓ propagates the original error body\n"
                    + "   ✓ clears its timer on success\n"
                    + "   ✓ is a no-op when attempts = 1\n"
                    + "   ✓ reports the attempt count in telemetry\n"
                    + "\n"
                    + " Test Files  1 passed (1)\n"
                    + "      Tests  9 passed (9)\n"
                    + "   Duration  1.42s (transform 210ms, collect 380ms)\n"
                    + "\n"
                    + " PASS  Waiting for file changes...\n"
                    + "   ✓ 9/9 checks green — done\n"
                    + "  demo-atlas: retry suite baseline green\n"
                    + "\n"
                    + " — press q to exit —",
                truncatedBefore: nil),
            RecentBlock(
                kind: .agent,
                text: "9/9 green — here is the exact change that keeps 409 out of the retry path:",
                truncatedBefore: nil),
            RecentBlock(
                kind: .tool,
                text: "$ git diff -- src/retry.ts\n"
                    + "@@ -18,2 +18,4 @@\n"
                    + "-const retryable = (s) => s >= 408\n"
                    + "+const RETRYABLE = new Set([408, 425, 429, 500])\n"
                    + "+const retryable = (s) => RETRYABLE.has(s)",
                truncatedBefore: nil),
            RecentBlock(
                kind: .agent,
                text: "The explicit set keeps 409 out of the retry path. Double-checking for any other retry sites before I push.",
                truncatedBefore: nil),
            RecentBlock(
                kind: .tool,
                text: "$ rg -n withRetry src/",
                truncatedBefore: nil),
        ]
    }

    static func recentLines(from blocks: [RecentBlock]) -> [String] {
        blocks.flatMap { $0.text.components(separatedBy: .newlines) }
    }
}
extension DemoFleet {

    /// Synthetic host fixture: display names + pinned X25519 keys used ONLY
    /// by the DEBUG multi-host evidence seeding (fresh simulators; real
    /// pairing is never touched by demo launch args).
    enum DemoHosts {
        static let hostAKey = Data(repeating: 9, count: 32).base64EncodedString()
        static let hostBKey = Data(repeating: 10, count: 32).base64EncodedString()
        static let hostCKey = Data(repeating: 11, count: 32).base64EncodedString()
        static let addHostKey = Data(repeating: 12, count: 32).base64EncodedString()
        static let urls = [
            "https://demo-host-a.example.ts.net",
            "https://demo-host-b.example.ts.net",
            "https://demo-host-c.example.ts.net",
        ]
        static let addHostURL = "demo-host-d.tail0123.ts.net"
    }

    /// Host A's LIVE rows: the same repo vocabulary as the single-host seed
    /// so multi-host frames read as one synthetic fleet. A blocked demo-orbit
    /// row SHARES its raw agent id with Host B's seed (C2 — equal raw ids
    /// coexist; the composite identity + row badge tell them apart).
    static func multiHostSeedA(now: UInt64) -> [String: Agent] {
        var agents: [String: Agent] = [:]
        func agent(_ id: String, tool: String, state: AgentState,
                   reason: String?, displayName: String, repo: String?, branch: String?,
                   seq: UInt64, tsOffset: UInt64, paneRef: String) -> Agent {
            Agent(agentId: id, tool: tool, state: state, reason: reason,
                  seq: seq, ts: now - tsOffset, capabilities: ["read_tail"],
                  workspace: Workspace(repo: repo, branch: branch,
                                       worktreePath: nil, prNumber: nil,
                                       ciStatus: .success, dirty: false,
                                       ahead: 0, behind: 0),
                  attachment: Attachment(kind: "herdr", reference: paneRef),
                  displayName: displayName, title: nil)
        }
        agents["herdr:demo-orbit-blocked"] = agent(
            "herdr:demo-orbit-blocked", tool: "claude", state: .blocked,
            reason: "waiting on a payload review decision",
            displayName: "demo-orbit-blocked", repo: "demo-orbit",
            branch: "demo-payload", seq: 8, tsOffset: 90, paneRef: "w23:p2")
        agents["herdr:demo-atlas-working"] = agent(
            "herdr:demo-atlas-working", tool: "claude", state: .working,
            reason: "streaming a segmented recent output",
            displayName: "demo-atlas-worker", repo: "demo-atlas",
            branch: "demo-recent", seq: 10, tsOffset: 12, paneRef: "w24:p1")
        agents["herdr:demo-ledger-idle"] = agent(
            "herdr:demo-ledger-idle", tool: "codex", state: .idle,
            reason: nil, displayName: "demo-ledger-idle", repo: "demo-ledger",
            branch: "demo-embed", seq: 5, tsOffset: 1800, paneRef: "w22:p2")
        return agents
    }

    /// Host B's RETAINED rows (host offline — every row stale, last seen
    /// ~6 minutes ago). Shares demo-orbit/demo-atlas repos AND one raw
    /// agent id with Host A so All-Hosts merges the repo subgroup and the
    /// composite badges stay distinct.
    static func multiHostSeedB(now: UInt64) -> [String: Agent] {
        var agents: [String: Agent] = [:]
        func agent(_ id: String, tool: String, state: AgentState,
                   reason: String?, displayName: String, repo: String?, branch: String?,
                   seq: UInt64, tsOffset: UInt64, paneRef: String) -> Agent {
            Agent(agentId: id, tool: tool, state: state, reason: reason,
                  seq: seq, ts: now - tsOffset, capabilities: ["read_tail"],
                  workspace: Workspace(repo: repo, branch: branch,
                                       worktreePath: nil, prNumber: nil,
                                       ciStatus: .success, dirty: false,
                                       ahead: 0, behind: 0),
                  attachment: Attachment(kind: "herdr", reference: paneRef),
                  displayName: displayName, title: nil)
        }
        agents["herdr:demo-orbit-blocked"] = agent(
            "herdr:demo-orbit-blocked", tool: "codex", state: .blocked,
            reason: "waiting on a payload review decision",
            displayName: "demo-orbit-blocked", repo: "demo-orbit",
            branch: "demo-payload", seq: 9, tsOffset: 360_000, paneRef: "w33:p2")
        agents["herdr:demo-atlas-working"] = agent(
            "herdr:demo-atlas-working", tool: "codex", state: .working,
            reason: "streaming a segmented recent output",
            displayName: "demo-atlas-worker", repo: "demo-atlas",
            branch: "demo-recent", seq: 7, tsOffset: 390_000, paneRef: "w34:p1")
        return agents
    }
}
#endif

// MARK: - #401 multi-host demo fixture (deterministic evidence seeding)

