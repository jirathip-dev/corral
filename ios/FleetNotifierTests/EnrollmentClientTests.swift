import CryptoKit
import XCTest
@testable import FleetNotifier

/// #486 device-half client conformance (#485 frozen §5.2–5.4) plus the C2b
/// recovery decision. Transport is a URLProtocol fixture only — no live
/// endpoint, QR, grant, Keychain, or private credential is touched. All
/// fixture material is deliberately synthetic.
final class EnrollmentClientTests: XCTestCase {
    // SAFETY: synthetic fixture material only — fixed 32-byte fills encoded
    // to base64, never a real redemption code or device key.
    private static let syntheticCode = Data(repeating: 7, count: 32).base64EncodedString()
    // SAFETY: synthetic fixture material only (fixed 32-byte fill).
    private static let syntheticPublicKey = Data(repeating: 9, count: 32).base64EncodedString()
    // SAFETY: fixed valid fixture URL literal (synthetic hostname, no network).
    private static let host = URL(string: "https://enroll-synthetic.example")!
    private static let keyId = "dev_5f3d2a1c9b8e7f60"

    private var session: URLSession?

    private var redeemURL: URL { Self.host.appendingPathComponent("/enroll/redeem") }
    private var statusURL: URL { Self.host.appendingPathComponent("/enroll/status") }
    private var grantsReadURL: URL { Self.host.appendingPathComponent("/grants-read") }

    override func setUp() {
        super.setUp()
        let configuration = URLSessionConfiguration.ephemeral
        configuration.protocolClasses = [EnrollmentFixtureURLProtocol.self]
        session = URLSession(configuration: configuration)
        EnrollmentFixtureURLProtocol.clear()
    }

    override func tearDown() {
        session?.invalidateAndCancel()
        session = nil
        EnrollmentFixtureURLProtocol.clear()
        super.tearDown()
    }

    private func client() throws -> EnrollmentClient {
        EnrollmentClient(host: Self.host, session: try XCTUnwrap(session))
    }

    private func driveClient() throws -> DriveClient {
        DriveClient(host: Self.host, session: try XCTUnwrap(session))
    }

    private func signer() -> DeviceSigner {
        DeviceSigner(key: Curve25519.Signing.PrivateKey())
    }

    private func bodyString(_ request: URLRequest) -> String? {
        request.httpBody.flatMap { String(data: $0, encoding: .utf8) }
    }

    private func script(_ responses: [URL: (Int, Data)]) {
        EnrollmentFixtureURLProtocol.setScript(responses)
    }

    // MARK: - Exact requests + response validation

    func testRedeemPostsTheExactCanonicalBodyAndDecodesPending() async throws {
        script([redeemURL: (200, Data(#"{"state":"pending","key_id":"\#(Self.keyId)","expires_ts":1900000000}"#.utf8))])

        let response = try await client().redeem(code: Self.syntheticCode,
                                                 publicKeyB64: Self.syntheticPublicKey)

        XCTAssertEqual(response, EnrollmentRedeemResponse(state: "pending",
                                                          keyId: Self.keyId,
                                                          expiresTs: 1_900_000_000))
        let requests = EnrollmentFixtureURLProtocol.requests
        XCTAssertEqual(requests.count, 1)
        let request = try XCTUnwrap(requests.first)
        XCTAssertEqual(request.httpMethod, "POST")
        XCTAssertEqual(request.url, redeemURL)
        XCTAssertEqual(request.value(forHTTPHeaderField: "Content-Type"), "application/json")
        XCTAssertEqual(bodyString(request),
                       #"{"v":1,"code":"\#(Self.syntheticCode)","public_key":"\#(Self.syntheticPublicKey)"}"#)
    }

    func testRedeemNamedBodyAppendsTheCosmeticNameAndTrimsIt() async throws {
        script([redeemURL: (200, Data(#"{"state":"pending","key_id":"\#(Self.keyId)","expires_ts":1900000000}"#.utf8))])

        _ = try await client().redeem(code: Self.syntheticCode,
                                      publicKeyB64: Self.syntheticPublicKey,
                                      name: "  iPhone  ")

        let request = try XCTUnwrap(EnrollmentFixtureURLProtocol.requests.first)
        XCTAssertEqual(bodyString(request),
                       #"{"v":1,"code":"\#(Self.syntheticCode)","public_key":"\#(Self.syntheticPublicKey)","name":"iPhone"}"#)
    }

    func testBlankNameKeepsTheUnnamedBody() async throws {
        script([redeemURL: (200, Data(#"{"state":"pending","key_id":"\#(Self.keyId)","expires_ts":1900000000}"#.utf8))])

        _ = try await client().redeem(code: Self.syntheticCode,
                                      publicKeyB64: Self.syntheticPublicKey,
                                      name: "   ")

        let request = try XCTUnwrap(EnrollmentFixtureURLProtocol.requests.first)
        XCTAssertEqual(bodyString(request),
                       #"{"v":1,"code":"\#(Self.syntheticCode)","public_key":"\#(Self.syntheticPublicKey)"}"#)
    }

    func testStatusPostsTheExactCanonicalBodyAndDecodesPending() async throws {
        script([statusURL: (200, Data(#"{"state":"pending","expires_ts":1900000000}"#.utf8))])

        let response = try await client().status(code: Self.syntheticCode)

        XCTAssertEqual(response, .pending(expiresTs: 1_900_000_000))
        let request = try XCTUnwrap(EnrollmentFixtureURLProtocol.requests.first)
        XCTAssertEqual(request.httpMethod, "POST")
        XCTAssertEqual(request.url, statusURL)
        XCTAssertEqual(bodyString(request), #"{"v":1,"code":"\#(Self.syntheticCode)"}"#)
    }

    func testStatusDecodesTheApprovedShapeWithGrantsVerbatim() async throws {
        script([statusURL: (200, Data(#"{"state":"approved","key_id":"\#(Self.keyId)","grants":["read_tail"],"expiry_ts":1800000000,"revoked":false}"#.utf8))])

        let response = try await client().status(code: Self.syntheticCode)

        XCTAssertEqual(response, .approved(keyId: Self.keyId,
                                           grants: [EnrollmentQRPayload.readTailScope],
                                           expiryTs: 1_800_000_000,
                                           revoked: false))
    }

    func testStatusUnknownStateIsATypedError() async throws {
        script([statusURL: (200, Data(#"{"state":"weird"}"#.utf8))])

        do {
            _ = try await client().status(code: Self.syntheticCode)
            XCTFail("an unknown state must not decode")
        } catch let error as EnrollmentClientError {
            XCTAssertEqual(error, .unexpectedState("weird"))
        }
    }

    func testRedeemNonPending200StateIsRejected() async throws {
        script([redeemURL: (200, Data(#"{"state":"approved","key_id":"\#(Self.keyId)","expires_ts":1900000000}"#.utf8))])

        do {
            _ = try await client().redeem(code: Self.syntheticCode, publicKeyB64: Self.syntheticPublicKey)
            XCTFail("redeem can never be authority")
        } catch let error as EnrollmentClientError {
            XCTAssertEqual(error, .unexpectedState("approved"))
        }
    }

    func testRedeemMalformed200BodyIsATypedError() async throws {
        script([redeemURL: (200, Data(#"{}"#.utf8))])

        do {
            _ = try await client().redeem(code: Self.syntheticCode, publicKeyB64: Self.syntheticPublicKey)
            XCTFail("a non-shape body must not decode")
        } catch let error as EnrollmentClientError {
            guard case .malformedResponse = error else {
                return XCTFail("expected .malformedResponse, got \(error)")
            }
        }
    }

    // MARK: - Frozen error vocabulary, secret-free failures

    func testRedeemServerErrorsCarryTheFrozenCodeVocabulary() async throws {
        let cases: [(status: Int, code: String)] = [
            (404, "enroll_unknown_code"),
            (409, "enroll_redeemed"),
            (409, "already_registered"),
            (410, "enroll_expired"),
            (400, "malformed_request"),
            (400, "bad_public_key"),
            (400, "bad_name"),
        ]
        for entry in cases {
            script([redeemURL: (entry.status, Data(#"{"error":"refused by host","code":"\#(entry.code)"}"#.utf8))])
            do {
                _ = try await client().redeem(code: Self.syntheticCode, publicKeyB64: Self.syntheticPublicKey)
                XCTFail("expected \(entry.code)")
            } catch let error as EnrollmentClientError {
                XCTAssertEqual(error, .server(status: entry.status,
                                              code: entry.code,
                                              message: "refused by host"))
            }
        }
    }

    func testServerErrorRedactsAnEchoedCode() async throws {
        script([redeemURL: (400, Data(#"{"error":"rejected \#(Self.syntheticCode)","code":"malformed_request"}"#.utf8))])

        do {
            _ = try await client().redeem(code: Self.syntheticCode, publicKeyB64: Self.syntheticPublicKey)
            XCTFail("expected a server error")
        } catch let error as EnrollmentClientError {
            let text = String(describing: error) + " " + (error.errorDescription ?? "")
            XCTAssertFalse(text.contains(Self.syntheticCode),
                           "error text must never echo the redemption code: \(text)")
            XCTAssertTrue(text.contains("<redacted>"), "an echoed code is redacted: \(text)")
        }
    }

    func testTransportFailureIssuesExactlyOneRequestAndNeverAutoRetries() async throws {
        // No script for /enroll/redeem: the fixture fails like an
        // unreachable host.
        do {
            _ = try await client().redeem(code: Self.syntheticCode, publicKeyB64: Self.syntheticPublicKey)
            XCTFail("an unreachable host must not yield a pending response")
        } catch let error as EnrollmentClientError {
            guard case .transport = error else {
                return XCTFail("expected .transport, got \(error)")
            }
            XCTAssertFalse((error.errorDescription ?? "").contains(Self.syntheticCode))
        }
        XCTAssertEqual(EnrollmentFixtureURLProtocol.requests.count, 1,
                       "a failed redeem must not be retried implicitly")

        // A second attempt is caller-driven: still no internal retry, so the
        // count grows by exactly one.
        _ = try? await client().redeem(code: Self.syntheticCode, publicKeyB64: Self.syntheticPublicKey)
        XCTAssertEqual(EnrollmentFixtureURLProtocol.requests.count, 2)
    }

    /// A lost redeem response is recovered by polling status — the frozen
    /// "status never 409" property — never by a blind second redeem.
    func testLostRedeemResponseIsRecoveredByStatusPolling() async throws {
        script([statusURL: (200, Data(#"{"state":"pending","expires_ts":1900000000}"#.utf8))])

        _ = try? await client().redeem(code: Self.syntheticCode, publicKeyB64: Self.syntheticPublicKey)
        let pending = try await client().status(code: Self.syntheticCode)

        XCTAssertEqual(pending, .pending(expiresTs: 1_900_000_000))
        let redeemRequests = EnrollmentFixtureURLProtocol.requests.filter { $0.url == redeemURL }
        XCTAssertEqual(redeemRequests.count, 1, "status polling must not trigger a second redeem")
    }

    // MARK: - Approval decision: pending is never authority

    func testPendingIsNeverAuthority() {
        let pending = EnrollmentStatusResponse.pending(expiresTs: 42)
        XCTAssertEqual(pending.approval, .pending(expiresTs: 42))
        XCTAssertNotEqual(pending.approval,
                          .approved(keyId: Self.keyId, grants: ["read_tail"], expiryTs: 42))
    }

    func testApprovedRecordWithoutReadTailGrantIsDenied() {
        let empty = EnrollmentStatusResponse.approved(keyId: Self.keyId, grants: [],
                                                      expiryTs: 1_800_000_000, revoked: false)
        XCTAssertEqual(empty.approval, .denied(.missingReadTailGrant))

        // A broader-but-wrong capability set does not satisfy read_tail.
        let diffOnly = EnrollmentStatusResponse.approved(keyId: Self.keyId, grants: ["read_diff"],
                                                         expiryTs: 1_800_000_000, revoked: false)
        XCTAssertEqual(diffOnly.approval, .denied(.missingReadTailGrant))
    }

    func testApprovedGrantsAreCarriedVerbatim() {
        let approved = EnrollmentStatusResponse.approved(keyId: Self.keyId, grants: ["read_tail"],
                                                         expiryTs: 1_800_000_000, revoked: false)
        XCTAssertEqual(approved.approval, .approved(keyId: Self.keyId,
                                                    grants: ["read_tail"],
                                                    expiryTs: 1_800_000_000))
    }

    func testApprovedRevokedRecordIsDenied() {
        let revoked = EnrollmentStatusResponse.approved(keyId: Self.keyId, grants: ["read_tail"],
                                                        expiryTs: 1_800_000_000, revoked: true)
        XCTAssertEqual(revoked.approval, .denied(.revoked),
                       "a revoked record is never pairing authority")
    }

    // MARK: - C2b: exactly-once signed probe after this device's own redeem

    func testRecoveryRefusesToProbeWithoutOwnRedemption() async throws {
        var recovery = EnrollmentApprovalRecovery()

        let outcome = await recovery.recoverAfterSessionLoss(drive: try driveClient(), signer: signer())

        XCTAssertEqual(outcome, .denied(.notRedeemed))
        XCTAssertTrue(EnrollmentFixtureURLProtocol.requests.isEmpty,
                      "no probe may leave the device before its own redemption")
    }

    func testRecoveryProbesExactlyOnceAfterOwnRedemption() async throws {
        var recovery = EnrollmentApprovalRecovery()
        recovery.noteRedeemed(keyId: Self.keyId)
        script([grantsReadURL: (200, Data(#"{"ok":true,"key_id":"\#(Self.keyId)","grants":["read_tail"],"expiry_ts":1800000000}"#.utf8))])

        let first = await recovery.recoverAfterSessionLoss(drive: try driveClient(), signer: signer())

        XCTAssertEqual(first, .approved(keyId: Self.keyId,
                                        grants: ["read_tail"],
                                        expiryTs: 1_800_000_000))
        XCTAssertEqual(EnrollmentFixtureURLProtocol.requests.count, 1)
        let probe = try XCTUnwrap(EnrollmentFixtureURLProtocol.requests.first)
        XCTAssertEqual(probe.url, grantsReadURL)
        XCTAssertEqual(probe.httpMethod, "POST")
        // The probe is the REAL signed DriveClient request (key_id +
        // signature + canonical request envelope), not a bespoke call.
        let body = try XCTUnwrap(probe.httpBody)
        let json = try XCTUnwrap(try JSONSerialization.jsonObject(with: body) as? [String: Any])
        XCTAssertEqual(json["key_id"] as? String, Self.keyId)
        XCTAssertNotNil(json["signature"])
        XCTAssertNotNil(json["request"])

        let second = await recovery.recoverAfterSessionLoss(drive: try driveClient(), signer: signer())
        XCTAssertEqual(second, .denied(.alreadyProbed))
        XCTAssertEqual(EnrollmentFixtureURLProtocol.requests.count, 1,
                       "the C2b probe is exactly-once")
    }

    func testRecoveryDeniesWhenProbeShowsMissingGrant() async throws {
        var recovery = EnrollmentApprovalRecovery()
        recovery.noteRedeemed(keyId: Self.keyId)
        script([grantsReadURL: (200, Data(#"{"ok":true,"key_id":"\#(Self.keyId)","grants":[],"expiry_ts":1800000000}"#.utf8))])

        let outcome = await recovery.recoverAfterSessionLoss(drive: try driveClient(), signer: signer())

        XCTAssertEqual(outcome, .denied(.missingReadTailGrant),
                       "registered without read_tail is not pairing success")
    }

    func testRecoveryDeniesWhenProbeIsRefused() async throws {
        var recovery = EnrollmentApprovalRecovery()
        recovery.noteRedeemed(keyId: Self.keyId)
        // The daemon refuses a revoked key's signed read with this shape.
        script([grantsReadURL: (403, Data(#"{"error":"device key revoked"}"#.utf8))])

        let outcome = await recovery.recoverAfterSessionLoss(drive: try driveClient(), signer: signer())

        XCTAssertEqual(outcome, .denied(.refused(#"HTTP 403: {"error":"device key revoked"}"#)))
    }

    func testRecoveryTransportFailureIsDeniedAndStaysConsumed() async throws {
        var recovery = EnrollmentApprovalRecovery()
        recovery.noteRedeemed(keyId: Self.keyId)

        let outcome = await recovery.recoverAfterSessionLoss(drive: try driveClient(), signer: signer())

        guard case .denied(.transport) = outcome else {
            return XCTFail("an unreachable host is a transport denial, got \(outcome)")
        }
        let second = await recovery.recoverAfterSessionLoss(drive: try driveClient(), signer: signer())
        XCTAssertEqual(second, .denied(.alreadyProbed))
        XCTAssertEqual(EnrollmentFixtureURLProtocol.requests.count, 1)
    }

    // MARK: - Source hygiene (no logging, clipboard, or persistence)

    func testEnrollmentSourcesCarryNoLoggingClipboardOrPersistenceCalls() throws {
        let testsDirectory = URL(fileURLWithPath: #filePath).deletingLastPathComponent()
        let sourcesDirectory = testsDirectory.deletingLastPathComponent()
        let sources = [
            sourcesDirectory.appendingPathComponent("FleetNotifier/Wire/EnrollmentPayload.swift"),
            sourcesDirectory.appendingPathComponent("FleetNotifier/Wire/EnrollmentClient.swift"),
        ]
        let forbidden = ["print(", "Logger(", "os_log(", "NSLog", "UIPasteboard", "UserDefaults", "FileManager"]
        for source in sources {
            let text = try String(contentsOf: source, encoding: .utf8)
            for token in forbidden {
                XCTAssertFalse(text.contains(token),
                               "\(source.lastPathComponent) must not contain \(token)")
            }
        }
    }
}

/// Per-URL response script + request capture for the enrollment tests (the
/// repo's URLProtocol fixture pattern; unscripted URLs behave like an
/// unreachable host). Test-bundle only — never part of the app target.
private final class EnrollmentFixtureURLProtocol: URLProtocol {
    private static let lock = NSLock()
    private static var scriptStorage: [URL: (status: Int, body: Data)] = [:]
    private static var requestsStorage: [URLRequest] = []

    static func setScript(_ script: [URL: (Int, Data)]) {
        lock.lock()
        scriptStorage = script
        requestsStorage = []
        lock.unlock()
    }

    static func clear() {
        setScript([:])
    }

    static var requests: [URLRequest] {
        lock.lock()
        defer { lock.unlock() }
        return requestsStorage
    }

    override class func canInit(with request: URLRequest) -> Bool { true }
    override class func canonicalRequest(for request: URLRequest) -> URLRequest { request }

    override func startLoading() {
        Self.lock.lock()
        guard let url = request.url else {
            Self.lock.unlock()
            client?.urlProtocol(self, didFailWithError: URLError(.badURL))
            return
        }
        var copy = request
        if let body = Self.bodyData(request) {
            copy.httpBody = body
        }
        Self.requestsStorage.append(copy)
        let scripted = Self.scriptStorage[url]
        Self.lock.unlock()
        guard let (statusCode, body) = scripted else {
            client?.urlProtocol(self, didFailWithError: URLError(.cannotConnectToHost))
            return
        }
        // SAFETY: fixed response built from the scripted status/body above.
        guard let response = HTTPURLResponse(url: url, statusCode: statusCode,
                                             httpVersion: "HTTP/1.1",
                                             headerFields: ["Content-Type": "application/json"]) else {
            client?.urlProtocol(self, didFailWithError: URLError(.badServerResponse))
            return
        }
        client?.urlProtocol(self, didReceive: response, cacheStoragePolicy: .notAllowed)
        if !body.isEmpty {
            client?.urlProtocol(self, didLoad: body)
        }
        client?.urlProtocolDidFinishLoading(self)
    }

    override func stopLoading() {}

    /// URLSession moves request bodies into a stream; read it back for
    /// exact-body assertions.
    private static func bodyData(_ request: URLRequest) -> Data? {
        if let body = request.httpBody {
            return body
        }
        guard let stream = request.httpBodyStream else {
            return nil
        }
        stream.open()
        defer { stream.close() }
        var data = Data()
        let bufferSize = 4096
        var buffer = [UInt8](repeating: 0, count: bufferSize)
        while stream.hasBytesAvailable {
            let read = stream.read(&buffer, maxLength: bufferSize)
            guard read > 0 else { break }
            data.append(buffer, count: read)
        }
        return data
    }
}
