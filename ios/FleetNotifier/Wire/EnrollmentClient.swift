import Foundation

// MARK: - Device half of host-approved enrollment (#486; frozen #485 §5.2–5.4)

/// `POST /enroll/redeem` 200 success body — a *pending request*, never
/// authority: the code is the only credential and the device is paired only
/// after the host owner approves it on the local owner channel.
struct EnrollmentRedeemResponse: Codable, Equatable, Sendable {
    static let pendingState = "pending"

    var state: String
    var keyId: String
    var expiresTs: UInt64

    enum CodingKeys: String, CodingKey {
        case state
        case keyId = "key_id"
        case expiresTs = "expires_ts"
    }
}

/// `POST /enroll/status` 200 body. Status NEVER answers 409 for a consumed
/// code (a lost redeem response must not brick polling) and reads the record
/// live from the registry.
enum EnrollmentStatusResponse: Equatable, Sendable {
    case pending(expiresTs: UInt64)
    case approved(keyId: String, grants: [String], expiryTs: UInt64, revoked: Bool)
}

/// What one observed status (or C2b probe) means for pairing. `pending` is
/// never authority; approval requires a live record that carries the
/// `read_tail` grant this pairing is for; the grant list is carried verbatim
/// — this client never adds, widens, or rewrites a grant.
enum EnrollmentApprovalOutcome: Equatable, Sendable {
    case pending(expiresTs: UInt64)
    case approved(keyId: String, grants: [String], expiryTs: UInt64)
    case denied(EnrollmentApprovalDenial)
}

enum EnrollmentApprovalDenial: Equatable, Sendable {
    /// The C2b probe ran before this device's own redemption was recorded.
    case notRedeemed
    /// The C2b probe is exactly-once; this is a second attempt.
    case alreadyProbed
    /// The approved record is revoked — never authority.
    case revoked
    /// The record is live but does not carry the `read_tail` grant.
    case missingReadTailGrant
    /// The signed probe was refused by the host (revoked/expired/unknown).
    case refused(String)
    /// The signed probe could not reach the host.
    case transport(String)
}

extension EnrollmentStatusResponse {
    /// Frozen decision table applied to one status observation.
    var approval: EnrollmentApprovalOutcome {
        switch self {
        case .pending(let expiresTs):
            return .pending(expiresTs: expiresTs)
        case .approved(let keyId, let grants, let expiryTs, let revoked):
            if revoked {
                return .denied(.revoked)
            }
            guard grants.contains(EnrollmentQRPayload.readTailScope) else {
                return .denied(.missingReadTailGrant)
            }
            return .approved(keyId: keyId, grants: grants, expiryTs: expiryTs)
        }
    }
}

/// Typed enrollment-client failures. Messages are secret-free: the
/// redemption code is redacted out of any daemon text before it is surfaced,
/// so no error string can echo pairing material.
enum EnrollmentClientError: Error, Equatable, LocalizedError {
    /// The request never produced an HTTP response.
    case transport(String)
    /// The host answered non-200 with the frozen `{error, code}` body (or a
    /// proxy-shaped body, surfaced with an `http_<status>` code).
    case server(status: Int, code: String, message: String)
    /// 200 with a body that is not the v1 shape this route promises.
    case malformedResponse(String)
    /// 200 with a state outside the frozen vocabulary.
    case unexpectedState(String)

    var errorDescription: String? {
        switch self {
        case .transport(let message):
            return message
        case .server(let status, let code, let message):
            return "HTTP \(status) \(code): \(message)"
        case .malformedResponse(let message):
            return message
        case .unexpectedState(let state):
            return "Unexpected enrollment state '\(state.prefix(32))'."
        }
    }
}

/// The device-side enrollment client: `POST /enroll/redeem` and
/// `POST /enroll/status` on the existing network listener. Same request
/// plumbing and response-validation discipline as `DriveClient` (whose file
/// is fenced read-only for this lane); no other route is touched, no owner
/// operation exists here, and nothing is persisted or logged.
struct EnrollmentClient: Sendable {
    static let redeemPath = "/enroll/redeem"
    static let statusPath = "/enroll/status"
    /// Bounded daemon-message echo; pairing material is redacted first.
    private static let maxEchoedMessageCharacters = 300

    let host: URL
    let session: URLSession

    init(host: URL, session: URLSession = .shared) {
        self.host = host
        self.session = session
    }

    /// Redeem one scanned code with this device's Ed25519 public key. A 200
    /// is a pending request; `state` other than `pending` is a protocol
    /// violation (redeem can never be authority). No automatic retry: a lost
    /// response is recovered by polling `status`, never by a blind second
    /// redeem.
    func redeem(code: String, publicKeyB64: String, name: String? = nil) async throws -> EnrollmentRedeemResponse {
        let trimmedName = name?.trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
        let body = trimmedName.isEmpty
            ? Self.redeemBody(code: code, publicKeyB64: publicKeyB64)
            : Self.redeemBodyNamed(code: code, publicKeyB64: publicKeyB64, name: trimmedName)
        let (status, data) = try await post(Self.redeemPath, body: body)
        guard status == 200 else {
            throw Self.serverError(status: status, data: data, code: code)
        }
        guard let decoded = try? JSONDecoder().decode(EnrollmentRedeemResponse.self, from: data) else {
            throw EnrollmentClientError.malformedResponse("redeem response is not the v1 pending shape")
        }
        guard decoded.state == EnrollmentRedeemResponse.pendingState else {
            throw EnrollmentClientError.unexpectedState(decoded.state)
        }
        return decoded
    }

    /// Read-only polling for a code the caller already holds.
    func status(code: String) async throws -> EnrollmentStatusResponse {
        let (status, data) = try await post(Self.statusPath, body: Self.statusBody(code: code))
        guard status == 200 else {
            throw Self.serverError(status: status, data: data, code: code)
        }
        guard let body = try? JSONDecoder().decode(EnrollmentStatusBody.self, from: data) else {
            throw EnrollmentClientError.malformedResponse("status response is not a v1 body")
        }
        switch body.state {
        case "pending":
            guard let expiresTs = body.expiryTs else {
                throw EnrollmentClientError.malformedResponse("pending status body carries no expires_ts")
            }
            return .pending(expiresTs: expiresTs)
        case "approved":
            guard let keyId = body.keyId, let expiryTs = body.expiryTs else {
                throw EnrollmentClientError.malformedResponse("approved status body is incomplete")
            }
            return .approved(keyId: keyId,
                             grants: body.grants ?? [],
                             expiryTs: expiryTs,
                             revoked: body.revoked ?? false)
        default:
            throw EnrollmentClientError.unexpectedState(body.state)
        }
    }

    // MARK: - Request plumbing (same semantics as DriveClient.post)

    private func post(_ path: String, body: Data) async throws -> (Int, Data) {
        try Task.checkCancellation()
        var request = URLRequest(url: host.appendingPathComponent(path))
        request.httpMethod = "POST"
        request.setValue("application/json", forHTTPHeaderField: "Content-Type")
        request.httpBody = body
        try Task.checkCancellation()
        let (data, response) = try await session.data(for: request)
        try Task.checkCancellation()
        guard let http = response as? HTTPURLResponse else {
            throw EnrollmentClientError.transport("non-HTTP response")
        }
        return (http.statusCode, data)
    }

    // MARK: - Exact wire bodies (frozen field order, no extra fields)

    static func redeemBody(code: String, publicKeyB64: String) -> Data {
        var json = "{\"v\":1,\"code\":"
        json += CanonicalJSON.escaped(code)
        json += ",\"public_key\":"
        json += CanonicalJSON.escaped(publicKeyB64)
        json += "}"
        return Data(json.utf8)
    }

    static func redeemBodyNamed(code: String, publicKeyB64: String, name: String) -> Data {
        var json = "{\"v\":1,\"code\":"
        json += CanonicalJSON.escaped(code)
        json += ",\"public_key\":"
        json += CanonicalJSON.escaped(publicKeyB64)
        json += ",\"name\":"
        json += CanonicalJSON.escaped(name)
        json += "}"
        return Data(json.utf8)
    }

    static func statusBody(code: String) -> Data {
        var json = "{\"v\":1,\"code\":"
        json += CanonicalJSON.escaped(code)
        json += "}"
        return Data(json.utf8)
    }

    // MARK: - Error mapping

    private struct EnrollmentErrorBody: Decodable {
        var error: String?
        var code: String?
    }

    private struct EnrollmentStatusBody: Decodable {
        var state: String
        var keyId: String?
        var grants: [String]?
        var expiryTs: UInt64?
        var revoked: Bool?

        enum CodingKeys: String, CodingKey {
            case state
            case keyId = "key_id"
            case grants
            case expiryTs = "expiry_ts"
            case revoked
        }
    }

    /// Map a non-200 body onto the frozen vocabulary. The code is redacted
    /// from any echoed text so a host/proxy that reflects it cannot leak
    /// pairing material through an error string.
    private static func serverError(status: Int, data: Data, code: String) -> EnrollmentClientError {
        let body = try? JSONDecoder().decode(EnrollmentErrorBody.self, from: data)
        let rawMessage = body?.error ?? String(data: data, encoding: .utf8) ?? "HTTP \(status)"
        var message = rawMessage.replacingOccurrences(of: code, with: "<redacted>")
        if message.count > maxEchoedMessageCharacters {
            message = String(message.prefix(maxEchoedMessageCharacters)) + "…"
        }
        return .server(status: status, code: body?.code ?? "http_\(status)", message: message)
    }
}

/// C2b (frozen §5.4 handoff): if `/enroll/status` answers 404/410 after THIS
/// device's own redemption, the daemon may have restarted mid-pairing
/// (sessions are in-memory). Probe the existing signed `POST /grants-read`
/// EXACTLY ONCE and treat "registered + `read_tail` granted" as pairing
/// succeeded. Nothing else is success: a refused probe, a missing grant, or
/// an unreachable host is an explicit denial. The probe is never automatic —
/// the caller records its own redeem first, and a second call is refused
/// without touching the network.
struct EnrollmentApprovalRecovery: Equatable, Sendable {
    private(set) var redeemedKeyId: String?
    private(set) var probeIssued = false

    /// Record the key id from THIS device's own `redeem` response. Without
    /// it the recovery refuses (`.notRedeemed`) and issues no request.
    mutating func noteRedeemed(keyId: String) {
        redeemedKeyId = keyId
        probeIssued = false
    }

    /// One signed grants-read probe through the production `DriveClient`
    /// path, then a frozen decision.
    mutating func recoverAfterSessionLoss(drive: DriveClient,
                                          signer: DeviceSigner) async -> EnrollmentApprovalOutcome {
        guard let keyId = redeemedKeyId else {
            return .denied(.notRedeemed)
        }
        guard !probeIssued else {
            return .denied(.alreadyProbed)
        }
        probeIssued = true
        do {
            let response = try await drive.fetchGrants(keyId: keyId, signer: signer)
            guard response.ok, response.grants.contains(EnrollmentQRPayload.readTailScope) else {
                return .denied(.missingReadTailGrant)
            }
            return .approved(keyId: response.keyId, grants: response.grants, expiryTs: response.expiryTs)
        } catch let error as DriveError {
            switch error {
            case .server(let status, _, let message, _):
                return .denied(.refused("HTTP \(status): \(message)"))
            case .network(let message):
                return .denied(.transport(message))
            case .encoding:
                return .denied(.transport("payload encoding failed"))
            }
        } catch {
            return .denied(.transport(error.localizedDescription))
        }
    }
}
