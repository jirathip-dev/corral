import CryptoKit
import Foundation
import Security

/// Device Ed25519 signer (D10/D13). CryptoKit's `Curve25519.Signing` is
/// software Ed25519 — the Secure Enclave hosts only EC P-256 keys, so the
/// raw private key lives in the Keychain (`WhenUnlockedThisDeviceOnly`),
/// which on modern iPhones is itself hardware-protected by the Secure
/// Enclave. The simulator uses the same Keychain path; if Keychain is
/// unavailable (restricted device/entitlement issues) a documented in-app
/// fallback store is used and surfaced with a warning banner in the UI.
final class DeviceSigner: @unchecked Sendable {
    let key: Curve25519.Signing.PrivateKey // gitleaks:allow — Swift member name, not a secret value

    init(key: Curve25519.Signing.PrivateKey) {
        self.key = key
    }

    var publicKeyB64: String {
        key.publicKey.rawRepresentation.base64EncodedString()
    }

    /// Raw 64-byte Ed25519 signature over the exact canonical bytes.
    func sign(_ bytes: Data) throws -> Data {
        try key.signature(for: bytes)
    }
}

/// Persistent device identity. One keypair per app install; the Keychain
/// holds the private key, UserDefaults holds the registration metadata.
enum DeviceKeyStore {
    enum Storage: Sendable {
        case keychain
        case insecureFallback
    }

    private static let keychainService = "com.corral.fleetnotifier.keys"
    private static let keychainAccount = "device-ed25519"
    private static let adminService = "com.corral.fleetnotifier.admin"
    private static let adminAccount = "host-admin-token"
    private static let fallbackKey = "fleetnotifier.deviceKeyRaw"
    private static let metaKey = "fleetnotifier.deviceMeta"

    struct DeviceMeta: Codable, Sendable {
        var keyId: String
        var host: String
        var grants: [String]
        var expiryTs: UInt64
        var registeredAt: UInt64
    }

    private static let defaults = UserDefaults.standard

    // MARK: - Keychain

    /// Load the device key, or generate + persist one when absent.
    static func loadOrCreate() throws -> (DeviceSigner, Storage) {
        if let data = keychainData() {
            return (DeviceSigner(key: try Curve25519.Signing.PrivateKey(rawRepresentation: data)), .keychain)
        }
        if let data = fallbackData() {
            return (DeviceSigner(key: try Curve25519.Signing.PrivateKey(rawRepresentation: data)), .insecureFallback)
        }
        let fresh = Curve25519.Signing.PrivateKey()
        let raw = fresh.rawRepresentation
        if persistKeychain(raw) {
            return (DeviceSigner(key: fresh), .keychain)
        }
        // Documented fallback (simulator edge cases, restricted devices):
        // plaintext store with an explicit in-app warning banner.
        defaults.set(raw.base64EncodedString(), forKey: fallbackKey)
        return (DeviceSigner(key: fresh), .insecureFallback)
    }

    static var storageLocation: Storage {
        if keychainData() != nil { return .keychain }
        if fallbackData() != nil { return .insecureFallback }
        return .keychain
    }

    private static func keychainData() -> Data? {
        let query: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: keychainService,
            kSecAttrAccount as String: keychainAccount,
            kSecReturnData as String: true,
            kSecMatchLimit as String: kSecMatchLimitOne,
        ]
        var item: CFTypeRef?
        let status = SecItemCopyMatching(query as CFDictionary, &item)
        guard status == errSecSuccess, let data = item as? Data else { return nil }
        return data
    }

    private static func persistKeychain(_ data: Data) -> Bool {
        if keychainData() != nil {
            let query: [String: Any] = [
                kSecClass as String: kSecClassGenericPassword,
                kSecAttrService as String: keychainService,
                kSecAttrAccount as String: keychainAccount,
            ]
            let update: [String: Any] = [kSecValueData as String: data]
            let status = SecItemUpdate(query as CFDictionary, update as CFDictionary)
            return status == errSecSuccess
        }
        let query: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: keychainService,
            kSecAttrAccount as String: keychainAccount,
            kSecValueData as String: data,
            kSecAttrAccessible as String: kSecAttrAccessibleWhenUnlockedThisDeviceOnly,
        ]
        let status = SecItemAdd(query as CFDictionary, nil)
        return status == errSecSuccess
    }

    private static func fallbackData() -> Data? {
        guard let b64 = defaults.string(forKey: fallbackKey) else { return nil }
        return Data(base64Encoded: b64)
    }

    /// Remove the device identity entirely (re-register flow).
    static func wipe() {
        let query: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: keychainService,
            kSecAttrAccount as String: keychainAccount,
        ]
        SecItemDelete(query as CFDictionary)
        defaults.removeObject(forKey: fallbackKey)
        defaults.removeObject(forKey: metaKey)
    }

    // MARK: - Registration metadata

    static func saveMeta(_ meta: DeviceMeta) {
        if let data = try? JSONEncoder().encode(meta) {
            defaults.set(data, forKey: metaKey)
        }
    }

    static func loadMeta() -> DeviceMeta? {
        guard let data = defaults.data(forKey: metaKey) else { return nil }
        return try? JSONDecoder().decode(DeviceMeta.self, from: data)
    }

    // MARK: - Host admin token (#209)

    /// The daemon's host admin token, stored in the Keychain (never in the
    /// plaintext fallback): the credential behind the Devices & Grants
    /// admin surface. Purely a host-admin bearer — never sent on the
    /// device-signed drive path.
    static func saveAdminToken(_ token: String) {
        let query: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: adminService,
            kSecAttrAccount as String: adminAccount,
            kSecReturnData as String: true,
            kSecMatchLimit as String: kSecMatchLimitOne,
        ]
        let data = Data(token.utf8)
        if SecItemCopyMatching(query as CFDictionary, nil) == errSecSuccess {
            let update: [String: Any] = [kSecValueData as String: data]
            SecItemUpdate(query as CFDictionary, update as CFDictionary)
        } else {
            var add = query
            add[kSecValueData as String] = data
            add[kSecAttrAccessible as String] = kSecAttrAccessibleWhenUnlockedThisDeviceOnly
            SecItemAdd(add as CFDictionary, nil)
        }
    }

    static func adminToken() -> String? {
        let query: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: adminService,
            kSecAttrAccount as String: adminAccount,
            kSecReturnData as String: true,
            kSecMatchLimit as String: kSecMatchLimitOne,
        ]
        var item: CFTypeRef?
        guard SecItemCopyMatching(query as CFDictionary, &item) == errSecSuccess,
              let data = item as? Data else { return nil }
        return String(data: data, encoding: .utf8)
    }

    static func clearAdminToken() {
        let query: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: adminService,
            kSecAttrAccount as String: adminAccount,
        ]
        SecItemDelete(query as CFDictionary)
    }
}
