import Foundation
import UserNotifications

/// Notification presentation (D16 → #354 L2). Both delivery paths — the
/// daemon's APNs push and the DEBUG-only local bridge — produce the SAME
/// [`PushPayload`] shape, so `didReceive` routes every tap through the
/// payload-bound deep link: notification tap → the agent's row with its
/// recents sheet open. There are no reply actions anymore (the whole drive
/// plane beyond read_tail was cut).
///
/// #397 composite targets: requests are scheduled under
/// [`PushPayload.requestIdentifier`] (namespaced by host + agent) and
/// grouped under [`PushPayload.threadIdentifier`], so equal raw agent ids
/// from two hosts stay distinct notifications. `onOpenAgent` carries the
/// payload's host identity so the model resolves EXACTLY one profile and
/// never guesses a host from a bare agent id.
final class LocalNotifier: NSObject, UNUserNotificationCenterDelegate, @unchecked Sendable {
    private let center = UNUserNotificationCenter.current()

    /// Called with the agent id + host identity (nil = legacy host-less
    /// payload) when a delivered notification is tapped.
    var onOpenAgent: (@MainActor @Sendable (String, String?) -> Void)? {
        didSet {
            // #397 follow-up: a response delivered while the handler was
            // not yet installed is queued (see `routeTap`) — flush it the
            // moment the model installs the handler, in delivery order.
            // Handler assignment happens on the main actor (AppModel init
            // / startLive; test fixtures).
            guard onOpenAgent != nil, !pendingTaps.isEmpty else { return }
            let queued = pendingTaps
            pendingTaps.removeAll()
            MainActor.assumeIsolated {
                for tap in queued {
                    self.onOpenAgent?(tap.agentId, tap.hostId)
                }
            }
        }
    }

    /// #397 follow-up: taps delivered before `onOpenAgent` was installed
    /// (a launch-time delivery racing the model's handler assignment). A
    /// `didReceive` response is a ONE-SHOT delivery — dropping it loses
    /// the user's tap forever, so it is held until the handler lands.
    /// MainActor-serialized: every mutation happens on the main actor.
    private var pendingTaps: [(agentId: String, hostId: String?)] = []
    private let pendingTapLimit = 3

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
            _ = error  // Denied notification permission: the board stays the product.
        }
    }

    /// Fire one state-change notification (started / blocked / finished).
    /// The identifier is episode- AND target-scoped so the same transition
    /// cannot stack: a blocked agent stays blocked → one banner; a
    /// finished agent stays idle → no repeat until the next episode. Two
    /// hosts with an equal raw agent id schedule DISTINCT requests (#397).
    func notify(_ payload: PushPayload) {
        guard isEnabled else { return }
        let content = UNMutableNotificationContent()
        content.title = payload.title ?? payload.agentId
        content.body = payload.body ?? ""
        content.sound = .default
        content.userInfo = payload.asUserInfo()
        // #397: the local bridge mirrors the daemon's composite thread-id
        // so both delivery paths group identically per (host, agent).
        content.threadIdentifier = payload.threadIdentifier
        let identifier = payload.requestIdentifier
        let request = UNNotificationRequest(identifier: identifier, content: content, trigger: nil)
        center.add(request) { _ in }
    }

    func removeAll() {
        center.removeAllPendingNotificationRequests()
        center.removeAllDeliveredNotifications()
    }

    /// #397: remove every pending + delivered notification whose request
    /// identifier belongs to one host's composite namespace (host removal
    /// / per-host disable). Legacy host-less requests are untouched when
    /// `hostId` is nil (they cannot belong to a named host).
    func removeAll(forHostId hostId: String?) {
        guard let hostId, !hostId.isEmpty else { return }
        center.getPendingNotificationRequests { requests in
            let doomed = requests
                .filter { $0.identifier.contains(hostId) }
                .map(\.identifier)
            if !doomed.isEmpty {
                self.center.removePendingNotificationRequests(withIdentifiers: doomed)
            }
        }
        center.getDeliveredNotifications { delivered in
            let doomed = delivered
                .filter { $0.request.identifier.contains(hostId) }
                .map(\.request.identifier)
            if !doomed.isEmpty {
                self.center.removeDeliveredNotifications(withIdentifiers: doomed)
            }
        }
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
            deliverTap(agentId: payload.agentId, hostId: payload.hostId)
        }
        completionHandler()
    }

    /// #397 follow-up: the notification-RESPONSE seam — the OS delegate
    /// callback and the unit tests both enter here (a UNNotificationResponse
    /// cannot be fabricated in tests). The system may call the delegate on
    /// a background queue, so the delivery state is MainActor-serialized:
    /// a tap whose handler is installed routes immediately; a tap that
    /// arrives before the handler (launch-time delivery racing startLive)
    /// is queued and flushed in order when the handler lands.
    func deliverTap(agentId: String, hostId: String?) {
        if Thread.isMainThread {
            MainActor.assumeIsolated {
                self.routeTap(agentId: agentId, hostId: hostId)
            }
        } else {
            DispatchQueue.main.sync {
                MainActor.assumeIsolated {
                    self.routeTap(agentId: agentId, hostId: hostId)
                }
            }
        }
    }

    @MainActor
    private func routeTap(agentId: String, hostId: String?) {
        if let onOpenAgent {
            onOpenAgent(agentId, hostId)
            return
        }
        pendingTaps.append((agentId, hostId))
        // Bounded FIFO: taps replay oldest-first; the cap only defends
        // against a pathological burst. Keep the EARLIEST taps.
        if pendingTaps.count > pendingTapLimit {
            pendingTaps.removeLast(pendingTaps.count - pendingTapLimit)
        }
    }
}
