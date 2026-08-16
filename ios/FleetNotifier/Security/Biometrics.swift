import Foundation
import LocalAuthentication

/// Biometric step-up gate (D10): Face ID (or Touch ID on older devices)
/// must succeed before a destructive command is sent. Injectable for tests.
struct Biometrics: Sendable {
    var evaluate: @Sendable () async -> Bool

    init(evaluate: @escaping @Sendable () async -> Bool = {
        await Biometrics.realEvaluation()
    }) {
        self.evaluate = evaluate
    }

    /// Face ID / Touch ID step-up gate.
    func authenticate() async -> Bool {
        await evaluate()
    }

    private static func realEvaluation() async -> Bool {
        let context = LAContext()
        var error: NSError?
        guard context.canEvaluatePolicy(.deviceOwnerAuthenticationWithBiometrics, error: &error) else {
            return false
        }
        do {
            return try await context.evaluatePolicy(.deviceOwnerAuthenticationWithBiometrics,
                                                    localizedReason: "Corral step-up: confirm before sending a destructive command to an agent.")
        } catch {
            return false
        }
    }
}
