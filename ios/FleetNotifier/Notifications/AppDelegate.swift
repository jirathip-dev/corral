import Foundation
import UIKit

/// APNs registration (D16): receives the device token from
/// `UIApplication` and enrolls it on the daemon via the signed
/// `POST /device-token` — the same proof-of-possession shape as /step-up,
/// so a stolen token alone cannot re-register push on another device.
///
/// Simulator builds have no APNs (`didFailToRegister…` fires): the DEBUG
/// local-notification bridge (see [`PushBridge`]) takes over so the whole
/// lock-screen flow is exercisable without certs.
final class AppDelegate: NSObject, UIApplicationDelegate {
    /// Whether this device holds a live APNs token. The DEBUG local bridge
    /// is active ONLY while this is false — a real device must not get
    /// doubled notifications (SSE bridge + APNs).
    static var apnsRegistered = false

    func application(_ application: UIApplication,
                     didRegisterForRemoteNotificationsWithDeviceToken deviceToken: Data) {
        let hex = deviceToken.map { String(format: "%02x", $0) }.joined()
        Self.apnsRegistered = true
        Task {
            await Self.uploadToken(hex)
        }
    }

    func application(_ application: UIApplication,
                     didFailToRegisterForRemoteNotificationsWithError error: Error) {
        // Simulator / missing aps-environment entitlement: keep the DEBUG
        // local bridge; on release builds this is a silent no-op (the
        // notifier just won't deliver until the profile has the entitlement).
        Self.apnsRegistered = false
    }

    /// Send the APNs token to the daemon, signed with the device key. A
    /// failure is logged nowhere user-visible: the daemon keeps the last
    /// good token, and the next launch re-registers.
    private static func uploadToken(_ hex: String) async {
        guard let meta = DeviceKeyStore.loadMeta(),
              let url = URL(string: meta.host),
              let (signer, _) = try? DeviceKeyStore.loadOrCreate() else {
            return
        }
        let client = DriveClient(host: url)
        _ = try? await client.registerDeviceToken(hex, keyId: meta.keyId, signer: signer)
    }
}

/// The D16 notification delivery policy:
/// - Release builds: APNs only. The SSE stream is a read path; it never
///   fires local notifications (a real device gets the push from Apple).
/// - DEBUG builds: when APNs is NOT registered (simulator — no push
///   service), the SSE blocked/done deltas fire `UNUserNotificationCenter`
///   directly — byte-identical payload shape — so the lock-screen canned
///   replies are exercisable end-to-end without any certificate.
enum PushBridge {
    static var shouldPresentLocally: Bool {
        #if DEBUG
        return !AppDelegate.apnsRegistered
        #else
        return false
        #endif
    }
}
