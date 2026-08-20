import Foundation
import os
import Observation

/// Thread-safe cursor for the SSE reconnect closure (the stream task reads
/// it off the main actor).
final class CursorBox: @unchecked Sendable {
    private let lock = NSLock()
    private var value: UInt64?

    func read() -> UInt64? {
        lock.lock()
        defer { lock.unlock() }
        return value
    }

    func write(_ newValue: UInt64?) {
        lock.lock()
        defer { lock.unlock() }
        value = newValue
    }
}

/// Fleet read-model store: applies snapshot/delta events from the SSE stream
/// (R2) and reports newly-blocked agents for local notifications.
@MainActor
final class FleetStore: ObservableObject {
    @Published private(set) var agents: [String: Agent] = [:]
    @Published private(set) var lastEventId: UInt64?
    @Published private(set) var connectionState: ConnectionState = .disconnected

    enum ConnectionState: Equatable, Sendable {
        case disconnected
        case connecting
        case connected
        case error(String)
    }

    /// Called with the newly-blocked agent's id when an agent transitions
    /// into a blocked waiting state (or its prompt_hash changes while
    /// blocked) — the local-notification hook.
    var onNewlyBlocked: (@MainActor @Sendable (String) -> Void)?

    /// Called once when an agent transitions INTO done (not on re-upserts
    /// while staying done) — the completion-notification hook (D16).
    var onNewlyDone: (@MainActor @Sendable (String) -> Void)?

    /// Review F2: the decode-failure hook — AppModel routes this into
    /// the dismissible, text-selectable banner so the reason is READABLE
    /// on device, where the acceptance gate runs.
    var onDecodeFailure: (@MainActor @Sendable (String) -> Void)?

    /// #92: the connection-error hook — AppModel routes this into the SAME
    /// dismissible, text-selectable banner as decode failures, so a refused
    /// or unreachable host is READABLE on device instead of an endless
    /// spinner.
    var onConnectionError: (@MainActor @Sendable (String) -> Void)?

    /// Review F2: fired when the stream re-establishes a 200 — an idle
    /// fleet emits NO frames (keep-alives are comments, never framed), so
    /// `apply()` would never run to clear a stale `.error` indicator.
    var onConnected: (@MainActor @Sendable () -> Void)?

    private static let log = Logger(subsystem: "com.corral.fleetnotifier", category: "stream")

    private var streamTask: Task<Void, Never>?
    /// Review F3: bumped on every `connect()`. Hop closures capture it, so
    /// a report from a PREVIOUS stream cannot land after `disconnect()` —
    /// or worse, after a NEW connect — and flip the state back.
    private var connectionGeneration = 0
    /// Review F4: last reported connection-error reason. The retry ladder
    /// re-raises every attempt (≤30s cadence); report only on change so a
    /// user-dismissed banner is not re-asserted forever and the log does
    /// not spam.
    private var lastConnectionErrorReason: String?
    /// Blocked-transition shadow for the live stream (was a closure-local;
    /// stored so `ingest` is a plain testable method — review F5).
    private var streamSeen: [String: WaitingOn] = [:]
    private let cursorBox = CursorBox()
    /// Shadow of last-seen agent states for done-transition detection.
    private var previousStates: [String: AgentState] = [:]

    // MARK: - Application

    func apply(_ event: FleetEvent) {
        switch event {
        case .snapshot(let snapshot):
            agents = snapshot.agents
            lastEventId = snapshot.rev
            cursorBox.write(snapshot.rev)
        case .delta(let delta):
            var next = agents
            for agent in delta.upd {
                next[agent.agentId] = agent
            }
            for id in delta.del {
                next.removeValue(forKey: id)
            }
            agents = next
            lastEventId = delta.rev
            cursorBox.write(delta.rev)
        }
        connectionState = .connected
        trackDone(event)
    }

    /// Diff-aware done detection: fire once per transition INTO done
    /// (staying done re-upserts nothing). Shadowed locally, so a delta
    /// reconnect cannot double-notify. A full snapshot replay (cold start /
    /// stale cursor) seeds only the shadow — it must NOT fire a completion
    /// for every already-done agent (F7: cold-start done storm).
    private func trackDone(_ event: FleetEvent) {
        switch event {
        case .snapshot(let snapshot):
            for (id, agent) in snapshot.agents {
                previousStates[id] = agent.state
            }
        case .delta(let delta):
            var transitioned: [String] = []
            for agent in delta.upd {
                if agent.state == .done, previousStates[agent.agentId] != .done {
                    transitioned.append(agent.agentId)
                }
                previousStates[agent.agentId] = agent.state
            }
            for id in delta.del {
                previousStates.removeValue(forKey: id)
            }
            for id in transitioned {
                onNewlyDone?(id)
            }
        }
    }

    /// Diff-aware blocking detection: notify when an agent becomes blocked
    /// on a NEW prompt (state→blocked, or prompt_hash changed while
    /// blocked). Idempotent per prompt_hash.
    func apply(_ event: FleetEvent, previous: inout [String: WaitingOn]) {
        let before = previous
        switch event {
        case .snapshot(let snapshot):
            for (id, agent) in snapshot.agents {
                previous[id] = agent.waitingOn
            }
        case .delta(let delta):
            for agent in delta.upd {
                previous[agent.agentId] = agent.waitingOn
            }
            for id in delta.del {
                previous.removeValue(forKey: id)
            }
        }
        switch event {
        case .snapshot(let snapshot):
            for (id, agent) in snapshot.agents {
                guard let waiting = agent.waitingOn else { continue }
                let wasBlocked = before[id]?.promptHash == waiting.promptHash
                if !wasBlocked {
                    onNewlyBlocked?(id)
                }
            }
        case .delta(let delta):
            for agent in delta.upd {
                guard let waiting = agent.waitingOn else { continue }
                let wasBlocked = before[agent.agentId]?.promptHash == waiting.promptHash
                if !wasBlocked {
                    onNewlyBlocked?(agent.agentId)
                }
            }
        }
        apply(withoutDiff: event)
    }

    private func apply(withoutDiff event: FleetEvent) {
        switch event {
        case .snapshot(let snapshot):
            agents = snapshot.agents
            lastEventId = snapshot.rev
            cursorBox.write(snapshot.rev)
        case .delta(let delta):
            var next = agents
            for agent in delta.upd { next[agent.agentId] = agent }
            for id in delta.del { next.removeValue(forKey: id) }
            agents = next
            lastEventId = delta.rev
            cursorBox.write(delta.rev)
        }
        connectionState = .connected
        trackDone(event)
    }

    func agent(_ id: String) -> Agent? {
        agents[id]
    }

    /// Remove a target immediately after a typed stale-agent refusal. The
    /// subsequent snapshot/SSE update may re-add a current identity, but the
    /// old row cannot keep rendering usable controls during the refresh.
    func removeAgent(_ id: String) {
        agents.removeValue(forKey: id)
        previousStates.removeValue(forKey: id)
        streamSeen.removeValue(forKey: id)
    }

    // NOTE: the pre-D25 `blockedAgents`/`sortedAgents` accessors were
    // removed with the board rework — ordering now lives ONLY in
    // `BoardModel.ordered` (blocked > working > done > idle > unknown),
    // so no second, contradictory rank can be reached for.

    // MARK: - Streaming

    /// Start (or resume) the SSE stream from the last seen rev. The daemon
    /// responds with a full snapshot when the cursor is too old (R2).
    func connect(client: CorraldClient) {
        guard streamTask == nil else { return }
        connectionState = .connecting
        connectionGeneration += 1
        lastConnectionErrorReason = nil
        // #91: a cursor is only valid while the store holds the state it is
        // a delta-base for — resetDevice() wipes the map but NOT the
        // persisted cursor, so an EMPTY store must not resume one (the
        // daemon would answer deltas-only and the board would stay empty).
        if agents.isEmpty && lastEventId != nil { lastEventId = nil }
        cursorBox.write(lastEventId)
        streamSeen = [:]
        let generation = connectionGeneration
        streamTask = Task { [weak self] in
            await client.stream(lastEventId: { [weak self] in
                self?.cursorBox.read()
            }, onEvent: { [weak self] frame in
                self?.ingest(frame)
            }, onConnected: { [weak self] in
                // The stream callback runs off the main actor; hop once.
                // F3: guard the hop — it must not land after disconnect()
                // (or on a newer connection) and flip the state back.
                Task { @MainActor in
                    guard let self, self.streamTask != nil,
                          self.connectionGeneration == generation else { return }
                    self.noteConnected()
                }
            }, onConnectionError: { [weak self] reason in
                // The stream callback runs off the main actor; hop once.
                // F3: guard the hop — it must not land after disconnect()
                // (or on a newer connection) and re-raise .error + banner.
                Task { @MainActor in
                    guard let self, self.streamTask != nil,
                          self.connectionGeneration == generation else { return }
                    self.noteConnectionError(reason)
                }
            })
        }
    }

    /// #92: visible + diagnosable connection-failure state (never a silent
    /// spinner). Mirrors `noteDecodeFailure`: os.Logger (retrievable from a
    /// detached TestFlight build via Console/sysdiagnose — print is not),
    /// the `.error` connection state, and the callback the owner routes to
    /// the full-width, copyable banner. A later good frame's `apply()`
    /// returns the state to `.connected`, so a transient failure is visible
    /// but not fatal.
    func noteConnectionError(_ reason: String) {
        // F4: the retry ladder re-reports every attempt; only surface on
        // change (first failure, or when the reason differs).
        guard reason != lastConnectionErrorReason else { return }
        lastConnectionErrorReason = reason
        Self.log.error("stream connection error: \(reason, privacy: .public)")
        connectionState = .error("stream disconnected — \(reason)")
        onConnectionError?(reason)
    }

    /// Review F2: the stream re-established a 200 — clear a stale `.error`
    /// (an idle fleet emits no frames, so `apply()` never runs to clear
    /// it). Also ends the F4 dedupe episode: a later failure after a real
    /// recovery is a NEW failure and must report again.
    func noteConnected() {
        lastConnectionErrorReason = nil
        connectionState = .connected
        onConnected?()
    }

    /// One frame off the wire: decode OFF-main (round-3 R-N4 — a large
    /// resnapshot must not become main-thread work), then a single
    /// main-actor hop applies the outcome. Frames still get one
    /// unstructured task each, so cross-frame execution order is not
    /// guaranteed by the language (round-3 R-N3: in practice main-actor
    /// enqueue at equal priority behaves FIFO; a mis-ordered error is
    /// corrected by the next applied frame). Returns the hop so tests
    /// await it deterministically (round-3 R-N5). Testable without a
    /// network (review F5).
    @discardableResult
    nonisolated func ingest(_ frame: SSEFrame) -> Task<Void, Never> {
        let outcome = CorraldClient.decode(frame)
        return Task { @MainActor in
            switch outcome {
            case .event(let event):
                self.apply(event, previous: &self.streamSeen)
            case .ignored:
                break
            case .failed(let reason):
                // #79 defect 2: an undecodable/unrecognized frame used
                // to vanish silently — the spinner spun forever with no
                // diagnostic. Surface it; a later good frame's apply()
                // returns the state to .connected (one torn frame is
                // visible, not fatal to the stream).
                self.noteDecodeFailure(reason)
            }
        }
    }

    /// #79: visible + diagnosable decode-failure state (never a silent
    /// spinner). Review F2: os.Logger (retrievable from a detached
    /// TestFlight build via Console/sysdiagnose — print is not), plus a
    /// callback the owner routes to the full-width, copyable banner.
    func noteDecodeFailure(_ reason: String) {
        Self.log.error("frame decode failed: \(reason, privacy: .public)")
        connectionState = .error("stream frame undecodable — \(reason)")
        onDecodeFailure?(reason)
    }

    /// Backgrounded = no connection (D5). Last-Event-ID is persisted by the
    /// owner, so `connect` resumes without a full snapshot when fresh.
    func disconnect() {
        streamTask?.cancel()
        streamTask = nil
        connectionState = .disconnected
    }

    func reset() {
        disconnect()
        agents = [:]
        lastEventId = nil
        cursorBox.write(nil)
        previousStates = [:]
        connectionState = .disconnected
    }

    /// Demo mode: seed the store directly (no daemon).
    func seedDemo(agents: [String: Agent], rev: UInt64) {
        self.agents = agents
        lastEventId = rev
        cursorBox.write(rev)
        connectionState = .disconnected
    }

    /// Demo transition: replace one agent record in place.
    func upsertDemo(_ agent: Agent) {
        var next = agents
        next[agent.agentId] = agent
        agents = next
    }

    func persistCursor() {
        if let lastEventId {
            UserDefaults.standard.set(String(lastEventId), forKey: "fleetnotifier.lastEventId")
        }
    }

    func restoreCursor() {
        if let raw = UserDefaults.standard.string(forKey: "fleetnotifier.lastEventId"),
           let rev = UInt64(raw) {
            lastEventId = rev
            cursorBox.write(rev)
        }
    }
}
