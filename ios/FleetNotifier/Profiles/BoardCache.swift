import Foundation

// MARK: - Durable board metadata cache (#399 C5)

/// One allowlisted board-cache row (C5): the ONLY fields a host profile's
/// durable cache may store. The DTO is enforced BY TYPE — it has no line,
/// block, transcript, token, or private-key fields, so read_tail content,
/// pairing tokens, and key material cannot reach durable storage through
/// this surface. `snapshot` builds rows from the read model's metadata
/// fields only.
struct BoardCacheRow: Codable, Equatable, Identifiable, Sendable {
    /// Composite identity (C2): `host_profile_id::raw_agent_id`. Stored
    /// per-host files keep rows scoped; the composite key is what #400's
    /// routing consumes.
    var compositeIdentity: String
    var hostProfileID: UUID
    /// Raw agent id unchanged from the wire.
    var agentID: String
    /// Raw herdr state token (working/idle/blocked/done/unknown).
    var state: String
    /// Epoch millis when the row last changed (agent.ts).
    var ts: UInt64
    /// Epoch millis when the current state was entered (state clock).
    var stateEnteredAt: UInt64
    /// Board-display metadata — ALREADY redacted by the daemon/client
    /// before it reaches this DTO (same values the live board renders).
    var displayName: String?
    var title: String?
    var reason: String?
    var tool: String?
    /// Pane reference (attachment "ref" — the debug pane id).
    var paneReference: String?
    /// Git topology facts the board renders.
    var repo: String?
    var branch: String?
    /// Worktree basename (never the full path).
    var basename: String?
    /// Epoch millis of the last time this row was seen on a live feed.
    var lastSeen: UInt64

    var id: String { compositeIdentity }

    enum CodingKeys: String, CodingKey {
        case compositeIdentity = "composite_identity"
        case hostProfileID = "host_profile_id"
        case agentID = "agent_id"
        case state, ts
        case stateEnteredAt = "state_entered_at"
        case displayName = "display_name"
        case title, reason, tool
        case paneReference = "pane_reference"
        case repo, branch, basename
        case lastSeen = "last_seen"
    }
}

/// Pure projection from the live read model to the allowlisted DTO set.
/// Any future board metadata the cache must hold gets a field HERE (and a
/// scan-test update) — never ad-hoc dictionaries.
enum BoardCacheDTO {
    static func snapshot(hostProfileID: UUID,
                         agents: [String: Agent],
                         stateEnteredAt: [String: UInt64],
                         now: UInt64) -> [BoardCacheRow] {
        agents.values.map { agent in
            BoardCacheRow(compositeIdentity: composite(hostProfileID: hostProfileID,
                                                       agentID: agent.agentId),
                          hostProfileID: hostProfileID,
                          agentID: agent.agentId,
                          state: agent.state.rawValue,
                          ts: agent.ts,
                          stateEnteredAt: stateEnteredAt[agent.agentId] ?? agent.ts,
                          displayName: agent.displayName,
                          title: agent.title,
                          reason: agent.reason,
                          tool: agent.tool,
                          paneReference: agent.attachment?.reference,
                          repo: agent.workspace.repo,
                          branch: agent.workspace.branch,
                          basename: agent.workspace.worktreePath.map {
                              ($0 as NSString).lastPathComponent
                          },
                          lastSeen: now)
        }
    }

    static func composite(hostProfileID: UUID, agentID: String) -> String {
        "\(hostProfileID.uuidString)::\(agentID)"
    }
}

/// File-backed per-profile durable metadata cache (C5): one JSON document
/// per host under the store directory, written ATOMICALLY with iOS file
/// protection. Never persists read_tail lines/blocks, transcript content,
/// pairing tokens, private key material, or unneeded waiting prompts —
/// the only values that can be encoded are the allowlisted `BoardCacheRow`
/// fields. In-memory when `directory` is nil (tests).
final class BoardCacheStore {
    private let directory: URL?
    private var memoryRows: [UUID: [BoardCacheRow]] = [:]

    init(directory: URL?) {
        self.directory = directory
    }

    func cacheFileURL(for profileID: UUID) -> URL? {
        directory?.appendingPathComponent("board-cache-\(profileID.uuidString).json")
    }

    /// Replace the host's durable cache atomically.
    func save(_ rows: [BoardCacheRow], for profileID: UUID) {
        guard let url = cacheFileURL(for: profileID) else {
            memoryRows[profileID] = rows
            return
        }
        let fm = FileManager.default
        if let directory {
            try? fm.createDirectory(at: directory, withIntermediateDirectories: true)
        }
        guard let data = try? JSONEncoder().encode(rows) else { return }
        try? data.write(to: url, options: [.atomic, .completeFileProtection])
        memoryRows[profileID] = rows
    }

    func load(for profileID: UUID) -> [BoardCacheRow]? {
        if let cached = memoryRows[profileID] { return cached }
        guard let url = cacheFileURL(for: profileID),
              let data = try? Data(contentsOf: url),
              let decoded = try? JSONDecoder().decode([BoardCacheRow].self, from: data) else {
            return nil
        }
        memoryRows[profileID] = decoded
        return decoded
    }

    /// Remove one host's cache document + in-memory rows (B7 purge).
    func remove(for profileID: UUID) {
        memoryRows.removeValue(forKey: profileID)
        guard let url = cacheFileURL(for: profileID) else { return }
        try? FileManager.default.removeItem(at: url)
    }
}
