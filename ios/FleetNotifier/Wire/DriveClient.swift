import Foundation

/// Typed drive-plane failure. `.server` carries the daemon's typed error
/// body (`{kind, message, request_id?}`) plus its HTTP status.
enum DriveError: Error, Equatable, Sendable {
    case server(status: Int, kind: String, message: String, requestId: String?)
    case network(String)
    case encoding

    var isStepUpRequired: Bool {
        if case .server(_, let kind, _, _) = self { return kind == "step_up_required" }
        return false
    }
}

/// Result of one drive command: the daemon's `DriveResponse` (200, `ok`
/// true or false — dispatch-level refusal) or a typed client error.
enum DriveResult: Equatable, Sendable {
    case dispatched(DriveResponse)
    case refused(DriveError)
}

/// The signed drive plane client (P4-conformance.md R1–R10).
///
/// - `register` — POST /register with the registration token; read-only
///   default (grants come back empty; the host promotes capabilities).
/// - `drive` — builds the canonical envelope bytes, signs them with the
///   device Ed25519 key, POSTs the SignedDrive body. Idempotent by
///   `request_id` (the daemon replays stored responses byte-identical).
/// - Step-up: destructive payloads require Face ID → `POST /step-up` mint →
///   retry with `X-Step-Up-Token`. Mirrored pre-flight AND reactive on 403.
struct DriveClient: Sendable {
    let host: URL
    let session: URLSession

    init(host: URL, session: URLSession = .shared) {
        self.host = host
        self.session = session
    }

    // MARK: - Request plumbing

    private func post(_ path: String, body: Data, headers: [String: String] = [:]) async throws -> (Int, Data) {
        var request = URLRequest(url: host.appendingPathComponent(path))
        request.httpMethod = "POST"
        request.setValue("application/json", forHTTPHeaderField: "Content-Type")
        request.httpBody = body
        for (key, value) in headers {
            request.setValue(value, forHTTPHeaderField: key)
        }
        let (data, response) = try await session.data(for: request)
        guard let http = response as? HTTPURLResponse else {
            throw DriveError.network("non-HTTP response")
        }
        return (http.statusCode, data)
    }

    // MARK: - Registration (R1)

    /// `POST /register {token, public_key}` → key_id with EMPTY grants
    /// (read-only default; the host promotes via /grants).
    func register(token: String, signer: DeviceSigner) async throws -> RegisterResponse {
        let body = CanonicalJSON.registerBody(token: token, publicKeyB64: signer.publicKeyB64)
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

    // MARK: - Step-up (R9)

    /// `POST /step-up` — signed proof of possession; single-use token.
    /// The host enforces freshness `|now - ts| < 60s`; this is only a
    /// device-clock sanity gate (a wildly wrong clock cannot mint).
    func mintStepUpToken(keyId: String, signer: DeviceSigner) async throws -> StepUpResponse {
        let deviceNow = Date().timeIntervalSince1970
        guard deviceNow > 1_600_000_000 else {
            throw DriveError.network("device clock is not set; step-up mint refused")
        }
        let ts = UInt64(deviceNow)
        let nonce = Data((0..<32).map { _ in UInt8.random(in: .min ... .max) }).base64EncodedString()
        let requestBytes = CanonicalJSON.stepUpBytes(keyId: keyId, purpose: "destructive", nonce: nonce, ts: ts)
        let signature = try signer.sign(requestBytes).base64EncodedString()
        let body = CanonicalJSON.stepUpBody(keyId: keyId, signatureB64: signature, requestBytes: requestBytes)
        let (status, data) = try await post("/step-up", body: body)
        guard status == 200 else {
            let typed = try? JSONDecoder().decode(DriveErrorBody.self, from: data)
            throw DriveError.server(status: status,
                                    kind: typed?.kind ?? "step_up_failed",
                                    message: typed?.message ?? "step-up mint failed",
                                    requestId: typed?.requestId)
        }
        return try JSONDecoder().decode(StepUpResponse.self, from: data)
    }

    // MARK: - Drive (R3–R9)

    /// Sign + send one envelope. `requestId` must be stable per logical
    /// command (the daemon replays stored responses byte-identical); pass
    /// the same id when retrying, or omit for a fresh one.
    ///
    /// Step-up: when the payload matches the daemon's destructive-pattern
    /// mirror, Face ID runs BEFORE the send, then `/step-up` mints a token
    /// and the drive carries `X-Step-Up-Token`. A server-side
    /// `step_up_required` refusal (mirror mismatch or an expired token) is
    /// answered reactively with the same flow — same request_id, so an
    /// attempt that actually dispatched replays instead of double-sending.
    @discardableResult
    func drive(capability: Capability, target: String, payload: CanonicalJSON.Value,
               rev: UInt64?, requestId: String? = nil, keyId: String, signer: DeviceSigner,
               biometrics: Biometrics = Biometrics(), stepUp: Bool = true) async -> DriveResult {
        let rid = requestId ?? Self.newRequestId()
        let bytes = CanonicalJSON.envelopeBytes(requestId: rid, capability: capability.rawValue,
                                                target: target, payload: payload, rev: rev)
        guard let signature = try? signer.sign(bytes).base64EncodedString() else {
            return .refused(.encoding)
        }
        let body = CanonicalJSON.signedDriveBody(keyId: keyId, signatureB64: signature, envelopeBytes: bytes)

        var result: DriveResult
        if stepUp && DestructivePatterns.required(payload) {
            guard await biometrics.authenticate() else {
                return .refused(.server(status: 403, kind: "step_up_denied",
                                        message: "Face ID step-up declined; the destructive command was not sent",
                                        requestId: rid))
            }
            do {
                let minted = try await mintStepUpToken(keyId: keyId, signer: signer)
                result = await send(body: body, rid: rid, stepUpToken: minted.token)
            } catch {
                return .refused(error as? DriveError ?? .network(error.localizedDescription))
            }
        } else {
            result = await send(body: body, rid: rid, stepUpToken: nil)
        }

        if stepUp, case .refused(let error) = result, error.isStepUpRequired {
            guard await biometrics.authenticate() else {
                return .refused(.server(status: 403, kind: "step_up_denied",
                                        message: "Face ID step-up declined; the destructive command was not sent",
                                        requestId: rid))
            }
            do {
                let minted = try await mintStepUpToken(keyId: keyId, signer: signer)
                result = await send(body: body, rid: rid, stepUpToken: minted.token)
            } catch {
                return .refused(error as? DriveError ?? .network(error.localizedDescription))
            }
        }
        return result
    }

    private func send(body: Data, rid: String, stepUpToken: String?) async -> DriveResult {
        do {
            var headers: [String: String] = [:]
            if let stepUpToken { headers["X-Step-Up-Token"] = stepUpToken }
            let (status, data) = try await post("/drive", body: body, headers: headers)
            if status == 200 {
                guard let response = try? JSONDecoder().decode(DriveResponse.self, from: data) else {
                    return .refused(.network("unparseable drive response"))
                }
                return .dispatched(response)
            }
            let typed = (try? JSONDecoder().decode(DriveErrorBody.self, from: data)) ??
                DriveErrorBody(kind: "http_\(status)", message: String(data: data, encoding: .utf8) ?? "HTTP \(status)", requestId: rid)
            return .refused(.server(status: status, kind: typed.kind, message: typed.message, requestId: typed.requestId))
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
