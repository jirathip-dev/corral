import Foundation

/// One push notification from the daemon (D16, issue #26) — the exact
/// payload the APNs body carries (`type` + claim keys + `aps`) AND the
/// DEBUG-only local-notification bridge embeds, so ONE handler serves both
/// paths.
///
/// Lock-screen replies are bound to the notification's `prompt_hash`
/// ([`NotificationReplyValidator`]): a stale notification (the agent moved
/// on, or the prompt changed) is refused with a typed refusal, mirroring
/// the daemon's `stale_approval` / `hash_mismatch` refusals — D13:
/// whitelisted canned surface, no free-text from the lock screen.
struct PushPayload: Sendable, Equatable {
    enum PushType: String, Sendable {
        case blocked
        case done
    }

    var type: PushType
    var agentId: String
    /// Blocked only: the claim the reply must echo.
    var promptHash: String?
    var approvalId: String?
    var waitingKind: WaitingOnKind?
    var choices: [String]
    var ts: UInt64?
    /// Display text from `aps.alert` (blocked body is the redacted prompt).
    var title: String?
    var body: String?

    static func blocked(agent: Agent, waiting: WaitingOn) -> PushPayload {
        PushPayload(type: .blocked, agentId: agent.agentId,
                    promptHash: waiting.promptHash,
                    approvalId: waiting.approvalId,
                    waitingKind: waiting.kind,
                    choices: waiting.choices,
                    ts: UInt64(Date().timeIntervalSince1970),
                    title: agent.displayName ?? agent.agentId,
                    body: waiting.prompt)
    }

    static func done(agentId: String) -> PushPayload {
        PushPayload(type: .done, agentId: agentId,
                    promptHash: nil, approvalId: nil, waitingKind: nil,
                    choices: [], ts: UInt64(Date().timeIntervalSince1970),
                    title: nil, body: nil)
    }

    // MARK: - Parsing (APNs userInfo and the local bridge's userInfo share
    // the same key shape: type/agent_id/prompt_hash/approval_id/choices/
    // kind/ts, plus aps.alert.{title,body})

    private enum Keys {
        static let type = "type"
        static let agentId = "agent_id"
        static let promptHash = "prompt_hash"
        static let approvalId = "approval_id"
        static let choices = "choices"
        static let kind = "kind"
        static let ts = "ts"
        static let aps = "aps"
        static let alert = "alert"
        static let title = "title"
        static let body = "body"
    }

    /// Parse a notification's `userInfo` into the push payload. Returns
    /// nil for anything that is not one of the daemon's two surfaces.
    static func parse(userInfo: [AnyHashable: Any]) -> PushPayload? {
        guard let typeRaw = userInfo[Keys.type] as? String,
              let type = PushType(rawValue: typeRaw),
              let agentId = userInfo[Keys.agentId] as? String else {
            return nil
        }
        let aps = userInfo[Keys.aps] as? [AnyHashable: Any]
        let alert = aps?[Keys.alert] as? [AnyHashable: Any]
        let kindRaw = userInfo[Keys.kind] as? String
        return PushPayload(
            type: type,
            agentId: agentId,
            promptHash: userInfo[Keys.promptHash] as? String,
            approvalId: userInfo[Keys.approvalId] as? String,
            waitingKind: kindRaw.flatMap(WaitingOnKind.init(rawValue:)),
            choices: (userInfo[Keys.choices] as? [String]) ?? [],
            ts: (userInfo[Keys.ts] as? NSNumber)?.uint64Value,
            title: alert?[Keys.title] as? String,
            body: alert?[Keys.body] as? String)
    }

    /// The userInfo dict the DEBUG local bridge embeds — byte-identical
    /// key shape to the daemon's APNs payload.
    func asUserInfo(title: String, body: String) -> [AnyHashable: Any] {
        var info: [AnyHashable: Any] = [
            Keys.type: type.rawValue,
            Keys.agentId: agentId,
            Keys.ts: ts ?? UInt64(Date().timeIntervalSince1970),
            Keys.aps: [Keys.alert: [Keys.title: title, Keys.body: body]],
        ]
        if let promptHash { info[Keys.promptHash] = promptHash }
        if let approvalId { info[Keys.approvalId] = approvalId }
        if let waitingKind { info[Keys.kind] = waitingKind.rawValue }
        if !choices.isEmpty { info[Keys.choices] = choices }
        return info
    }
}

/// Typed local refusals for a notification reply (D16). The daemon is the
/// authority — these make the refusal immediate, typed and readable before
/// any signed bytes leave the phone.
enum ReplyRefusal: Error, Equatable, Sendable {
    /// The agent is gone or no longer waiting (daemon: `no_waiting_approval`
    /// / `stale_approval`).
    case stale
    /// The prompt changed since the notification fired — the reply is
    /// bound to a hash that no longer matches the live claim (daemon:
    /// `hash_mismatch`).
    case hashMismatch
}

/// Pure stale-hash rejection logic (unit-tested). The reply to a lock-screen
/// action may only be built from the notification's OWN claim, and only if
/// it still matches the live snapshot claim.
enum NotificationReplyValidator {
    static func validate(payload: PushPayload, liveAgent: Agent?) -> Result<WaitingOn, ReplyRefusal> {
        guard let liveAgent, let waiting = liveAgent.waitingOn else {
            return .failure(.stale)
        }
        guard let payloadHash = payload.promptHash, waiting.promptHash == payloadHash else {
            return .failure(.hashMismatch)
        }
        return .success(waiting)
    }
}
