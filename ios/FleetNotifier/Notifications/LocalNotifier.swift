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

/// Notification presentation + reply routing (D16). Both delivery paths —
/// the daemon's APNs push and the DEBUG-only local bridge — produce the
/// SAME [`PushPayload`] dict shape, so `didReceive` routes every reply
/// through the payload-bound handler: the lock-screen action is bound to
/// the notification's OWN `prompt_hash`, and a stale hash is refused with
/// a typed refusal before any signed bytes leave the phone.
final class LocalNotifier: NSObject, UNUserNotificationCenterDelegate, @unchecked Sendable {
    static let blockedCategory = "AGENT_BLOCKED"

    private let center = UNUserNotificationCenter.current()

    /// Called with `(payload, action)` when a notification action fires —
    /// the app validates the payload's claim against the live snapshot and
    /// executes the signed approve through its DriveController.
    var onReply: (@MainActor @Sendable (PushPayload, CannedChoice.Action) -> Void)?

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
        // Done notifications carry NO category: a plain completion has no
        // reply surface (D16).
        center.setNotificationCategories([blocked])
    }

    /// Fire a blocked notification (D16 surface 1). Idempotent per
    /// prompt_hash: same agent + same hash → same identifier, so a
    /// repeating block does not stack notifications. The userInfo embeds
    /// the payload — identical to the APNs body the daemon sends.
    func notifyBlocked(_ payload: PushPayload, title: String, prompt: String) {
        let content = UNMutableNotificationContent()
        content.title = title
        content.body = prompt
        content.sound = .default
        content.categoryIdentifier = Self.blockedCategory
        content.userInfo = payload.asUserInfo(title: title, body: prompt)
        let identifier = "blocked-\(payload.agentId)-\(payload.promptHash ?? "?")"
        let request = UNNotificationRequest(identifier: identifier, content: content, trigger: nil)
        center.add(request) { _ in }
    }

    /// Fire a done notification (D16 surface 2): plain completion, no
    /// category, no actions.
    func notifyDone(_ payload: PushPayload) {
        let content = UNMutableNotificationContent()
        content.title = payload.title ?? payload.agentId
        content.body = payload.body ?? "Agent finished"
        content.sound = .default
        content.userInfo = payload.asUserInfo(title: content.title, body: content.body)
        let identifier = "done-\(payload.agentId)-\(payload.ts ?? 0)"
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
        // App foregrounded: the blocked card is already visible; still
        // show the banner so the lock-screen action is reachable.
        completionHandler([.banner, .sound])
    }

    func userNotificationCenter(_ center: UNUserNotificationCenter,
                                didReceive response: UNNotificationResponse,
                                withCompletionHandler completionHandler: @escaping () -> Void) {
        let info = response.notification.request.content.userInfo
        guard let payload = PushPayload.parse(userInfo: info),
              payload.type == .blocked,
              let action = CannedChoice.Action(rawValue: response.actionIdentifier) else {
            completionHandler()
            return
        }
        Task { @MainActor in
            onReply?(payload, action)
        }
        completionHandler()
    }
}
