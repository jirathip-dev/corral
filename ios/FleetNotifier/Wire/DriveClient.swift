import Foundation

/// Typed drive-plane failure. `.server` carries the daemon's typed error
/// body (`{kind, message, request_id?}`) plus its HTTP status.
enum DriveError: LocalizedError, Equatable, Sendable {
    case server(status: Int, kind: String, message: String, requestId: String?)
    case network(String)
    case encoding

    /// F1: `error.localizedDescription` used to fall back to the generic
    /// NSError bridge ("The operation couldn't be completed. (...)") and
    /// DISCARD the payload message — so a non-200 stream failure surfaced
    /// with zero diagnostic content (no status, no URL). Surface the
    /// underlying message instead.
    var errorDescription: String? {
        switch self {
        case .server(let status, let kind, let message, _):
            return "HTTP \(status) \(kind): \(message)"
        case .network(let message):
            return message
        case .encoding:
            return "payload encoding failed"
        }
    }
}

/// Result of one drive command: the daemon's `DriveResponse` (200, `ok`
/// true or false — dispatch-level refusal) or a typed client error.
enum DriveResult: Equatable, Sendable {
    case dispatched(DriveResponse)
    case refused(DriveError)
}

/// The signed read-plane client (P4-conformance.md R1–R10).
///
/// - `register` — POST /register with the registration token; read-only
///   default (grants come back empty; the host promotes capabilities
///   out-of-band on the registry after the #354 cut).
/// - `fetchGrants` — signed POST /grants-read (#101) self-service refresh.
/// - `registerDeviceToken` — signed POST /device-token (D16 push pairing).
/// - `drive` — builds the canonical envelope bytes, signs them with the
///   device Ed25519 key, POSTs the SignedDrive body. Idempotent by
///   `request_id` (the daemon replays stored responses byte-identical).
///   #354 L2: the only drive this client sends is the retained read_tail.
struct DriveClient: Sendable {
    private static let cancellationMessage = "drive cancelled"

    let host: URL
    let session: URLSession

    init(host: URL, session: URLSession = .shared) {
        self.host = host
        self.session = session
    }

    // MARK: - Request plumbing

    private func post(_ path: String, body: Data, headers: [String: String] = [:]) async throws -> (Int, Data) {
        try Task.checkCancellation()
        var request = URLRequest(url: host.appendingPathComponent(path))
        request.httpMethod = "POST"
        request.setValue("application/json", forHTTPHeaderField: "Content-Type")
        request.httpBody = body
        for (key, value) in headers {
            request.setValue(value, forHTTPHeaderField: key)
        }
        try Task.checkCancellation()
        let (data, response) = try await session.data(for: request)
        try Task.checkCancellation()
        guard let http = response as? HTTPURLResponse else {
            throw DriveError.network("non-HTTP response")
        }
        return (http.statusCode, data)
    }

    // MARK: - Registration (R1)

    /// `POST /register {token, public_key, name?}` → key_id with EMPTY
    /// grants (read-only default). `name` is the optional cosmetic device
    /// label (#209) stored by the daemon.
    func register(token: String, signer: DeviceSigner, name: String? = nil) async throws -> RegisterResponse {
        let trimmed = name?.trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
        let body: Data
        if trimmed.isEmpty {
            body = CanonicalJSON.registerBody(token: token, publicKeyB64: signer.publicKeyB64)
        } else {
            body = CanonicalJSON.registerBodyNamed(token: token, publicKeyB64: signer.publicKeyB64, name: trimmed)
        }
        let (status, data) = try await post("/register", body: body)
        guard status == 200 else {
            let typed = try? JSONDecoder().decode(DriveErrorBody.self, from: data)
            throw DriveError.server(status: status,
                                    kind: typed?.kind ?? "register_failed",
                                    message: typed?.message ?? String(data: data, encoding: .utf8) ?? "register failed",
                                    requestId: typed?.requestId)
        }
        return try JSONDecoder().decode(RegisterResponse.self, from: data)
    }

    // MARK: - Push registration (D16)

    /// `POST /device-token` — enroll (or clear, with an empty token) this
    /// device's APNs token on the daemon. Signed proof of possession of the
    /// device key (same canonical-bytes discipline as the read requests), so
    /// a stolen token alone cannot re-register push on another device.
    func registerDeviceToken(_ deviceToken: String, keyId: String,
                             signer: DeviceSigner) async throws -> DeviceTokenResponse {
        let ts = UInt64(Date().timeIntervalSince1970)
        let requestBytes = CanonicalJSON.deviceTokenBytes(keyId: keyId, deviceToken: deviceToken, ts: ts)
        let signature = try signer.sign(requestBytes).base64EncodedString()
        let body = CanonicalJSON.deviceTokenBody(keyId: keyId, signatureB64: signature, requestBytes: requestBytes)
        let (status, data) = try await post("/device-token", body: body)
        guard status == 200 else {
            let typed = try? JSONDecoder().decode(DriveErrorBody.self, from: data)
            throw DriveError.server(status: status,
                                    kind: typed?.kind ?? "device_token_failed",
                                    message: typed?.message ?? "device-token registration failed",
                                    requestId: typed?.requestId)
        }
        return try JSONDecoder().decode(DeviceTokenResponse.self, from: data)
    }

    // MARK: - Grants refresh (#101)

    /// `POST /grants-read` — signed self-service read of THIS key's CURRENT
    /// grants + expiry. Same proof-of-possession discipline as
    /// `/device-token` (canonical `{key_id, request, ts}` bytes, freshness
    /// enforced by the host), so a host-side promotion reaches the phone
    /// without admin involvement or a device reset.
    func fetchGrants(keyId: String, signer: DeviceSigner) async throws -> GrantsReadResponse {
        let ts = UInt64(Date().timeIntervalSince1970)
        let requestBytes = CanonicalJSON.grantsReadBytes(keyId: keyId, request: "grants-read", ts: ts)
        let signature = try signer.sign(requestBytes).base64EncodedString()
        let body = CanonicalJSON.grantsReadBody(keyId: keyId, signatureB64: signature, requestBytes: requestBytes)
        let (status, data) = try await post("/grants-read", body: body)
        guard status == 200 else {
            let typed = try? JSONDecoder().decode(DriveErrorBody.self, from: data)
            throw DriveError.server(status: status,
                                    kind: typed?.kind ?? "grants_read_failed",
                                    message: typed?.message ?? "grants-read failed",
                                    requestId: typed?.requestId)
        }
        return try JSONDecoder().decode(GrantsReadResponse.self, from: data)
    }

    // MARK: - Drive (R3–R8)

    /// Sign + send one read envelope. `requestId` must be stable per logical
    /// command (the daemon replays stored responses byte-identical); pass
    /// the same id when retrying, or omit for a fresh one. Read drives never
    /// require step-up (the whole destructive plane was removed by #354 L1).
    @discardableResult
    func drive(capability: Capability, target: String, payload: CanonicalJSON.Value,
               rev: UInt64?, requestId: String? = nil, keyId: String, signer: DeviceSigner) async -> DriveResult {
        guard !Task.isCancelled else { return .refused(.network(Self.cancellationMessage)) }
        let rid = requestId ?? Self.newRequestId()
        let bytes = CanonicalJSON.envelopeBytes(requestId: rid, capability: capability.rawValue,
                                                target: target, payload: payload, rev: rev)
        guard let signature = try? signer.sign(bytes).base64EncodedString() else {
            return .refused(.encoding)
        }
        let body = CanonicalJSON.signedDriveBody(keyId: keyId, signatureB64: signature, envelopeBytes: bytes)
        return await send(body: body, rid: rid)
    }

    private func send(body: Data, rid: String) async -> DriveResult {
        guard !Task.isCancelled else { return .refused(.network(Self.cancellationMessage)) }
        do {
            let (status, data) = try await post("/drive", body: body)
            if status == 200 {
                guard let response = try? JSONDecoder().decode(DriveResponse.self, from: data) else {
                    return .refused(.network("unparseable drive response"))
                }
                return .dispatched(response)
            }
            let typed = (try? JSONDecoder().decode(DriveErrorBody.self, from: data)) ??
                DriveErrorBody(kind: "http_\(status)", message: String(data: data, encoding: .utf8) ?? "HTTP \(status)", requestId: rid)
            return .refused(.server(status: status, kind: typed.kind, message: typed.message, requestId: typed.requestId))
        } catch is CancellationError {
            return .refused(.network(Self.cancellationMessage))
        } catch {
            return .refused(.network(error.localizedDescription))
        }
    }

    /// Fresh idempotency key. UUIDs are fine: request_id is opaque to the
    /// daemon and only needs uniqueness + stability across retries.
    static func newRequestId() -> String {
        UUID().uuidString.lowercased()
    }
}
