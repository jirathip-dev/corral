import AVFoundation
import CryptoKit
import XCTest
@testable import FleetNotifier

/// #486: the Add Host QR enrollment flow — scan → live-key verification →
/// identity + read-only scope confirmation → host-approved pairing — plus
/// every negative path the issue names (malformed/expired/reused code,
/// wrong or changed host key, duplicate host, denied camera permission,
/// offline host, interrupted pairing), the credential-isolation proof for
/// equal raw agent ids across hosts, and the privacy guards that keep the
/// transient pairing material out of views, logs, and persistence.
///
/// Transport is a URLProtocol fixture only: no live host, camera, Keychain,
/// or real code is touched. All fixture material is synthetic.
@MainActor
final class EnrollmentPairingFlowTests: XCTestCase {
    // SAFETY: synthetic fixture material only — fixed 32-byte fills encoded
    // to base64, never a real redemption code, host key, or device key.
    private static let macKey = Data(repeating: 0x11, count: 32).base64EncodedString()
    // SAFETY: synthetic fixture material only (fixed 32-byte fill).
    private static let hostBKey = Data(repeating: 0x22, count: 32).base64EncodedString()
    // SAFETY: synthetic fixture material only (fixed 32-byte fill).
    private static let wrongKey = Data(repeating: 0x33, count: 32).base64EncodedString()
    // SAFETY: synthetic fixture material only (fixed 32-byte fill).
    private static let newCode = Data(repeating: 0x44, count: 32).base64EncodedString()
    // SAFETY: synthetic fixture material only (fixed 32-byte fill).
    private static let reusedCode = Data(repeating: 0x55, count: 32).base64EncodedString()
    private static let enrolledKeyID = "dev_qr_enrolled"
    private static let keyExpiry: UInt64 = 1_800_000_000

    // SAFETY: fixed valid fixture URLs under distinct synthetic hostnames.
    private let macURL = URL(string: "https://qr-mac.example")!
    // SAFETY: fixed valid fixture URL under a distinct synthetic hostname.
    private let bURL = URL(string: "https://qr-b.example")!

    private var suiteName = ""
    private var model: AppModel?
    private var session: URLSession?
    private var defaults: UserDefaults?
    private var store: HostProfileStore?
    private var baseScript: [URL: (Int, Data, Bool)] = [:]
    /// ONE device key for the whole scenario — the real app presents the
    /// same device key to every host.
    private let deviceSigner = DeviceSigner(key: Curve25519.Signing.PrivateKey())

    private var enrollRedeemURL: URL { bURL.appendingPathComponent("/enroll/redeem") }
    private var enrollStatusURL: URL { bURL.appendingPathComponent("/enroll/status") }
    private var grantsReadURL: URL { bURL.appendingPathComponent("/grants-read") }
    private var eventsURL: URL { bURL.appendingPathComponent("/events") }
    private var macEventsURL: URL { macURL.appendingPathComponent("/events") }
    private var driveURL: URL { bURL.appendingPathComponent("/drive") }
    private var macDriveURL: URL { macURL.appendingPathComponent("/drive") }

    private func cleanup() {
        model?.stopLive()
        model = nil
        session?.invalidateAndCancel()
        session = nil
        KeyContinuityGate.reset()
        EnrollmentFlowURLProtocol.clear()
        if !suiteName.isEmpty {
            // SAFETY: suiteName was freshly minted per test.
            UserDefaults(suiteName: suiteName)!.removePersistentDomain(forName: suiteName)
            suiteName = ""
        }
    }

    private func waitUntil(_ condition: @autoclosure () -> Bool,
                           timeout: TimeInterval = 5) async {
        let deadline = Date().addingTimeInterval(timeout)
        while !condition(), Date() < deadline {
            try? await Task.sleep(nanoseconds: 20_000_000)
        }
    }

    private func now() -> UInt64 { UInt64(Date().timeIntervalSince1970) }

    /// The canonical v1 producer text (frozen #485 §5.1): fixed field order,
    /// no insignificant whitespace.
    private func payloadText(hostKey: String = EnrollmentPairingFlowTests.hostBKey,
                             endpoint: String? = nil,
                             code: String = EnrollmentPairingFlowTests.newCode,
                             expiresTs: UInt64? = nil) -> String {
        let url = endpoint ?? bURL.absoluteString
        let deadline = expiresTs ?? (now() + 600)
        return "{\"v\":1,\"host_key\":\"\(hostKey)\",\"endpoint\":\"\(url)\",\"code\":\"\(code)\",\"expires_ts\":\(deadline),\"scope\":\"read_tail\"}"
    }

    private func hostKeyBody(_ key: String) -> Data {
        Data("{\"algorithm\":\"X25519\",\"public_key\":\"\(key)\"}".utf8)
    }

    /// One pinned ACTIVE "Mac" profile + the scripted endpoints the flow
    /// touches (B's `/host-key` serves B's key; `/events` holds open so the
    /// post-commit `startLive()` connect is deterministic). The scenario
    /// script is kept so tests can extend it (never silently drop the
    /// endpoints the flow needs).
    private func makeModel(macKey: String = EnrollmentPairingFlowTests.macKey,
                           hostBKey: String = EnrollmentPairingFlowTests.hostBKey) -> AppModel {
        suiteName = "corral.h486.enroll.\(UUID().uuidString)"
        // SAFETY: a fresh UUID suite name is always a valid suite.
        let defaults = UserDefaults(suiteName: suiteName)!
        let store = HostProfileStore(directory: nil, defaults: defaults)
        // SAFETY: fixed fixture key literal from the constants above.
        let mac = try! store.addProfile(displayName: "Mac",
                                        urlString: macURL.absoluteString,
                                        hostKeyB64: macKey,
                                        fingerprint: "FINGER-MAC",
                                        keyId: "dev_mac",
                                        grants: ["read_tail"],
                                        expiryTs: Self.keyExpiry,
                                        registeredAt: 1)
        defaults.set(mac.id.uuidString, forKey: "fleetnotifier.activeHostProfileID")
        self.defaults = defaults
        self.store = store
        baseScript = [
            bURL.appendingPathComponent("/host-key"): (200, hostKeyBody(hostBKey), false),
            macURL.appendingPathComponent("/host-key"): (200, hostKeyBody(macKey), false),
            eventsURL: (200, Data(), true),
            macEventsURL: (200, Data(), true),
        ]
        script(baseScript)
        return makeModelInstance(defaults: defaults, store: store)
    }

    /// A second AppModel over the SAME store/defaults (a relaunch), or the
    /// first one during `makeModel`.
    private func makeModelInstance(defaults: UserDefaults, store: HostProfileStore) -> AppModel {
        let configuration = URLSessionConfiguration.ephemeral
        configuration.protocolClasses = [EnrollmentFlowURLProtocol.self]
        let session = URLSession(configuration: configuration)
        self.session = session
        let signer = deviceSigner
        let model = AppModel(session: session, defaults: defaults,
                             identityLoader: { (signer, .insecureFallback) },
                             loadMeta: { nil }, saveMeta: { _ in },
                             wipeIdentity: {},
                             profileStore: store,
                             enrollmentPollInterval: 0.01)
        self.model = model
        return model
    }

    /// The base scenario script, extended (or shortened) per test. Response
    /// sequences advance one entry per request and repeat the last one.
    private func sequence(_ overrides: [URL: [(Int, Data, Bool)]])
        -> [URL: [(Int, Data, Bool)]] {
        var merged = baseScript.mapValues { [$0] }
        for (url, entries) in overrides {
            merged[url] = entries
        }
        return merged
    }

    private func script(_ responses: [URL: (Int, Data, Bool)]) {
        EnrollmentFlowURLProtocol.setScript(responses)
    }

    private func requests(to url: URL) -> [URLRequest] {
        EnrollmentFlowURLProtocol.requests.filter { $0.url?.absoluteString == url.absoluteString }
    }

    private func bodyString(_ request: URLRequest?) -> String? {
        request?.httpBody.flatMap { String(data: $0, encoding: .utf8) }
    }

    private func statusPending(_ expiresTs: UInt64) -> Data {
        Data("{\"state\":\"pending\",\"expires_ts\":\(expiresTs)}".utf8)
    }

    private func statusApproved(keyId: String = EnrollmentPairingFlowTests.enrolledKeyID,
                                grants: String = "[\"read_tail\"]",
                                revoked: String = "false") -> Data {
        Data("{\"state\":\"approved\",\"key_id\":\"\(keyId)\",\"grants\":\(grants),\"expiry_ts\":\(Self.keyExpiry),\"revoked\":\(revoked)}".utf8)
    }

    private func redeemOK(expiresTs: UInt64? = nil) -> Data {
        Data("{\"state\":\"pending\",\"key_id\":\"\(Self.enrolledKeyID)\",\"expires_ts\":\(expiresTs ?? (now() + 600))}".utf8)
    }

    private func serverRefusal(status: Int, code: String) -> Data {
        Data("{\"error\":\"refused\",\"code\":\"\(code)\"}".utf8)
    }

    // MARK: - Scan: live-key verification + the confirmation payload

    func testScanVerifiesTheLiveHostKeyAndShowsIdentityAndReadOnlyScope() async throws {
        defer { cleanup() }
        let model = makeModel()

        await model.handleScannedEnrollmentCode(payloadText())

        let enrollment = try XCTUnwrap(model.addHostDraft.enrollment,
                                       "a verified scan must move the draft into the enrollment phase")
        XCTAssertEqual(enrollment.phase, .reviewing,
                       "nothing is sent to the host before the user confirms")
        XCTAssertEqual(enrollment.urlString, bURL.absoluteString)
        XCTAssertEqual(enrollment.hostKeyB64, Self.hostBKey,
                       "the exact host identity the code carries is shown for confirmation")
        XCTAssertEqual(enrollment.fingerprint,
                       HostKeyTrust.fingerprint(forBase64: Self.hostBKey),
                       "the confirmation shows the derived fingerprint of the verified key")
        XCTAssertEqual(enrollment.scope, "read_tail",
                       "the confirmation states the read-only scope the code grants")
        XCTAssertEqual(enrollment.displayName, "qr-b",
                       "the host name is prefilled from the code's endpoint")
        XCTAssertEqual(model.addHostDraft.urlString, bURL.absoluteString,
                       "the entry phase keeps the scanned endpoint for correction")
        XCTAssertNil(model.addHostDraft.prepared)
        XCTAssertEqual(requests(to: enrollRedeemURL).count, 0,
                       "the code is NOT redeemed before the user confirms")
        XCTAssertEqual(model.profiles.count, 1, "nothing is committed by a scan")
    }

    func testScanRejectsACodeWhoseHostKeyDiffersFromTheLiveKey() async throws {
        defer { cleanup() }
        let model = makeModel()

        await model.handleScannedEnrollmentCode(payloadText(hostKey: Self.wrongKey))

        XCTAssertNil(model.addHostDraft.enrollment, "a key mismatch never reaches confirmation")
        let message = try XCTUnwrap(model.addHostDraft.errorMessage)
        XCTAssertTrue(message.contains("does not match the host's live key"),
                      "the mismatch must be explicit: \(message)")
        XCTAssertFalse(message.contains(Self.newCode),
                       "the error must never echo the pairing code")
        XCTAssertEqual(model.profiles.count, 1)
        XCTAssertEqual(model.activeProfile?.displayName, "Mac")
    }

    func testScanRejectsMalformedAndExpiredCodes() async throws {
        defer { cleanup() }
        let model = makeModel()

        await model.handleScannedEnrollmentCode("https://not-a-corral-code.example")
        let malformed = try XCTUnwrap(model.addHostDraft.errorMessage)
        XCTAssertTrue(malformed.lowercased().contains("not a corral host code"), malformed)
        XCTAssertNil(model.addHostDraft.enrollment)

        model.addHostDraft.errorMessage = nil
        await model.handleScannedEnrollmentCode(payloadText(expiresTs: now() - 1))
        let expired = try XCTUnwrap(model.addHostDraft.errorMessage)
        XCTAssertTrue(expired.lowercased().contains("expired"), expired)
        XCTAssertNil(model.addHostDraft.enrollment)

        XCTAssertEqual(model.profiles.count, 1, "no failure path commits a profile")
        XCTAssertEqual(requests(to: enrollRedeemURL).count, 0)
    }

    func testScanRejectsADuplicateHostIdentityAndNeverRedeems() async throws {
        defer { cleanup() }
        // The scanned code carries the ALREADY-PINNED Mac key, and the
        // endpoint answers with that same key (the host identity already
        // exists under another address).
        let model = makeModel()
        var current = baseScript
        current[bURL.appendingPathComponent("/host-key")] = (200, hostKeyBody(Self.macKey), false)
        script(current)

        await model.handleScannedEnrollmentCode(payloadText(hostKey: Self.macKey))

        let message = try XCTUnwrap(model.addHostDraft.errorMessage)
        XCTAssertTrue(message.contains("already paired with that host key"), message)
        XCTAssertNil(model.addHostDraft.enrollment)
        XCTAssertEqual(model.profiles.count, 1)
        XCTAssertEqual(requests(to: enrollRedeemURL).count, 0,
                       "a duplicate host must never reach the redeem route")
    }

    func testChangedHostKeyOnAPairedAddressIsActionableAndTouchesNoProfile() async throws {
        defer { cleanup() }
        // Same address as Mac, but the host (live and in the code) now
        // answers with a DIFFERENT key: rotation/reinstall.
        let model = makeModel()
        var current = currentScript()
        current[macURL.appendingPathComponent("/host-key")] = (200, hostKeyBody(Self.hostBKey), false)
        script(current)
        let macID = try XCTUnwrap(model.activeProfile?.id)
        let macKeyBefore = model.activeProfile?.hostKeyB64

        await model.handleScannedEnrollmentCode(payloadText(hostKey: Self.hostBKey,
                                                            endpoint: macURL.absoluteString))

        let message = try XCTUnwrap(model.addHostDraft.errorMessage)
        XCTAssertTrue(message.contains("different key"), message)
        XCTAssertTrue(message.contains("remove that host entry"),
                      "the changed-key state must name the recovery: \(message)")
        XCTAssertNil(model.addHostDraft.enrollment)
        XCTAssertEqual(model.profiles.count, 1)
        XCTAssertEqual(model.activeProfileID, macID)
        XCTAssertEqual(model.activeProfile?.hostKeyB64, macKeyBefore,
                       "an existing profile is untouched by the rejected scan")
        XCTAssertEqual(requests(to: enrollRedeemURL).count, 0)
    }

    func testOfflineHostDuringScanIsActionableAndCommitsNothing() async throws {
        defer { cleanup() }
        let model = makeModel()
        // B stops answering: `/host-key` is unscripted (unreachable).
        EnrollmentFlowURLProtocol.setScript([:])

        await model.handleScannedEnrollmentCode(payloadText())

        let message = try XCTUnwrap(model.addHostDraft.errorMessage)
        XCTAssertTrue(message.contains("Could not reach the host"), message)
        XCTAssertNil(model.addHostDraft.enrollment)
        XCTAssertEqual(model.profiles.count, 1)
    }

    // MARK: - Redeem refusals + the explicit approval wait

    func testReusedCodeIsActionableAndCommitsNothing() async throws {
        defer { cleanup() }
        let model = makeModel()
        var current = currentScript()
        current[enrollRedeemURL] = (409, serverRefusal(status: 409, code: "enroll_redeemed"), false)
        script(current)
        await model.handleScannedEnrollmentCode(payloadText(code: Self.reusedCode))
        XCTAssertEqual(model.addHostDraft.enrollment?.phase, .reviewing)

        let outcome = await model.completeEnrollmentPairing()

        guard case .failure(let failure) = outcome else {
            return XCTFail("a reused code must fail, got \(outcome)")
        }
        XCTAssertTrue(failure.message.contains("already used"), failure.message)
        guard case .failed(let phaseMessage) = try XCTUnwrap(model.addHostDraft.enrollment?.phase) else {
            return XCTFail("the sheet must settle on the explicit failure phase")
        }
        XCTAssertTrue(phaseMessage.contains("already used"),
                      "the visible state names the reused code: \(phaseMessage)")
        XCTAssertEqual(model.profiles.count, 1)
        XCTAssertEqual(model.activeProfile?.displayName, "Mac",
                       "a refused redeem leaves the paired hosts untouched")
    }

    func testRedeemTransportFailureIsActionableAndCommitsNothing() async throws {
        defer { cleanup() }
        let model = makeModel()
        var current = currentScript()
        current[enrollRedeemURL] = nil
        script(current)
        await model.handleScannedEnrollmentCode(payloadText())

        let outcome = await model.completeEnrollmentPairing()

        guard case .failure(let failure) = outcome else {
            return XCTFail("an unreachable host must fail, got \(outcome)")
        }
        XCTAssertTrue(failure.message.contains("Could not reach the host"), failure.message)
        guard case .failed = try XCTUnwrap(model.addHostDraft.enrollment?.phase) else {
            return XCTFail("the unreachable host must leave an explicit failure state")
        }
        XCTAssertEqual(model.profiles.count, 1)
        XCTAssertEqual(requests(to: enrollStatusURL).count, 0,
                       "a failed redeem never starts the approval wait")
    }

    func testWaitingForApprovalIsExplicitThenApprovalCommitsReadOnlyProfileAndStartsTheBoard() async throws {
        defer { cleanup() }
        let model = makeModel()
        // pending → pending → approved (the last response repeats).
        EnrollmentFlowURLProtocol.setSequence(sequence([
            enrollRedeemURL: [(200, redeemOK(), false)],
            enrollStatusURL: [(200, statusPending(now() + 600), false),
                              (200, statusPending(now() + 600), false),
                              (200, statusApproved(), false)],
            eventsURL: [(200, Data(), true)],
            macEventsURL: [(200, Data(), true)],
            bURL.appendingPathComponent("/host-key"): [(200, hostKeyBody(Self.hostBKey), false)],
            macURL.appendingPathComponent("/host-key"): [(200, hostKeyBody(Self.macKey), false)],
        ]))
        await model.handleScannedEnrollmentCode(payloadText())

        let outcome = await model.completeEnrollmentPairing()

        XCTAssertEqual(outcome, .success)
        XCTAssertEqual(model.addHostDraft, AppModel.AddHostDraft(),
                       "a successful pairing clears the whole scene-scoped draft")
        XCTAssertEqual(model.profiles.count, 2, "exactly one new profile")
        XCTAssertEqual(model.profiles.first { $0.displayName == "Mac" }?.keyId, "dev_mac",
                       "the existing host profile survives the commit untouched")
        let added = try XCTUnwrap(model.profiles.first { $0.urlString == bURL.absoluteString })
        XCTAssertEqual(added.keyId, Self.enrolledKeyID,
                       "the committed profile carries the host-approved key id")
        XCTAssertEqual(added.grants, ["read_tail"],
                       "the approved grants ride verbatim — never widened")
        XCTAssertEqual(added.hostKeyB64, Self.hostBKey)
        XCTAssertEqual(added.fingerprint, HostKeyTrust.fingerprint(forBase64: Self.hostBKey))
        XCTAssertEqual(model.activeProfileID, added.id,
                       "the freshly paired host becomes the active binding")
        await waitUntil(!requests(to: eventsURL).isEmpty)
        XCTAssertFalse(requests(to: eventsURL).isEmpty,
                       "the board stream opens for the paired host without any registry edit")
        // The redeem presented THIS DEVICE's key, never a registration token.
        let redeemBody = bodyString(requests(to: enrollRedeemURL).first)
        XCTAssertEqual(redeemBody?.contains("\"token\""), false)
        XCTAssertEqual(requests(to: enrollRedeemURL).count, 1,
                       "exactly one redeem — a lost response is recovered by polling")
    }

    func testRevokedApprovalIsExplicitAndCommitsNothing() async throws {
        defer { cleanup() }
        let model = makeModel()
        EnrollmentFlowURLProtocol.setSequence(sequence([
            enrollRedeemURL: [(200, redeemOK(), false)],
            enrollStatusURL: [(200, statusApproved(revoked: "true"), false)],
        ]))
        await model.handleScannedEnrollmentCode(payloadText())

        let outcome = await model.completeEnrollmentPairing()

        guard case .failure(let failure) = outcome else {
            return XCTFail("a revoked record is never authority, got \(outcome)")
        }
        XCTAssertTrue(failure.message.contains("revoked"), failure.message)
        XCTAssertEqual(model.profiles.count, 1)
        XCTAssertEqual(model.activeProfile?.displayName, "Mac")
    }

    func testApprovalWithoutReadTailGrantIsExplicitAndCommitsNothing() async throws {
        defer { cleanup() }
        let model = makeModel()
        EnrollmentFlowURLProtocol.setSequence(sequence([
            enrollRedeemURL: [(200, redeemOK(), false)],
            enrollStatusURL: [(200, statusApproved(grants: "[\"read_diff\"]"), false)],
        ]))
        await model.handleScannedEnrollmentCode(payloadText())

        let outcome = await model.completeEnrollmentPairing()

        guard case .failure(let failure) = outcome else {
            return XCTFail("an approval without read_tail is not this pairing, got \(outcome)")
        }
        XCTAssertTrue(failure.message.contains("read_tail"), failure.message)
        XCTAssertEqual(model.profiles.count, 1)
    }

    func testWaitingExpiresWithoutApprovalAndCommitsNothing() async throws {
        defer { cleanup() }
        let model = makeModel()
        // The code's own deadline is 2 s out; the host never approves.
        let deadline = now() + 2
        EnrollmentFlowURLProtocol.setSequence(sequence([
            enrollRedeemURL: [(200, redeemOK(expiresTs: deadline), false)],
            enrollStatusURL: [(200, statusPending(deadline), false)],
        ]))
        await model.handleScannedEnrollmentCode(payloadText(expiresTs: deadline))

        let outcome = await model.completeEnrollmentPairing()

        guard case .failure(let failure) = outcome else {
            return XCTFail("an unapproved pairing must fail explicitly, got \(outcome)")
        }
        XCTAssertTrue(failure.message.contains("did not approve"), failure.message)
        guard case .failed(let phaseMessage) = try XCTUnwrap(model.addHostDraft.enrollment?.phase) else {
            return XCTFail("missing approval must be an explicit state, never a silent empty board")
        }
        XCTAssertTrue(phaseMessage.contains("did not approve"),
                      "missing approval is an explicit state, never a silent empty board")
        XCTAssertEqual(model.profiles.count, 1)
        XCTAssertEqual(requests(to: eventsURL).count, 0,
                       "no board stream may open for an unapproved pairing")
    }

    func testStoppingTheWaitMarksInterruptedAndCommitsNothing() async throws {
        defer { cleanup() }
        let model = makeModel()
        EnrollmentFlowURLProtocol.setSequence(sequence([
            enrollRedeemURL: [(200, redeemOK(), false)],
            enrollStatusURL: [(200, statusPending(now() + 600), false)],
        ]))
        await model.handleScannedEnrollmentCode(payloadText())
        let pairing = Task { await model.completeEnrollmentPairing() }
        await waitUntil(model.addHostDraft.enrollment?.phase == .waiting)

        model.stopEnrollmentWait()
        let outcome = await pairing.value

        guard case .failure = outcome else {
            return XCTFail("a stopped wait must not report success, got \(outcome)")
        }
        XCTAssertEqual(model.addHostDraft.enrollment?.phase, .interrupted,
                       "the stopped wait is an explicit state, not an endless spinner")
        XCTAssertFalse(model.addHostDraft.isWorking)
        XCTAssertEqual(model.profiles.count, 1, "an interrupted pairing commits nothing")
        XCTAssertEqual(model.activeProfile?.displayName, "Mac")
    }

    func testClearingTheDraftDropsTheTransientCodeAndNeverRedeems() async throws {
        defer { cleanup() }
        let model = makeModel()
        await model.handleScannedEnrollmentCode(payloadText())
        XCTAssertNotNil(model.addHostDraft.enrollment)

        model.clearAddHostDraft()

        XCTAssertEqual(model.addHostDraft, AppModel.AddHostDraft(),
                       "Cancel clears every value including the enrollment phase")
        let outcome = await model.completeEnrollmentPairing()
        XCTAssertEqual(outcome, .failure(.inProgress),
                       "a cleared draft cannot be confirmed")
        XCTAssertEqual(requests(to: enrollRedeemURL).count, 0,
                       "the dropped code is never redeemed")
    }

    // MARK: - C2b recovery (daemon restarted mid-pairing)

    func testStatus404AfterRedeemFallsBackToTheSingleSignedGrantsReadProbe() async throws {
        defer { cleanup() }
        let model = makeModel()
        EnrollmentFlowURLProtocol.setSequence(sequence([
            enrollRedeemURL: [(200, redeemOK(), false)],
            enrollStatusURL: [(404, serverRefusal(status: 404, code: "enroll_unknown_code"), false)],
            grantsReadURL: [(200, Data("{\"ok\":true,\"key_id\":\"\(Self.enrolledKeyID)\",\"grants\":[\"read_tail\"],\"expiry_ts\":\(Self.keyExpiry)}".utf8), false)],
            eventsURL: [(200, Data(), true)],
            macEventsURL: [(200, Data(), true)],
            bURL.appendingPathComponent("/host-key"): [(200, hostKeyBody(Self.hostBKey), false)],
            macURL.appendingPathComponent("/host-key"): [(200, hostKeyBody(Self.macKey), false)],
        ]))
        await model.handleScannedEnrollmentCode(payloadText())

        let outcome = await model.completeEnrollmentPairing()

        XCTAssertEqual(outcome, .success)
        XCTAssertEqual(requests(to: grantsReadURL).count, 1,
                       "the C2b probe is EXACTLY ONCE")
        let added = try XCTUnwrap(model.profiles.first { $0.urlString == bURL.absoluteString })
        XCTAssertEqual(added.grants, ["read_tail"])
        XCTAssertEqual(model.activeProfileID, added.id)
    }

    func testRefusedRecoveryProbeIsExplicitAndNeverRepeats() async throws {
        defer { cleanup() }
        let model = makeModel()
        EnrollmentFlowURLProtocol.setSequence(sequence([
            enrollRedeemURL: [(200, redeemOK(), false)],
            enrollStatusURL: [(404, serverRefusal(status: 404, code: "enroll_unknown_code"), false)],
            grantsReadURL: [(403, serverRefusal(status: 403, code: "revoked"), false)],
        ]))
        await model.handleScannedEnrollmentCode(payloadText())

        let outcome = await model.completeEnrollmentPairing()

        guard case .failure = outcome else {
            return XCTFail("a refused probe is not success, got \(outcome)")
        }
        XCTAssertEqual(requests(to: grantsReadURL).count, 1,
                       "a refused probe must never be retried inside the flow")
        XCTAssertEqual(model.profiles.count, 1)
        XCTAssertEqual(model.activeProfile?.displayName, "Mac")
    }

    // MARK: - Camera permission states

    func testCameraAuthorizationStatesAreActionable() {
        XCTAssertEqual(EnrollmentCameraState.forAuthorization(.authorized), .ready)
        XCTAssertEqual(EnrollmentCameraState.forAuthorization(.denied), .denied,
                       "a denied camera permission is an explicit, recoverable state")
        XCTAssertEqual(EnrollmentCameraState.forAuthorization(.restricted), .denied)
        XCTAssertEqual(EnrollmentCameraState.forAuthorization(.notDetermined), .checking)
    }

    // MARK: - Equal raw agent ids across hosts (credential/route isolation)

    func testEqualRawAgentIdsOnDifferentHostsCannotShareCredentialsOrReadRoutes() async throws {
        defer { cleanup() }
        let model = makeModel()
        let macID = try XCTUnwrap(model.activeProfile?.id)
        model.startLive()
        await waitUntil(model.keyContinuityState == .verified)
        EnrollmentFlowURLProtocol.setSequence(sequence([
            enrollRedeemURL: [(200, redeemOK(), false)],
            enrollStatusURL: [(200, statusApproved(), false)],
            eventsURL: [(200, Data(), true)],
            macEventsURL: [(200, Data(), true)],
            driveURL: [(200, Data("{\"request_id\":\"r1\",\"ok\":true,\"rev\":5,\"result\":{\"lines\":[\"from-host-b\"],\"blocks\":[]}}".utf8), false)],
        ]))
        await model.handleScannedEnrollmentCode(payloadText())
        let outcome = await model.completeEnrollmentPairing()
        XCTAssertEqual(outcome, .success)
        let enrolled = try XCTUnwrap(model.profiles.first { $0.urlString == bURL.absoluteString })
        XCTAssertEqual(model.activeProfileID, enrolled.id,
                       "the QR pairing binds the enrolled host as the active binding")
        // Relaunch with the FIRST host still the persisted active binding —
        // the real app path on the next launch: the enrolled host then
        // streams through the coordinator with ITS OWN credentials.
        model.stopLive()
        // SAFETY: both values are set by makeModel in this same test.
        let defaults = try XCTUnwrap(self.defaults)
        // SAFETY: both values are set by makeModel in this same test.
        let store = try XCTUnwrap(self.store)
        defaults.set(macID.uuidString, forKey: "fleetnotifier.activeHostProfileID")
        let relaunched = makeModelInstance(defaults: defaults, store: store)
        relaunched.startLive()
        await waitUntil(relaunched.keyContinuityState == .verified)
        let coordinator = try XCTUnwrap(relaunched.coordinator)
        await waitUntil(coordinator.allowsLiveWork(profileID: enrolled.id))

        // The SAME raw agent id exists on BOTH hosts with different identity
        // stamps; each store carries its own row.
        relaunched.fleet.apply(.snapshot(Snapshot(schemaVersion: 5, rev: 5, generatedAt: 0,
                                                  agents: ["herdr:dup": agent("herdr:dup", state: .working,
                                                                              host: Self.macKey, ts: 5)])))
        let enrolledStore = try XCTUnwrap(coordinator.store(profileID: enrolled.id))
        enrolledStore.apply(.snapshot(Snapshot(schemaVersion: 5, rev: 5, generatedAt: 0,
                                               agents: ["herdr:dup": agent("herdr:dup", state: .blocked,
                                                                           host: Self.hostBKey, ts: 5)])))
        let bAgent = try XCTUnwrap(enrolledStore.agent("herdr:dup"))
        XCTAssertEqual(relaunched.fleetAgent(hostProfileID: enrolled.id, agentID: "herdr:dup")?.state,
                       .blocked, "the enrolled host resolves its OWN row")

        // Drive with a client bound to the OTHER (active) host on purpose:
        // the composite route must still sign with the ENROLLED key id
        // against the enrolled URL — equal raw ids never share credentials
        // or read routes.
        // SAFETY: the test's own fixture session was created above.
        let clientForMac = DriveClient(host: macURL, session: session ?? .shared)
        relaunched.driveReadTail(agent: bAgent, hostProfileID: enrolled.id, driveClient: clientForMac)
        await waitUntil(!requests(to: driveURL).isEmpty)

        XCTAssertTrue(requests(to: macDriveURL).isEmpty,
                      "a read for the enrolled host must NEVER reach the other host")
        let driveBody = try XCTUnwrap(requests(to: driveURL).first?.httpBody)
        let json = try XCTUnwrap(try JSONSerialization.jsonObject(with: driveBody) as? [String: Any])
        XCTAssertEqual(json["key_id"] as? String, Self.enrolledKeyID,
                       "the read is signed with the ENROLLED profile's key id")
        let envelope = try XCTUnwrap(json["envelope"] as? [String: Any])
        XCTAssertEqual(envelope["target"] as? String, "herdr:dup",
                       "the raw agent id is sent untouched")
        await waitUntil(coordinator.tailPane(profileID: enrolled.id, agentID: "herdr:dup") != nil)
        XCTAssertEqual(coordinator.tailPane(profileID: enrolled.id,
                                            agentID: "herdr:dup")?.lines,
                       ["from-host-b"])
        XCTAssertNil(relaunched.fleet.tailPane(for: "herdr:dup"),
                     "the enrolled host's output never lands in the other host's store")
    }

    // MARK: - Transient pairing material (privacy)

    func testScannedDraftCarriesNoPairingMaterial() async throws {
        defer { cleanup() }
        let model = makeModel()

        await model.handleScannedEnrollmentCode(payloadText())

        let dump = String(describing: model.addHostDraft)
        XCTAssertFalse(dump.contains(Self.newCode),
                       "the sheet's state must never hold the redemption code: \(dump)")
        XCTAssertFalse(dump.contains("host_key"),
                       "the draft carries no raw payload fields: \(dump)")
    }

    func testEnrollmentSourcesCarryNoLoggingClipboardOrPersistenceCalls() throws {
        let testsDirectory = URL(fileURLWithPath: #filePath).deletingLastPathComponent()
        let sourcesDirectory = testsDirectory.deletingLastPathComponent()
        let sources = [
            sourcesDirectory.appendingPathComponent("FleetNotifier/App/EnrollmentPairing.swift"),
            sourcesDirectory.appendingPathComponent("FleetNotifier/UI/EnrollmentScannerView.swift"),
        ]
        let forbidden = ["print(", "Logger(", "os_log(", "NSLog", "UIPasteboard", "UserDefaults", "FileManager"]
        for source in sources {
            let text = try String(contentsOf: source, encoding: .utf8)
            for token in forbidden {
                XCTAssertFalse(text.contains(token),
                               "\(source.lastPathComponent) must not contain \(token)")
            }
        }
        // The transient code lives in ONE model-owned slot and is never
        // reachable from a view.
        let appModel = try String(contentsOf: sourcesDirectory
            .appendingPathComponent("FleetNotifier/App/AppModel.swift"), encoding: .utf8)
        XCTAssertTrue(appModel.contains("private var enrollmentRedemptionCode: String?"),
                      "the code's only home is the model's private transient slot")
        let views = try String(contentsOf: sourcesDirectory
            .appendingPathComponent("FleetNotifier/UI/FleetViews.swift"), encoding: .utf8)
        XCTAssertFalse(views.contains("enrollmentRedemptionCode"),
                       "no view may reach the redemption code")
    }

    // MARK: - Fixture helpers

    private func agent(_ id: String, state: AgentState, host: String, ts: UInt64) -> Agent {
        Agent(agentId: id, state: state, ts: ts,
              capabilities: ["read_tail"], host: host,
              workspace: Workspace(repo: "corral", branch: "main"))
    }

    private func currentScript() -> [URL: (Int, Data, Bool)] {
        EnrollmentFlowURLProtocol.script
    }
}

/// Per-URL response script + request capture for the #486 flow tests.
/// Sequences advance one response per request and then REPEAT the last
/// response, so a "pending … approved" script settles deterministically
/// while a single-response script stays stable. Unscripted URLs behave like
/// an unreachable host. Test-bundle only — never part of the app target.
private final class EnrollmentFlowURLProtocol: URLProtocol {
    private static let lock = NSLock()
    private static var scriptStorage: [URL: [(status: Int, body: Data, holdOpen: Bool)]] = [:]
    private static var requestsStorage: [URLRequest] = []

    static func setScript(_ script: [URL: (Int, Data, Bool)]) {
        setSequence(script.mapValues { [$0] })
    }

    static func setSequence(_ sequence: [URL: [(Int, Data, Bool)]]) {
        lock.lock()
        scriptStorage = sequence.mapValues { entries in
            entries.map { (status: $0.0, body: $0.1, holdOpen: $0.2) }
        }
        requestsStorage = []
        lock.unlock()
    }

    static func clear() {
        setSequence([:])
    }

    static var script: [URL: (Int, Data, Bool)] {
        lock.lock()
        defer { lock.unlock() }
        return scriptStorage.mapValues { entries in
            let last = entries.last ?? (status: 200, body: Data(), holdOpen: false)
            return (last.status, last.body, last.holdOpen)
        }
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
        var scripted: (status: Int, body: Data, holdOpen: Bool)?
        if var entries = Self.scriptStorage[url], !entries.isEmpty {
            let entry = entries.removeFirst()
            if entries.isEmpty {
                entries = [entry]
            }
            Self.scriptStorage[url] = entries
            scripted = entry
        }
        Self.lock.unlock()
        guard let entry = scripted else {
            client?.urlProtocol(self, didFailWithError: URLError(.cannotConnectToHost))
            return
        }
        // SAFETY: fixed response built from the scripted status/body above.
        guard let response = HTTPURLResponse(url: url, statusCode: entry.status,
                                             httpVersion: "HTTP/1.1",
                                             headerFields: ["Content-Type": "application/json"]) else {
            client?.urlProtocol(self, didFailWithError: URLError(.badServerResponse))
            return
        }
        client?.urlProtocol(self, didReceive: response, cacheStoragePolicy: .notAllowed)
        if !entry.body.isEmpty {
            client?.urlProtocol(self, didLoad: entry.body)
        }
        if entry.holdOpen {
            return
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
