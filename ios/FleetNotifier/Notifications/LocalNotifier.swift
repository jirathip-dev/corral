import Foundation
import UserNotifications

/// Notification presentation (D16 → #354 L2). Both delivery paths — the
/// daemon's APNs push and the DEBUG-only local bridge — produce the SAME
/// [`PushPayload`] shape, so `didReceive` routes every tap through the
/// payload-bound deep link: notification tap → the agent's row with its
/// recents sheet open. There are no reply actions anymore (the whole drive
/// plane beyond read_tail was cut).
final class LocalNotifier: NSObject, UNUserNotificationCenterDelegate, @unchecked Sendable {
    private let center = UNUserNotificationCenter.current()

    /// Called with the agent id when a delivered notification is tapped.
    var onOpenAgent: (@MainActor @Sendable (String) -> Void)?

    /// Global on/off (spec: notification pairing has ONE control). When
    /// disabled nothing is scheduled or presented.
    var isEnabled = true

    override init() {
        super.init()
        center.delegate = self
    }

    func requestAuthorization() async {
        do {
            // No badge: spec has no catch-up/badge on foreground.
            _ = try await center.requestAuthorization(options: [.alert, .sound])
        } catch {
            // Denied notification permission: the board stays the product.
        }
    }

    /// Fire one state-change notification (started / blocked / finished).
    /// The identifier is episode-scoped so the same transition cannot stack:
    /// a blocked agent stays blocked → one banner; a finished agent stays
    /// idle → no repeat until the next episode.
    func notify(_ payload: PushPayload) {
        guard isEnabled else { return }
        let content = UNMutableNotificationContent()
        content.title = payload.title ?? payload.agentId
        content.body = payload.body ?? ""
        content.sound = .default
        content.userInfo = payload.asUserInfo()
        let identifier = "\(payload.type.rawValue)-\(payload.agentId)"
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
        // App foregrounded: the SSE stream is the live view; still banner so
        // the state change is visible even with the app open.
        completionHandler(isEnabled ? [.banner, .sound] : [])
    }

    func userNotificationCenter(_ center: UNUserNotificationCenter,
                                didReceive response: UNNotificationResponse,
                                withCompletionHandler completionHandler: @escaping () -> Void) {
        // Tap (default action) → deep link to the agent row with recents
        // open. No action buttons are registered for these categories.
        if response.actionIdentifier == UNNotificationDefaultActionIdentifier,
           let payload = PushPayload.parse(userInfo: response.notification.request.content.userInfo) {
            Task { @MainActor in
                onOpenAgent?(payload.agentId)
            }
        }
        completionHandler()
    }
}
