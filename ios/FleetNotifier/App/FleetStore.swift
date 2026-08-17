import Foundation
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

    private var streamTask: Task<Void, Never>?
    private let cursorBox = CursorBox()

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
    }

    func agent(_ id: String) -> Agent? {
        agents[id]
    }

    var blockedAgents: [Agent] {
        agents.values.filter { $0.isBlocked }
            .sorted { $0.ts > $1.ts }
    }

    var sortedAgents: [Agent] {
        agents.values.sorted { a, b in
            let rank: (Agent) -> Int = {
                switch $0.state {
                case .blocked: return 0
                case .working: return 1
                case .idle: return 2
                case .done: return 3
                case .unknown: return 4
                }
            }
            if rank(a) == rank(b) { return a.ts > b.ts }
            return rank(a) < rank(b)
        }
    }

    // MARK: - Streaming

    /// Start (or resume) the SSE stream from the last seen rev. The daemon
    /// responds with a full snapshot when the cursor is too old (R2).
    func connect(client: CorraldClient) {
        guard streamTask == nil else { return }
        connectionState = .connecting
        cursorBox.write(lastEventId)
        var seen: [String: WaitingOn] = [:]
        streamTask = Task { [weak self] in
            await client.stream(lastEventId: { [weak self] in
                self?.cursorBox.read()
            }, onEvent: { [weak self] frame in
                guard let self else { return }
                guard let event = CorraldClient.decode(frame) else { return }
                Task { @MainActor in
                    self.apply(event, previous: &seen)
                }
            })
        }
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
