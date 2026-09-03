import Foundation

// MARK: - #354 L2 read-only board (pure projections)

/// The v2 board shape, computed as a pure function of the agent set so it is
/// unit-testable and stable across renders:
///
/// - `statuses` — one section per raw herdr state, in the LOCKED board order
///   Blocked → Working → Idle → Unknown. `workspace.repo` is ROW METADATA
///   only — never a grouping key, so there is no repo section, no orphan
///   bucket, and no cross-repo blocked promotion: a blocked agent appears
///   exactly once, first overall because its section is first.
/// - A Done section renders ONLY when the daemon actually reports `done`
///   (a bucket exists only when it has agents). herdr 0.8.2 finished panes
///   fall back to idle, so live boards normally show no Done section; a
///   wire-`done` still RANKS with idle (state-token rank 2) and its section
///   is emitted directly after Idle, before Unknown.
///
/// Within every section the ordering is the v2 attention rank — blocked >
/// working > idle/done > unknown — then ts desc, then agent id for
/// determinism.
enum BoardModel {

    struct StatusSection: Equatable, Identifiable {
        let state: AgentState
        let agents: [Agent]

        var id: String { state.rawValue }

        /// Header label: the raw status name (data — never transformed)
        /// with the visible agent count.
        var header: String {
            "\(state.displayName) (\(agents.count))"
        }
    }

    struct Sections: Equatable {
        /// Status-grouped buckets in the locked order. A bucket exists only
        /// when it has agents, so the done bucket renders only when herdr
        /// reports done.
        let statuses: [StatusSection]
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

    /// Status-grouped buckets in the locked order: blocked → working →
    /// idle → unknown, with a done bucket emitted only when herdr reports
    /// done (wire-`done` ranks with idle — its section sits after Idle,
    /// before Unknown). Repo is never a grouping key; an agent appears in
    /// exactly one section.
    static func sections(_ agents: [Agent]) -> Sections {
        let lockedOrder: [AgentState] = [.blocked, .working, .idle, .done, .unknown]
        let statuses = lockedOrder.compactMap { state -> StatusSection? in
            let members = ordered(agents.filter { $0.state == state })
            return members.isEmpty ? nil : StatusSection(state: state, agents: members)
        }
        return Sections(statuses: statuses)
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
