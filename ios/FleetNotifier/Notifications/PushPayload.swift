import Foundation

/// One state-change push from the daemon (#354 L2) — the exact payload an
/// APNs body carries (`type` + agent identity + `aps`) AND the DEBUG-only
/// local-notification bridge embeds, so ONE handler serves both paths.
///
/// Transitions (herdr 0.8.2 vocabulary; spec amendment 09-02):
/// - started: idle→working (episode start)
/// - blocked: →blocked
/// - finished: working→idle (episode end, fires ONCE per episode)
///
/// Content contract: title "agent · repo", body "state · branch". Tap
/// deep-links to the agent's row with recents open — no reply actions.
struct PushPayload: Sendable, Equatable {
    enum PushType: String, Sendable {
        case started
        case blocked
        case finished
    }

    var type: PushType
    var agentId: String
    var ts: UInt64?
    /// Display text from `aps.alert` (title "agent · repo", body "state · branch").
    var title: String?
    var body: String?

    static func transition(type: PushType, agent: Agent) -> PushPayload {
        let repo = agent.workspace.repo ?? "no repo"
        let branch = agent.workspace.branch ?? "no branch"
        let state = StateStyle.style(for: agent.state).label
        let name = agent.displayName ?? agent.agentId
        return PushPayload(
            type: type,
            agentId: agent.agentId,
            ts: UInt64(Date().timeIntervalSince1970),
            title: "\(name) · \(repo)",
            body: "\(state) · \(branch)")
    }

    // MARK: - Parsing (APNs userInfo and the local bridge's userInfo share
    // the same key shape: type/agent_id/ts, plus aps.alert.{title,body})

    private enum Keys {
        static let type = "type"
        static let agentId = "agent_id"
        static let ts = "ts"
        static let aps = "aps"
        static let alert = "alert"
        static let title = "title"
        static let body = "body"
    }

    /// Parse a notification's `userInfo` into the push payload. Returns
    /// nil for anything that is not one of the three transition surfaces.
    static func parse(userInfo: [AnyHashable: Any]) -> PushPayload? {
        guard let typeRaw = userInfo[Keys.type] as? String,
              let type = PushType(rawValue: typeRaw),
              let agentId = userInfo[Keys.agentId] as? String else {
            return nil
        }
        let aps = userInfo[Keys.aps] as? [AnyHashable: Any]
        let alert = aps?[Keys.alert] as? [AnyHashable: Any]
        return PushPayload(
            type: type,
            agentId: agentId,
            ts: (userInfo[Keys.ts] as? NSNumber)?.uint64Value,
            title: alert?[Keys.title] as? String,
            body: alert?[Keys.body] as? String)
    }

    /// The userInfo dict the DEBUG local bridge embeds — byte-identical
    /// key shape to the daemon's APNs payload.
    func asUserInfo() -> [AnyHashable: Any] {
        [
            Keys.type: type.rawValue,
            Keys.agentId: agentId,
            Keys.ts: ts ?? UInt64(Date().timeIntervalSince1970),
            Keys.aps: [Keys.alert: [
                Keys.title: title ?? "",
                Keys.body: body ?? "",
            ]],
        ]
    }
}
