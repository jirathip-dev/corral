import Foundation
import XCTest
@testable import FleetNotifier

/// #452 — dedicated ACTUAL reconnect-loop tests for `CorraldClient.stream()`.
///
/// Every test drives the REAL `stream()` over a real `URLSession` whose
/// `URLProtocol` scripts per-attempt behaviour, and asserts RUNTIME
/// observations: the recorded wait sequence (injected `sleep`), callback
/// order, request headers and the single-flight high-water mark. There are
/// no helper-only and no source-string assertions here.
///
/// Seams used: `StreamRetryPolicy` with a 1 s `stableSessionThreshold`,
/// plus injected `sleep`/`randomUnit`/`now`. Production defaults are
/// untouched; all existing `CorraldClient(host:session:)` call sites keep
/// compiling unchanged.
///
/// REGISTRATION FENCE: this file is intentionally NOT registered in
/// `FleetNotifier.xcodeproj` yet (source phase). Canonical registration,
/// the Release digest re-pin and the full gate battery queue behind the
/// #451/#448 refresh handoff — do not claim these tests ran until that
/// handoff registers them.
///
/// RED at the pristine phase base (ba155631): the base lacks the seam API,
/// so the base leg is a COMPILE-ONLY seams patch on a scratch worktree —
/// `StreamRetryPolicy` + the three defaulted init parameters + stored
/// properties, with the ORIGINAL loop body (reset on the 200 guard, no
/// jitter, no delivery ledger). That patch is behaviour-preserving by
/// construction; against it the criterion/ladder tests below fail on
/// BEHAVIOUR (flat [1, 1, 1, …] waits, jitter pins identical, no sustained
/// reset), while the preservation guards (callback order, cancellation,
/// cursor, per-host isolation) pass. On the head all tests pass, and
/// mutations that restore the 200-reset or strip the delivery ledger prove
/// each witness bites.
final class StreamRetryBackoffTests: XCTestCase {

    private typealias Step = StreamRetryLoopURLProtocol.Step

    private static let hostAKey = "g452-retry-a.example"
    private static let hostBKey = "g452-retry-b.example"
    private static let hostCKey = "g452-retry-c.example"

    // SAFETY: fixed valid URL literal for the fixture host.
    private let hostA = URL(string: "http://g452-retry-a.example")!
    // SAFETY: fixed valid URL literal for the fixture host.
    private let hostB = URL(string: "http://g452-retry-b.example")!
    // SAFETY: fixed valid URL literal for the fixture host.
    private let hostC = URL(string: "http://g452-retry-c.example")!

    /// The dedicated 1 s threshold keeps every sustained fixture short;
    /// production ships the 30 s default (`StreamRetryPolicy.default`).
    private static let testPolicy = StreamRetryPolicy(
        baseDelay: 1, maxDelay: 30, jitterFraction: 0.5, stableSessionThreshold: 1)

    private var sessions: [URLSession] = []

    override func setUp() {
        super.setUp()
        StreamRetryLoopURLProtocol.reset()
        sessions = []
    }

    override func tearDown() {
        for session in sessions { session.invalidateAndCancel() }
        sessions = []
        StreamRetryLoopURLProtocol.reset()
        super.tearDown()
    }

    // MARK: - Harness

    private func makeClient(host: URL,
                            waits: WaitRecorder,
                            policy: StreamRetryPolicy? = nil,
                            random: Double = 0,
                            randomSource: ScriptedValues? = nil,
                            clock: ScriptedValues? = nil,
                            sleepImpl: (@Sendable (TimeInterval) async throws -> Void)? = nil) -> CorraldClient {
        let config = URLSessionConfiguration.ephemeral
        config.timeoutIntervalForRequest = 60
        config.protocolClasses = [StreamRetryLoopURLProtocol.self]
        let session = URLSession(configuration: config)
        sessions.append(session)
        let sleep: @Sendable (TimeInterval) async throws -> Void =
            sleepImpl ?? { wait in waits.append(wait) }
        return CorraldClient(
            host: host,
            session: session,
            retryPolicy: policy ?? Self.testPolicy,
            sleep: sleep,
            randomUnit: { randomSource?.next() ?? random },
            now: { clock?.next() ?? ProcessInfo.processInfo.systemUptime })
    }

    private func startStream(_ client: CorraldClient,
                             cursor: CursorHolder,
                             log: CallbackLog) -> (task: Task<Void, Never>, returned: ReturnFlag) {
        let returned = ReturnFlag()
        let task = Task {
            await client.stream(
                lastEventId: { cursor.cursor },
                onEvent: { _ in log.recordFrame() },
                onConnected: { log.record("connected") },
                onConnectionError: { log.recordError($0) },
                onStreamEnded: { log.record("ended") },
                onActivity: nil)
            returned.mark()
        }
        return (task, returned)
    }

    private func waitFor(_ condition: () -> Bool,
                         timeout: TimeInterval = 10) async -> Bool {
        let deadline = Date().addingTimeInterval(timeout)
        while Date() < deadline {
            if condition() { return true }
            try? await Task.sleep(nanoseconds: 25_000_000)
        }
        return condition()
    }

    private func assertWaits(_ recorder: WaitRecorder,
                             prefix expected: [TimeInterval],
                             file: StaticString = #filePath,
                             line: UInt = #line) {
        let waits = recorder.waits
        XCTAssertGreaterThanOrEqual(waits.count, expected.count,
                                    "expected at least \(expected.count) waits, got \(waits)",
                                    file: file, line: line)
        for (index, value) in expected.enumerated() where index < waits.count {
            XCTAssertEqual(waits[index], value, accuracy: 1e-9,
                           "wait #\(index + 1) of \(waits)", file: file, line: line)
        }
    }

    // MARK: - AC1: unstable 200/EOF must escalate

    /// A server that answers 200 and closes immediately (zero body bytes)
    /// must ESCALATE: [1, 2, 4, 8, 16, 30]. Before #452 the ladder reset on
    /// the 200 header alone, so the retry stayed on the 1 s rung forever.
    /// The injected random source is pinned to 0, so every wait is exactly
    /// the nominal rung.
    func testImmediate200EOFAttemptsEscalate() async {
        StreamRetryLoopURLProtocol.script(
            Array(repeating: Step.headersOnlyEOF, count: 8), forHost: Self.hostAKey)
        let waits = WaitRecorder()
        let log = CallbackLog()
        let run = startStream(makeClient(host: hostA, waits: waits),
                              cursor: CursorHolder(), log: log)

        let escalated = await waitFor({ waits.count >= 6 })
        run.task.cancel()
        let returned = await waitFor({ run.returned.isDone }, timeout: 5)
        XCTAssertTrue(escalated, "the ladder must produce at least six recorded waits")
        XCTAssertTrue(returned, "cancellation must end stream() promptly")
        assertWaits(waits, prefix: [1, 2, 4, 8, 16, 30])
        XCTAssertGreaterThanOrEqual(log.endedCount, 6,
                                    "every clean-EOF attempt reports onStreamEnded")
        XCTAssertEqual(StreamRetryLoopURLProtocol.concurrentHighWater, 1,
                       "attempts must never overlap (single-flight)")
    }

    /// ONE delivered line followed by an immediate EOF is NOT a sustained
    /// session — it must keep escalating. This is the explicit probe
    /// against the rejected one-line criterion (span 0 can never qualify).
    func testSingleLineThenEOFStillEscalates() async {
        StreamRetryLoopURLProtocol.script(
            Array(repeating: Step.lineThenEOF(": keep-alive"), count: 3),
            forHost: Self.hostAKey)
        let waits = WaitRecorder()
        let run = startStream(makeClient(host: hostA, waits: waits),
                              cursor: CursorHolder(), log: CallbackLog())

        let saw = await waitFor({ waits.count >= 3 })
        run.task.cancel()
        _ = await waitFor({ run.returned.isDone }, timeout: 5)
        XCTAssertTrue(saw)
        assertWaits(waits, prefix: [1, 2, 4])
    }

    /// "One early line followed by silence" is explicitly NOT healthy:
    /// silence adds no span, so the ladder keeps escalating even though the
    /// attempt lived well past the threshold.
    func testSilentAfterOneLineDoesNotReset() async {
        StreamRetryLoopURLProtocol.script(
            [Step.lineThenSilence(": keep-alive", seconds: 1.1),
             Step.lineThenSilence(": keep-alive", seconds: 1.1),
             Step.headersOnlyEOF],
            forHost: Self.hostAKey)
        let waits = WaitRecorder()
        let run = startStream(makeClient(host: hostA, waits: waits),
                              cursor: CursorHolder(), log: CallbackLog())

        let saw = await waitFor({ waits.count >= 3 }, timeout: 15)
        run.task.cancel()
        _ = await waitFor({ run.returned.isDone }, timeout: 5)
        XCTAssertTrue(saw)
        assertWaits(waits, prefix: [1, 2, 4])
    }

    // MARK: - AC2: sustained healthy sessions collapse the ladder

    /// Heartbeat-only idle session: comment keep-alives spanning the
    /// threshold reset the ladder. The [unstable, unstable, sustained,
    /// unstable] shape discriminates the reset from BOTH the base (which
    /// never escalates: [1, 1, 1, 1]) and a broken never-reset ladder
    /// (which would yield [1, 2, 4, 8]).
    func testSustainedHeartbeatOnlySessionResetsLadder() async {
        StreamRetryLoopURLProtocol.script(
            [Step.headersOnlyEOF,
             Step.headersOnlyEOF,
             Step.commentsThenEOF(interval: 0.35, count: 5),
             Step.headersOnlyEOF],
            forHost: Self.hostAKey)
        let waits = WaitRecorder()
        let run = startStream(makeClient(host: hostA, waits: waits),
                              cursor: CursorHolder(), log: CallbackLog())

        let saw = await waitFor({ waits.count >= 4 }, timeout: 15)
        run.task.cancel()
        _ = await waitFor({ run.returned.isDone }, timeout: 5)
        XCTAssertTrue(saw)
        assertWaits(waits, prefix: [1, 2, 1, 2])
    }

    /// Data sessions count too: framed deltas spanning the threshold are
    /// healthy delivery (comments are not required).
    func testSustainedDataSessionResetsLadder() async {
        StreamRetryLoopURLProtocol.script(
            [Step.headersOnlyEOF,
             Step.headersOnlyEOF,
             Step.framesThenEOF(interval: 1.1, count: 2),
             Step.headersOnlyEOF],
            forHost: Self.hostAKey)
        let waits = WaitRecorder()
        let log = CallbackLog()
        let run = startStream(makeClient(host: hostA, waits: waits),
                              cursor: CursorHolder(), log: log)

        let saw = await waitFor({ waits.count >= 4 }, timeout: 15)
        run.task.cancel()
        _ = await waitFor({ run.returned.isDone }, timeout: 5)
        XCTAssertTrue(saw)
        assertWaits(waits, prefix: [1, 2, 1, 2])
        XCTAssertGreaterThanOrEqual(log.frameCount, 2,
                                    "both scripted deltas must reach onEvent")
    }

    /// Threshold boundary, below: with the injected clock scripted to 0.9 s
    /// of delivered span the ladder keeps escalating ([1, 2, 4, 8]).
    func testDeliveredSpanBelowThresholdDoesNotReset() async {
        StreamRetryLoopURLProtocol.script(
            [Step.headersOnlyEOF,
             Step.headersOnlyEOF,
             Step.linesThenEOF([": a", ": b"]),
             Step.headersOnlyEOF],
            forHost: Self.hostAKey)
        let waits = WaitRecorder()
        let run = startStream(makeClient(host: hostA, waits: waits,
                                         clock: ScriptedValues([0, 0.9])),
                              cursor: CursorHolder(), log: CallbackLog())

        let saw = await waitFor({ waits.count >= 4 })
        run.task.cancel()
        _ = await waitFor({ run.returned.isDone }, timeout: 5)
        XCTAssertTrue(saw)
        assertWaits(waits, prefix: [1, 2, 4, 8])
    }

    /// Threshold boundary, at: the SAME shape with the injected clock at
    /// exactly 1.0 s resets ([1, 2, 1, 2]) — the two tests differ ONLY in
    /// the injected clock values.
    func testDeliveredSpanAtThresholdResetsLadder() async {
        StreamRetryLoopURLProtocol.script(
            [Step.headersOnlyEOF,
             Step.headersOnlyEOF,
             Step.linesThenEOF([": a", ": b"]),
             Step.headersOnlyEOF],
            forHost: Self.hostAKey)
        let waits = WaitRecorder()
        let run = startStream(makeClient(host: hostA, waits: waits,
                                         clock: ScriptedValues([0, 1])),
                              cursor: CursorHolder(), log: CallbackLog())

        let saw = await waitFor({ waits.count >= 4 })
        run.task.cancel()
        _ = await waitFor({ run.returned.isDone }, timeout: 5)
        XCTAssertTrue(saw)
        assertWaits(waits, prefix: [1, 2, 1, 2])
    }

    // MARK: - AC3: bounded jitter, injected source

    /// Jitter floor: u = 1 removes exactly `jitterFraction` (0.5) of every
    /// nominal rung — [0.5, 1, 2, 4, 8, 15].
    func testJitterFloorScalesEveryRung() async {
        StreamRetryLoopURLProtocol.script(
            Array(repeating: Step.headersOnlyEOF, count: 6), forHost: Self.hostAKey)
        let waits = WaitRecorder()
        let run = startStream(makeClient(host: hostA, waits: waits, random: 1),
                              cursor: CursorHolder(), log: CallbackLog())

        let saw = await waitFor({ waits.count >= 6 })
        run.task.cancel()
        _ = await waitFor({ run.returned.isDone }, timeout: 5)
        XCTAssertTrue(saw)
        assertWaits(waits, prefix: [0.5, 1, 2, 4, 8, 15])
    }

    /// Intermediate injected jitter values stay inside the documented
    /// envelope: wait = nominal × (1 − 0.5 × u).
    func testJitterIntermediateValuesStayWithinBounds() async {
        StreamRetryLoopURLProtocol.script(
            Array(repeating: Step.headersOnlyEOF, count: 5), forHost: Self.hostAKey)
        let waits = WaitRecorder()
        let run = startStream(makeClient(host: hostA, waits: waits,
                                         randomSource: ScriptedValues([0, 0.25, 0.5, 0.75, 1])),
                              cursor: CursorHolder(), log: CallbackLog())

        let saw = await waitFor({ waits.count >= 5 })
        run.task.cancel()
        _ = await waitFor({ run.returned.isDone }, timeout: 5)
        XCTAssertTrue(saw)
        assertWaits(waits, prefix: [1, 1.75, 3, 5, 8])
    }

    /// Two clients fed different jitter draws must not retry in lockstep —
    /// the synchronized-retry hazard the jitter exists to break.
    func testTwoClientsDeSynchronizeRetryWaits() async {
        StreamRetryLoopURLProtocol.script(
            Array(repeating: Step.headersOnlyEOF, count: 2), forHost: Self.hostAKey)
        StreamRetryLoopURLProtocol.script(
            Array(repeating: Step.headersOnlyEOF, count: 2), forHost: Self.hostBKey)
        let waitsA = WaitRecorder()
        let waitsB = WaitRecorder()
        let runA = startStream(makeClient(host: hostA, waits: waitsA, random: 0),
                               cursor: CursorHolder(), log: CallbackLog())
        let runB = startStream(makeClient(host: hostB, waits: waitsB, random: 1),
                               cursor: CursorHolder(), log: CallbackLog())

        let saw = await waitFor({ waitsA.count >= 1 && waitsB.count >= 1 })
        runA.task.cancel()
        runB.task.cancel()
        _ = await waitFor({ runA.returned.isDone && runB.returned.isDone }, timeout: 5)
        XCTAssertTrue(saw)
        XCTAssertEqual(waitsA.waits.first ?? -1, 1, accuracy: 1e-9)
        XCTAssertEqual(waitsB.waits.first ?? -1, 0.5, accuracy: 1e-9)
        XCTAssertNotEqual(waitsA.waits.first ?? -1, waitsB.waits.first ?? -2)
    }

    // MARK: - AC4: honest callbacks, cancellation

    /// Non-200, transport error and clean EOF keep their honest callback
    /// order — [error(500), connected+ended, error(timeout),
    /// connected+ended] — while the ladder escalates [1, 2, 4, 8].
    func testNon200AndErrorAndEOFCallbackOrder() async {
        StreamRetryLoopURLProtocol.script(
            [Step.non200(500),
             Step.headersOnlyEOF,
             Step.fail(URLError.Code.timedOut),
             Step.headersOnlyEOF],
            forHost: Self.hostAKey)
        let waits = WaitRecorder()
        let log = CallbackLog()
        let run = startStream(makeClient(host: hostA, waits: waits),
                              cursor: CursorHolder(), log: log)

        let saw = await waitFor({ waits.count >= 4 && log.eventKinds.count >= 6 })
        run.task.cancel()
        _ = await waitFor({ run.returned.isDone }, timeout: 5)
        XCTAssertTrue(saw)
        XCTAssertEqual(Array(log.eventKinds.prefix(6)),
                       ["error", "connected", "ended", "error", "connected", "ended"])
        let errors = log.errors
        XCTAssertTrue(errors.first?.contains("events failed: HTTP 500") ?? false,
                      "the 500 must name its status; got \(errors)")
        XCTAssertFalse(errors.count < 2 || errors[1].isEmpty,
                       "the transport error must report a reason")
        assertWaits(waits, prefix: [1, 2, 4, 8])
    }

    /// Cancellation while the ladder WAITS: stream() ends promptly, reports
    /// nothing, and neither schedules nor dispatches another retry.
    func testCancellationDuringWaitExitsCleanly() async {
        StreamRetryLoopURLProtocol.script([Step.headersOnlyEOF], forHost: Self.hostAKey)
        let waits = WaitRecorder()
        let log = CallbackLog()
        let run = startStream(makeClient(host: hostA, waits: waits,
                                         sleepImpl: { wait in
                                             waits.append(wait)
                                             try await Task.sleep(nanoseconds: 30_000_000_000)
                                         }),
                              cursor: CursorHolder(), log: log)

        let waiting = await waitFor({ waits.count >= 1 })
        run.task.cancel()
        let returned = await waitFor({ run.returned.isDone }, timeout: 5)
        XCTAssertTrue(waiting, "the first wait must be in flight before cancelling")
        XCTAssertTrue(returned, "cancellation during the wait must end stream() promptly")
        XCTAssertEqual(waits.count, 1, "no wait may be scheduled after cancellation")
        XCTAssertTrue(log.errors.isEmpty, "cancellation is not an error")
        XCTAssertEqual(log.endedCount, 0, "cancellation is not a clean EOF")
        XCTAssertEqual(StreamRetryLoopURLProtocol.requests(forHost: Self.hostAKey).count, 1,
                       "no retry may be dispatched after cancellation")
        try? await Task.sleep(nanoseconds: 300_000_000)
        XCTAssertEqual(StreamRetryLoopURLProtocol.requests(forHost: Self.hostAKey).count, 1,
                       "no late retry may appear after cancellation")
    }

    /// Cancellation while the READ is in flight (200 acked, open and
    /// silent): prompt exit, no error, no ended, no retry.
    func testCancellationDuringReadExitsCleanly() async {
        StreamRetryLoopURLProtocol.script([Step.hold], forHost: Self.hostAKey)
        let waits = WaitRecorder()
        let log = CallbackLog()
        let run = startStream(makeClient(host: hostA, waits: waits),
                              cursor: CursorHolder(), log: log)

        let connected = await waitFor({ log.connectedCount >= 1 })
        run.task.cancel()
        let returned = await waitFor({ run.returned.isDone }, timeout: 5)
        XCTAssertTrue(connected, "the attempt must be inside its read before cancelling")
        XCTAssertTrue(returned, "cancellation during the read must end stream() promptly")
        XCTAssertTrue(log.errors.isEmpty, "cancellation is not an error")
        XCTAssertEqual(log.endedCount, 0, "cancellation is not a clean EOF")
        XCTAssertEqual(waits.count, 0, "a cancelled read never reaches a wait")
        XCTAssertEqual(StreamRetryLoopURLProtocol.requests(forHost: Self.hostAKey).count, 1)
        try? await Task.sleep(nanoseconds: 300_000_000)
        XCTAssertEqual(StreamRetryLoopURLProtocol.requests(forHost: Self.hostAKey).count, 1,
                       "no late retry may appear after cancellation")
    }

    // MARK: - AC5: per-host isolation + accepted-cursor resumes

    /// Host A's escalating ladder must not leak into host B's healthy
    /// ladder: each stream() owns its ladder and delivery ledger.
    func testPerHostLaddersAreIsolated() async {
        StreamRetryLoopURLProtocol.script(
            Array(repeating: Step.headersOnlyEOF, count: 4), forHost: Self.hostAKey)
        StreamRetryLoopURLProtocol.script(
            [Step.commentsThenEOF(interval: 0.35, count: 5), Step.headersOnlyEOF],
            forHost: Self.hostBKey)
        let waitsA = WaitRecorder()
        let waitsB = WaitRecorder()
        let runA = startStream(makeClient(host: hostA, waits: waitsA),
                               cursor: CursorHolder(), log: CallbackLog())
        let runB = startStream(makeClient(host: hostB, waits: waitsB),
                               cursor: CursorHolder(), log: CallbackLog())

        let saw = await waitFor({ waitsA.count >= 4 && waitsB.count >= 2 }, timeout: 15)
        runA.task.cancel()
        runB.task.cancel()
        _ = await waitFor({ runA.returned.isDone && runB.returned.isDone }, timeout: 5)
        XCTAssertTrue(saw)
        assertWaits(waitsA, prefix: [1, 2, 4, 8])
        assertWaits(waitsB, prefix: [1, 2])
    }

    /// Cursor resume: every attempt carries the ACCEPTED cursor
    /// (rev + epoch) — unchanged while the ladder escalates — and switches
    /// to the next accepted value as soon as the owner stores it. A one-way
    /// latch holds the wait before attempt 4, so the cursor update lands
    /// deterministically BEFORE that attempt is built (with instant
    /// injected sleeps attempt 4 would otherwise race the update).
    func testCursorResumesFromAcceptedStateAcrossEscalation() async {
        StreamRetryLoopURLProtocol.script(
            [Step.headersOnlyEOF, Step.headersOnlyEOF, Step.headersOnlyEOF, Step.hold],
            forHost: Self.hostCKey)
        let waits = WaitRecorder()
        let latch = LatchedGate()
        let cursor = CursorHolder(SSECursor(rev: 42, epoch: "E1"))
        let run = startStream(makeClient(host: hostC, waits: waits,
                                         sleepImpl: { wait in
                                             waits.append(wait)
                                             while waits.count >= 3, !latch.isOpen, !Task.isCancelled {
                                                 try? await Task.sleep(nanoseconds: 10_000_000)
                                             }
                                         }),
                              cursor: cursor, log: CallbackLog())

        let sawThree = await waitFor({
            StreamRetryLoopURLProtocol.requests(forHost: Self.hostCKey).count >= 3
        })
        XCTAssertTrue(sawThree)
        let firstThree = StreamRetryLoopURLProtocol.requests(forHost: Self.hostCKey).prefix(3)
        for record in firstThree {
            XCTAssertEqual(record.lastEventID, "42")
            XCTAssertEqual(record.epoch, "E1")
        }

        cursor.set(rev: 43, epoch: "E2")
        latch.open()
        let sawFour = await waitFor({
            StreamRetryLoopURLProtocol.requests(forHost: Self.hostCKey).count >= 4
        })
        run.task.cancel()
        _ = await waitFor({ run.returned.isDone }, timeout: 5)
        XCTAssertTrue(sawFour)
        let fourth = StreamRetryLoopURLProtocol.requests(forHost: Self.hostCKey)[3]
        XCTAssertEqual(fourth.lastEventID, "43")
        XCTAssertEqual(fourth.epoch, "E2")
    }
}

// MARK: - Test doubles

/// Deterministic value source for the injected jitter / clock seams: each
/// call consumes the next scripted value; once exhausted it keeps returning
/// the last one.
private final class ScriptedValues: @unchecked Sendable {
    private let lock = NSLock()
    private let values: [Double]
    private var index = 0

    init(_ values: [Double]) { self.values = values }

    func next() -> Double {
        lock.lock()
        defer { lock.unlock() }
        guard !values.isEmpty else { return 0 }
        let value = values[min(index, values.count - 1)]
        index += 1
        return value
    }
}

/// Records every wait the loop asks its injected `sleep` for.
private final class WaitRecorder: @unchecked Sendable {
    private let lock = NSLock()
    private var storage: [TimeInterval] = []

    func append(_ wait: TimeInterval) {
        lock.lock()
        storage.append(wait)
        lock.unlock()
    }

    var waits: [TimeInterval] {
        lock.lock()
        defer { lock.unlock() }
        return storage
    }

    var count: Int {
        lock.lock()
        defer { lock.unlock() }
        return storage.count
    }
}

/// Ordered callback evidence from the real `stream()` invocations.
private final class CallbackLog: @unchecked Sendable {
    private let lock = NSLock()
    private var kindsStorage: [String] = []
    private var errorsStorage: [String] = []
    private var framesStorage = 0

    func record(_ kind: String) {
        lock.lock()
        kindsStorage.append(kind)
        lock.unlock()
    }

    func recordError(_ message: String) {
        lock.lock()
        kindsStorage.append("error")
        errorsStorage.append(message)
        lock.unlock()
    }

    func recordFrame() {
        lock.lock()
        framesStorage += 1
        lock.unlock()
    }

    var eventKinds: [String] {
        lock.lock()
        defer { lock.unlock() }
        return kindsStorage
    }

    var errors: [String] {
        lock.lock()
        defer { lock.unlock() }
        return errorsStorage
    }

    var frameCount: Int {
        lock.lock()
        defer { lock.unlock() }
        return framesStorage
    }

    var connectedCount: Int {
        lock.lock()
        defer { lock.unlock() }
        return kindsStorage.filter { $0 == "connected" }.count
    }

    var endedCount: Int {
        lock.lock()
        defer { lock.unlock() }
        return kindsStorage.filter { $0 == "ended" }.count
    }
}

/// The accepted-cursor holder the tests own (FleetStore owns acceptance in
/// production; at this transport seam the closure simply reads it).
private final class CursorHolder: @unchecked Sendable {
    private let lock = NSLock()
    private var value: SSECursor?

    init(_ value: SSECursor? = nil) { self.value = value }

    var cursor: SSECursor? {
        lock.lock()
        defer { lock.unlock() }
        return value
    }

    func set(rev: UInt64, epoch: String?) {
        lock.lock()
        value = SSECursor(rev: rev, epoch: epoch)
        lock.unlock()
    }
}

/// Set once the `stream()` invocation has actually returned.
private final class ReturnFlag: @unchecked Sendable {
    private let lock = NSLock()
    private var done = false

    var isDone: Bool {
        lock.lock()
        defer { lock.unlock() }
        return done
    }

    func mark() {
        lock.lock()
        done = true
        lock.unlock()
    }
}

/// One-way latch for deterministic test sequencing (release a blocked
/// injected step exactly once the test has set up the state it needs).
private final class LatchedGate: @unchecked Sendable {
    private let lock = NSLock()
    private var opened = false

    var isOpen: Bool {
        lock.lock()
        defer { lock.unlock() }
        return opened
    }

    func open() {
        lock.lock()
        opened = true
        lock.unlock()
    }
}

/// Per-host scripted `/events` fixture for the ACTUAL `stream()` byte path
/// (same shape as the suite's `HeartbeatURLProtocol`: lock-guarded statics,
/// per-instance serial queue + timer that `stopLoading()` tears down). Each
/// script step maps to ONE attempt; a record is kept per request (host,
/// `Last-Event-ID`, `Corral-Epoch`) and the concurrent-attempt high-water
/// mark proves single-flight.
private final class StreamRetryLoopURLProtocol: URLProtocol {
    enum Step {
        /// 200 headers + immediate EOF with ZERO body bytes.
        case headersOnlyEOF
        /// 200 + one line + immediate EOF.
        case lineThenEOF(String)
        /// 200 + the given lines, then immediate EOF.
        case linesThenEOF([String])
        /// 200 + one line, then `seconds` of silence, then a clean EOF.
        case lineThenSilence(String, seconds: TimeInterval)
        /// 200 + `count` comment keep-alive lines spaced `interval`, then EOF.
        case commentsThenEOF(interval: TimeInterval, count: Int)
        /// 200 + `count` framed deltas spaced `interval`, then EOF.
        case framesThenEOF(interval: TimeInterval, count: Int)
        /// 200 + immediate EOF carrying a non-200 status.
        case non200(Int)
        /// Transport failure with the given code (no response).
        case fail(URLError.Code)
        /// 200 acked, no bytes, never finishes (cancellation fixture).
        case hold
    }

    struct RequestRecord {
        let host: String
        let lastEventID: String?
        let epoch: String?
    }

    private static let frameText = "event: delta\ndata: {\"rev\":1}\n\n"

    private static let lock = NSLock()
    private static var scriptsStorage: [String: [Step]] = [:]
    private static var scriptIndexesStorage: [String: Int] = [:]
    private static var recordsStorage: [String: [RequestRecord]] = [:]
    private static var activeStorage = 0
    private static var highWaterStorage = 0

    static func reset() {
        lock.lock()
        scriptsStorage = [:]
        scriptIndexesStorage = [:]
        recordsStorage = [:]
        activeStorage = 0
        highWaterStorage = 0
        lock.unlock()
    }

    static func script(_ steps: [Step], forHost host: String) {
        lock.lock()
        scriptsStorage[host] = steps
        scriptIndexesStorage[host] = 0
        lock.unlock()
    }

    static func requests(forHost host: String) -> [RequestRecord] {
        lock.lock()
        defer { lock.unlock() }
        return recordsStorage[host] ?? []
    }

    static var concurrentHighWater: Int {
        lock.lock()
        defer { lock.unlock() }
        return highWaterStorage
    }

    private static func nextStep(forHost host: String) -> Step {
        lock.lock()
        defer { lock.unlock() }
        let steps = scriptsStorage[host] ?? []
        let index = scriptIndexesStorage[host] ?? 0
        scriptIndexesStorage[host] = index + 1
        guard !steps.isEmpty else { return .hold }
        return steps[min(index, steps.count - 1)]
    }

    private static func record(_ request: URLRequest, forHost host: String) {
        lock.lock()
        defer { lock.unlock() }
        recordsStorage[host, default: []].append(
            RequestRecord(host: host,
                          lastEventID: request.value(forHTTPHeaderField: "Last-Event-ID"),
                          epoch: request.value(forHTTPHeaderField: "Corral-Epoch")))
    }

    private static func enterActive() {
        lock.lock()
        activeStorage += 1
        highWaterStorage = max(highWaterStorage, activeStorage)
        lock.unlock()
    }

    private static func exitActive() {
        lock.lock()
        activeStorage = max(0, activeStorage - 1)
        lock.unlock()
    }

    private let queue = DispatchQueue(label: "corral.g452.retry.protocol")
    private var timer: DispatchSourceTimer?
    private var stopped = false
    private var countedActive = false

    override class func canInit(with request: URLRequest) -> Bool { true }
    override class func canonicalRequest(for request: URLRequest) -> URLRequest { request }

    override func startLoading() {
        // SAFETY: the request always carries the fixture URL.
        guard let url = request.url, let host = url.host else {
            client?.urlProtocol(self, didFailWithError: URLError(.badURL))
            return
        }
        Self.enterActive()
        countedActive = true
        Self.record(request, forHost: host)
        switch Self.nextStep(forHost: host) {
        case .headersOnlyEOF:
            deliverHeaders(url: url)
            finish()
        case .lineThenEOF(let line):
            deliverHeaders(url: url)
            write(line + "\n")
            finish()
        case .linesThenEOF(let lines):
            deliverHeaders(url: url)
            for line in lines { write(line + "\n") }
            finish()
        case .lineThenSilence(let line, let seconds):
            deliverHeaders(url: url)
            write(line + "\n")
            queue.asyncAfter(deadline: .now() + seconds) { [weak self] in
                guard let self, !self.stopped else { return }
                self.finish()
            }
        case .commentsThenEOF(let interval, let count):
            deliverHeaders(url: url)
            write(": keep-alive\n")
            startTimer(interval: interval, writes: max(0, count - 1)) { [weak self] in
                self?.write(": keep-alive\n")
            } onExhausted: { [weak self] in
                self?.finish()
            }
        case .framesThenEOF(let interval, let count):
            deliverHeaders(url: url)
            write(Self.frameText)
            startTimer(interval: interval, writes: max(0, count - 1)) { [weak self] in
                self?.write(Self.frameText)
            } onExhausted: { [weak self] in
                self?.finish()
            }
        case .non200(let status):
            // SAFETY: fixed HTTPURLResponse construction from the request's own URL.
            let response = HTTPURLResponse(
                url: url, statusCode: status, httpVersion: "HTTP/1.1", headerFields: nil)!
            client?.urlProtocol(self, didReceive: response, cacheStoragePolicy: .notAllowed)
            finish()
        case .fail(let code):
            releaseActive()
            client?.urlProtocol(self, didFailWithError: URLError(code))
        case .hold:
            deliverHeaders(url: url)
        }
    }

    override func stopLoading() {
        releaseActive()
        queue.async { [weak self] in
            guard let self else { return }
            self.stopped = true
            self.timer?.cancel()
            self.timer = nil
        }
    }

    private func deliverHeaders(url: URL) {
        // SAFETY: fixed HTTPURLResponse construction from the request's own URL.
        let response = HTTPURLResponse(
            url: url, statusCode: 200, httpVersion: "HTTP/1.1",
            headerFields: ["Content-Type": "text/event-stream"])!
        client?.urlProtocol(self, didReceive: response, cacheStoragePolicy: .notAllowed)
    }

    private func write(_ text: String) {
        guard !stopped else { return }
        client?.urlProtocol(self, didLoad: Data(text.utf8))
    }

    private func finish() {
        releaseActive()
        client?.urlProtocolDidFinishLoading(self)
    }

    private func releaseActive() {
        guard countedActive else { return }
        countedActive = false
        Self.exitActive()
    }

    /// Writes `writes` scheduled payloads at `interval`, then fires
    /// `onExhausted` right after the final write (a clean EOF).
    private func startTimer(interval: TimeInterval,
                            writes: Int,
                            onWrite: @escaping () -> Void,
                            onExhausted: @escaping () -> Void) {
        queue.async { [weak self] in
            guard let self, self.timer == nil, !self.stopped else { return }
            var remaining = writes
            let timer = DispatchSource.makeTimerSource(queue: self.queue)
            self.timer = timer
            timer.schedule(deadline: .now() + interval, repeating: interval)
            timer.setEventHandler { [weak self] in
                guard let self, !self.stopped else { return }
                if remaining > 0 {
                    remaining -= 1
                    onWrite()
                }
                if remaining == 0 {
                    self.timer?.cancel()
                    self.timer = nil
                    onExhausted()
                }
            }
            timer.resume()
        }
    }
}
