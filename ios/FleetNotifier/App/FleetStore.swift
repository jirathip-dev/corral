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

    private static let log = Logger(subsystem: "com.corral.fleetnotifier", category: "stream")

    private var streamTask: Task<Void, Never>?
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
        cursorBox.write(lastEventId)
        streamSeen = [:]
        streamTask = Task { [weak self] in
            await client.stream(lastEventId: { [weak self] in
                self?.cursorBox.read()
            }, onEvent: { [weak self] frame in
                self?.ingest(frame)
            })
        }
    }

    /// One frame off the wire. A SINGLE main-actor hop per frame (review
    /// F6): decode happens inside the hop, so frames are enqueued in
    /// arrival order and an error cannot be re-ordered after the good
    /// frame that follows it. Testable without a network (review F5).
    nonisolated func ingest(_ frame: SSEFrame) {
        Task { @MainActor in
            switch CorraldClient.decode(frame) {
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
