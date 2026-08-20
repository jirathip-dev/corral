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

    private let identityLifecycle: IdentityLifecycle
    private let session: URLSession
    private let identityProvider: @Sendable () -> DeviceSigner?
    private let beforeDeviceTokenUpload: @Sendable () async -> Void

    /// Dependencies are injectable so lifecycle races can be tested with a
    /// URLProtocol session and an in-memory identity, without touching APNs or
    /// the device key store.
    init(identityLifecycle: IdentityLifecycle = .shared,
         session: URLSession = .shared,
         identityProvider: @escaping @Sendable () -> DeviceSigner? = {
             try? DeviceKeyStore.loadOrCreate().0
         },
         beforeDeviceTokenUpload: @escaping @Sendable () async -> Void = {}) {
        self.identityLifecycle = identityLifecycle
        self.session = session
        self.identityProvider = identityProvider
        self.beforeDeviceTokenUpload = beforeDeviceTokenUpload
        super.init()
    }

    func application(_ application: UIApplication,
                     didRegisterForRemoteNotificationsWithDeviceToken deviceToken: Data) {
        let hex = deviceToken.map { String(format: "%02x", $0) }.joined()
        Self.apnsRegistered = true
        _ = startDeviceTokenUpload(hex)
    }

    func application(_ application: UIApplication,
                     didFailToRegisterForRemoteNotificationsWithError error: Error) {
        // Simulator / missing aps-environment entitlement: keep the DEBUG
        // local bridge; on release builds this is a silent no-op (the
        // notifier just won't deliver until the profile has the entitlement).
        Self.apnsRegistered = false
    }

    /// Send the APNs token to the daemon, signed with the CURRENT identity.
    /// The shared lifecycle owns cancellation on reset/demo and every
    /// suspension is followed by a generation/identity check.
    @discardableResult
    func startDeviceTokenUpload(_ hex: String) -> Task<Void, Never>? {
        let lifecycle = identityLifecycle
        let expected = lifecycle.current()
        guard expected.mode == .live,
              expected.hostURL != nil,
              expected.keyId != nil,
              expected.signerPublicKeyB64 != nil,
              let signer = identityProvider(),
              signer.publicKeyB64 == expected.signerPublicKeyB64,
              lifecycle.isCurrent(expected) else {
            return nil
        }
        let session = self.session
        let beforeUpload = self.beforeDeviceTokenUpload
        return lifecycle.launch { context in
            guard context.mode == .live,
                  let hostURL = context.hostURL,
                  let keyId = context.keyId,
                  context.signerPublicKeyB64 == signer.publicKeyB64,
                  !Task.isCancelled,
                  lifecycle.isCurrent(context) else { return }
            await beforeUpload()
            guard !Task.isCancelled, lifecycle.isCurrent(context) else { return }
            let client = DriveClient(host: hostURL, session: session)
            // Keep the identity check adjacent to the request. Reset/demo
            // can invalidate the context while the preflight boundary was
            // suspended, and no retired token should cross this point.
            guard !Task.isCancelled, lifecycle.isCurrent(context) else { return }
            _ = try? await client.registerDeviceToken(hex, keyId: keyId, signer: signer)
            guard !Task.isCancelled, lifecycle.isCurrent(context) else { return }
        }
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
