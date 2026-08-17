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
    /// grammar, ported verbatim):
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
        let name = branch.trimmingCharacters(in: .whitespaces)
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

/// The line-1 issue chip: authoritative `⑂ #N` (first `issues` ref, G23)
/// when the daemon joined one, else the D21 inferred `~#N` / `~#N?` marker.
/// Display-only either way — chip numbers never reach a drive payload.
enum IssueChip: Equatable {
    case authoritative(UInt64)
    case inferred(InferredIssue)

    var label: String {
        switch self {
        case .authoritative(let number): return "⑂ #\(number)"
        case .inferred(let inferred): return inferred.marker
        }
    }

    /// True for the flagged / unvalidated inferred form (`~#N?`).
    var isFlagged: Bool {
        if case .inferred(let inferred) = self { return !inferred.known }
        return false
    }

    static func chip(for agent: Agent) -> IssueChip? {
        if let first = agent.issues.first {
            return .authoritative(first.number)
        }
        if let inferred = IssueInference.infer(branch: agent.workspace.branch,
                                               known: agent.knownIssueNumbers) {
            return .inferred(inferred)
        }
        return nil
    }
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
/// Within every section the ordering is the D25 rank — blocked > working >
/// done > idle > unknown — then ts desc, then agent id for determinism.
enum BoardModel {
    struct RepoSection: Equatable {
        /// `nil` = the orphan bucket (agents without `workspace.repo`).
        let repo: String?
        let agents: [Agent]
    }

    struct Sections: Equatable {
        let needsYou: [Agent]
        let repos: [RepoSection]
        let idleDone: [Agent]
    }

    /// D25 state rank: blocked > working > done > idle > unknown.
    static func stateRank(_ state: AgentState) -> Int {
        switch state {
        case .blocked: return 0
        case .working: return 1
        case .done: return 2
        case .idle: return 3
        case .unknown: return 4
        }
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

        var byRepo: [String?: [Agent]] = [:]
        for agent in active {
            byRepo[agent.workspace.repo, default: []].append(agent)
        }
        let repos = byRepo
            .map { RepoSection(repo: $0.key, agents: ordered($0.value)) }
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
}
