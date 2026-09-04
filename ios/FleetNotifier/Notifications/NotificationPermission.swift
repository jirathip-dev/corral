import Foundation
import UserNotifications

/// #389: the Settings guidance posture for the system notification
/// permission. `.denied` and `.restricted` are the blocked states — the
/// Notifications section then shows WHY and an 'Open iOS Settings' action
/// instead of the enable toggle silently failing (iOS will never deliver a
/// token or a local notification while the permission is blocked).
enum NotificationPermissionState: Equatable, Sendable {
    case notDetermined
    case denied
    case restricted
    case granted

    init(status: UNAuthorizationStatus) {
        switch status {
        case .authorized, .provisional, .ephemeral:
            self = .granted
        case .denied:
            self = .denied
        case .notDetermined:
            self = .notDetermined
        @unknown default:
            // UNAuthorizationStatus has no .restricted member today
            // (notDetermined/denied/authorized/provisional/ephemeral), but
            // the issue's spec names ".denied/.restricted" as the blocked
            // bucket. Any unrecognized status maps to the blocked guidance,
            // never to a silent prompt loop on an OS-restricted device.
            self = .denied
        }
    }

    /// Whether the OS blocks notifications entirely (Settings guidance +
    /// 'Open iOS Settings' action replaces the plain caption in this state).
    var showsBlockedGuidance: Bool {
        self == .denied || self == .restricted
    }
}

/// #389: the model asks the OS notification center through this seam so the
/// permission-aware enable flow and the Settings guidance state are
/// unit-testable without a real `UNUserNotificationCenter` (a real center in
/// the test host would show the system prompt).
protocol NotificationPermissionProviding: Sendable {
    func currentPermission() async -> NotificationPermissionState
    /// Prompts once when the state is `.notDetermined`; returns whether the
    /// user granted (alert + sound — the same options the first-live
    /// requestAuthorization uses; no badge: the spec has no catch-up/badge).
    func requestAuthorization() async -> Bool
}

/// Production implementation over `UNUserNotificationCenter.current()`.
struct SystemNotificationPermissionProvider: NotificationPermissionProviding {
    private let center: UNUserNotificationCenter

    init(center: UNUserNotificationCenter = .current()) {
        self.center = center
    }

    func currentPermission() async -> NotificationPermissionState {
        let settings = await center.notificationSettings()
        return NotificationPermissionState(status: settings.authorizationStatus)
    }

    func requestAuthorization() async -> Bool {
        do {
            return try await center.requestAuthorization(options: [.alert, .sound])
        } catch {
            // A failed prompt request behaves like a denial: the Settings
            // section shows the blocked guidance rather than silently
            // enabling a toggle nothing can deliver.
            return false
        }
    }
}
