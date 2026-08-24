import Foundation

// MARK: - Fleet filter/search model (#166 item 5)

/// Pure, testable filter/search model for the fleet board. The UI renders a
/// pinned filter-chip row (`All · Needs you · repo₁…repoₙ`) and a
/// `.searchable` field; this model is the single source of truth for what
/// passes the chip + query, independent of SwiftUI. Mirrors the egui search
/// (#168).
enum BoardFilterChip: Equatable, Hashable, Sendable {
    case all
    case needsYou
    case repo(String)

    var label: String {
        switch self {
        case .all: return "All"
        case .needsYou: return "Needs you"
        case .repo(let name): return name
        }
    }
}

enum BoardFilter {
    /// Chips pinned above the list, in display order: All, Needs you, then
    /// every named repo present in the snapshot (sorted). Repos not present
    /// in the snapshot are never pinned, so the row cannot drift from data.
    static func chips(for agents: [Agent]) -> [BoardFilterChip] {
        var repos = Set<String>()
        for agent in agents {
            if let repo = agent.workspace.repo {
                repos.insert(repo)
            }
        }
        return [.all, .needsYou] + repos.sorted().map(BoardFilterChip.repo)
    }

    /// True when the agent survives this filter chip.
    static func keeps(_ chip: BoardFilterChip, _ agent: Agent) -> Bool {
        switch chip {
        case .all: return true
        case .needsYou: return agent.isBlocked
        case .repo(let name): return agent.workspace.repo == name
        }
    }

    /// Case-insensitive search over repo / branch / title / issue. An empty
    /// query keeps every agent.
    static func matches(_ query: String, _ agent: Agent) -> Bool {
        let q = query.trimmingCharacters(in: .whitespacesAndNewlines).lowercased()
        if q.isEmpty { return true }
        return searchableText(agent).lowercased().contains(q)
    }

    /// The text searched for one agent: repo, branch, title, displayName,
    /// agentId, and the authoritative + inferred issue markers (so `158`
    /// matches an `⑂ #158` chip and a `~#158?` flag alike). Title and
    /// identity are all always included — a user reading the row's
    /// `displayName`/`agentId` must be able to find that agent too.
    static func searchableText(_ agent: Agent) -> String {
        var parts: [String] = []
        if let repo = agent.workspace.repo { parts.append(repo) }
        if let branch = agent.workspace.branch { parts.append(branch) }
        if let title = agent.title { parts.append(title) }
        if let name = agent.displayName { parts.append(name) }
        parts.append(agent.agentId)
        for issue in agent.workspace.issues { parts.append("\(issue.number)") }
        for chip in IssueChip.chips(for: agent) { parts.append(chip.label) }
        return parts.joined(separator: " ")
    }

    /// Apply chip + query together (the board's filtered projection).
    static func filtered(_ agents: [Agent], chip: BoardFilterChip, query: String) -> [Agent] {
        agents.filter { keeps(chip, $0) && matches(query, $0) }
    }
}
