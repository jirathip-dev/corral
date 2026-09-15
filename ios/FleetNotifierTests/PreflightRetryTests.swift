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
    /// #554: transport-level lifetime accounting. Every `startLoading` is a
    /// dispatched request and every `stopLoading` is a torn-down one (finished,
    /// cancelled — including by `URLSession.invalidateAndCancel()`), so
    /// `started - stopped` is the number of transport tasks still alive.
    private static var startsStorage: [String: Int] = [:]
    private static var stopsStorage: [String: Int] = [:]
    private static var deliveriesStorage: [String: Int] = [:]

    static func setScript(_ script: [URL: [Outcome]]) {
        lock.lock()
        queuesStorage = script
        requestsStorage = []
        startsStorage = [:]
        stopsStorage = [:]
        deliveriesStorage = [:]
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

    /// #554: how many times a request for `url` has been DISPATCHED.
    static func startCount(to url: URL) -> Int {
        lock.lock()
        defer { lock.unlock() }
        return startsStorage[url.absoluteString] ?? 0
    }

    /// #554: how many times a request for `url` has been TORN DOWN.
    static func stopCount(to url: URL) -> Int {
        lock.lock()
        defer { lock.unlock() }
        return stopsStorage[url.absoluteString] ?? 0
    }

    /// #554: how many times a COMPLETE response for `url` has been handed to
    /// the transport (a hold-open request is never counted).
    static func deliveredCount(to url: URL) -> Int {
        lock.lock()
        defer { lock.unlock() }
        return deliveriesStorage[url.absoluteString] ?? 0
    }

    /// #554: dispatched / torn-down totals across every URL — equal totals
    /// after a teardown mean no transport task outlived it.
    static var startedTotal: Int {
        lock.lock()
        defer { lock.unlock() }
        return startsStorage.values.reduce(0, +)
    }

    static var stoppedTotal: Int {
        lock.lock()
        defer { lock.unlock() }
        return stopsStorage.values.reduce(0, +)
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
        Self.startsStorage[url.absoluteString, default: 0] += 1
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
                Self.lock.lock()
                Self.deliveriesStorage[url.absoluteString, default: 0] += 1
                Self.lock.unlock()
                client?.urlProtocolDidFinishLoading(self)
            }
            // holdOpen: the stream is torn down by disconnect(), never by EOF.
        }
    }

    override func stopLoading() {
        // #554: a torn-down transport task (finished or cancelled — including
        // by a session's `invalidateAndCancel()`). Recorded per URL so a test
        // can watch ONE request's lifetime.
        Self.lock.lock()
        if let url = request.url?.absoluteString {
            Self.stopsStorage[url, default: 0] += 1
        }
        Self.lock.unlock()
    }
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
    /// surviving stream, no retry, no later re-poll and no auto-repair.
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
        let eventsAtMismatch = requests(urls[1].appendingPathComponent("/events"))
        XCTAssertLessThanOrEqual(eventsAtMismatch, 1, "at most the speculative owner")
        XCTAssertTrue((try XCTUnwrap(coordinator.store(profileID: profiles[1].id))).agents.isEmpty)
        XCTAssertNil((try XCTUnwrap(coordinator.store(profileID: profiles[1].id))).lastEventId)
        XCTAssertNotEqual((try XCTUnwrap(coordinator.store(profileID: profiles[1].id))).connectionState, .connected)
        XCTAssertFalse(coordinator.allowsLiveWork(profileID: profiles[1].id))
        // A later start / pull must not re-poll a known mismatch.
        coordinator.startSessionIfNeeded(profiles[1])
        _ = await coordinator.refreshAll(profiles: profiles)
        await settle(0.35)
        XCTAssertEqual(requests(urls[1].appendingPathComponent("/host-key")), 1,
                       "a mismatch must never be re-polled or auto-repaired")
        XCTAssertEqual(requests(urls[1].appendingPathComponent("/events")), eventsAtMismatch, "terminal mismatch never reopens")
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
        XCTAssertEqual(requests(urls[1].appendingPathComponent("/events")), 1, "one speculative stream, despite repeated key retries")
        XCTAssertTrue((try XCTUnwrap(coordinator.store(profileID: profiles[1].id))).agents.isEmpty)
        XCTAssertNil((try XCTUnwrap(coordinator.store(profileID: profiles[1].id))).lastEventId)
        XCTAssertNotEqual((try XCTUnwrap(coordinator.store(profileID: profiles[1].id))).connectionState, .connected)
        XCTAssertFalse(coordinator.allowsLiveWork(profileID: profiles[1].id))
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
        XCTAssertEqual(requests(urls[1].appendingPathComponent("/events")), 1, "extra starts never stack speculative streams")
        XCTAssertTrue((try XCTUnwrap(coordinator.store(profileID: profiles[1].id))).agents.isEmpty)
        XCTAssertNil((try XCTUnwrap(coordinator.store(profileID: profiles[1].id))).lastEventId)
        XCTAssertNotEqual((try XCTUnwrap(coordinator.store(profileID: profiles[1].id))).connectionState, .connected)
        XCTAssertFalse(coordinator.allowsLiveWork(profileID: profiles[1].id))
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
        XCTAssertEqual(requests(urls[1].appendingPathComponent("/events")), 1, "B opened transport but is not verified")
        XCTAssertTrue((try XCTUnwrap(coordinator.store(profileID: profiles[1].id))).agents.isEmpty)
        XCTAssertNil((try XCTUnwrap(coordinator.store(profileID: profiles[1].id))).lastEventId)
        XCTAssertNotEqual((try XCTUnwrap(coordinator.store(profileID: profiles[1].id))).connectionState, .connected)
        XCTAssertFalse(coordinator.allowsLiveWork(profileID: profiles[1].id))
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
        script[Self.hostURL.appendingPathComponent("/events")] = [.holdOpen]
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
        XCTAssertEqual(requests(Self.hostURL.appendingPathComponent("/events")), 1, "one speculative owner despite repeated starts")
        XCTAssertTrue(model.fleet.agents.isEmpty)
        XCTAssertNil(model.fleet.lastEventId)
        XCTAssertNotEqual(model.fleet.connectionState, .connected)
    }

    /// AC2 parity: a mismatching active host never retains a stream, never
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
        let eventsAtMismatch = requests(Self.hostURL.appendingPathComponent("/events"))
        XCTAssertLessThanOrEqual(eventsAtMismatch, 1, "at most the speculative owner")
        XCTAssertTrue(model.fleet.agents.isEmpty)
        XCTAssertNil(model.fleet.lastEventId)
        XCTAssertNotEqual(model.fleet.connectionState, .connected)
        XCTAssertFalse(model.fleet.isStreaming)
        XCTAssertEqual(model.banner?.kind, "host_key_mismatch")
        // Later startLive/refresh calls must not re-poll or auto-repair.
        model.startLive()
        await model.refreshFleet()
        await settle(0.35)
        XCTAssertEqual(requests(Self.hostURL.appendingPathComponent("/host-key")), 1)
        XCTAssertEqual(requests(Self.hostURL.appendingPathComponent("/events")), eventsAtMismatch, "terminal mismatch never reopens")
        XCTAssertTrue(model.fleet.agents.isEmpty)
        XCTAssertNil(model.fleet.lastEventId)
        XCTAssertNotEqual(model.fleet.connectionState, .connected)
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
        XCTAssertEqual(requests(Self.hostURL.appendingPathComponent("/events")), 1, "transport is open but verification is still required")
        XCTAssertTrue(model.fleet.agents.isEmpty)
        XCTAssertNil(model.fleet.lastEventId)
        XCTAssertNotEqual(model.fleet.connectionState, .connected)
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

// MARK: - #554 per-live-session transport (warm-return stalls)
//
// The owner's warm-return stall: `URLSession.shared`'s TCP/HTTP2 pool survives
// a background, so the first foreground `/host-key` preflight could be
// scheduled onto a connection that went stale while the app was suspended and
// hang until the OS noticed. These tests drive the PRODUCTION scene seam
// (`handleScenePhaseChange` → startLive/stopLive) and observe real URLSession
// identities, real request lifetimes and real store state — never source text.

/// #554: lock-guarded record of what ONE completion observed — completions run
/// off the main actor.
private final class ProbeCompletion: @unchecked Sendable {
    private let lock = NSLock()
    private var stored: Error?

    func record(_ error: Error?) {
        lock.lock()
        stored = error
        lock.unlock()
    }

    var error: Error? {
        lock.lock()
        defer { lock.unlock() }
        return stored
    }
}

/// #554 acceptance, entirely through the production scene seam:
/// (a) a foreground cycle installs a DIFFERENT live session and invalidates the
/// previous one, (b) a completion the retired session delivered is dropped,
/// (c) a never-responding `/host-key` fails inside the ladder's own bound and
/// the #451 ladder engages, (d) the stream timeout and `StreamLiveness` budget
/// are unchanged, (e) rapid flapping leaves exactly one live session with zero
/// leaked transport tasks.
@MainActor
final class LiveSessionTransportTests: XCTestCase {
    private static let hostString = "https://h554.example"
    /// #554 round 2: the coordinator-owned (non-active) host of the F1 pin.
    /// It carries its OWN pinned identity — a duplicate key is refused by the
    /// store's identity check.
    private static let secondaryHostString = "https://h554-b.example"
    private static let pinnedKey = Data(repeating: 4, count: 32).base64EncodedString()
    private static let secondaryPinnedKey = Data(repeating: 5, count: 32).base64EncodedString()

    private var suiteName = ""
    private var model: AppModel?
    private var injected: URLSession?
    private var secondaryProfile: HostProfile?

    private func cleanup() {
        model?.stopLive()
        model = nil
        secondaryProfile = nil
        injected?.invalidateAndCancel()
        injected = nil
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

    private func hostURL() throws -> URL {
        try XCTUnwrap(URL(string: Self.hostString))
    }

    private func probeURL() throws -> URL {
        try XCTUnwrap(URL(string: Self.hostString + "/__554_probe__"))
    }

    private func keyURL() throws -> URL {
        try hostURL().appendingPathComponent("/host-key")
    }

    private func scriptedSession(_ script: [URL: [PreflightRetryURLProtocol.Outcome]]) -> URLSession {
        PreflightRetryURLProtocol.setScript(script)
        let config = URLSessionConfiguration.ephemeral
        config.protocolClasses = [PreflightRetryURLProtocol.self]
        let session = URLSession(configuration: config)
        self.injected = session
        return session
    }

    /// One pinned ACTIVE profile bound at init (mode `.live`), mirroring the
    /// production launch wiring. `secondary: true` also pairs ONE pinned
    /// coordinator-owned (non-active) host — the profile whose host clients the
    /// coordinator builds from the transport it adopted.
    private func makeModel(session: URLSession,
                           policy: HostPreflightRetryPolicy,
                           secondary: Bool = false) throws -> AppModel {
        suiteName = "corral.h554.live.\(UUID().uuidString)"
        let defaults = try XCTUnwrap(UserDefaults(suiteName: suiteName))
        let store = HostProfileStore(directory: nil, defaults: defaults)
        let profile = try store.addProfile(displayName: "Host 554",
                                          urlString: Self.hostString,
                                          hostKeyB64: Self.pinnedKey,
                                          fingerprint: "FINGER",
                                          keyId: "dev_554_a",
                                          grants: ["read_tail"],
                                          expiryTs: 1_800_000_000,
                                          registeredAt: 1)
        secondaryProfile = secondary
            ? try store.addProfile(displayName: "Host 554 B",
                                   urlString: Self.secondaryHostString,
                                   hostKeyB64: Self.secondaryPinnedKey,
                                   fingerprint: "FINGER B",
                                   keyId: "dev_554_b",
                                   grants: ["read_tail"],
                                   expiryTs: 1_800_000_000,
                                   registeredAt: 1)
            : nil
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

    /// (a) A background→foreground cycle installs a DIFFERENT live session and
    /// `invalidateAndCancel()`s the previous one — observed through the
    /// production seam: a task STARTED on the retired session is torn down by
    /// the model and completes as cancelled.
    func testForegroundCycleInstallsANewSessionAndInvalidatesTheRetiredOne() async throws {
        defer { cleanup() }
        let host = try hostURL()
        let key = try keyURL()
        let probe = try probeURL()
        var script: [URL: [PreflightRetryURLProtocol.Outcome]] = [:]
        // Session #1's preflight hangs; the foreground session's own preflight
        // is answered, so the new transport is provably functional.
        script[key] = [.holdOpen, .ok(Self.pinnedKey)]
        script[host.appendingPathComponent("/events")] = [.holdOpen]
        script[probe] = [.holdOpen]
        let session = scriptedSession(script)
        // One slow ladder step: session #1 gets exactly ONE (hanging) attempt
        // before the boundary, so the answered attempt below belongs to the
        // foreground session.
        let model = try makeModel(
            session: session,
            policy: HostPreflightRetryPolicy(baseInterval: 1, maxInterval: 1,
                                             attemptTimeout: 0.2))
        model.startLive()
        let first = try XCTUnwrap(model.liveSession, "startLive must install a live session")
        XCTAssertFalse(first === session,
                       "the live session must be its own session, not the injected base one")

        let finished = expectation(description: "a task started on the retired session fails")
        let completion = ProbeCompletion()
        first.dataTask(with: probe) { _, _, error in
            completion.record(error)
            finished.fulfill()
        }.resume()
        await waitUntil(PreflightRetryURLProtocol.startCount(to: probe) == 1)
        XCTAssertEqual(PreflightRetryURLProtocol.startCount(to: probe), 1,
                       "premise: the probe request is in flight on the live session")
        XCTAssertEqual(PreflightRetryURLProtocol.stopCount(to: probe), 0,
                       "premise: the probe is still open across the boundary")
        await waitUntil(PreflightRetryURLProtocol.startCount(to: key) == 1)
        XCTAssertEqual(PreflightRetryURLProtocol.deliveredCount(to: key), 0,
                       "premise: the first session's hanging preflight delivered nothing")

        model.handleScenePhaseChange(.background)
        model.handleScenePhaseChange(.active)
        let second = try XCTUnwrap(model.liveSession, "foreground must install a live session")
        XCTAssertFalse(first === second, "(a) every foreground builds its OWN session")
        await fulfillment(of: [finished], timeout: 3)
        let failure = try XCTUnwrap(completion.error as NSError?)
        XCTAssertEqual(failure.domain, NSURLErrorDomain)
        XCTAssertEqual(failure.code, NSURLErrorCancelled,
                       "(a) invalidateAndCancel() must tear the retired session's task down")
        // The foreground session's OWN preflight is answered on the derived
        // transport — the retired session's attempt never delivered a response.
        await waitUntil(PreflightRetryURLProtocol.deliveredCount(to: key) == 1, timeout: 3)
        XCTAssertEqual(PreflightRetryURLProtocol.deliveredCount(to: key), 1,
                       "(a) the foreground session's preflight must run on the derived transport")
        await waitUntil(model.keyContinuityState == .verified, timeout: 3)
        XCTAssertEqual(model.keyContinuityState, .verified,
                       "(a) the fresh session verifies against an empty pool")
    }

    /// (b) A completion belonging to the RETIRED session is dropped: a pull
    /// refresh left in flight across the boundary completes as cancelled, and
    /// that completion must not be applied to the new live session's state
    /// (a spurious refresh failure) — nor may the retired session's verification
    /// carry over, so the new session must re-verify with its own pool.
    func testCompletionFromTheRetiredSessionIsNeverApplied() async throws {
        defer { cleanup() }
        let host = try hostURL()
        let key = try keyURL()
        var script: [URL: [PreflightRetryURLProtocol.Outcome]] = [:]
        // The FIRST session verifies; every later session's preflight hangs, so
        // nothing but a re-verification on the new session could flip Live.
        script[key] = [.ok(Self.pinnedKey), .holdOpen]
        script[host.appendingPathComponent("/events")] = [.holdOpen]
        script[host.appendingPathComponent("/snapshot")] = [.holdOpen]
        let session = scriptedSession(script)
        let snapshotURL = host.appendingPathComponent("/snapshot")
        let model = try makeModel(
            session: session,
            policy: HostPreflightRetryPolicy(baseInterval: 0.05, maxInterval: 0.05))
        model.startLive()
        await waitUntil(model.keyContinuityState == .verified)
        XCTAssertEqual(model.keyContinuityState, .verified,
                       "premise: the first live session verified its host")
        let first = try XCTUnwrap(model.liveSession)

        // A LIVE pull refresh on that session, held in flight by the transport.
        let pull = Task { @MainActor in await model.refreshFleet() }
        await waitUntil(PreflightRetryURLProtocol.startCount(to: snapshotURL) == 1)
        XCTAssertEqual(PreflightRetryURLProtocol.startCount(to: snapshotURL), 1,
                       "premise: the refresh is in flight on the live session")
        XCTAssertEqual(PreflightRetryURLProtocol.stopCount(to: snapshotURL), 0,
                       "premise: the refresh has not completed yet")

        model.handleScenePhaseChange(.background)
        model.handleScenePhaseChange(.active)
        let second = try XCTUnwrap(model.liveSession)
        XCTAssertFalse(first === second, "the completion belongs to a retired session")
        await pull.value
        await settle(0.4)

        XCTAssertNotEqual(model.banner?.kind, "fleet_refresh",
                          "(b) a retired session's completion must never be applied to state")
        XCTAssertEqual(model.keyContinuityState, .pending,
                       "(b) the new session must re-verify; a retired verification never carries over")
        XCTAssertNotEqual(model.fleet.connectionState, .connected,
                          "(b) and the retired session must never leave the store Live")
        XCTAssertTrue(model.fleet.agents.isEmpty)
    }

    /// (c) A `/host-key` that never responds fails inside the ladder's own
    /// injectable bound — not the transport's request timeout — and the #451
    /// ladder engages and retries it. The measured elapsed first-attempt time
    /// is recorded in the test log against the constant.
    func testNeverRespondingHostKeyFailsWithinTheBoundAndEngagesTheLadder() async throws {
        defer { cleanup() }
        XCTAssertLessThanOrEqual(HostPreflightRetryPolicy.default.attemptTimeout, 5,
                                 "the production preflight bound is at most 5s")
        let host = try hostURL()
        let key = try keyURL()
        var script: [URL: [PreflightRetryURLProtocol.Outcome]] = [:]
        script[key] = [.holdOpen]
        script[host.appendingPathComponent("/events")] = [.holdOpen]
        let session = scriptedSession(script)
        let policy = HostPreflightRetryPolicy(baseInterval: 0.05, maxInterval: 0.05,
                                              attemptTimeout: 0.3)
        let model = try makeModel(session: session, policy: policy)
        let started = Date()
        model.startLive()
        await waitUntil(PreflightRetryURLProtocol.stopCount(to: key) >= 1, timeout: 3)
        let firstAttemptFailure = Date().timeIntervalSince(started)
        XCTAssertGreaterThanOrEqual(PreflightRetryURLProtocol.stopCount(to: key), 1,
                                    "the hanging attempt must be torn down, not left hanging")
        XCTAssertLessThanOrEqual(firstAttemptFailure, policy.attemptTimeout + 0.5,
                                 "a never-responding /host-key must fail within the injectable bound")
        await waitUntil(PreflightRetryURLProtocol.startCount(to: key) >= 3, timeout: 3)
        let attempts = PreflightRetryURLProtocol.startCount(to: key)
        print("ISSUE554_PREFLIGHT bound=\(policy.attemptTimeout) first_failure_after="
              + "\(firstAttemptFailure) attempts=\(attempts)")
        XCTAssertGreaterThanOrEqual(attempts, 3,
                                    "the #451 ladder must engage and retry the bounded attempt")
        XCTAssertEqual(model.keyContinuityState, .pending,
                       "an unverified host must stay paused while the ladder retries")
        XCTAssertNotEqual(model.fleet.connectionState, .connected,
                          "an unverified host must never open a live stream")
    }

    /// (d) The live session INHERITS the transport timeouts, so the SSE stream's
    /// own 60s request timeout and the #425 `StreamLiveness` budget (0.5 × 60)
    /// are unchanged.
    func testLiveSessionKeepsTheStreamTimeoutAndTheLivenessBudget() async throws {
        defer { cleanup() }
        let host = try hostURL()
        var script: [URL: [PreflightRetryURLProtocol.Outcome]] = [:]
        script[host.appendingPathComponent("/host-key")] = [.ok(Self.pinnedKey)]
        script[host.appendingPathComponent("/events")] = [.holdOpen]
        let session = scriptedSession(script)
        let model = try makeModel(
            session: session,
            policy: HostPreflightRetryPolicy(baseInterval: 0.05, maxInterval: 0.05))
        model.startLive()
        let live = try XCTUnwrap(model.liveSession)
        XCTAssertEqual(live.configuration.timeoutIntervalForRequest,
                       session.configuration.timeoutIntervalForRequest,
                       "(d) the derived session inherits the base session's timeouts")
        XCTAssertEqual(live.configuration.timeoutIntervalForRequest, 60,
                       "(d) the SSE stream's request timeout is unchanged")
        XCTAssertFalse(live.configuration.waitsForConnectivity,
                       "an attempt must fail fast instead of parking on connectivity")
        XCTAssertEqual(live.configuration.protocolClasses?.contains { $0 == PreflightRetryURLProtocol.self },
                       true, "the injected transport stack must ride along")
        await waitUntil(model.fleet.isStreaming)
        XCTAssertTrue(model.fleet.isStreaming, "the verified host must open its stream")
        XCTAssertEqual(model.fleet.streamLiveness.inactivityBudget, 30,
                       "(d) StreamLiveness stays 0.5 x the 60s stream timeout")
        XCTAssertEqual(model.fleet.streamLiveness.tickInterval, 5)
        XCTAssertEqual(StreamLiveness(transportTimeout: 60).inactivityBudget, 30)
    }

    /// (e) Rapid phase flapping leaves exactly ONE live session at a time, never
    /// reuses (or aliases) a retired one, and leaks no transport task: every
    /// dispatched request is torn down once the last boundary is crossed.
    func testRapidPhaseFlappingLeavesOneLiveSessionAndNoLeakedTasks() async throws {
        defer { cleanup() }
        let host = try hostURL()
        let probe = try probeURL()
        var script: [URL: [PreflightRetryURLProtocol.Outcome]] = [:]
        script[host.appendingPathComponent("/host-key")] = [.holdOpen]
        script[host.appendingPathComponent("/events")] = [.holdOpen]
        script[probe] = [.holdOpen]
        let session = scriptedSession(script)
        let model = try makeModel(
            session: session,
            policy: HostPreflightRetryPolicy(baseInterval: 0.05, maxInterval: 0.05,
                                             attemptTimeout: 0.2))
        model.startLive()
        var probes: [(URLSession, ProbeCompletion)] = []
        for _ in 0..<4 {
            let live = try XCTUnwrap(model.liveSession)
            let completion = ProbeCompletion()
            live.dataTask(with: probe) { _, _, error in completion.record(error) }.resume()
            probes.append((live, completion))
            model.handleScenePhaseChange(.background)
            model.handleScenePhaseChange(.active)
            // Let the re-installed session actually dispatch its preflight and
            // stream, so the boundary below tears down REAL in-flight work.
            await settle(0.08)
        }
        let current = try XCTUnwrap(model.liveSession)
        var identities = probes.map { ObjectIdentifier($0.0) }
        identities.append(ObjectIdentifier(current))
        XCTAssertEqual(Set(identities).count, identities.count,
                       "(e) flapping must never reuse or alias a live session")

        // A repeated startLive INSIDE one live session must reuse it.
        model.startLive()
        model.startLive()
        XCTAssertTrue(model.liveSession === current,
                      "(e) repeated startLive in one live session must not replace it")

        model.handleScenePhaseChange(.background)
        XCTAssertNil(model.liveSession, "(e) the last boundary retires the live session")
        await settle(0.8)
        for (index, entry) in probes.enumerated() {
            let failure = entry.1.error as NSError?
            XCTAssertEqual(failure?.code, NSURLErrorCancelled,
                           "(e) retired live session #\(index) must be invalidated")
        }
        let started = PreflightRetryURLProtocol.startedTotal
        let stopped = PreflightRetryURLProtocol.stoppedTotal
        print("ISSUE554_TASKS started=\(started) stopped=\(stopped)")
        // A LEAK is a dispatched request that never came back. URLSession may
        // ALSO tear down a task that was cancelled before its transport ever
        // started (measured: a leg after a fresh simulator erase reported more
        // teardowns than dispatches), so the sound invariant is "no dispatch is
        // left unmatched", not strict equality.
        XCTAssertGreaterThanOrEqual(stopped, started,
                                    "(e) zero leaked transport tasks across the flap")
        XCTAssertGreaterThanOrEqual(PreflightRetryURLProtocol.stopCount(to: probe),
                                    PreflightRetryURLProtocol.startCount(to: probe),
                                    "(e) every probe request dispatched on a retired session must be torn down")
        XCTAssertGreaterThan(PreflightRetryURLProtocol.startCount(to: probe), 0,
                             "(e) premise: the flapping dispatched real transport work")
    }

    /// (F1 — round 2) The class the round-1 suite could not see: task CREATION
    /// on a retired transport, not completion delivery. `stopLive()` clears
    /// `liveSession` while `mode` stays live, so a foreground-reachable path
    /// still runs inside that window: Settings ▸ Retry
    /// (`retryHostConnection(_:)`) drives the coordinator's
    /// `startSessionIfNeeded`, and the coordinator built its client from the
    /// transport it CACHED (`urlSession`, adopted in `startLive()`). Nothing
    /// repaired that holder when the live session was invalidated, so the retry
    /// created a task on the invalidated `URLSession` — an uncatchable
    /// `NSGenericException` that killed the process (the reviewer's PROBE2:
    /// exit 65, `liveSession_nil=true`, `mode=live`). This pin drives that
    /// route through the production scene seam and fails RED at 42ca305.
    func testRetryDuringRetirementNeverDispatchesOnTheRetiredTransport() async throws {
        defer { cleanup() }
        let host = try hostURL()
        let secondaryHost = try XCTUnwrap(URL(string: Self.secondaryHostString))
        let secondaryKey = secondaryHost.appendingPathComponent("/host-key")
        let secondaryEvents = secondaryHost.appendingPathComponent("/events")
        let secondarySnapshot = secondaryHost.appendingPathComponent("/snapshot")
        let activeSnapshot = host.appendingPathComponent("/snapshot")
        let probe = try probeURL()
        var script: [URL: [PreflightRetryURLProtocol.Outcome]] = [:]
        script[try keyURL()] = [.ok(Self.pinnedKey)]
        script[host.appendingPathComponent("/events")] = [.holdOpen]
        script[secondaryKey] = [.ok(Self.secondaryPinnedKey)]
        script[secondaryEvents] = [.holdOpen]
        script[secondarySnapshot] = [.fail(.timedOut)]
        script[activeSnapshot] = [.fail(.timedOut)]
        script[probe] = [.holdOpen]
        let session = scriptedSession(script)
        let model = try makeModel(
            session: session,
            policy: HostPreflightRetryPolicy(baseInterval: 0.05, maxInterval: 0.05),
            secondary: true)
        let secondary = try XCTUnwrap(secondaryProfile, "premise: the coordinator host is paired")
        model.startLive()
        let retiring = try XCTUnwrap(model.liveSession, "startLive must install a live session")
        XCTAssertFalse(retiring === session,
                       "the live session is its own transport, not the base one")
        await waitUntil(model.keyContinuityState == .verified, timeout: 3)
        XCTAssertEqual(model.keyContinuityState, .verified,
                       "premise: the ACTIVE host verified on the live session")
        await waitUntil(model.coordinator?.store(profileID: secondary.id)?.isStreaming == true,
                        timeout: 3)
        XCTAssertEqual(model.coordinator?.store(profileID: secondary.id)?.isStreaming, true,
                       "premise: the coordinator host runs on the live session too")

        // A task dispatched on the live session BEFORE the boundary can only
        // complete as cancelled afterwards — the observation that the retired
        // transport really is invalidated.
        let retired = expectation(description: "the retired session's task fails")
        let completion = ProbeCompletion()
        retiring.dataTask(with: probe) { _, _, error in
            completion.record(error)
            retired.fulfill()
        }.resume()
        await waitUntil(PreflightRetryURLProtocol.startCount(to: probe) == 1)
        XCTAssertEqual(PreflightRetryURLProtocol.startCount(to: probe), 1,
                       "premise: the probe is in flight on the live session")

        // The retirement boundary (background, or the first half of a host
        // switch): the live session is retired and its pool killed, while the
        // model stays in `.live` and keeps serving live paths.
        model.handleScenePhaseChange(.background)
        XCTAssertNil(model.liveSession, "premise: the boundary retires the live session")
        XCTAssertEqual(model.mode, .live, "premise: a live path is still reachable")
        await fulfillment(of: [retired], timeout: 3)
        XCTAssertEqual((completion.error as NSError?)?.code, NSURLErrorCancelled,
                       "the retired transport is invalidated, not merely dropped")
        // The transport's own teardown record lags the completion callback, so
        // this is a bounded wait, not a same-tick read.
        await waitUntil(PreflightRetryURLProtocol.stopCount(to: probe) >= 1, timeout: 2)
        XCTAssertGreaterThanOrEqual(PreflightRetryURLProtocol.stopCount(to: probe), 1,
                                    "the retirement tore its in-flight task down")

        // The reviewer's abort site, reached exactly as the app reaches it.
        // At 42ca305 the task created here is fatal.
        let keyBefore = PreflightRetryURLProtocol.startCount(to: secondaryKey)
        let eventsBefore = PreflightRetryURLProtocol.startCount(to: secondaryEvents)
        let deliveredBefore = PreflightRetryURLProtocol.deliveredCount(to: secondaryKey)
        model.retryHostConnection(secondary)
        // The retry's work is asynchronous: the stream attempt and the bounded
        // preflight both dispatch from inside their own tasks, so each is
        // given its own bounded wait before it is asserted on.
        await waitUntil(PreflightRetryURLProtocol.startCount(to: secondaryKey) > keyBefore, timeout: 3)
        XCTAssertGreaterThan(PreflightRetryURLProtocol.startCount(to: secondaryKey), keyBefore,
                             "the retry must still dispatch — on a VALID transport")
        await waitUntil(PreflightRetryURLProtocol.startCount(to: secondaryEvents) > eventsBefore,
                        timeout: 3)
        XCTAssertGreaterThan(PreflightRetryURLProtocol.startCount(to: secondaryEvents), eventsBefore,
                             "and the coordinator host's stream must be dispatched too")
        await waitUntil(PreflightRetryURLProtocol.deliveredCount(to: secondaryKey) > deliveredBefore,
                        timeout: 3)
        XCTAssertGreaterThan(PreflightRetryURLProtocol.deliveredCount(to: secondaryKey), deliveredBefore,
                             "the bounded preflight must actually be answered")
        XCTAssertNil(model.liveSession, "a retry must not invent a live session")

        // The next foreground owns the only live transport, and the live path
        // that runs afterwards dispatches on it — never on the retired one.
        model.handleScenePhaseChange(.active)
        let foreground = try XCTUnwrap(model.liveSession, "foreground must install a live session")
        XCTAssertFalse(foreground === retiring, "the next live session is a NEW transport")
        XCTAssertFalse(foreground === session, "and never the base session")
        await waitUntil(model.keyContinuityState == .verified, timeout: 3)
        let snapshotBefore = PreflightRetryURLProtocol.startCount(to: activeSnapshot)
        await model.refreshFleet()
        XCTAssertGreaterThan(PreflightRetryURLProtocol.startCount(to: activeSnapshot), snapshotBefore,
                             "a live-path dispatch after the foreground still works")
        XCTAssertTrue(model.liveSession === foreground,
                      "the foreground session is the only live transport in use")
    }
}
