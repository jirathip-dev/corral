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
///
/// ## Composite target (#397)
///
/// `hostId` (wire `host_id`) is the owning daemon's pinned X25519 public
/// key — the same key the phone pins per host profile. It is parsed and
/// retained for BOTH the APNs path and the DEBUG local bridge so a tap can
/// resolve EXACTLY one host profile; a display name/URL is never a routing
/// identity. Equal raw agent ids on two hosts stay distinct: request
/// identifiers and thread identifiers are namespaced by the composite
/// target (`hostId` + raw `agentId`).
struct PushPayload: Sendable, Equatable {
    enum PushType: String, Sendable {
        case started
        case blocked
        case finished
    }

    var type: PushType
    var agentId: String
    /// #397: the owning daemon's X25519 host public key (base64), nil on
    /// legacy payloads (pre-#397 daemons) and the legacy single-host
    /// DEBUG bridge.
    var hostId: String?
    var ts: UInt64?
    /// Display text from `aps.alert` (title "agent · repo", body "state · branch").
    var title: String?
    var body: String?

    init(type: PushType,
         agentId: String,
         hostId: String? = nil,
         ts: UInt64? = nil,
         title: String? = nil,
         body: String? = nil) {
        self.type = type
        self.agentId = agentId
        self.hostId = hostId
        self.ts = ts
        self.title = title
        self.body = body
    }

    static func transition(type: PushType, agent: Agent,
                           hostId: String? = nil) -> PushPayload {
        let repo = agent.workspace.repo ?? "no repo"
        let branch = agent.workspace.branch ?? "no branch"
        let state = StateStyle.style(for: agent.state).label
        let name = agent.displayName ?? agent.agentId
        return PushPayload(
            type: type,
            agentId: agent.agentId,
            hostId: hostId,
            ts: UInt64(Date().timeIntervalSince1970),
            title: "\(name) · \(repo)",
            body: "\(state) · \(branch)")
    }

    // MARK: - Parsing (APNs userInfo and the local bridge's userInfo share
    // the same key shape: type/agent_id/host_id/ts, plus aps.alert.{title,body})

    private enum Keys {
        static let type = "type"
        static let agentId = "agent_id"
        static let hostId = "host_id"
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
            hostId: userInfo[Keys.hostId] as? String,
            ts: (userInfo[Keys.ts] as? NSNumber)?.uint64Value,
            title: alert?[Keys.title] as? String,
            body: alert?[Keys.body] as? String)
    }

    /// The userInfo dict the DEBUG local bridge embeds — byte-identical
    /// key shape to the daemon's APNs payload (host_id included when the
    /// owning host is pinned, #397).
    func asUserInfo() -> [AnyHashable: Any] {
        var info: [AnyHashable: Any] = [
            Keys.type: type.rawValue,
            Keys.agentId: agentId,
            Keys.ts: ts ?? UInt64(Date().timeIntervalSince1970),
            Keys.aps: [Keys.alert: [
                Keys.title: title ?? "",
                Keys.body: body ?? "",
            ]],
        ]
        if let hostId {
            info[Keys.hostId] = hostId
        }
        return info
    }

    // MARK: - Composite identifiers (#397)

    /// The local-notification request identifier: namespaced by the
    /// composite target so equal raw agent ids on two hosts never
    /// overwrite each other's pending/delivered notification. Legacy
    /// host-less payloads keep the pre-#397 identifier shape.
    var requestIdentifier: String {
        if let hostId {
            return "\(type.rawValue)-\(hostId)-\(agentId)"
        }
        return "\(type.rawValue)-\(agentId)"
    }

    /// The Notification Center thread identifier: the daemon's composite
    /// `host_id::agent_id` aps.thread-id for host-bearing payloads, else
    /// the raw agent id (legacy parity). The DEBUG bridge mirrors this so
    /// both paths group identically.
    var threadIdentifier: String {
        if let hostId {
            return "\(hostId)::\(agentId)"
        }
        return agentId
    }

    /// #397: the wire host identity of the composite target (nil on
    /// legacy payloads) — used to route a tap to EXACTLY one profile.
    var compositeHostId: String? { hostId }
}
