import Foundation

// MARK: - #399 B4 push-registration gate

/// App-wide gate consulted by the APNs device-token upload path so a
/// token NEVER reaches a host whose pinned identity is unverified or
/// mismatched ("no push-register reaching the replacement identity").
///
/// Defaults to ALLOW (legacy flows, unit fixtures without a profile
/// store); `AppModel` installs the real async predicate when it owns a
/// pinned profile. The predicate is async because the model's continuity
/// state is main-actor-owned.
enum KeyContinuityGate {
    private static let lock = NSLock()
    private static var predicate: @Sendable () async -> Bool = { true }

    static func setPushPredicate(_ newPredicate: @escaping @Sendable () async -> Bool) {
        lock.lock()
        predicate = newPredicate
        lock.unlock()
    }

    static func allowsPushRegistration() async -> Bool {
        lock.lock()
        let current = predicate
        lock.unlock()
        return await current()
    }

    static func reset() {
        setPushPredicate { true }
    }
}
