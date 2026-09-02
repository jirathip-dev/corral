import Foundation

// MARK: - #354 L2 read-only board (pure projections)

/// The v2 board shape, computed as a pure function of the agent set so it is
/// unit-testable and stable across renders:
///
/// - `blocked` — every blocked agent, cross-repo, pinned to the TOP of the
///   board (attention first). A PROMOTION, not a filter: the same agents
///   also appear inside their repo section.
/// - `repos` — one section per `workspace.repo` (named repos sorted by
///   name), holding EVERY agent of that repo — working, idle, blocked,
///   unknown — in attention order. The orphan bucket (repo = nil) sorts
///   last. A finished (idle-fallback) agent therefore STAYS in its repo
///   section until the daemon replaces/deletes it: the last-done-per-repo
///   retention rule. There is no collapsed cross-repo bucket anymore; the
///   status chip on each row carries the state.
///
/// Within every section the ordering is the v2 attention rank — blocked >
/// working > idle/done > unknown — then ts desc, then agent id for
/// determinism.
enum BoardModel {

    struct RepoSection: Equatable, Identifiable {
        /// `nil` = the orphan bucket (agents without `workspace.repo`).
        let repo: String?
        let agents: [Agent]

        var id: String { repo ?? "\u{FFFC}no-repo" }

        /// Header label: the repo name (data — never transformed) or the
        /// orphan bucket marker, with the visible agent count.
        var header: String {
            "\(repo ?? "no repo") (\(agents.count))"
        }
    }

    struct Sections: Equatable {
        /// Cross-repo blocked promotion, pinned above the repo sections.
        let blocked: [Agent]
        let repos: [RepoSection]
    }

    /// The canonical board ordering: v2 rank, then ts desc, then agent id.
    static func ordered(_ agents: [Agent]) -> [Agent] {
        agents.sorted { a, b in
            let ra = stateRank(a.state), rb = stateRank(b.state)
            if ra != rb { return ra < rb }
            if a.ts != b.ts { return a.ts > b.ts }
            return a.agentId < b.agentId
        }
    }

    /// v2 attention rank. Delegates to `StateStyle` (colors/rank contract
    /// mirroring `contracts/state-tokens.json`) so board ordering can never
    /// diverge from the state vocabulary again: blocked(0) > working(1) >
    /// idle/done(2) > unknown(3). A wire `done` ranks with idle — herdr
    /// finished panes fall back to idle.
    static func stateRank(_ state: AgentState) -> Int {
        StateStyle.style(for: state).rank
    }

    static func sections(_ agents: [Agent]) -> Sections {
        let blocked = ordered(agents.filter(\.isBlocked))
        let orderedAgents = ordered(agents)

        var byRepo: [String?: [Agent]] = [:]
        for agent in orderedAgents {
            byRepo[agent.workspace.repo, default: []].append(agent)
        }
        let repos = byRepo
            .map { RepoSection(repo: $0.key, agents: $0.value) }
            .sorted { a, b in
                switch (a.repo, b.repo) {
                case (let x?, let y?): return x < y
                case (.some, .none): return true
                case (.none, .some): return false
                case (.none, .none): return false
                }
            }
        return Sections(blocked: blocked, repos: repos)
    }

    // MARK: - Persistent connection indicator (#166 review F1)

    /// The persistent board connection indicator, modeled as a pure function
    /// of the fleet's connection state. It lives in the List's first pinned
    /// section header (the board chrome) — a stale or connecting board is
    /// never silently presented as live.
    static func connectionStatus(for state: FleetStore.ConnectionState) -> FleetConnectionStatus {
        switch state {
        case .connected: return .connected
        case .connecting: return .connecting
        case .disconnected: return .offline
        case .error(let message): return .error(message)
        }
    }
}

/// The four visual states of the persistent connection indicator (review
/// F1). Pure + testable; the view renders the label/spinner from this.
enum FleetConnectionStatus: Equatable, Sendable {
    case connected
    case connecting
    case offline
    case error(String)

    /// The on-screen text. `nil` when connected (no indicator rendered).
    var label: String? {
        switch self {
        case .connected: return nil
        case .connecting: return "connecting"
        case .offline: return "offline"
        case .error(let message): return "⚠ \(message)"
        }
    }

    /// True when the view should show the small spinner (not text).
    var isSpinner: Bool { self == .connecting }
}
