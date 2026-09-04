import CryptoKit
import Foundation

// MARK: - Host key trust helpers (#399 B3/B4)

/// The daemon's `GET /host-key` response: the stable host identity is an
/// X25519 public key, never a hostname or path.
struct HostKeyResponse: Codable, Equatable, Sendable {
    var algorithm: String
    /// Base64 of the raw 32-byte X25519 public key.
    var publicKey: String
    var note: String?

    enum CodingKeys: String, CodingKey {
        case algorithm
        case publicKey = "public_key"
        case note
    }
}

/// Pure X25519 host-key validation + fingerprint derivation (#399 B3).
///
/// The wire contract (src/auth/http.rs): `{algorithm: "X25519",
/// public_key: <base64 of 32 bytes>, note}`. The full key is pinned
/// verbatim; the fingerprint is a stable human-readable digest derived
/// from the RAW key bytes (SHA-256, uppercase hex in 4-char groups) — it
/// is never sent anywhere and never used as an identifier.
enum HostKeyTrust {
    /// The daemon's advertised algorithm string for host identity.
    static let x25519Algorithm = "X25519"
    /// Raw X25519 public key byte length.
    static let keyByteCount = 32

    /// `GET /host-key` payload passes the declared form: algorithm is
    /// X25519 and the key is base64 of exactly 32 bytes.
    static func isWellFormed(_ response: HostKeyResponse) -> Bool {
        response.algorithm == x25519Algorithm
            && rawKey(from: response.publicKey) != nil
    }

    /// Raw 32 bytes from a base64 public key, or nil when the key is not
    /// base64 of exactly 32 bytes.
    static func rawKey(from base64: String) -> Data? {
        guard let decoded = Data(base64Encoded: base64), decoded.count == keyByteCount else {
            return nil
        }
        return decoded
    }

    /// True when the response's key equals the pinned key (base64 is
    /// canonical for the wire, so string equality is exact).
    static func matches(_ response: HostKeyResponse, pinnedKeyB64: String) -> Bool {
        isWellFormed(response) && response.publicKey == pinnedKeyB64
    }

    /// Human-readable fingerprint: SHA-256 over the raw key bytes,
    /// uppercase hex in 4-character groups. Nil when the base64 is not a
    /// well-formed key. Deterministic — the same key always derives the
    /// same fingerprint, so a confirmation can be compared by eye.
    static func fingerprint(forBase64 keyB64: String) -> String? {
        guard let raw = rawKey(from: keyB64) else { return nil }
        let digest = SHA256.hash(data: raw)
        let hex = digest.map { String(format: "%02X", $0) }.joined()
        let groups = stride(from: 0, to: hex.count, by: 4).map { index in
            let start = hex.index(hex.startIndex, offsetBy: index)
            let end = hex.index(start, offsetBy: 4, limitedBy: hex.endIndex) ?? hex.endIndex
            return String(hex[start..<end])
        }
        return groups.joined(separator: " ")
    }

    /// Short confirmation form: the leading groups plus an elided tail
    /// (the full fingerprint stays copyable in the details row).
    static func shortFingerprint(_ fingerprint: String) -> String {
        let groups = fingerprint.split(separator: " ")
        guard groups.count > 4 else { return fingerprint }
        return groups.prefix(4).joined(separator: " ") + " …"
    }
}
