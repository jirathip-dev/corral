import Foundation

/// Deterministic JSON serialization matching Rust `serde_json::to_vec` byte
/// for byte, so an Ed25519 signature covers the daemon's exact canonical
/// envelope bytes (P4-conformance.md / src/drive/mod.rs).
///
/// serde_json rules mirrored here:
/// - Compact output: no whitespace anywhere.
/// - Struct field order is preserved (envelope: request_id, capability,
///   target, payload, rev; step-up: key_id, purpose, nonce, ts).
/// - Object keys are serialized in sorted order — serde_json's `Value` map
///   is a BTreeMap, so payload objects sort lexicographically by byte.
/// - Strings escape only `"`, `\` and control chars < 0x20 (short escapes
///   \b \t \n \f \r; everything else as `\u00xx`, lowercase hex). Non-ASCII
///   passes through as raw UTF-8.
/// - Integers serialize as plain decimals; `null` for nil optionals.
enum CanonicalJSON {

    /// A JSON value that can be emitted deterministically. Payload objects
    /// built from the typed drive payloads are the only values signed.
    enum Value: Sendable {
        case string(String)
        case int(Int64)
        case uint(UInt64)
        case bool(Bool)
        case null
        /// Keys are sorted by UTF-8 byte order on encode (serde_json BTreeMap).
        case object([(key: String, value: Value)])
        case array([Value])

        static func object(_ pairs: [String: Value]) -> Value {
            .object(pairs.map { (key: $0.key, value: $0.value) })
        }
    }

    // MARK: - Payload shapes (sorted-key objects, kind-tagged)

    /// `{"kind":"prompt","text":...}` (single key set; order irrelevant but
    /// kept byte-identical to serde's sorted output).
    static func promptPayload(text: String) -> Value {
        .object([(key: "kind", value: .string("prompt")),
                 (key: "text", value: .string(text))])
    }

    /// `{"kind":"read_tail","lines":N}`; `lines: null` when absent (serde
    /// has no skip attr on `lines`, so None serializes as JSON null).
    static func readTailPayload(lines: UInt32?) -> Value {
        .object([(key: "kind", value: .string("read_tail")),
                 (key: "lines", value: lines.map { .uint(UInt64($0)) } ?? .null)])
    }

    /// Interrupt/kill/attach commands take a JSON null payload in the drive
    /// contract. Keeping the builder named prevents the UI from accidentally
    /// inventing a prompt-shaped payload for an interrupt.
    static func interruptPayload() -> Value { .null }
    static func killPayload() -> Value { .null }
    static func attachPayload() -> Value { .null }

    /// `{"approval_id":...,"choice":...,"kind":"approve","prompt_hash":...}`
    /// — sorted byte order: approval_id < choice < kind < prompt_hash.
    static func approvePayload(approvalId: String, promptHash: String, choice: String) -> Value {
        .object([(key: "approval_id", value: .string(approvalId)),
                 (key: "choice", value: .string(choice)),
                 (key: "kind", value: .string("approve")),
                 (key: "prompt_hash", value: .string(promptHash))])
    }

    // MARK: - Envelope / step-up canonical bytes (fixed struct field order)

    /// `canonical_envelope_bytes` — the exact bytes a signature must cover.
    /// `rev` is omitted when nil (`skip_serializing_if = "Option::is_none"`).
    static func envelopeBytes(requestId: String, capability: String, target: String,
                              payload: Value, rev: UInt64?) -> Data {
        var json = "{"
        json += "\"request_id\":"
        json += escaped(requestId)
        json += ",\"capability\":"
        json += escaped(capability)
        json += ",\"target\":"
        json += escaped(target)
        json += ",\"payload\":"
        json += encode(payload)
        if let rev {
            json += ",\"rev\":\(rev)"
        }
        json += "}"
        return Data(json.utf8)
    }

    /// `canonical_step_up_bytes` — fixed order key_id, purpose, nonce, ts.
    static func stepUpBytes(keyId: String, purpose: String, nonce: String, ts: UInt64) -> Data {
        var json = "{\"key_id\":"
        json += escaped(keyId)
        json += ",\"purpose\":"
        json += escaped(purpose)
        json += ",\"nonce\":"
        json += escaped(nonce)
        json += ",\"ts\":\(ts)}"
        return Data(json.utf8)
    }

    /// `canonical_device_token_bytes` (D16) — fixed order key_id,
    /// device_token, ts (mirror of `DeviceTokenRequest` in src/push/).
    static func deviceTokenBytes(keyId: String, deviceToken: String, ts: UInt64) -> Data {
        var json = "{\"key_id\":"
        json += escaped(keyId)
        json += ",\"device_token\":"
        json += escaped(deviceToken)
        json += ",\"ts\":\(ts)}"
        return Data(json.utf8)
    }

    /// `canonical_grants_read_bytes` (#101) — fixed order key_id, request,
    /// ts (mirror of `GrantsReadRequest` in src/push/payload.rs).
    static func grantsReadBytes(keyId: String, request: String, ts: UInt64) -> Data {
        var json = "{\"key_id\":"
        json += escaped(keyId)
        json += ",\"request\":"
        json += escaped(request)
        json += ",\"ts\":\(ts)}"
        return Data(json.utf8)
    }

    // MARK: - Value encoding

    static func encode(_ value: Value) -> String {
        switch value {
        case .string(let s): return escaped(s)
        case .int(let i): return String(i)
        case .uint(let u): return String(u)
        case .bool(let b): return b ? "true" : "false"
        case .null: return "null"
        case .object(let pairs):
            let sorted = pairs.sorted { $0.key.utf8ByteLessThan($1.key) }
            var json = "{"
            for (i, pair) in sorted.enumerated() {
                if i > 0 { json += "," }
                json += escaped(pair.key)
                json += ":"
                json += encode(pair.value)
            }
            json += "}"
            return json
        case .array(let items):
            var json = "["
            for (i, item) in items.enumerated() {
                if i > 0 { json += "," }
                json += encode(item)
            }
            json += "]"
            return json
        }
    }

    /// The full `SignedDrive` body: `{key_id, signature, envelope}`.
    /// The envelope inside is byte-identical to the signed canonical bytes.
    static func signedDriveBody(keyId: String, signatureB64: String, envelopeBytes: Data) -> Data {
        var json = "{\"key_id\":"
        json += escaped(keyId)
        json += ",\"signature\":"
        json += escaped(signatureB64)
        json += ",\"envelope\":"
        json += String(data: envelopeBytes, encoding: .utf8) ?? "{}"
        json += "}"
        return Data(json.utf8)
    }

    /// `{token, public_key}` registration body.
    static func registerBody(token: String, publicKeyB64: String) -> Data {
        var json = "{\"token\":"
        json += escaped(token)
        json += ",\"public_key\":"
        json += escaped(publicKeyB64)
        json += "}"
        return Data(json.utf8)
    }

    /// `POST /register` body with the cosmetic device display name (#209).
    static func registerBodyNamed(token: String, publicKeyB64: String, name: String) -> Data {
        var json = "{\"token\":"
        json += escaped(token)
        json += ",\"public_key\":"
        json += escaped(publicKeyB64)
        json += ",\"name\":"
        json += escaped(name)
        json += "}"
        return Data(json.utf8)
    }

    /// `{key_id, signature, request}` step-up body.
    static func stepUpBody(keyId: String, signatureB64: String, requestBytes: Data) -> Data {
        var json = "{\"key_id\":"
        json += escaped(keyId)
        json += ",\"signature\":"
        json += escaped(signatureB64)
        json += ",\"request\":"
        json += String(data: requestBytes, encoding: .utf8) ?? "{}"
        json += "}"
        return Data(json.utf8)
    }

    /// `{key_id, signature, request}` device-token body (D16).
    static func deviceTokenBody(keyId: String, signatureB64: String, requestBytes: Data) -> Data {
        var json = "{\"key_id\":"
        json += escaped(keyId)
        json += ",\"signature\":"
        json += escaped(signatureB64)
        json += ",\"request\":"
        json += String(data: requestBytes, encoding: .utf8) ?? "{}"
        json += "}"
        return Data(json.utf8)
    }

    /// `{key_id, signature, request}` grants-read body (#101).
    static func grantsReadBody(keyId: String, signatureB64: String, requestBytes: Data) -> Data {
        var json = "{\"key_id\":"
        json += escaped(keyId)
        json += ",\"signature\":"
        json += escaped(signatureB64)
        json += ",\"request\":"
        json += String(data: requestBytes, encoding: .utf8) ?? "{}"
        json += "}"
        return Data(json.utf8)
    }

    // MARK: - String escaping (serde_json parity)

    /// JSON-escaped, quoted string. Escapes `"`, `\` and control chars
    /// (< 0x20) with serde_json's exact spellings; non-ASCII stays raw.
    static func escaped(_ s: String) -> String {
        var out = "\""
        for scalar in s.unicodeScalars {
            switch scalar.value {
            case 0x22: out += "\\\""
            case 0x5C: out += "\\\\"
            case 0x08: out += "\\b"
            case 0x09: out += "\\t"
            case 0x0A: out += "\\n"
            case 0x0C: out += "\\f"
            case 0x0D: out += "\\r"
            case 0x00...0x1F: out += "\\u" + String(format: "%04x", scalar.value)
            default: out.unicodeScalars.append(scalar)
            }
        }
        out += "\""
        return out
    }

    /// Unescape a JSON string body (without surrounding quotes) — used only
    /// for tests that round-trip escaped payloads.
    static func unescaped(_ s: String) -> String {
        var out = ""
        let scalars = Array(s.unicodeScalars)
        var i = 0
        while i < scalars.count {
            let c = scalars[i]
            if c == "\\", i + 1 < scalars.count {
                i += 1
                switch scalars[i] {
                case "\"": out += "\""
                case "\\": out += "\\"
                case "b": out += "\u{8}"
                case "t": out += "\t"
                case "n": out += "\n"
                case "f": out += "\u{c}"
                case "r": out += "\r"
                case "u":
                    if i + 4 < scalars.count {
                        let hex = String(String.UnicodeScalarView(scalars[(i + 1)...(i + 4)])).lowercased()
                        if let v = UInt32(hex, radix: 16), let sc = UnicodeScalar(v) {
                            out.unicodeScalars.append(sc)
                            i += 4
                        }
                    }
                default: out.unicodeScalars.append(scalars[i])
                }
            } else {
                out.unicodeScalars.append(c)
            }
            i += 1
        }
        return out
    }
}

extension String {
    /// Lexicographic byte-order comparison of UTF-8 (matches Rust BTreeMap
    /// key ordering, which is byte order on `String`).
    func utf8ByteLessThan(_ other: String) -> Bool {
        utf8.lexicographicallyPrecedes(other.utf8)
    }
}
