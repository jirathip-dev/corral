import CryptoKit
import Foundation
import XCTest
@testable import FleetNotifier

// MARK: - #451 transient host-key preflight retries (dedicated lane file)
//
// Transport-path tests for the bounded, cancellable per-host `/host-key`
// preflight ladder. Deliberately a SEPARATE file from the shared
// `FleetNotifierTests.swift` (phase fence: #457 owns the shared file, the
// generated project and the pinned digests). Every assertion observes real
// URLRequests, real coordinator postures/streams and real store state —
// never source strings.

/// Scripted transport for the preflight ladder: a per-URL queue of
/// outcomes, consumed in order. The LAST outcome repeats for every later
/// request, so a wrong implementation that polls again is observable as a
/// request-count increase, and a hold-open `/events` stays open like a
/// real SSE stream.
private final class PreflightRetryURLProtocol: URLProtocol {
    enum Outcome {
        case failure(URLError.Code)
        case response(status: Int, body: Data, holdOpen: Bool)

        /// A well-formed X25519 `/host-key` 200 for the given key.
        static func ok(_ publicKeyB64: String) -> Outcome {
            // SAFETY: fixed fixture JSON interpolated from a fixture key.
            .response(status: 200,
                      body: Data(#"{"algorithm":"X25519","public_key":"\#(publicKeyB64)"}"#.utf8),
                      holdOpen: false)
        }

        /// A scripted JSON 200 (snapshot bodies encode through the real Codable).
        static func json(_ body: Data) -> Outcome {
            .response(status: 200, body: body, holdOpen: false)
        }

        /// A hold-open SSE stream (`/events`).
        static let holdOpen = Outcome.response(status: 200, body: Data(), holdOpen: true)

        static func fail(_ code: URLError.Code) -> Outcome {
            .failure(code)
        }
    }

    private static let lock = NSLock()
    private static var queuesStorage: [URL: [Outcome]] = [:]
    private static var requestsStorage: [URLRequest] = []

    static func setScript(_ script: [URL: [Outcome]]) {
        lock.lock()
        queuesStorage = script
        requestsStorage = []
        lock.unlock()
    }

    static func clearScript() {
        setScript([:])
    }

    static func requestCount(to url: URL) -> Int {
        lock.lock()
        defer { lock.unlock() }
        return requestsStorage.filter { $0.url?.absoluteString == url.absoluteString }.count
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
        Self.requestsStorage.append(request)
        var outcome: Outcome?
        if let queue = Self.queuesStorage[url], let first = queue.first {
            outcome = first
            if queue.count > 1 {
                Self.queuesStorage[url] = Array(queue.dropFirst())
            }
        }
        Self.lock.unlock()
        guard let outcome else {
            client?.urlProtocol(self, didFailWithError: URLError(.cannotConnectToHost))
            return
        }
        switch outcome {
        case .failure(let code):
            client?.urlProtocol(self, didFailWithError: URLError(code))
        case .response(let status, let body, let holdOpen):
            // SAFETY: fixed HTTP response construction from a scripted URL.
            let response = HTTPURLResponse(
                url: url, statusCode: status, httpVersion: "HTTP/1.1",
                headerFields: holdOpen
                    ? ["Content-Type": "text/event-stream"]
                    : ["Content-Type": "application/json"])!
            client?.urlProtocol(self, didReceive: response, cacheStoragePolicy: .notAllowed)
            if !body.isEmpty {
                client?.urlProtocol(self, didLoad: body)
            }
            if !holdOpen {
                client?.urlProtocolDidFinishLoading(self)
            }
            // holdOpen: the stream is torn down by disconnect(), never by EOF.
        }
    }

    override func stopLoading() {}
}

/// #451 AC1-AC5, coordinator half: a pinned SECONDARY host whose preflight
/// fails transiently recovers on its own ladder (no foreground/manual
/// retry/restart), with bounded pacing, one owner, terminal mismatch,
/// cancellation and three-host isolation.
@MainActor
final class PreflightRetryCoordinatorTests: XCTestCase {
    private var suiteName = ""
    private var coordinator: HostStreamCoordinator?
    private var session: URLSession?

    private func cleanup() {
        coordinator?.stopAll()
        coordinator = nil
        session?.invalidateAndCancel()
        session = nil
        PreflightRetryURLProtocol.clearScript()
        if !suiteName.isEmpty {
            // SAFETY: suiteName was freshly minted per test.
            UserDefaults(suiteName: suiteName)!.removePersistentDomain(forName: suiteName)
            suiteName = ""
        }
    }

    private func waitUntil(_ condition: @autoclosure () -> Bool,
                           timeout: TimeInterval = 3) async {
        let deadline = Date().addingTimeInterval(timeout)
        while !condition(), Date() < deadline {
            try? await Task.sleep(nanoseconds: 25_000_000)
        }
    }

    /// Bounded settle beat for "nothing more happened" assertions.
    private func settle(_ seconds: TimeInterval = 0.3) async {
        try? await Task.sleep(nanoseconds: UInt64(seconds * 1_000_000_000))
    }

    private func requests(_ url: URL) -> Int {
        PreflightRetryURLProtocol.requestCount(to: url)
    }

    private static func hostKey(_ value: UInt8) -> String {
        Data(repeating: value, count: 32).base64EncodedString()
    }

    /// Three pinned profiles with EQUAL raw agent ids under distinct URLs.
    private func makeThreeHostStore() -> (HostProfileStore, [HostProfile], [URL], [String]) {
        // SAFETY: a fresh UUID suite name is always a valid suite.
        let store = HostProfileStore(directory: nil,
                                     defaults: UserDefaults(suiteName: suiteName)!)
        // SAFETY: fixed valid fixture URLs (distinct hostnames).
        let urls = [URL(string: "https://h451-a.example")!,
                    URL(string: "https://h451-b.example")!,
                    URL(string: "https://h451-c.example")!]
        let keys = [Self.hostKey(1), Self.hostKey(2), Self.hostKey(3)]
        var profiles: [HostProfile] = []
        for index in 0..<3 {
            // SAFETY: fixture addProfile calls only throw on invalid fixture input.
            let profile = try! store.addProfile(displayName: "Host \(index)",
                                                urlString: urls[index].absoluteString,
                                                hostKeyB64: keys[index],
                                                fingerprint: "FINGER",
                                                keyId: "dev_451_h\(index)",
                                                grants: ["read_tail"],
                                                expiryTs: 1_800_000_000,
                                                registeredAt: 1)
            profiles.append(profile)
        }
        return (store, profiles, urls, keys)
    }

    private func makeCoordinator(store: HostProfileStore,
                                 session: URLSession,
                                 policy: HostPreflightRetryPolicy) -> HostStreamCoordinator {
        // SAFETY: a fresh UUID suite name is always a valid suite.
        let coordinator = HostStreamCoordinator(defaults: UserDefaults(suiteName: suiteName)!,
                                                session: session,
                                                profileStore: store,
                                                preflightPolicy: policy,
                                                signerProvider: { nil })
        self.coordinator = coordinator
        return coordinator
    }

    private func scriptedSession(_ script: [URL: [PreflightRetryURLProtocol.Outcome]]) -> URLSession {
        PreflightRetryURLProtocol.setScript(script)
        let config = URLSessionConfiguration.ephemeral
        config.protocolClasses = [PreflightRetryURLProtocol.self]
        let session = URLSession(configuration: config)
        self.session = session
        return session
    }

    private func snapshotData(rev: UInt64, agents: [String: Agent] = [:]) throws -> Data {
        try JSONEncoder().encode(Snapshot(schemaVersion: 5, rev: rev,
                                          generatedAt: 0, agents: agents))
    }

    /// AC1/AC6: host B's `/host-key` fails its first two attempts, then
    /// succeeds. With NO second startLive, NO Settings Retry and NO app
    /// restart, B must verify and open exactly one `/events` stream; A and
    /// C are unaffected.
    func testOfflineSecondaryHostAutoRecoversWithoutForegroundOrManualRetry() async throws {
        suiteName = "corral.h451.recover.\(UUID().uuidString)"
        defer { cleanup() }
        let (store, profiles, urls, keys) = makeThreeHostStore()
        let policy = HostPreflightRetryPolicy(baseInterval: 0.05, maxInterval: 0.05)
        var script: [URL: [PreflightRetryURLProtocol.Outcome]] = [:]
        for (index, url) in urls.enumerated() {
            script[url.appendingPathComponent("/events")] = [.holdOpen]
            script[url.appendingPathComponent("/host-key")] = [.ok(keys[index])]
        }
        // Host B: offline for its first two preflight attempts.
        script[urls[1].appendingPathComponent("/host-key")] = [
            .fail(.cannotConnectToHost), .fail(.timedOut), .ok(keys[1]),
        ]
        let session = scriptedSession(script)
        let coordinator = makeCoordinator(store: store, session: session, policy: policy)
        coordinator.update(profiles: profiles, startStreams: true)
        await waitUntil(coordinator.posture(profileID: profiles[1].id) == .verified)
        XCTAssertEqual(coordinator.posture(profileID: profiles[1].id), .verified,
                       "host B must auto-recover from transient preflight failures")
        await waitUntil(requests(urls[1].appendingPathComponent("/events")) >= 1)
        await settle(0.15)
        XCTAssertEqual(requests(urls[1].appendingPathComponent("/host-key")), 3,
                       "B retried exactly to its first success (one-shot would stay at 1)")
        XCTAssertEqual(requests(urls[1].appendingPathComponent("/events")), 1,
                       "B opened exactly one SSE stream")
        XCTAssertTrue(try XCTUnwrap(coordinator.store(profileID: profiles[0].id)).isStreaming)
        XCTAssertTrue(try XCTUnwrap(coordinator.store(profileID: profiles[1].id)).isStreaming)
        XCTAssertTrue(try XCTUnwrap(coordinator.store(profileID: profiles[2].id)).isStreaming)
        XCTAssertEqual(coordinator.posture(profileID: profiles[0].id), .verified)
        XCTAssertEqual(coordinator.posture(profileID: profiles[2].id), .verified)
    }

    /// AC2: a host that answers a DIFFERENT pinned key is terminal — no
    /// stream, no retry, no later re-poll and no auto-repair.
    func testKeyMismatchNeverRetriesOrOpensAStream() async throws {
        suiteName = "corral.h451.mismatch.\(UUID().uuidString)"
        defer { cleanup() }
        let (store, profiles, urls, keys) = makeThreeHostStore()
        let policy = HostPreflightRetryPolicy(baseInterval: 0.05, maxInterval: 0.05)
        let replacement = Self.hostKey(9)
        let snapshot = try snapshotData(rev: 5)
        var script: [URL: [PreflightRetryURLProtocol.Outcome]] = [:]
        for (index, url) in urls.enumerated() {
            script[url.appendingPathComponent("/events")] = [.holdOpen]
            script[url.appendingPathComponent("/snapshot")] = [.json(snapshot)]
            script[url.appendingPathComponent("/host-key")] = [.ok(keys[index])]
        }
        script[urls[1].appendingPathComponent("/host-key")] = [.ok(replacement)]
        let session = scriptedSession(script)
        let coordinator = makeCoordinator(store: store, session: session, policy: policy)
        coordinator.update(profiles: profiles, startStreams: true)
        await waitUntil(coordinator.posture(profileID: profiles[1].id) == .mismatch)
        XCTAssertEqual(coordinator.posture(profileID: profiles[1].id), .mismatch,
                       "a different key must fail closed")
        XCTAssertEqual(requests(urls[1].appendingPathComponent("/events")), 0,
                       "a mismatch never opens a stream")
        // A later start / pull must not re-poll a known mismatch.
        coordinator.startSessionIfNeeded(profiles[1])
        _ = await coordinator.refreshAll(profiles: profiles)
        await settle(0.35)
        XCTAssertEqual(requests(urls[1].appendingPathComponent("/host-key")), 1,
                       "a mismatch must never be re-polled or auto-repaired")
        XCTAssertEqual(requests(urls[1].appendingPathComponent("/events")), 0)
        let bStore = try XCTUnwrap(coordinator.store(profileID: profiles[1].id))
        guard case .error = bStore.connectionState else {
            return XCTFail("the mismatch must publish a truthful store reason")
        }
        XCTAssertFalse(bStore.isStreaming)
    }

    /// AC1/AC2: transient failures retry at a BOUNDED RATE with no finite
    /// availability window — the ladder keeps going past any fixed attempt
    /// count (the old 12-attempt cap left a host returning later stuck) and
    /// never bursts. The host publishes a truthful unavailable reason and
    /// stays fail-closed while it cannot verify.
    func testTransientFailuresRetryAtABoundedRateWithoutACutoff() async throws {
        suiteName = "corral.h451.rate.\(UUID().uuidString)"
        defer { cleanup() }
        let (store, profiles, urls, keys) = makeThreeHostStore()
        let policy = HostPreflightRetryPolicy(baseInterval: 0.05, maxInterval: 0.05)
        var script: [URL: [PreflightRetryURLProtocol.Outcome]] = [:]
        for (index, url) in urls.enumerated() {
            script[url.appendingPathComponent("/events")] = [.holdOpen]
            script[url.appendingPathComponent("/host-key")] = [.ok(keys[index])]
        }
        script[urls[1].appendingPathComponent("/host-key")] = [.fail(.cannotConnectToHost)]
        let session = scriptedSession(script)
        let coordinator = makeCoordinator(store: store, session: session, policy: policy)
        coordinator.update(profiles: profiles, startStreams: true)
        // The ladder must cross any fixed attempt window (the old cap was
        // 12) and still be retrying.
        await waitUntil(requests(urls[1].appendingPathComponent("/host-key")) >= 14, timeout: 4)
        XCTAssertGreaterThanOrEqual(requests(urls[1].appendingPathComponent("/host-key")), 14,
                                    "an unreachable host must keep retrying past any fixed attempt window")
        // Bounded RATE: over a measured window the attempt count grows by
        // at most window/baseInterval (+ scheduling slack) — never a burst
        // and never a stacked multiple.
        let before = requests(urls[1].appendingPathComponent("/host-key"))
        let started = Date()
        await settle(0.5)
        let elapsed = Date().timeIntervalSince(started)
        let growth = requests(urls[1].appendingPathComponent("/host-key")) - before
        XCTAssertGreaterThanOrEqual(growth, 2, "the ladder must still be retrying (not stopped)")
        XCTAssertLessThanOrEqual(growth, Int(elapsed / policy.baseInterval) + 3,
                                 "bounded rate — no burst, no stacked ladders")
        XCTAssertEqual(requests(urls[1].appendingPathComponent("/events")), 0,
                       "an unverified host never opens a stream")
        XCTAssertEqual(coordinator.posture(profileID: profiles[1].id), .verifying,
                       "a still-unreachable host stays fail-closed")
        let bStore = try XCTUnwrap(coordinator.store(profileID: profiles[1].id))
        guard case .error = bStore.connectionState else {
            return XCTFail("the retry failure must publish a truthful store reason")
        }
        XCTAssertFalse(coordinator.allowsLiveWork(profileID: profiles[1].id))
    }

    /// AC3: repeated start calls while the ladder is running must never
    /// stack a second verification owner — the attempt rate stays ONE
    /// ladder's bounded cadence.
    func testRepeatedStartCallsNeverStackASecondRetryOwner() async throws {
        suiteName = "corral.h451.owner.\(UUID().uuidString)"
        defer { cleanup() }
        let (store, profiles, urls, keys) = makeThreeHostStore()
        let policy = HostPreflightRetryPolicy(baseInterval: 0.05, maxInterval: 0.05)
        var script: [URL: [PreflightRetryURLProtocol.Outcome]] = [:]
        for (index, url) in urls.enumerated() {
            script[url.appendingPathComponent("/events")] = [.holdOpen]
            script[url.appendingPathComponent("/host-key")] = [.ok(keys[index])]
        }
        script[urls[1].appendingPathComponent("/host-key")] = [.fail(.cannotConnectToHost)]
        let session = scriptedSession(script)
        let coordinator = makeCoordinator(store: store, session: session, policy: policy)
        coordinator.update(profiles: profiles, startStreams: true)
        // While B's ladder is in flight, three more starts land on B.
        for _ in 0..<3 {
            coordinator.startSessionIfNeeded(profiles[1])
        }
        await waitUntil(requests(urls[1].appendingPathComponent("/host-key")) >= 3, timeout: 2)
        let before = requests(urls[1].appendingPathComponent("/host-key"))
        let started = Date()
        await settle(0.5)
        let elapsed = Date().timeIntervalSince(started)
        let growth = requests(urls[1].appendingPathComponent("/host-key")) - before
        XCTAssertGreaterThanOrEqual(growth, 2, "the single ladder must keep retrying")
        XCTAssertLessThanOrEqual(growth, Int(elapsed / policy.baseInterval) + 3,
                                 "one owner: extra starts must not stack ladders (would multiply the rate)")
        XCTAssertEqual(requests(urls[1].appendingPathComponent("/events")), 0)
    }

    /// AC3: background (stopAll) ends an in-flight ladder — no request
    /// after the boundary.
    func testBackgroundStopAllEndsAnInFlightLadder() async throws {
        suiteName = "corral.h451.background.\(UUID().uuidString)"
        defer { cleanup() }
        let (store, profiles, urls, keys) = makeThreeHostStore()
        let policy = HostPreflightRetryPolicy(baseInterval: 0.25, maxInterval: 0.25)
        var script: [URL: [PreflightRetryURLProtocol.Outcome]] = [:]
        for (index, url) in urls.enumerated() {
            script[url.appendingPathComponent("/events")] = [.holdOpen]
            script[url.appendingPathComponent("/host-key")] = [.ok(keys[index])]
        }
        script[urls[1].appendingPathComponent("/host-key")] = [.fail(.cannotConnectToHost)]
        let session = scriptedSession(script)
        let coordinator = makeCoordinator(store: store, session: session, policy: policy)
        coordinator.update(profiles: profiles, startStreams: true)
        await waitUntil(requests(urls[1].appendingPathComponent("/host-key")) >= 2, timeout: 3)
        let before = requests(urls[1].appendingPathComponent("/host-key"))
        XCTAssertGreaterThanOrEqual(before, 2, "the ladder must have retried before the boundary")
        coordinator.stopAll()
        await settle(0.8)
        XCTAssertEqual(requests(urls[1].appendingPathComponent("/host-key")), before,
                       "background stopAll ends the ladder — no retry after the boundary")
    }

    /// AC3: removing ONE host ends that host's ladder and never touches the
    /// other hosts' streams.
    func testHostRemovalEndsThatHostsLadderOnly() async throws {
        suiteName = "corral.h451.removal.\(UUID().uuidString)"
        defer { cleanup() }
        let (store, profiles, urls, keys) = makeThreeHostStore()
        let policy = HostPreflightRetryPolicy(baseInterval: 0.25, maxInterval: 0.25)
        var script: [URL: [PreflightRetryURLProtocol.Outcome]] = [:]
        for (index, url) in urls.enumerated() {
            script[url.appendingPathComponent("/events")] = [.holdOpen]
            script[url.appendingPathComponent("/host-key")] = [.ok(keys[index])]
        }
        script[urls[1].appendingPathComponent("/host-key")] = [.fail(.cannotConnectToHost)]
        let session = scriptedSession(script)
        let coordinator = makeCoordinator(store: store, session: session, policy: policy)
        coordinator.update(profiles: profiles, startStreams: true)
        await waitUntil(requests(urls[1].appendingPathComponent("/host-key")) >= 2, timeout: 3)
        let before = requests(urls[1].appendingPathComponent("/host-key"))
        XCTAssertGreaterThanOrEqual(before, 2, "the ladder must have retried before removal")
        coordinator.remove(profileID: profiles[1].id)
        XCTAssertNil(coordinator.store(profileID: profiles[1].id),
                     "the removed host's session must be gone")
        await settle(0.8)
        XCTAssertEqual(requests(urls[1].appendingPathComponent("/host-key")), before,
                       "removal ends that host's ladder")
        XCTAssertTrue(try XCTUnwrap(coordinator.store(profileID: profiles[0].id)).isStreaming)
        XCTAssertTrue(try XCTUnwrap(coordinator.store(profileID: profiles[2].id)).isStreaming)
    }

    /// AC4: with one host failing preflight, the healthy hosts keep
    /// streaming, their generations do not move and equal raw agent ids
    /// stay isolated per host in the composite projection.
    func testPartialFailureLeavesHealthyHostsAndEqualRawIDsIsolated() async throws {
        suiteName = "corral.h451.isolation.\(UUID().uuidString)"
        defer { cleanup() }
        let (store, profiles, urls, keys) = makeThreeHostStore()
        let policy = HostPreflightRetryPolicy(baseInterval: 0.05, maxInterval: 0.05)
        var script: [URL: [PreflightRetryURLProtocol.Outcome]] = [:]
        for (index, url) in urls.enumerated() {
            script[url.appendingPathComponent("/events")] = [.holdOpen]
            script[url.appendingPathComponent("/host-key")] = [.ok(keys[index])]
        }
        script[urls[1].appendingPathComponent("/host-key")] = [.fail(.cannotConnectToHost)]
        let session = scriptedSession(script)
        let coordinator = makeCoordinator(store: store, session: session, policy: policy)
        coordinator.update(profiles: profiles, startStreams: true)
        await waitUntil(coordinator.posture(profileID: profiles[0].id) == .verified, timeout: 2)
        await waitUntil(coordinator.posture(profileID: profiles[2].id) == .verified, timeout: 2)
        let storeA = try XCTUnwrap(coordinator.store(profileID: profiles[0].id))
        let storeC = try XCTUnwrap(coordinator.store(profileID: profiles[2].id))
        // The SAME raw id on both healthy hosts (host-stamped with each pin).
        storeA.apply(.snapshot(Snapshot(
            schemaVersion: 5, rev: 7, generatedAt: 0,
            agents: ["dup": Agent(agentId: "dup", state: .working, ts: 7,
                                  capabilities: ["read_tail"], host: keys[0],
                                  workspace: Workspace(repo: "corral", branch: "main"))])))
        storeC.apply(.snapshot(Snapshot(
            schemaVersion: 5, rev: 7, generatedAt: 0,
            agents: ["dup": Agent(agentId: "dup", state: .idle, ts: 7,
                                  capabilities: ["read_tail"], host: keys[2],
                                  workspace: Workspace(repo: "corral", branch: "main"))])))
        let generationA = storeA.connectionGeneration
        let generationC = storeC.connectionGeneration
        // B keeps retrying its preflight the whole time.
        await waitUntil(requests(urls[1].appendingPathComponent("/host-key")) >= 3, timeout: 3)
        XCTAssertGreaterThanOrEqual(requests(urls[1].appendingPathComponent("/host-key")), 3,
                                    "host B keeps retrying during the isolation window")
        XCTAssertEqual(storeA.connectionGeneration, generationA,
                       "host B's retries must never bump host A's connection generation")
        XCTAssertEqual(storeC.connectionGeneration, generationC)
        XCTAssertEqual(storeA.agent("dup")?.state, .working)
        XCTAssertEqual(storeC.agent("dup")?.state, .idle)
        XCTAssertNil(try XCTUnwrap(coordinator.store(profileID: profiles[1].id)).agent("dup"))
        let rows = coordinator.aggregateRows(profiles: profiles,
                                             activeStoreProvider: { nil },
                                             now: 10_000)
        let dupIdentities = Set(rows.filter { $0.identity.agentID == "dup" }
            .map(\.identity.description))
        XCTAssertEqual(dupIdentities, [
            CompositeAgentID(hostProfileID: profiles[0].id, agentID: "dup").description,
            CompositeAgentID(hostProfileID: profiles[2].id, agentID: "dup").description,
        ], "each host's equal raw id must stay under its OWN composite identity")
    }

    /// AC1 (late recovery): a pinned secondary host that stays unreachable
    /// well past any fixed attempt window (15 failures — the old cap was
    /// 12), then becomes reachable, verifies and opens exactly one stream
    /// automatically — no second start, no manual retry, no pull. Healthy
    /// siblings keep streaming untouched.
    func testLateReachabilityRecoversAutomaticallyPastAnyFixedWindow() async throws {
        suiteName = "corral.h451.late.\(UUID().uuidString)"
        defer { cleanup() }
        let (store, profiles, urls, keys) = makeThreeHostStore()
        let policy = HostPreflightRetryPolicy(baseInterval: 0.03, maxInterval: 0.03)
        var script: [URL: [PreflightRetryURLProtocol.Outcome]] = [:]
        for (index, url) in urls.enumerated() {
            script[url.appendingPathComponent("/events")] = [.holdOpen]
            script[url.appendingPathComponent("/host-key")] = [.ok(keys[index])]
        }
        // B: unreachable for 15 attempts, then reachable.
        var bQueue: [PreflightRetryURLProtocol.Outcome] =
            Array(repeating: .fail(.cannotConnectToHost), count: 15)
        bQueue.append(.ok(keys[1]))
        script[urls[1].appendingPathComponent("/host-key")] = bQueue
        let session = scriptedSession(script)
        let coordinator = makeCoordinator(store: store, session: session, policy: policy)
        coordinator.update(profiles: profiles, startStreams: true)
        await waitUntil(requests(urls[1].appendingPathComponent("/host-key")) >= 13, timeout: 4)
        XCTAssertGreaterThanOrEqual(requests(urls[1].appendingPathComponent("/host-key")), 13,
                                    "the ladder must keep retrying past the old fixed window")
        await waitUntil(coordinator.posture(profileID: profiles[1].id) == .verified, timeout: 4)
        XCTAssertEqual(coordinator.posture(profileID: profiles[1].id), .verified,
                       "a host that becomes reachable LATE must verify automatically")
        await waitUntil(requests(urls[1].appendingPathComponent("/events")) >= 1, timeout: 2)
        await settle(0.15)
        XCTAssertEqual(requests(urls[1].appendingPathComponent("/events")), 1,
                       "exactly one stream, opened once")
        XCTAssertEqual(requests(urls[1].appendingPathComponent("/host-key")), 16,
                       "one owner: the queue advanced one attempt at a time")
        XCTAssertTrue(try XCTUnwrap(coordinator.store(profileID: profiles[0].id)).isStreaming)
        XCTAssertTrue(try XCTUnwrap(coordinator.store(profileID: profiles[2].id)).isStreaming)
        XCTAssertEqual(requests(urls[0].appendingPathComponent("/host-key")), 1,
                       "a healthy sibling must not be re-verified by B's retry episode")
        XCTAssertEqual(requests(urls[2].appendingPathComponent("/host-key")), 1)
    }

    /// AC5: a pull-refresh of a pinned host that NEVER reached SSE proves
    /// reachability NOW — the single preflight owner restarts immediately
    /// (instead of waiting for the next capped tick) and the host goes live
    /// with exactly one stream once reachable; a healthy host's pull stays
    /// snapshot-only and a second pull is a no-op.
    func testPullRefreshGivesANeverVerifiedHostAnImmediatePreflight() async throws {
        suiteName = "corral.h451.pull.\(UUID().uuidString)"
        defer { cleanup() }
        let (store, profiles, urls, keys) = makeThreeHostStore()
        // Slow cadence: no natural tick inside the test window, so the
        // immediate post-pull attempt is observable.
        let policy = HostPreflightRetryPolicy(baseInterval: 2, maxInterval: 2)
        let snapshot = try snapshotData(rev: 5)
        var script: [URL: [PreflightRetryURLProtocol.Outcome]] = [:]
        for (index, url) in urls.enumerated() {
            script[url.appendingPathComponent("/events")] = [.holdOpen]
            script[url.appendingPathComponent("/host-key")] = [.ok(keys[index])]
            script[url.appendingPathComponent("/snapshot")] = [.json(snapshot)]
        }
        // B: the launch attempt fails; the pull's immediate attempt fails
        // too; the running ladder's next tick sees the host reachable.
        script[urls[1].appendingPathComponent("/host-key")] = [
            .fail(.cannotConnectToHost), .fail(.cannotConnectToHost), .ok(keys[1]),
        ]
        let session = scriptedSession(script)
        let coordinator = makeCoordinator(store: store, session: session, policy: policy)
        coordinator.update(profiles: profiles, startStreams: true)
        await waitUntil(requests(urls[1].appendingPathComponent("/host-key")) >= 1, timeout: 2)
        await settle(0.3)
        XCTAssertEqual(requests(urls[1].appendingPathComponent("/host-key")), 1,
                       "the slow ladder has not ticked again yet")
        XCTAssertEqual(requests(urls[1].appendingPathComponent("/events")), 0,
                       "B never reached SSE before the pull")
        let eventsA = requests(urls[0].appendingPathComponent("/events"))
        XCTAssertEqual(eventsA, 1, "host A streams before the pull")
        _ = await coordinator.refreshAll(profiles: profiles)
        // The pull restarted the single owner: the next attempt lands
        // immediately, not after the 2 s cadence.
        await waitUntil(requests(urls[1].appendingPathComponent("/host-key")) >= 2, timeout: 0.8)
        XCTAssertEqual(requests(urls[1].appendingPathComponent("/host-key")), 2,
                       "the pull must give the never-verified host an immediate attempt")
        // The pull's attempt failed; the still-running ladder recovers on
        // its own — automatically, with no further user action.
        await waitUntil(coordinator.posture(profileID: profiles[1].id) == .verified, timeout: 4)
        XCTAssertEqual(coordinator.posture(profileID: profiles[1].id), .verified,
                       "the never-SSE host must recover automatically once reachable")
        await waitUntil(requests(urls[1].appendingPathComponent("/events")) >= 1, timeout: 2)
        await settle(0.15)
        XCTAssertEqual(requests(urls[1].appendingPathComponent("/events")), 1,
                       "exactly one stream")
        // A second pull on an already-streaming host is a no-op.
        _ = await coordinator.refreshAll(profiles: profiles)
        await settle(0.2)
        XCTAssertEqual(requests(urls[1].appendingPathComponent("/events")), 1,
                       "an already-streaming host opens no second stream")
        XCTAssertEqual(requests(urls[0].appendingPathComponent("/events")), eventsA,
                       "a healthy host's pull stays snapshot-only")
    }
}

/// #451 AC5 parity, ACTIVE host: the AppModel-owned host runs the SAME
/// bounded preflight ladder — auto-recovery without foreground, one owner,
/// terminal mismatch, background cancellation and pull re-arm.
@MainActor
final class PreflightRetryActiveHostTests: XCTestCase {
    private static let pinnedKey = Data(repeating: 9, count: 32).base64EncodedString()
    private static let replacementKey = Data(repeating: 7, count: 32).base64EncodedString()
    // SAFETY: fixed valid fixture URL literal.
    private static let hostURL = URL(string: "https://h451-active.example")!

    private var suiteName = ""
    private var model: AppModel?
    private var session: URLSession?

    private func cleanup() {
        model?.stopLive()
        model = nil
        session?.invalidateAndCancel()
        session = nil
        PreflightRetryURLProtocol.clearScript()
        KeyContinuityGate.reset()
        if !suiteName.isEmpty {
            // SAFETY: suiteName was freshly minted per test.
            UserDefaults(suiteName: suiteName)!.removePersistentDomain(forName: suiteName)
            suiteName = ""
        }
    }

    private func waitUntil(_ condition: @autoclosure () -> Bool,
                           timeout: TimeInterval = 3) async {
        let deadline = Date().addingTimeInterval(timeout)
        while !condition(), Date() < deadline {
            try? await Task.sleep(nanoseconds: 25_000_000)
        }
    }

    private func settle(_ seconds: TimeInterval = 0.3) async {
        try? await Task.sleep(nanoseconds: UInt64(seconds * 1_000_000_000))
    }

    private func requests(_ url: URL) -> Int {
        PreflightRetryURLProtocol.requestCount(to: url)
    }

    private func scriptedSession(_ script: [URL: [PreflightRetryURLProtocol.Outcome]]) -> URLSession {
        PreflightRetryURLProtocol.setScript(script)
        let config = URLSessionConfiguration.ephemeral
        config.protocolClasses = [PreflightRetryURLProtocol.self]
        let session = URLSession(configuration: config)
        self.session = session
        return session
    }

    private func snapshotData(rev: UInt64, agents: [String: Agent] = [:]) throws -> Data {
        try JSONEncoder().encode(Snapshot(schemaVersion: 5, rev: rev,
                                          generatedAt: 0, agents: agents))
    }

    /// One pinned ACTIVE profile bound at init (mode `.live`); the test
    /// drives `startLive()` itself.
    private func makeModel(session: URLSession,
                           policy: HostPreflightRetryPolicy) -> AppModel {
        suiteName = "corral.h451.active.\(UUID().uuidString)"
        // SAFETY: a fresh UUID suite name is always a valid suite.
        let defaults = UserDefaults(suiteName: suiteName)!
        let store = HostProfileStore(directory: nil, defaults: defaults)
        // SAFETY: fixture addProfile calls only throw on invalid fixture input.
        let profile = try! store.addProfile(displayName: "Host A",
                                            urlString: Self.hostURL.absoluteString,
                                            hostKeyB64: Self.pinnedKey,
                                            fingerprint: "FINGER",
                                            keyId: "dev_451_a",
                                            grants: ["read_tail"],
                                            expiryTs: 1_800_000_000,
                                            registeredAt: 1)
        defaults.set(profile.id.uuidString, forKey: "fleetnotifier.activeHostProfileID")
        let signer = DeviceSigner(key: Curve25519.Signing.PrivateKey())
        let model = AppModel(session: session, defaults: defaults,
                             identityLoader: { (signer, .insecureFallback) },
                             loadMeta: { nil }, saveMeta: { _ in },
                             wipeIdentity: {}, profileStore: store,
                             preflightRetryPolicy: policy)
        self.model = model
        return model
    }

    /// AC5 parity: the active host's preflight recovers from transient
    /// failures with no further foreground call — the retry banner clears
    /// once the host verifies, and exactly one stream opens.
    func testActiveHostPreflightRecoversWithoutAnotherForeground() async {
        var script: [URL: [PreflightRetryURLProtocol.Outcome]] = [:]
        script[Self.hostURL.appendingPathComponent("/events")] = [.holdOpen]
        script[Self.hostURL.appendingPathComponent("/host-key")] = [
            .fail(.cannotConnectToHost), .fail(.timedOut), .ok(Self.pinnedKey),
        ]
        let session = scriptedSession(script)
        let policy = HostPreflightRetryPolicy(baseInterval: 0.05, maxInterval: 0.05)
        let model = makeModel(session: session, policy: policy)
        model.startLive()
        await waitUntil(model.keyContinuityState == .verified)
        XCTAssertEqual(model.keyContinuityState, .verified,
                       "the ACTIVE host must auto-recover like the coordinator hosts")
        await waitUntil(requests(Self.hostURL.appendingPathComponent("/events")) >= 1)
        await settle(0.15)
        XCTAssertEqual(requests(Self.hostURL.appendingPathComponent("/host-key")), 3)
        XCTAssertEqual(requests(Self.hostURL.appendingPathComponent("/events")), 1,
                       "exactly one stream")
        XCTAssertNil(model.banner, "the retry notice clears once the host verifies")
        XCTAssertTrue(model.fleet.isStreaming)
    }

    /// AC3 parity: repeated startLive() calls while the ladder is running
    /// never stack a second owner — the attempt rate stays ONE ladder's
    /// bounded cadence.
    func testActiveHostRepeatedStartLiveNeverStacksASecondLadder() async {
        var script: [URL: [PreflightRetryURLProtocol.Outcome]] = [:]
        script[Self.hostURL.appendingPathComponent("/host-key")] = [.fail(.cannotConnectToHost)]
        let session = scriptedSession(script)
        let policy = HostPreflightRetryPolicy(baseInterval: 0.05, maxInterval: 0.05)
        let model = makeModel(session: session, policy: policy)
        model.startLive()
        model.startLive()
        model.startLive()
        model.startLive()
        await waitUntil(requests(Self.hostURL.appendingPathComponent("/host-key")) >= 3, timeout: 2)
        let before = requests(Self.hostURL.appendingPathComponent("/host-key"))
        let started = Date()
        await settle(0.5)
        let elapsed = Date().timeIntervalSince(started)
        let growth = requests(Self.hostURL.appendingPathComponent("/host-key")) - before
        XCTAssertGreaterThanOrEqual(growth, 2, "the single ladder must keep retrying")
        XCTAssertLessThanOrEqual(growth, Int(elapsed / policy.baseInterval) + 3,
                                 "one owner: extra startLive calls must not stack ladders")
        XCTAssertEqual(model.keyContinuityState, .pending, "fail-closed while unverified")
        XCTAssertEqual(requests(Self.hostURL.appendingPathComponent("/events")), 0)
    }

    /// AC2 parity: a mismatching active host never opens a stream, never
    /// re-polls after the terminal mismatch, and keeps the fail-closed
    /// banner.
    func testActiveHostKeyMismatchIsTerminalAndNeverRePolled() async {
        var script: [URL: [PreflightRetryURLProtocol.Outcome]] = [:]
        script[Self.hostURL.appendingPathComponent("/events")] = [.holdOpen]
        script[Self.hostURL.appendingPathComponent("/host-key")] = [.ok(Self.replacementKey)]
        let session = scriptedSession(script)
        let policy = HostPreflightRetryPolicy(baseInterval: 0.05, maxInterval: 0.05)
        let model = makeModel(session: session, policy: policy)
        model.startLive()
        await waitUntil(model.keyContinuityState == .mismatch)
        XCTAssertEqual(model.keyContinuityState, .mismatch)
        XCTAssertEqual(requests(Self.hostURL.appendingPathComponent("/host-key")), 1,
                       "a mismatch is terminal — no retry")
        XCTAssertEqual(requests(Self.hostURL.appendingPathComponent("/events")), 0,
                       "a mismatch never opens a stream")
        XCTAssertEqual(model.banner?.kind, "host_key_mismatch")
        // Later startLive/refresh calls must not re-poll or auto-repair.
        model.startLive()
        await model.refreshFleet()
        await settle(0.35)
        XCTAssertEqual(requests(Self.hostURL.appendingPathComponent("/host-key")), 1)
        XCTAssertEqual(requests(Self.hostURL.appendingPathComponent("/events")), 0)
    }

    /// AC5 parity: an ordinary pull gives the never-verified ACTIVE host an
    /// immediate fresh preflight (not waiting for the ladder's next capped
    /// tick) and the host goes live with exactly one stream — no
    /// foreground/restart/manual retry.
    func testActiveHostPullRefreshGivesAnImmediatePreflight() async throws {
        let snapshot = try snapshotData(rev: 5)
        var script: [URL: [PreflightRetryURLProtocol.Outcome]] = [:]
        script[Self.hostURL.appendingPathComponent("/events")] = [.holdOpen]
        script[Self.hostURL.appendingPathComponent("/snapshot")] = [.json(snapshot)]
        script[Self.hostURL.appendingPathComponent("/host-key")] = [
            .fail(.cannotConnectToHost), .fail(.cannotConnectToHost), .ok(Self.pinnedKey),
        ]
        let session = scriptedSession(script)
        // Slow cadence: no natural tick inside the test window.
        let policy = HostPreflightRetryPolicy(baseInterval: 2, maxInterval: 2)
        let model = makeModel(session: session, policy: policy)
        model.startLive()
        await waitUntil(requests(Self.hostURL.appendingPathComponent("/host-key")) >= 1,
                        timeout: 2)
        await settle(0.3)
        XCTAssertEqual(requests(Self.hostURL.appendingPathComponent("/host-key")), 1,
                       "the slow ladder has not ticked again yet")
        XCTAssertEqual(model.keyContinuityState, .pending)
        XCTAssertEqual(requests(Self.hostURL.appendingPathComponent("/events")), 0)
        await model.refreshFleet()
        await waitUntil(requests(Self.hostURL.appendingPathComponent("/host-key")) >= 2,
                        timeout: 0.8)
        XCTAssertEqual(requests(Self.hostURL.appendingPathComponent("/host-key")), 2,
                       "the pull must give the never-verified active host an immediate attempt")
        // The pull's attempt failed; the ladder recovers on its own.
        await waitUntil(model.keyContinuityState == .verified, timeout: 4)
        XCTAssertEqual(model.keyContinuityState, .verified,
                       "the active host must recover automatically once reachable")
        await waitUntil(requests(Self.hostURL.appendingPathComponent("/events")) >= 1)
        XCTAssertEqual(requests(Self.hostURL.appendingPathComponent("/events")), 1,
                       "exactly one stream")
        XCTAssertNil(model.banner)
    }

    /// AC1/AC5 parity (late recovery): the ACTIVE host that stays
    /// unreachable well past any fixed attempt window, then becomes
    /// reachable, verifies and opens exactly one stream automatically — no
    /// further startLive, no pull, no restart.
    func testActiveHostLateReachabilityRecoversWithoutAnotherForeground() async {
        var script: [URL: [PreflightRetryURLProtocol.Outcome]] = [:]
        script[Self.hostURL.appendingPathComponent("/events")] = [.holdOpen]
        var queue: [PreflightRetryURLProtocol.Outcome] =
            Array(repeating: .fail(.cannotConnectToHost), count: 15)
        queue.append(.ok(Self.pinnedKey))
        script[Self.hostURL.appendingPathComponent("/host-key")] = queue
        let session = scriptedSession(script)
        let policy = HostPreflightRetryPolicy(baseInterval: 0.03, maxInterval: 0.03)
        let model = makeModel(session: session, policy: policy)
        model.startLive()
        await waitUntil(requests(Self.hostURL.appendingPathComponent("/host-key")) >= 13,
                        timeout: 4)
        XCTAssertGreaterThanOrEqual(requests(Self.hostURL.appendingPathComponent("/host-key")), 13,
                                    "the ACTIVE ladder must keep retrying past the old fixed window")
        await waitUntil(model.keyContinuityState == .verified, timeout: 4)
        XCTAssertEqual(model.keyContinuityState, .verified,
                       "a late-reachable ACTIVE host must verify automatically")
        await waitUntil(requests(Self.hostURL.appendingPathComponent("/events")) >= 1)
        await settle(0.15)
        XCTAssertEqual(requests(Self.hostURL.appendingPathComponent("/host-key")), 16,
                       "one owner: the queue advanced one attempt at a time")
        XCTAssertEqual(requests(Self.hostURL.appendingPathComponent("/events")), 1,
                       "exactly one stream")
        XCTAssertNil(model.banner, "the retry notice clears once the host verifies")
    }

    /// AC3 parity: background (stopLive) ends an in-flight active-host
    /// ladder — no request after the boundary, state back to `.pending`.
    func testActiveHostBackgroundStopEndsRetries() async {
        var script: [URL: [PreflightRetryURLProtocol.Outcome]] = [:]
        script[Self.hostURL.appendingPathComponent("/host-key")] = [.fail(.cannotConnectToHost)]
        let session = scriptedSession(script)
        let policy = HostPreflightRetryPolicy(baseInterval: 0.25, maxInterval: 0.25)
        let model = makeModel(session: session, policy: policy)
        model.startLive()
        await waitUntil(requests(Self.hostURL.appendingPathComponent("/host-key")) >= 2, timeout: 3)
        let before = requests(Self.hostURL.appendingPathComponent("/host-key"))
        XCTAssertGreaterThanOrEqual(before, 2, "the ladder must have retried before the boundary")
        model.stopLive()
        await settle(0.8)
        XCTAssertEqual(requests(Self.hostURL.appendingPathComponent("/host-key")), before,
                       "background stopLive ends the ladder — no retry after the boundary")
        XCTAssertEqual(model.keyContinuityState, .pending)
    }
}
