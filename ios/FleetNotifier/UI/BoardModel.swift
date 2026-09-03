import Foundation

// MARK: - #354/#371 L2 read-only board (pure projections)

/// The v2 board shape, computed as a pure function of the agent set so it is
/// unit-testable and stable across renders:
///
/// - `statuses` — one section per raw herdr state, in the LOCKED board order
///   Blocked → Working → Idle → Done → Unknown (a Done section renders ONLY
///   when the daemon actually reports `done`; herdr 0.8.2 finished panes
///   fall back to idle, so live boards normally show no Done section).
/// - #371 board v2: EVERY section (incl. Blocked — uniform) groups its rows
///   into always-open REPO SUBGROUPS: named repos in alphabetical order,
///   then the `Other` subgroup (no repo / unknown repo) LAST. A blocked
///   agent appears exactly once, first overall because its section is
///   first; repo is a grouping key INSIDE a status bucket only.
/// - Section ordering is the v2 attention rank — blocked > working >
///   idle/done > unknown — then ts desc, then agent id for determinism;
///   subgroup bucketing never reorders members.
///
/// The #364 B repo filter chips pick WHICH agents these sections bucket;
/// filtering never regroups — a filtered board shows the same locked
/// sections, each holding only the selected repo's subgroup.
enum BoardModel {

    /// The synthetic subgroup label for agents whose workspace repo is
    /// missing/unknown (`workspace.repo == nil`). Renders gray (surface2),
    /// AFTER the alphabetical named-repo subgroups (design lock). A repo
    /// literally named "Other" folds into this bucket so subgroup ids can
    /// never collide with an orphan subgroup.
    static let otherRepoLabel = "Other"

    /// The repo grouping key of one agent: `nil` (→ Other) when the
    /// workspace repo is missing, empty, or literally the Other label.
    static func repoKey(of agent: Agent) -> String? {
        guard let repo = agent.workspace.repo,
              !repo.isEmpty,
              repo != otherRepoLabel else { return nil }
        return repo
    }

    // MARK: - #364 B repo filter chips (pure projections)

    /// One repo filter chip: a workspace repo present in the current
    /// fleet plus the LIVE count of agents working in it. Orphan agents
    /// (repo == nil) are deliberately absent — they surface under the
    /// All chip only.
    struct RepoFilterChip: Equatable, Sendable, Identifiable {
        let repo: String
        let count: Int
        var id: String { repo }
    }

    /// The chip set for a fleet: one chip per distinct workspace repo,
    /// alphabetical, with the live agent count. `workspace.repo` stays
    /// ROW METADATA — chips filter which agents the status sections
    /// bucket; they never group the board.
    static func repoFilters(_ agents: [Agent]) -> [RepoFilterChip] {
        let counts = agents.reduce(into: [String: Int]()) { counts, agent in
            guard let repo = agent.workspace.repo else { return }
            counts[repo, default: 0] += 1
        }
        return counts.keys.sorted()
            .map { RepoFilterChip(repo: $0, count: counts[$0] ?? 0) }
    }

    /// The agent set a repo filter keeps: `nil` = All (no filtering); a
    /// repo keeps exactly the agents whose workspace repo matches.
    static func agents(_ agents: [Agent], in repo: String?) -> [Agent] {
        guard let repo else { return agents }
        return agents.filter { $0.workspace.repo == repo }
    }

    /// Reconcile the user's chosen filter against the CURRENT chip set:
    /// keep it while the repo still exists, fall back to All (nil) when
    /// it has vanished. Pure and nil-safe, so a render can never filter
    /// against a repo the fleet no longer has.
    static func reconcile(_ filter: String?, against chips: [RepoFilterChip]) -> String? {
        guard let filter else { return nil }
        return chips.contains { $0.repo == filter } ? filter : nil
    }

    /// One always-open repo subgroup INSIDE a status section (#371): the
    /// repo name + its agent rows, with a count for the header chip. The
    /// `Other` subgroup (`repo == nil`, no repo / unknown repo) carries the
    /// synthetic Other label and always sits AFTER the alphabetical named
    /// repos. Subgroups are NOT collapsible — they render as a plain
    /// tinted band + rows, never a disclosure control.
    struct RepoSubgroup: Equatable, Identifiable {
        /// The workspace repo; `nil` = the Other subgroup. Never the empty
        /// string or the literal Other label (see `repoKey(of:)`).
        let repo: String?
        let agents: [Agent]

        var id: String { repo ?? BoardModel.otherRepoLabel }

        /// Header label: repo name (or Other) + the visible agent count.
        var displayName: String { repo ?? BoardModel.otherRepoLabel }
        var header: String { "\(displayName) (\(agents.count))" }
    }

    struct StatusSection: Equatable, Identifiable {
        let state: AgentState
        /// The section's repo subgroups in render order: named repos
        /// alphabetical, then Other last. Never empty for a live section.
        let subgroups: [RepoSubgroup]

        var id: String { state.rawValue }

        /// The TOTAL agent count across the section's subgroups (the
        /// count the section header shows; rescopes with a repo filter).
        var total: Int {
            subgroups.reduce(into: 0) { $0 += $1.agents.count }
        }

        /// Header label: the raw status name (data — never transformed)
        /// with the TOTAL agent count across subgroups.
        var header: String {
            "\(state.displayName) (\(total))"
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
    /// before Unknown). EVERY section groups its members into always-open
    /// repo subgroups (alphabetical, Other last) — repo never reorders
    /// across statuses, so an agent appears in exactly one section, in
    /// exactly one subgroup.
    static func sections(_ agents: [Agent]) -> Sections {
        let lockedOrder: [AgentState] = [.blocked, .working, .idle, .done, .unknown]
        let statuses = lockedOrder.compactMap { state -> StatusSection? in
            let members = ordered(agents.filter { $0.state == state })
            return members.isEmpty ? nil
                : StatusSection(state: state, subgroups: subgroups(of: members))
        }
        return Sections(statuses: statuses)
    }

    /// Bucket one already-ordered status section's members into repo
    /// subgroups: named repos alphabetical first, then the Other subgroup
    /// (no repo / unknown repo) LAST. Bucketing is a stable partition —
    /// each subgroup preserves the section's ts-desc/id order.
    static func subgroups(of members: [Agent]) -> [RepoSubgroup] {
        var byRepo: [String: [Agent]] = [:]
        var orphans: [Agent] = []
        for agent in members {
            if let repo = repoKey(of: agent) {
                byRepo[repo, default: []].append(agent)
            } else {
                orphans.append(agent)
            }
        }
        var subgroups = byRepo.keys.sorted()
            .map { RepoSubgroup(repo: $0, agents: byRepo[$0] ?? []) }
        if !orphans.isEmpty {
            subgroups.append(RepoSubgroup(repo: nil, agents: orphans))
        }
        return subgroups
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
