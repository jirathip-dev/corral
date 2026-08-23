import Foundation

// MARK: - Branch-name issue inference (D21, ported from clients/egui/src/infer.rs)

/// Branch-name → issue inference, DISPLAY-ONLY (DECISIONS.md D21).
///
/// A worktree branch like `issue-431-embed-project-management` hints at issue
/// 431, but the hint is NOT authoritative: authoritative linkage is the
/// snapshot's per-agent `issues` (daemon `GhIssueRef`, joined from GitHub's
/// `closingIssuesReferences` — G23). This parses the branch name, validates
/// the number against that fetched set, and renders a visually distinct
/// marker:
///
/// - `~#431` — inferred AND present in the fetched issue set;
/// - `~#431?` — inferred but NOT in the fetched set (flagged, never asserted
///   as real; a daemon without the G23 join validates against an empty set,
///   so every inference is flagged).
///
/// HARD RULE: inferred numbers are NEVER action-driving. The only public
/// surface is `InferredIssue.marker` (a display string); drive payload
/// builders (`CanonicalJSON` / `DriveClient`) take agent_id + waiting-on
/// claims and have no access to an inferred number — pinned by
/// `testInferredNumbersNeverReachDrivePayloads`.
///
/// Inference is pure + deterministic (branch + fetched set in → marker out),
/// so rendering is stable across renders and never flickers.
struct InferredIssue: Equatable {
    let number: UInt64
    /// True only when `number` is present in the fetched authoritative set.
    let known: Bool

    /// The distinct display marker: `~#N` (validated) or `~#N?` (flagged).
    /// The `~` prefix keeps it visually distinct from the authoritative
    /// `⑂ #N` refs.
    var marker: String {
        known ? "~#\(number)" : "~#\(number)?"
    }
}

enum IssueInference {
    /// Pure branch-name inference + validation against the fetched issue
    /// set. `nil` when the branch name infers no issue.
    static func infer(branch: String?, known: Set<UInt64>) -> InferredIssue? {
        guard let branch, let number = issueNumber(fromBranch: branch) else {
            return nil
        }
        return InferredIssue(number: number, known: known.contains(number))
    }

    /// Parse `issue-<N>…` / `#<N>…` style branch-name forms (the egui
    /// grammar in `clients/egui/src/infer.rs`, matched case-by-case):
    ///
    /// - `issue-<N>-…`, `issues-<N>…`, `issue/<N>…`, `issues/<N>…` anywhere
    ///   in the name;
    /// - `#<N>…` anywhere in the name.
    ///
    /// Precedence (F2, documented upstream): the `#<N>` form wins when both
    /// appear. Returns `nil` for every other shape: no number, number zero,
    /// leading non-digit after the marker, overflow, or bare numbers with no
    /// `issue`/`#` marker.
    static func issueNumber(fromBranch branch: String) -> UInt64? {
        let name = branch.trimmingCharacters(in: .whitespacesAndNewlines)
        if name.isEmpty {
            return nil
        }
        if let pos = name.firstIndex(of: "#"),
           let n = leadingNumber(name[name.index(after: pos)...]) {
            return n
        }
        for marker in ["issue-", "issues-", "issue/", "issues/"] {
            if let range = name.range(of: marker),
               let n = leadingNumber(name[range.upperBound...]) {
                return n
            }
        }
        return nil
    }

    /// Leading decimal digits of `text` as a nonzero `UInt64`.
    private static func leadingNumber(_ text: Substring) -> UInt64? {
        let digits = text.prefix(while: \.isASCIIDigit)
        guard !digits.isEmpty, let n = UInt64(digits), n > 0 else {
            return nil
        }
        return n
    }
}

private extension Character {
    var isASCIIDigit: Bool { isASCII && isNumber }
}

// MARK: - Issue chip (line 1 of the D24 row)

/// The line-1 issue chips: the authoritative `⑂ #N` (first `issues` ref,
/// G23; `+n` when the PR closes more) and, alongside it, the D21 inferred
/// `~#N` / `~#N?` marker — computed UNCONDITIONALLY against the agent's
/// authoritative issue set, exactly as the egui board does, so the
/// validated `~#N` form is reachable. The one redundant case is dropped:
/// an inference that merely repeats the authoritative chip's own number.
/// Display-only either way — chip numbers never reach a drive payload.
enum IssueChip: Equatable {
    case authoritative(UInt64, more: Int)
    case inferred(InferredIssue)

    var label: String {
        switch self {
        case .authoritative(let number, let more):
            return more > 0 ? "⑂ #\(number) +\(more)" : "⑂ #\(number)"
        case .inferred(let inferred):
            return inferred.marker
        }
    }

    /// True for the flagged / unvalidated inferred form (`~#N?`).
    var isFlagged: Bool {
        if case .inferred(let inferred) = self { return !inferred.known }
        return false
    }

    static func chips(for agent: Agent) -> [IssueChip] {
        var chips: [IssueChip] = []
        let issues = agent.workspace.issues
        if let first = issues.first {
            chips.append(.authoritative(first.number, more: issues.count - 1))
        }
        if let inferred = IssueInference.infer(branch: agent.workspace.branch,
                                               known: agent.knownIssueNumbers),
           !(inferred.known && inferred.number == issues.first?.number) {
            chips.append(.inferred(inferred))
        }
        return chips
    }
}

// MARK: - Tappable controls and selection

/// A navigation value for the agent detail surface. It carries only the
/// opaque, source-stable agent id; the detail view resolves the current
/// record again before every action.
struct AgentRoute: Hashable, Sendable {
    let agentId: String
}

/// State for the Idle/Done disclosure header. The UI owns the animation;
/// this value owns the testable collapsed → expanded transition.
struct IdleDoneDisclosure: Equatable, Sendable {
    var isExpanded = false

    mutating func toggle() {
        isExpanded.toggle()
    }

    var stateLabel: String { isExpanded ? "Expanded" : "Collapsed" }
}

/// The state that FleetView actually binds to for both disclosure and
/// navigation. Keeping the route array here means deletion reconciliation
/// mutates the same path that NavigationStack renders, rather than a shadow
/// selection bookkeeping object that can drift from the visible destination.
struct FleetViewState: Equatable, Sendable {
    var idleDoneDisclosure = IdleDoneDisclosure()
    var navigationPath: [AgentRoute] = []

    mutating func toggleIdleDone() {
        idleDoneDisclosure.toggle()
    }

    mutating func setIdleDoneExpanded(_ expanded: Bool) {
        idleDoneDisclosure.isExpanded = expanded
    }

    /// Test and non-SwiftUI callers can open the same route value used by the
    /// row NavigationLink.
    mutating func open(agentId: String) {
        navigationPath = [AgentRoute(agentId: agentId)]
    }

    /// A deleted agent must be removed from the actual NavigationStack path.
    mutating func reconcile(availableAgentIds: Set<String>) {
        navigationPath.removeAll { !availableAgentIds.contains($0.agentId) }
    }
}

/// The actions exposed by the per-agent detail surface. The board renders
/// only actions that are enabled by both the agent capability and the device
/// grant; the detail surface also renders disabled explanations.
enum RowAction: Equatable, Sendable {
    case approveDeny
    case prompt
    case interrupt
    case tail
    case kill
    case attach

    var label: String {
        switch self {
        case .approveDeny: return "Approval"
        case .prompt: return "Prompt"
        case .interrupt: return "Interrupt"
        case .tail: return "Recent output"
        case .kill: return "Kill"
        case .attach: return "Attach"
        }
    }

    var capability: Capability? {
        switch self {
        case .approveDeny: return .approve
        case .prompt: return .prompt
        case .interrupt: return .interrupt
        case .tail: return .readTail
        case .kill: return .kill
        case .attach: return .attach
        }
    }
}

struct AgentActionAvailability: Equatable, Sendable {
    let action: RowAction
    let isEnabled: Bool
    let disabledReason: String?

    var label: String { action.label }
}

// MARK: - D25 hierarchy (sections + within-repo ordering)

/// The D25 board shape, computed as a pure function of the agent set so it
/// is unit-testable and stable across renders:
///
/// - `needsYou` — every blocked agent, cross-repo, ts desc. A PROMOTION,
///   not a filter: the same agents also appear in their repo section.
/// - `repos` — one section per `workspace.repo` (named repos sorted by
///   name), holding the repo's blocked/working/unknown agents; the orphan
///   bucket (repo = nil) sorts last (D25).
/// - `idleDone` — every idle/done agent, cross-repo, collapsed by default
///   in the UI (D25/D28: 2/3 of rows are idle or done at any moment; the
///   board surfaces active work and tucks the rest away).
///
/// Within every section the ordering is the D25 rank — blocked > done >
/// working > idle > unknown — then ts desc, then agent id for determinism.
enum BoardModel {
    struct RepoSection: Equatable {
        /// `nil` = the orphan bucket (agents without `workspace.repo`).
        let repo: String?
        /// The ACTIVE agents shown in the section (idle/done live in the
        /// collapsed bucket instead).
        let agents: [Agent]
        /// Every agent of this repo, including its idle/done ones — so the
        /// header can say "corral (2/8)" instead of under-reporting.
        let total: Int

        /// Header count label: `2/8` when idle/done agents are tucked away
        /// in the collapsed bucket, plain `2` when nothing is hidden.
        var countLabel: String {
            total > agents.count ? "\(agents.count)/\(total)" : "\(agents.count)"
        }
    }

    struct Sections: Equatable {
        let needsYou: [Agent]
        let repos: [RepoSection]
        let idleDone: [Agent]
    }

    /// D25 state rank. Delegates to the shared `StateStyle` contract
    /// (`contracts/state-tokens.json`) so board ordering can never diverge
    /// from the state vocabulary again: blocked(0) > done(1) > working(2) >
    /// idle(3) > unknown(4). This is the approved attention-order, re-pointed
    /// from the old working-before-done convention (carried finding 8b).
    static func stateRank(_ state: AgentState) -> Int {
        StateStyle.style(for: state).rank
    }

    /// The canonical board ordering: rank, then ts desc, then agent id.
    static func ordered(_ agents: [Agent]) -> [Agent] {
        agents.sorted { a, b in
            let ra = stateRank(a.state), rb = stateRank(b.state)
            if ra != rb { return ra < rb }
            if a.ts != b.ts { return a.ts > b.ts }
            return a.agentId < b.agentId
        }
    }

    static func sections(_ agents: [Agent]) -> Sections {
        let needsYou = ordered(agents.filter(\.isBlocked))
        let idleDone = ordered(agents.filter { $0.state == .idle || $0.state == .done })
        let active = agents.filter { $0.state != .idle && $0.state != .done }

        var totalByRepo: [String?: Int] = [:]
        for agent in agents {
            totalByRepo[agent.workspace.repo, default: 0] += 1
        }
        var byRepo: [String?: [Agent]] = [:]
        for agent in active {
            byRepo[agent.workspace.repo, default: []].append(agent)
        }
        let repos = byRepo
            .map {
                RepoSection(repo: $0.key, agents: ordered($0.value),
                            total: totalByRepo[$0.key] ?? $0.value.count)
            }
            .sorted { a, b in
                switch (a.repo, b.repo) {
                case (let x?, let y?): return x < y
                case (.some, .none): return true
                case (.none, .some): return false
                case (.none, .none): return false
                }
            }
        return Sections(needsYou: needsYou, repos: repos, idleDone: idleDone)
    }

    /// Enabled actions for one agent. The per-action detail surface uses
    /// `actionAvailability` below to explain disabled controls; this helper
    /// is the compact enabled-only projection used by board-level policy and
    /// tests.
    static func rowActions(agent: Agent, grants: Set<Capability>) -> [RowAction] {
        actionAvailability(agent: agent, grants: grants)
            .filter(\.isEnabled)
            .map(\.action)
    }

    /// Full action matrix for the detail surface. Reasons deliberately name
    /// the missing capability or grant so a read-only device never presents
    /// an unexplained disabled control.
    static func actionAvailability(agent: Agent,
                                   grants: Set<Capability>) -> [AgentActionAvailability] {
        [
            availability(.tail, agent: agent, grants: grants),
            availability(.prompt, agent: agent, grants: grants),
            availability(.interrupt, agent: agent, grants: grants),
            availability(.kill, agent: agent, grants: grants),
            availability(.attach, agent: agent, grants: grants),
            availability(.approveDeny, agent: agent, grants: grants),
        ]
    }

    private static func availability(_ action: RowAction, agent: Agent,
                                     grants: Set<Capability>) -> AgentActionAvailability {
        if action == .approveDeny {
            guard agent.isBlocked, let waiting = agent.waitingOn else {
                return AgentActionAvailability(
                    action: action,
                    isEnabled: false,
                    disabledReason: "Approval is available only while this agent is blocked on a live claim.")
            }
            if waiting.kind == .crash {
                return AgentActionAvailability(
                    action: action,
                    isEnabled: false,
                    disabledReason: "Crash states do not accept approval replies.")
            }
        }

        guard let capability = action.capability else {
            return AgentActionAvailability(action: action, isEnabled: false,
                                           disabledReason: "This action is not available for this agent.")
        }
        guard agent.capabilities.contains(capability.rawValue) else {
            return AgentActionAvailability(
                action: action,
                isEnabled: false,
                disabledReason: "\(capability.rawValue): not available for this agent.")
        }
        guard grants.contains(capability) else {
            // Kill gets a plain-language reason (issue #166 item 4): the
            // sentence names the missing grant too, so the existing
            // "name the grant" contract tests stay green while a read-only
            // device says WHY in human terms instead of a bare token.
            if action == .kill {
                return AgentActionAvailability(
                    action: action,
                    isEnabled: false,
                    disabledReason: "You don't have permission to kill agents on this host (missing the kill grant — ask the host).")
            }
            return AgentActionAvailability(
                action: action,
                isEnabled: false,
                disabledReason: "requires the \(capability.rawValue) grant — ask the host.")
        }
        return AgentActionAvailability(action: action, isEnabled: true, disabledReason: nil)
    }

    // MARK: - Answer-loop prominence (#166 item 3)

    /// ONE primary action per state, chosen by contract order:
    /// blocked → answer, working → interrupt, done → attach/PR, and
    /// idle/unknown → none. Everything else lives in the overflow menu.
    static func primaryAction(for agent: Agent) -> RowPrimaryAction {
        switch agent.state {
        case .blocked: return .answer
        case .working: return .interrupt
        case .done: return .attach
        case .idle, .unknown: return .none
        }
    }

    // MARK: - Zero-state rule (#166 item 7)

    /// The cross-repo "Needs you" section is hidden entirely when no agent
    /// is blocked — no `Needs you (0)` header, no "No blocked agents" empty
    /// row. Returns `nil` in that case (a testable pure projection the view
    /// uses to decide whether to render the section at all).
    static func needsYouSection(_ agents: [Agent]) -> [Agent]? {
        let blocked = ordered(agents.filter(\.isBlocked))
        return blocked.isEmpty ? nil : blocked
    }
}

/// The primary action for the answer loop, rendered as the single prominent
/// control on the row/detail surface (issue #166 item 3).
enum RowPrimaryAction: Equatable, Sendable {
    case answer
    case interrupt
    case attach
    case none

    var label: String {
        switch self {
        case .answer: return "Answer"
        case .interrupt: return "Interrupt"
        case .attach: return "Attach"
        case .none: return ""
        }
    }
}
