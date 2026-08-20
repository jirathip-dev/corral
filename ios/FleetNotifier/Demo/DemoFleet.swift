#if DEBUG
import Foundation

/// Seeded Debug demo fleet: renders the
/// full Fleet Notifier surface — blocked agents of every kind, choices,
/// workspace/PR/CI columns — with no daemon reachable. Demo drives are
/// answered locally and mutate the seeded fleet.
enum DemoFleet {

    /// Deterministic seed so the demo is stable across launches.
    static func seed(rev: UInt64 = 1) -> [String: Agent] {
        let now = UInt64(Date().timeIntervalSince1970 * 1000)
        var agents: [String: Agent] = [:]

        func hash(_ text: String) -> String {
            let digest = SHA256Sum.hex(of: text)
            return "sha256:\(digest)"
        }

        func agent(_ id: String, tool: String, source: String = "herdr", state: AgentState,
                   reason: String?, waiting: WaitingOn?, capabilities: [String],
                   displayName: String, title: String?, workspace: Workspace = Workspace(),
                   seq: UInt64 = 1, tsOffset: UInt64 = 0) -> Agent {
            Agent(agentId: id, source: source, tool: tool, state: state, reason: reason,
                  seq: seq, ts: now - tsOffset, capabilities: capabilities, waitingOn: waiting,
                  workspace: workspace, displayName: displayName, title: title)
        }

        let tail = { (cap: String) -> [String] in
            ["read_tail", "prompt", "interrupt", cap]
        }

        // Blocked on an approve-tool prompt with a menu (the classic flow).
        let approvePrompt = "Approve this change to push 2 commits to feat/plush-v2?"
        agents["herdr:demo-approve"] = agent("herdr:demo-approve", tool: "codex", state: .blocked,
            reason: "waiting for approval",
            waiting: WaitingOn(kind: .approveTool, prompt: approvePrompt,
                               promptHash: hash(approvePrompt),
                               approvalId: Claim.approvalId(agentId: "herdr:demo-approve", promptHash: hash(approvePrompt)),
                               choices: ["y", "n"]),
            capabilities: tail("approve"),
            displayName: "demo-approve", title: "Push plush catalog v2",
            // Authoritative closing-issue ref (G23) on the WORKSPACE — the
            // wire location the daemon emits — so the demo `⑂ #N` chip goes
            // through the same read path as live data. The other demo agents
            // exercise the inferred `~#N?` / no-chip paths.
            workspace: Workspace(repo: "project-hearthwild", branch: "feat/plush-v2",
                                 worktreePath: "~/worktrees/project-hearthwild/feat-plush-v2",
                                 prNumber: 42, ciStatus: .pending, dirty: true, ahead: 3, behind: 0,
                                 issues: [GhIssueRef(repo: "project-hearthwild", number: 41,
                                                     state: "open", title: "Plush catalog v2")]),
            seq: 5, tsOffset: 30)

        // Blocked on a menu with explicit choices.
        let menuPrompt = "Select an option:\n1) Ship now\n2) Hold for review\n3) Abort"
        agents["herdr:demo-menu"] = agent("herdr:demo-menu", tool: "opencode", state: .blocked,
            reason: "menu prompt",
            waiting: WaitingOn(kind: .menu, prompt: menuPrompt, promptHash: hash(menuPrompt),
                               approvalId: Claim.approvalId(agentId: "herdr:demo-menu", promptHash: hash(menuPrompt)),
                               choices: ["1", "2", "3"]),
            capabilities: tail("approve"),
            displayName: "demo-menu", title: "Ship or hold the migration",
            workspace: Workspace(repo: "synergy-costing", branch: "fix-migration-late-arrival",
                                 worktreePath: "~/worktrees/synergy-costing/fix-migration",
                                 prNumber: 431, ciStatus: .failure, dirty: false, ahead: 8, behind: 2),
            seq: 4, tsOffset: 90)

        // Blocked on a free-form question.
        let question = "What should the new branch be named?"
        agents["herdr:demo-question"] = agent("herdr:demo-question", tool: "claude", state: .blocked,
            reason: "asked a question",
            waiting: WaitingOn(kind: .answerQuestion, prompt: question, promptHash: hash(question),
                               approvalId: Claim.approvalId(agentId: "herdr:demo-question", promptHash: hash(question)),
                               choices: []),
            capabilities: tail("approve"),
            displayName: "demo-question", title: "Rename tracking branch",
            workspace: Workspace(repo: "sendmeter", branch: "native-637-testflight",
                                 worktreePath: "~/worktrees/sendmeter/native-637",
                                 prNumber: 637, ciStatus: .success, dirty: true, ahead: 1, behind: 0),
            seq: 3, tsOffset: 240)

        // Crash kind: never approvable.
        agents["herdr:demo-crash"] = agent("herdr:demo-crash", tool: "gemini", state: .blocked,
            reason: "agent crashed",
            waiting: WaitingOn(kind: .crash, prompt: "Agent crashed (exit 139).", promptHash: hash("Agent crashed"),
                               approvalId: Claim.approvalId(agentId: "herdr:demo-crash", promptHash: hash("Agent crashed")),
                               choices: []),
            capabilities: tail("approve"),
            displayName: "demo-crash", title: "Parallel docs sweep",
            workspace: Workspace(repo: "herdr-board", branch: "w3/ios-fleet-notifier",
                                 worktreePath: "~/worktrees/herdr-board/corral-p4-w3",
                                 prNumber: 15, ciStatus: .unknown, dirty: true, ahead: 4, behind: 1),
            seq: 2, tsOffset: 600)

        // Working, idle, done.
        agents["herdr:demo-working"] = agent("herdr:demo-working", tool: "opencode", state: .working,
            reason: "running the test suite",
            waiting: nil, capabilities: ["read_tail", "interrupt", "prompt"],
            displayName: "demo-working", title: "Wire canonical bytes unit tests",
            workspace: Workspace(repo: "herdr-board", branch: "w3/ios-fleet-notifier",
                                 worktreePath: "~/worktrees/herdr-board/corral-p4-w3",
                                 prNumber: 15, ciStatus: .pending, dirty: true, ahead: 4, behind: 1),
            seq: 9, tsOffset: 20)

        agents["herdr:demo-idle"] = agent("herdr:demo-idle", tool: "claude", state: .idle,
            reason: nil, waiting: nil, capabilities: ["read_tail", "prompt"],
            displayName: "demo-idle", title: "Investigate SSO migration",
            workspace: Workspace(repo: "synergy-costing", branch: "issue-431-embed-pm",
                                 worktreePath: "~/worktrees/synergy-costing/issue-431",
                                 prNumber: 439, ciStatus: .success, dirty: false, ahead: 0, behind: 0),
            seq: 8, tsOffset: 1800)

        agents["herdr:demo-done"] = agent("herdr:demo-done", tool: "codex", state: .done,
            reason: "task complete",
            waiting: nil, capabilities: ["read_tail"],
            displayName: "demo-done", title: "Update plush catalog",
            workspace: Workspace(repo: "project-hearthwild", branch: "gauntlet-54-catalog",
                                 worktreePath: "~/worktrees/project-hearthwild/gauntlet-54",
                                 prNumber: 412, ciStatus: .success, dirty: false, ahead: 2, behind: 0),
            seq: 7, tsOffset: 3600)

        return agents
    }

    /// Demo drive responder: canned responses, no daemon.
    static func respond(to capability: Capability, payload: CanonicalJSON.Value,
                        agent: Agent, rev: UInt64) -> DriveResult {
        switch capability {
        case .readTail:
            let result: CodableValue = .object([
                "agent_id": .string(agent.agentId),
                "lines": .array([
                    .string("(demo) last lines of \(agent.displayName ?? agent.agentId)"),
                    .string("…seeded tail, no daemon in demo mode")
                ])
            ])
            return .dispatched(DriveResponse(requestId: "demo", ok: true, error: nil, errorKind: nil,
                                             rev: rev, result: result))
        case .approve:
            // Fake the agent un-blocking; the store owner applies the
            // transition via `simulateUnblock`.
            return .dispatched(DriveResponse(requestId: "demo", ok: true, error: nil, errorKind: nil,
                                             rev: rev + 1, result: nil))
        default:
            return .dispatched(DriveResponse(requestId: "demo", ok: true, error: nil, errorKind: nil,
                                             rev: rev + 1, result: nil))
        }
    }
}

/// Standalone SHA-256 hex (CryptoKit is not needed for the hash; kept
/// dependency-free so the demo seed can mint prompt hashes like the
/// daemon's source adapters do).
enum SHA256Sum {
    static func hex(of text: String) -> String {
        let bytes = Array(text.utf8)
        let digest = sha256(bytes)
        return digest.map { String(format: "%02x", $0) }.joined()
    }

    private static func sha256(_ message: [UInt8]) -> [UInt8] {
        var state: [UInt32] = [0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a,
                               0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19]
        var buffer = [UInt8]()
        buffer.reserveCapacity(64)

        func ingest(_ chunk: [UInt8]) {
            buffer += chunk
            while buffer.count >= 64 {
                transform(&state, Array(buffer[0..<64]))
                buffer.removeFirst(64)
            }
        }

        ingest(message)
        let bitLength = UInt64(message.count) * 8
        ingest([0x80])
        while buffer.count % 64 != 56 {
            ingest([0x00])
        }
        var lengthBytes = [UInt8](repeating: 0, count: 8)
        for i in 0..<8 {
            lengthBytes[7 - i] = UInt8((bitLength >> (UInt64(i) * 8)) & 0xff)
        }
        ingest(lengthBytes)

        var out = [UInt8](repeating: 0, count: 32)
        for i in 0..<8 {
            for j in 0..<4 {
                out[i * 4 + j] = UInt8((state[i] >> UInt32((3 - j) * 8)) & 0xff)
            }
        }
        return out
    }

    private static func transform(_ state: inout [UInt32], _ block: [UInt8]) {
        var w = [UInt32](repeating: 0, count: 64)
        for i in 0..<16 {
            let o = i * 4
            w[i] = UInt32(block[o]) << 24 | UInt32(block[o + 1]) << 16 | UInt32(block[o + 2]) << 8 | UInt32(block[o + 3])
        }
        for i in 16..<64 {
            let s0 = rotr(w[i - 15], 7) ^ rotr(w[i - 15], 18) ^ (w[i - 15] >> 3)
            let s1 = rotr(w[i - 2], 17) ^ rotr(w[i - 2], 19) ^ (w[i - 2] >> 10)
            w[i] = w[i - 16] &+ s0 &+ w[i - 7] &+ s1
        }
        var a = state[0], b = state[1], c = state[2], d = state[3]
        var e = state[4], f = state[5], g = state[6], h = state[7]
        for i in 0..<64 {
            let s1 = rotr(e, 6) ^ rotr(e, 11) ^ rotr(e, 25)
            let ch = (e & f) ^ (~e & g)
            let temp1 = h &+ s1 &+ ch &+ sha256K[i] &+ w[i]
            let s0 = rotr(a, 2) ^ rotr(a, 13) ^ rotr(a, 22)
            let maj = (a & b) ^ (a & c) ^ (b & c)
            let temp2 = s0 &+ maj
            h = g; g = f; f = e; e = d &+ temp1
            d = c; c = b; b = a; a = temp1 &+ temp2
        }
        state[0] &+= a; state[1] &+= b; state[2] &+= c; state[3] &+= d
        state[4] &+= e; state[5] &+= f; state[6] &+= g; state[7] &+= h
    }

    private static func rotr(_ x: UInt32, _ n: UInt32) -> UInt32 {
        (x >> n) | (x << (32 - n))
    }
}

private let sha256K: [UInt32] = [
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
    0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
    0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
    0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
    0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
    0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
    0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
    0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
]
#endif
