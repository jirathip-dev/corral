import Foundation
import UserNotifications

/// Canned lock-screen answers (Approve/Deny/Continue) bound to the live
/// claim's `prompt_hash`. Choice resolution is pure so it is unit-tested:
/// menu membership must hold server-side, so a canned action only maps to a
/// choice the claim will accept (`choice ∈ choices[]` for Menu/ApproveTool;
/// free-form for AnswerQuestion; Crash is never approvable).
enum CannedChoice {
    enum Action: String, CaseIterable, Sendable {
        case approve = "APPROVE"
        case deny = "DENY"
        case `continue` = "CONTINUE"
    }

    /// Conventional affirmative/negative/continue spellings, in preference
    /// order; the first one present in `choices` wins.
    private static let approveSpellings = ["y", "yes", "approve", "ok", "accept", "continue"]
    private static let denySpellings = ["n", "no", "deny", "cancel", "skip", "abort"]
    private static let continueSpellings = ["continue", "c", "proceed", "y"]

    /// Resolve a canned action to an exact choice string for the claim, or
    /// nil when the action cannot be answered for this claim.
    static func choice(for action: Action, kind: WaitingOnKind, choices: [String]) -> String? {
        guard kind != .crash else { return nil }
        let preferred: [String]
        switch action {
        case .approve: preferred = approveSpellings
        case .deny: preferred = denySpellings
        case .continue: preferred = continueSpellings
        }
        if !choices.isEmpty {
            return preferred.first { choices.contains($0) } ?? (action == .approve ? choices.first : nil)
        }
        // Menu/ApproveTool with no extracted choices is lenient server-side
        // (the adapter sends the text); AnswerQuestion is free-form.
        switch action {
        case .approve: return "yes"
        case .deny: return "no"
        case .continue: return "continue"
        }
    }

    /// Display title for a canned action on a notification button.
    static func title(for action: Action) -> String {
        switch action {
        case .approve: return "Approve"
        case .deny: return "Deny"
        case .continue: return "Continue"
        }
    }
}

/// Local-notification path (D5/D12): when an agent blocks, a local
/// notification fires with the claim; the lock-screen actions execute the
/// canned approve with the byte-for-byte `approval_id` + `prompt_hash` from
/// the snapshot. APNs/relay is OUT of v1 — `notificationHook` documents the
/// seam a future relay would fill (register device token, push payload =
/// claim dict below).
final class LocalNotifier: NSObject, UNUserNotificationCenterDelegate, @unchecked Sendable {
    static let blockedCategory = "AGENT_BLOCKED"
    static let crashedCategory = "AGENT_CRASHED"

    private let center = UNUserNotificationCenter.current()

    /// Called with `(agentId, action)` when a notification action fires —
    /// the app executes the signed approve through its DriveController.
    var onAction: (@MainActor @Sendable (String, CannedChoice.Action) -> Void)?

    override init() {
        super.init()
        center.delegate = self
    }

    func requestAuthorization() async {
        do {
            _ = try await center.requestAuthorization(options: [.alert, .sound, .badge])
        } catch {
            // Denied notification permission: in-app blocked cards still
            // surface the claim.
        }
    }

    func registerCategories() {
        let blockedActions = CannedChoice.Action.allCases.map { action in
            UNNotificationAction(identifier: action.rawValue,
                                 title: CannedChoice.title(for: action),
                                 options: [.authenticationRequired])
        }
        let blocked = UNNotificationCategory(identifier: Self.blockedCategory,
                                             actions: blockedActions,
                                             intentIdentifiers: [],
                                             options: [])
        let crashed = UNNotificationCategory(identifier: Self.crashedCategory,
                                             actions: [],
                                             intentIdentifiers: [],
                                             options: [])
        center.setNotificationCategories([blocked, crashed])
    }

    /// Claim payload stored on the notification — the app re-reads the
    /// LIVE snapshot claim before driving, never a stale copy; the
    /// prompt_hash here is only for dedupe/debug.
    struct ClaimPayload {
        let agentId: String
        let kind: WaitingOnKind
        let promptHash: String
        let approvalId: String?
        let choices: [String]
    }

    /// Fire a notification for a newly-blocked agent. Idempotent per
    /// prompt_hash: same agent + same hash → same identifier, so a
    /// repeating block does not stack notifications.
    func notifyBlocked(_ claim: ClaimPayload, title: String, prompt: String) {
        let content = UNMutableNotificationContent()
        content.title = title
        content.body = prompt
        content.sound = .default
        content.categoryIdentifier = claim.kind == .crash ? Self.crashedCategory : Self.blockedCategory
        content.userInfo = [
            "agent_id": claim.agentId,
            "kind": claim.kind.rawValue,
            "prompt_hash": claim.promptHash,
            "approval_id": claim.approvalId ?? "",
            "choices": claim.choices,
        ]
        let identifier = "blocked-\(claim.agentId)-\(claim.promptHash)"
        let request = UNNotificationRequest(identifier: identifier, content: content, trigger: nil)
        center.add(request) { _ in }
    }

    func removeAll() {
        center.removeAllPendingNotificationRequests()
        center.removeAllDeliveredNotifications()
    }

    // MARK: - UNUserNotificationCenterDelegate

    func userNotificationCenter(_ center: UNUserNotificationCenter,
                                willPresent notification: UNNotification,
                                withCompletionHandler completionHandler: @escaping (UNNotificationPresentationOptions) -> Void) {
        // App foregrounded: the blocked card is already visible.
        completionHandler([.banner, .sound])
    }

    func userNotificationCenter(_ center: UNUserNotificationCenter,
                                didReceive response: UNNotificationResponse,
                                withCompletionHandler completionHandler: @escaping () -> Void) {
        let info = response.notification.request.content.userInfo
        guard let agentId = info["agent_id"] as? String,
              let action = CannedChoice.Action(rawValue: response.actionIdentifier) else {
            completionHandler()
            return
        }
        Task { @MainActor in
            onAction?(agentId, action)
        }
        completionHandler()
    }
}
