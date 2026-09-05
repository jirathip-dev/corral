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

    // MARK: - #386 status-section collapse (per-session view state)

    /// Which status sections the user collapsed during THIS board session.
    /// A fresh state has every section EXPANDED; `toggle(_:)` collapses an
    /// expanded section and expands a collapsed one. The state is
    /// in-memory only and is NEVER persisted (consistent with #373 recents
    /// blocks): the board owns one instance per session, so a relaunch
    /// always starts fully expanded. Sections are keyed by
    /// `AgentState.rawValue` — the same stable id `StatusSection` exposes —
    /// so no wire model needs new conformances.
    struct StatusSectionCollapse: Equatable, Sendable {
        private(set) var collapsed: Set<String> = []

        /// A fresh board session: every status section expanded.
        static let fresh = StatusSectionCollapse()

        func isCollapsed(_ state: AgentState) -> Bool {
            collapsed.contains(state.rawValue)
        }

        /// Collapse an expanded section / expand a collapsed one.
        mutating func toggle(_ state: AgentState) {
            if collapsed.contains(state.rawValue) {
                collapsed.remove(state.rawValue)
            } else {
                collapsed.insert(state.rawValue)
            }
        }

        /// Collapse a section (no-op when already collapsed). Idempotent —
        /// the deterministic evidence driver uses this so a re-fired task
        /// can never undo a collapse (toggle stays the interactive path).
        mutating func collapse(_ state: AgentState) {
            collapsed.insert(state.rawValue)
        }
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

// MARK: - #401 multi-host board projections (D1-D7 view-model, consumes #400 rows)

/// #401: the multi-host board consumes `HostBoardRow` (the #400 composite
/// read-model row with staleness + last-seen facts) and NEVER re-derives
/// ranking: rows arrive in `HostBoardProjection`'s canonical + live-first
/// order. The projections below are pure view-model helpers over those rows
/// plus per-host runtime facts — host/repo filters, host chips (D2/D3/D4),
/// the board-level outage summary (D7), and the status-section → repo-
/// subgroup buckets that MERGE equal repo names across hosts (D5).
extension BoardModel {

    /// Per-host runtime facts the host-filter chips and Settings host rows
    /// render from (D3/D7). Produced by the model from #399/#400 state
    /// (`connectionState`/posture/continuity) — never re-derived here.
    struct HostRuntimeFacts: Equatable, Sendable {
        var isConnected = false
        var isConnecting = false
        var keyMismatch = false
        var awaitingFingerprint = false
    }

    /// The health posture of one host chip (D3/D7). Textual label ALWAYS
    /// rides with the color — color is never the only channel (D8).
    enum HostChipHealth: Equatable, Sendable {
        case live
        case connecting
        case offline
        case keyMismatch
        case awaitingFingerprint

        var label: String {
            switch self {
            case .live: return "live"
            case .connecting: return "connecting"
            case .offline: return "offline"
            case .keyMismatch: return "key mismatch"
            case .awaitingFingerprint: return "awaiting fingerprint"
            }
        }
    }

    /// Classify one host's runtime facts into the chip health vocabulary.
    /// Mismatch and unconfirmed pins fail closed and OUT-rank any store
    /// connection state (B4/B6: such a host is never live).
    static func hostChipHealth(for facts: HostRuntimeFacts) -> HostChipHealth {
        if facts.keyMismatch { return .keyMismatch }
        if facts.awaitingFingerprint { return .awaitingFingerprint }
        if facts.isConnected { return .live }
        if facts.isConnecting { return .connecting }
        return .offline
    }

    /// #401 D8: the Catppuccin TOKEN for each host health posture — the
    /// single mapping both the board chips and the Settings host rows
    /// resolve, so the four themes can never diverge. Color is NEVER the
    /// only channel: the textual health label and VoiceOver values always
    /// accompany it.
    static func hostHealthToken(_ health: HostChipHealth) -> CatppuccinToken {
        switch health {
        case .live: return .green
        case .connecting: return .yellow
        case .offline: return .peach
        case .keyMismatch: return .red
        case .awaitingFingerprint: return .surface2
        }
    }

    /// One host-filter chip (D2/D3): a profile in user-controlled order, its
    /// TOTAL lane count (independent of the repo filter), and its health.
    /// `profileID == nil` is the All chip carrying the UNIFIED lane count and
    /// the aggregate (partial) health of every host.
    struct HostFilterChip: Identifiable, Equatable, Sendable {
        let profileID: UUID?
        let displayName: String
        let laneCount: Int
        let health: HostChipHealth

        var isAll: Bool { profileID == nil }
        var id: String { profileID?.uuidString ?? "all-hosts" }
    }

    /// Lane counts per host profile over an aggregate row set (D3).
    static func laneCounts(_ rows: [HostBoardRow]) -> [UUID: Int] {
        rows.reduce(into: [:]) { counts, row in
            counts[row.identity.hostProfileID, default: 0] += 1
        }
    }

    /// The host chip row in render order: All first, then every host in the
    /// given (user-controlled) order — zero-lane and offline hosts stay
    /// visible (D3). The All chip shows the unified count and the aggregate
    /// partial health: live only when EVERY host is live; otherwise the
    /// worst posture (mismatch > awaiting > offline > connecting) so the
    /// chip can never claim full health during a partial outage (D3/D7).
    static func hostChips(hosts: [HostFilterChip]) -> [HostFilterChip] {
        let allLaneCount = hosts.reduce(0) { $0 + $1.laneCount }
        let kinds = Set(hosts.map(\.health))
        let allHealth: HostChipHealth
        if kinds.isEmpty || kinds == [.live] {
            allHealth = .live
        } else if kinds.contains(.keyMismatch) {
            allHealth = .keyMismatch
        } else if kinds.contains(.awaitingFingerprint) {
            allHealth = .awaitingFingerprint
        } else if kinds.contains(.offline) {
            allHealth = .offline
        } else {
            allHealth = .connecting
        }
        return [HostFilterChip(profileID: nil, displayName: "All",
                               laneCount: allLaneCount, health: allHealth)]
            + hosts
    }

    /// The compact board-level outage summary (D7): nil when every host is
    /// live; otherwise "1 host offline"-style text naming each unreachable
    /// kind. Never a full-width reconnect banner.
    static func hostOutageSummary(hosts: [HostFilterChip]) -> String? {
        var parts: [String] = []
        let offline = hosts.filter { $0.health == .offline }.count
        if offline == 1 {
            parts.append("1 host offline")
        } else if offline > 1 {
            parts.append("\(offline) hosts offline")
        }
        let mismatch = hosts.filter { $0.health == .keyMismatch }.count
        if mismatch == 1 {
            parts.append("1 host key mismatch")
        } else if mismatch > 1 {
            parts.append("\(mismatch) hosts key mismatch")
        }
        let awaiting = hosts.filter { $0.health == .awaitingFingerprint }.count
        if awaiting == 1 {
            parts.append("1 host awaiting fingerprint")
        } else if awaiting > 1 {
            parts.append("\(awaiting) hosts awaiting fingerprint")
        }
        return parts.isEmpty ? nil : parts.joined(separator: " · ")
    }

    /// D2/D4: keep the rows of ONE host (nil = every host).
    static func rows(_ rows: [HostBoardRow], forHost hostProfileID: UUID?) -> [HostBoardRow] {
        guard let hostProfileID else { return rows }
        return rows.filter { $0.identity.hostProfileID == hostProfileID }
    }

    /// D4: keep the rows of one repo (nil = every repo). Host + repo both
    /// apply because the caller filters by host first, then repo.
    static func rows(_ rows: [HostBoardRow], in repo: String?) -> [HostBoardRow] {
        guard let repo else { return rows }
        return rows.filter { $0.agent.workspace.repo == repo }
    }

    /// D4: the repo chip set over the ROWS of the selected host (All Hosts =
    /// the unified set). Choices AND counts recalculate when the host
    /// selection changes; chips never zero other chips' counts.
    static func repoFilters(_ rows: [HostBoardRow]) -> [RepoFilterChip] {
        let counts = rows.reduce(into: [String: Int]()) { counts, row in
            guard let repo = row.agent.workspace.repo else { return }
            counts[repo, default: 0] += 1
        }
        return counts.keys.sorted()
            .map { RepoFilterChip(repo: $0, count: counts[$0] ?? 0) }
    }

    /// #401 D5/C6/C7: one always-open repo subgroup INSIDE a host-status
    /// section. Rows from SEVERAL hosts sharing one repo name land in the
    /// same subgroup (no host sections/tabs — D5); the subgroup preserves
    /// the incoming #400 ranking (live rows before stale rows inside the
    /// same (state, repo) bucket — C7) and the raw state tokens are never
    /// recast.
    struct HostRepoSubgroup: Equatable, Identifiable, Sendable {
        /// The workspace repo; `nil` = the Other subgroup (no repo /
        /// unknown repo), always rendered last.
        let repo: String?
        let rows: [HostBoardRow]

        var id: String { repo ?? BoardModel.otherRepoLabel }
        var displayName: String { repo ?? BoardModel.otherRepoLabel }
        var header: String { "\(displayName) (\(rows.count))" }
    }

    /// #401 D5: one status section of the multi-host board — raw herdr state
    /// in the locked attention order, holding its repo subgroups.
    struct HostStatusSection: Equatable, Identifiable, Sendable {
        let state: AgentState
        let subgroups: [HostRepoSubgroup]

        var id: String { state.rawValue }
        var total: Int { subgroups.reduce(0) { $0 + $1.rows.count } }
        var header: String { "\(state.displayName) (\(total))" }
    }

    struct HostSections: Equatable, Sendable {
        let statuses: [HostStatusSection]
    }

    /// Bucket ALREADY-RANKED rows into the locked status sections and repo
    /// subgroups. The incoming order is #400's canonical + live-first order;
    /// this partition is a STABLE filter/bucket pass (D5/C7) — it never
    /// re-sorts rows or recasts a state.
    static func hostSections(_ rows: [HostBoardRow]) -> HostSections {
        let lockedOrder: [AgentState] = [.blocked, .working, .idle, .done, .unknown]
        let statuses = lockedOrder.compactMap { state -> HostStatusSection? in
            let members = rows.filter { $0.agent.state == state }
            guard !members.isEmpty else { return nil }
            return HostStatusSection(state: state, subgroups: hostSubgroups(of: members))
        }
        return HostSections(statuses: statuses)
    }

    /// Bucket one status partition into repo subgroups: named repos in
    /// alphabetical order, then the Other subgroup (no repo / unknown repo)
    /// LAST. Equal repo names from several hosts share one subgroup (D5);
    /// each subgroup preserves the incoming row order.
    static func hostSubgroups(of rows: [HostBoardRow]) -> [HostRepoSubgroup] {
        var byRepo: [String: [HostBoardRow]] = [:]
        var orphans: [HostBoardRow] = []
        for row in rows {
            if let repo = repoKey(of: row.agent) {
                byRepo[repo, default: []].append(row)
            } else {
                orphans.append(row)
            }
        }
        var subgroups = byRepo.keys.sorted()
            .map { HostRepoSubgroup(repo: $0, rows: byRepo[$0] ?? []) }
        if !orphans.isEmpty {
            subgroups.append(HostRepoSubgroup(repo: nil, rows: orphans))
        }
        return subgroups
    }

    /// Grants/expiry read-out text for a host Settings row (D7). Expiry is
    /// the daemon epoch-seconds grant deadline, rendered in UTC date text so
    /// the copy is locale-independent and testable.
    static func expiryText(epochSeconds: UInt64?) -> String? {
        guard let epochSeconds, epochSeconds > 0 else { return nil }
        let date = Date(timeIntervalSince1970: TimeInterval(epochSeconds))
        var calendar = Calendar(identifier: .gregorian)
        calendar.timeZone = TimeZone(identifier: "UTC") ?? calendar.timeZone
        let parts = calendar.dateComponents([.year, .month, .day], from: date)
        guard let year = parts.year, let month = parts.month, let day = parts.day else {
            return nil
        }
        return String(format: "%04d-%02d-%02d", year, month, day)
    }
}
