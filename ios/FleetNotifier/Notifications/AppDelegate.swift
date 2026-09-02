import Foundation
import UIKit

private final class DeviceTokenState: @unchecked Sendable {
    private let lock = NSLock()
    private var latestToken: String?
    private var accepted: (IdentityLifecycle.Context, String)?
    private var uploaded: (IdentityLifecycle.Context, String)?

    func remember(_ token: String) {
        lock.lock()
        latestToken = token
        lock.unlock()
    }

    var pending: String? {
        lock.lock()
        defer { lock.unlock() }
        return latestToken
    }

    /// Suppress duplicate callbacks for the same lifecycle identity while
    /// still allowing a token retained during demo to retry under the next
    /// live generation.
    func begin(_ context: IdentityLifecycle.Context, token: String) -> Bool {
        lock.lock()
        defer { lock.unlock() }
        if let uploaded,
           sameIdentity(uploaded.0, context), uploaded.1 == token {
            return false
        }
        if let accepted, accepted.0 == context, accepted.1 == token {
            return false
        }
        accepted = (context, token)
        return true
    }

    func succeeded(_ context: IdentityLifecycle.Context, token: String) {
        lock.lock()
        if let accepted, accepted.0 == context, accepted.1 == token {
            uploaded = (context, token)
        }
        lock.unlock()
    }

    func failed(_ context: IdentityLifecycle.Context, token: String) {
        lock.lock()
        if let accepted, accepted.0 == context, accepted.1 == token {
            self.accepted = nil
        }
        lock.unlock()
    }

    func clear() {
        lock.lock()
        latestToken = nil
        accepted = nil
        uploaded = nil
        lock.unlock()
    }

    private func sameIdentity(_ lhs: IdentityLifecycle.Context,
                              _ rhs: IdentityLifecycle.Context) -> Bool {
        lhs.hostURL == rhs.hostURL
            && lhs.keyId == rhs.keyId
            && lhs.signerPublicKeyB64 == rhs.signerPublicKeyB64
    }
}

/// APNs registration (D16 → #354 L2): receives the device token from
/// `UIApplication` and enrolls it on the daemon via the signed
/// `POST /device-token` (proof of possession of the device key — a stolen
/// token alone cannot re-register push on another device).
///
/// Simulator builds have no APNs (`didFailToRegister…` fires): the DEBUG
/// local-notification bridge (see [`PushBridge`]) takes over so the
/// start/blocked/finished state-change notifications are exercisable
/// without certs.
final class AppDelegate: NSObject, UIApplicationDelegate {
    /// Whether this device holds a live APNs token. The DEBUG local bridge
    /// is active ONLY while this is false — a real device must not get
    /// doubled notifications (SSE bridge + APNs).
    static var apnsRegistered = false
    static weak var shared: AppDelegate?

    private let identityLifecycle: IdentityLifecycle
    private let session: URLSession
    private let identityProvider: @Sendable () -> DeviceSigner?
    private let beforeDeviceTokenUpload: @Sendable () async -> Void
    private let deviceTokenState = DeviceTokenState()

    /// SwiftUI's `@UIApplicationDelegateAdaptor` constructs the delegate with
    /// the zero-argument `NSObject` entrypoint. Delegate to the injectable
    /// initializer so launch uses the same production defaults as the DI path.
    convenience override init() {
        self.init(identityLifecycle: .shared,
                  session: .shared,
                  identityProvider: { try? DeviceKeyStore.loadOrCreate().0 },
                  beforeDeviceTokenUpload: {})
    }

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
        Self.shared = self
    }

    func application(_ application: UIApplication,
                     didRegisterForRemoteNotificationsWithDeviceToken deviceToken: Data) {
        let hex = deviceToken.map { String(format: "%02x", $0) }.joined()
        _ = receiveDeviceToken(hex)
    }

    /// Testable equivalent of the OS callback. The token is retained before
    /// checking lifecycle mode, so a callback delivered during demo can be
    /// retried after the app returns to a valid live identity.
    @discardableResult
    func receiveDeviceToken(_ hex: String) -> Task<Void, Never>? {
        Self.apnsRegistered = true
        deviceTokenState.remember(hex)
        return startDeviceTokenUploadIfPossible(hex)
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
        deviceTokenState.remember(hex)
        return startDeviceTokenUploadIfPossible(hex)
    }

    /// Retry a token retained while the lifecycle was non-live. AppModel
    /// calls this independently of one-time notification setup whenever it
    /// has installed a valid live identity.
    @discardableResult
    func retryPendingDeviceTokenUpload() -> Task<Void, Never>? {
        guard let token = deviceTokenState.pending else { return nil }
        return startDeviceTokenUploadIfPossible(token)
    }

    /// Reset retires the token as well as the identity that could authorize
    /// it. A future registration must wait for a fresh OS callback.
    func clearRetainedDeviceToken() {
        deviceTokenState.clear()
        Self.apnsRegistered = false
    }

    @discardableResult
    private func startDeviceTokenUploadIfPossible(_ hex: String) -> Task<Void, Never>? {
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
        guard deviceTokenState.begin(expected, token: hex) else { return nil }
        let session = self.session
        let beforeUpload = self.beforeDeviceTokenUpload
        let tokenState = self.deviceTokenState
        return lifecycle.launch { context in
            guard context == expected,
                  context.mode == .live,
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
            do {
                _ = try await client.registerDeviceToken(hex, keyId: keyId, signer: signer)
                guard !Task.isCancelled, lifecycle.isCurrent(context) else { return }
                tokenState.succeeded(context, token: hex)
            } catch {
                guard !Task.isCancelled, lifecycle.isCurrent(context) else { return }
                tokenState.failed(context, token: hex)
            }
        }
    }
}

/// The D16 → #354 L2 notification delivery policy:
/// - Release builds: APNs only. The SSE stream is a read path; it never
///   fires local notifications (a real device gets the push from Apple once
///   the daemon-side APNs provisioning checkpoint is met).
/// - DEBUG builds: when APNs is NOT registered (simulator — no push
///   service), the SSE state-transition deltas fire `UNUserNotificationCenter`
///   directly — byte-identical payload shape — so the state-change
///   notifications and their deep link are exercisable without any
///   certificate.
enum PushBridge {
    static var shouldPresentLocally: Bool {
        #if DEBUG
        return !AppDelegate.apnsRegistered
        #else
        return false
        #endif
    }
}
