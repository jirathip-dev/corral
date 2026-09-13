import Foundation

// MARK: - Host enrollment QR payload v1 (#486; frozen #485 contract §5.1)

/// The host's v1 enrollment payload, exactly as the daemon produces it
/// (`src/auth/owner.rs::qr_payload_v1`): strict canonical JSON text, fixed
/// field order, no insignificant whitespace, no extra fields:
///
/// `{"v":1,"host_key":"<b64 32 B>","endpoint":"https://…","code":"<b64 32 B>","expires_ts":<unix s>,"scope":"read_tail"}`
///
/// The payload carries no authority: the `code` is a single-use, short-lived
/// redemption credential and pairing only completes through the host's
/// explicit owner approval. This parser is the device-side gate the frozen
/// contract requires — unknown `v`, missing/extra fields, non-canonical
/// text, a non-`https` endpoint, malformed key material, any `scope` other
/// than `read_tail`, and an already-past deadline are all rejected.
///
/// Whitespace INSIDE a JSON string value is string content, never
/// insignificant JSON whitespace: such values are rejected by the per-field
/// rules below (the producer can never emit them), not by a blanket
/// whitespace ban. Only whitespace between tokens — which the canonical
/// form forbids — plus leading/trailing whitespace is classified as
/// non-canonical text.
struct EnrollmentQRPayload: Equatable, Sendable {
    /// The only protocol version this client speaks.
    static let supportedVersion = 1
    /// The literal scope every v1 payload must carry — never `read_diff`,
    /// never any control capability.
    static let readTailScope = "read_tail"
    /// The daemon's own cap on the minted endpoint string
    /// (`MAX_ENDPOINT_CHARS` in `src/auth/owner.rs`).
    static let maxEndpointCharacters = 256
    /// Canonical field order the producer emits; any other order is
    /// non-canonical.
    static let canonicalFields = ["v", "host_key", "endpoint", "code", "expires_ts", "scope"]

    let v: Int
    let hostKeyB64: String
    let endpoint: String
    let code: String
    let expiresTs: UInt64
    let scope: String

    /// Frozen precedence edge (`src/auth/authorizer.rs`): `now >= expires_ts`
    /// is already expired.
    func isExpired(now: UInt64) -> Bool {
        now >= expiresTs
    }
}

/// Typed, user-actionable parse failures. No case ever carries the
/// redemption `code`, so no error text can leak pairing material.
enum EnrollmentPayloadError: Error, Equatable, LocalizedError {
    /// The scanned text is not a JSON object at all.
    case malformedText
    /// A required field is absent (named from the frozen field list — never
    /// from untrusted input).
    case missingField(String)
    /// A field outside the frozen v1 set is present. Deliberately carries NO
    /// payload: the field name is untrusted QR text and could be a copy of
    /// the redemption code, so it is never retained or reflected.
    case extraField
    /// Valid JSON object, but not the canonical producer text (field order,
    /// insignificant whitespace, duplicate keys, non-canonical numbers).
    case notCanonical
    case unsupportedVersion(Int)
    /// The scope is not the frozen `read_tail`. Carries NO payload for the
    /// same reason as `extraField`: the scope value is untrusted QR text.
    case unsupportedScope
    /// The endpoint value itself is unusable (non-https, whitespace/control
    /// characters, unparseable, or over the producer's cap).
    case invalidEndpoint
    case invalidHostKey
    case invalidCode
    /// `now >= expires_ts`.
    case expired

    var errorDescription: String? {
        switch self {
        case .malformedText:
            return "Not a Corral host code — the scanned text is not a JSON object."
        case .missingField(let field):
            return "Host code is missing the \(field) field."
        case .extraField:
            return "Host code carries an unexpected field."
        case .notCanonical:
            return "Host code is not the canonical v1 enrollment payload."
        case .unsupportedVersion(let v):
            return "Unsupported host code version \(v)."
        case .unsupportedScope:
            return "Unsupported host code scope — pairing is read-only."
        case .invalidEndpoint:
            return "Host code carries an unusable endpoint (https:// required)."
        case .invalidHostKey:
            return "Host code carries a malformed host key."
        case .invalidCode:
            return "Host code carries a malformed redemption code."
        case .expired:
            return "This host code has expired — mint a new one on the host."
        }
    }
}

extension EnrollmentQRPayload {
    /// Strict parse of one scanned QR text. `now` is injectable so the
    /// expiry edge is deterministic in tests; production passes the wall
    /// clock. Throws `EnrollmentPayloadError`; error text never contains the
    /// `code`.
    static func parse(_ text: String, now: UInt64) throws -> EnrollmentQRPayload {
        // Leading/trailing whitespace is the one whitespace class that is
        // insignificant around the whole document; the producer emits none.
        guard text == text.trimmingCharacters(in: .whitespacesAndNewlines) else {
            throw EnrollmentPayloadError.notCanonical
        }
        guard let data = text.data(using: .utf8),
              let object = try? JSONSerialization.jsonObject(with: data),
              let objectFields = object as? [String: Any] else {
            throw EnrollmentPayloadError.malformedText
        }
        let present = Set(objectFields.keys)
        if let missing = canonicalFields.first(where: { !present.contains($0) }) {
            throw EnrollmentPayloadError.missingField(missing)
        }
        if !present.subtracting(Set(canonicalFields)).isEmpty {
            throw EnrollmentPayloadError.extraField
        }
        // Shape + order: the canonical producer text contains no whitespace
        // between tokens, no reordered fields, and no duplicate keys.
        guard let captures = canonicalCaptures(text) else {
            throw EnrollmentPayloadError.notCanonical
        }
        guard let version = Int(captures.v),
              let expiresTs = UInt64(captures.expires) else {
            throw EnrollmentPayloadError.notCanonical
        }
        let hostKey = CanonicalJSON.unescaped(captures.hostKey)
        let endpoint = CanonicalJSON.unescaped(captures.endpoint)
        let code = CanonicalJSON.unescaped(captures.code)
        let scope = CanonicalJSON.unescaped(captures.scope)
        // Byte-exact canonical form: re-serializing the parsed values in the
        // producer's field order must reproduce the scanned text verbatim.
        // This rejects insignificant whitespace, `\u`-escaped ASCII that the
        // producer would emit raw, and any other non-canonical spelling.
        guard canonicalText(v: version, hostKey: hostKey, endpoint: endpoint,
                            code: code, expiresTs: expiresTs, scope: scope) == text else {
            throw EnrollmentPayloadError.notCanonical
        }
        guard version == supportedVersion else {
            throw EnrollmentPayloadError.unsupportedVersion(version)
        }
        guard scope == readTailScope else {
            throw EnrollmentPayloadError.unsupportedScope
        }
        guard isWellFormedEndpoint(endpoint) else {
            throw EnrollmentPayloadError.invalidEndpoint
        }
        guard isCanonicalKeyMaterial(hostKey) else {
            throw EnrollmentPayloadError.invalidHostKey
        }
        guard isCanonicalKeyMaterial(code) else {
            throw EnrollmentPayloadError.invalidCode
        }
        guard now < expiresTs else {
            throw EnrollmentPayloadError.expired
        }
        return EnrollmentQRPayload(v: version, hostKeyB64: hostKey, endpoint: endpoint,
                                   code: code, expiresTs: expiresTs, scope: scope)
    }

    private struct Captures {
        var v: String
        var hostKey: String
        var endpoint: String
        var code: String
        var expires: String
        var scope: String
    }

    /// Field order + JSON string tokens + decimal integers, anchored. String
    /// slots capture a full JSON string token (escapes included) so that
    /// whitespace INSIDE a value is captured as content and decided by the
    /// per-field validators below, never by this shape check.
    private static let canonicalPattern = #"^\{"v":([0-9]+),"host_key":"((?:[^"\\]|\\.)*)","endpoint":"((?:[^"\\]|\\.)*)","code":"((?:[^"\\]|\\.)*)","expires_ts":([0-9]+),"scope":"((?:[^"\\]|\\.)*)"\}$"#

    private static func canonicalCaptures(_ text: String) -> Captures? {
        guard let regex = try? NSRegularExpression(pattern: canonicalPattern) else { return nil }
        let full = NSRange(text.startIndex..<text.endIndex, in: text)
        guard let match = regex.firstMatch(in: text, range: full),
              match.range == full else { return nil }
        func group(_ index: Int) -> String? {
            guard let range = Range(match.range(at: index), in: text) else { return nil }
            return String(text[range])
        }
        guard let v = group(1), let hostKey = group(2), let endpoint = group(3),
              let code = group(4), let expires = group(5), let scope = group(6) else {
            return nil
        }
        return Captures(v: v, hostKey: hostKey, endpoint: endpoint,
                        code: code, expires: expires, scope: scope)
    }

    private static func canonicalText(v: Int, hostKey: String, endpoint: String,
                                      code: String, expiresTs: UInt64, scope: String) -> String {
        var json = "{\"v\":\(v),\"host_key\":"
        json += CanonicalJSON.escaped(hostKey)
        json += ",\"endpoint\":"
        json += CanonicalJSON.escaped(endpoint)
        json += ",\"code\":"
        json += CanonicalJSON.escaped(code)
        json += ",\"expires_ts\":\(expiresTs),\"scope\":"
        json += CanonicalJSON.escaped(scope)
        json += "}"
        return json
    }

    /// `https://` only (the daemon refuses to mint anything else), within the
    /// producer's character cap, and free of whitespace/control characters —
    /// the same content rule the mint applies (`src/auth/owner.rs:355-364`).
    /// Structural parseability is delegated to the existing
    /// `HostURLForm.normalized` helper.
    private static func isWellFormedEndpoint(_ endpoint: String) -> Bool {
        guard endpoint.hasPrefix("https://"),
              !endpoint.isEmpty,
              endpoint.count <= maxEndpointCharacters else { return false }
        let forbidden = CharacterSet.whitespacesAndNewlines.union(.controlCharacters)
        guard endpoint.rangeOfCharacter(from: forbidden) == nil else { return false }
        return HostURLForm.normalized(endpoint) != nil
    }

    /// Exactly 32 raw bytes AND a canonical STANDARD-alphabet encoding
    /// (re-encoding the decoded bytes reproduces the input, so unpadded or
    /// otherwise non-canonical base64 is rejected). Reuses the existing
    /// `HostKeyTrust.rawKey` shape check for the base64+length half.
    private static func isCanonicalKeyMaterial(_ value: String) -> Bool {
        guard HostKeyTrust.rawKey(from: value) != nil,
              let decoded = Data(base64Encoded: value) else { return false }
        return decoded.base64EncodedString() == value
    }
}
