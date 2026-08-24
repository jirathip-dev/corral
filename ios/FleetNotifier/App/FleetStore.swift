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
    /// Last successful bounded read_tail result per agent. This is deliberately
    /// client-side display state, not part of the SSE read model.
    @Published private(set) var tails: [String: [String]] = [:]
    /// #167: per-agent live-tail pane (blocks + four-state machine). The
    /// daemon now serves `{lines, blocks}` additively; the block renderer
    /// reads this pane, the legacy text surface reads `tails`.
    @Published private(set) var tailPanes: [String: TailPane] = [:]
    /// Lazy, bounded per-agent full-chat panes. Never prefetched: a pane is
    /// created only when the detail surface asks the daemon for a page.
    @Published private(set) var transcripts: [String: TranscriptPane] = [:]
    @Published private(set) var lastEventId: UInt64?
    @Published private(set) var connectionState: ConnectionState = .disconnected
    /// #166 review F2: client-side state-entered wall clock (epoch millis).
    /// Seeded from `agent.ts` at first sight; updated ONLY when `state`
    /// actually changes on a delta/snapshot, never on title/reason churn, so
    /// a mid-state label/title update does not reset the duration. The seed
    /// may be later than the true state-entry time (see `ios/README.md`).
    @Published private(set) var stateEnteredAt: [String: UInt64] = [:]

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
    private let cursorDefaults: UserDefaults
    /// Shadow of last-seen agent states for done-transition detection.
    private var previousStates: [String: AgentState] = [:]
    /// Monotonic pane-generation source; each reset mints a fresh value so a
    /// late response from an older page cannot fold into the current pane.
    private var transcriptGeneration: UInt64 = 0

    init(defaults: UserDefaults = .standard) {
        self.cursorDefaults = defaults
    }

    // MARK: - Application

    private func accepts(_ event: FleetEvent) -> Bool {
        let current = lastEventId ?? 0
        switch event {
        case .snapshot(let snapshot):
            // Recovery snapshots may race a newer SSE delta. Equal revisions
            // are safe replacements; older snapshots are stale responses.
            return snapshot.rev >= current
        case .delta(let delta):
            // A duplicate/late delta must not mutate records even if its
            // cursor is already behind the current state.
            return delta.rev > current
        }
    }

    func apply(_ event: FleetEvent) {
        // #166 review F2: every apply path must track `stateEnteredAt`. The
        // snapshot/refresh path (AppModel → `fleet.apply`) and the streaming
        // path (`apply(_:previous:)`) both converge here, so the client-side
        // state clock is seeded on first sight and re-stamped on state
        // change regardless of which entry point delivered the event.
        apply(withoutDiff: event)
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
        guard accepts(event) else { return }
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
        guard accepts(event) else { return }
        switch event {
        case .snapshot(let snapshot):
            let old = agents
            agents = snapshot.agents
            tails = tails.filter { snapshot.agents[$0.key] != nil }
            transcripts = transcripts.filter { snapshot.agents[$0.key] != nil }
            lastEventId = snapshot.rev
            cursorBox.write(snapshot.rev)
            updateStateEnteredAt(old: old, new: snapshot.agents)
        case .delta(let delta):
            var next = agents
            for agent in delta.upd { next[agent.agentId] = agent }
            for id in delta.del { next.removeValue(forKey: id) }
            for id in delta.del {
                tails.removeValue(forKey: id)
                transcripts.removeValue(forKey: id)
            }
            let old = agents
            agents = next
            lastEventId = delta.rev
            cursorBox.write(delta.rev)
            updateStateEnteredAt(old: old, new: next)
        }
        connectionState = .connected
        trackDone(event)
    }

    /// Seed/advance `stateEnteredAt` from a state transition only. An agent
    /// first seen stores its current `ts` as the (possibly late) seed; a
    /// state change re-stamps `ts`; an unchanged state keeps the stored value
    /// so a reason/title re-write cannot reset the clock. Deleted ids are
    /// pruned.
    private func updateStateEnteredAt(old: [String: Agent], new: [String: Agent]) {
        var next = stateEnteredAt
        for (id, agent) in new {
            guard let previous = old[id] else {
                next[id] = agent.ts
                continue
            }
            if previous.state != agent.state {
                next[id] = agent.ts
            }
        }
        let ids = Set(new.keys)
        next = next.filter { ids.contains($0.key) }
        stateEnteredAt = next
    }

    func agent(_ id: String) -> Agent? {
        agents[id]
    }

    func tail(for id: String) -> [String]? {
        tails[id]
    }

    /// #167: the segmented blocks for the live tail (nil = never loaded).
    func tailBlocks(for id: String) -> [TranscriptBlock]? {
        tailPanes[id]?.blocks
    }

    /// #167: the full live-tail pane (blocks + state).
    func tailPane(for id: String) -> TailPane? {
        tailPanes[id]
    }

    func transcript(_ id: String) -> TranscriptPane? {
        transcripts[id]
    }

    struct TranscriptFetch: Equatable, Sendable {
        let generation: UInt64
        let cursor: String?
    }

    /// Create/load the pane and mark one fetch in flight. `newest` resets
    /// under a fresh generation; `cursor` is only meaningful when extending
    /// an existing walk. Returns nil when the agent is gone, a fetch is
    /// already in flight, or no pane exists for an older-page request.
    func prepareTranscriptFetch(agent id: String, cursor: String?,
                                newest: Bool, autoReload: Bool = false) -> TranscriptFetch? {
        guard agents[id] != nil else { return nil }
        if autoReload {
            guard let pane = transcripts[id], pane.loading,
                  pane.autoReloaded, pane.generation > 0 else {
                return nil
            }
            var next = pane
            next.beginFetch()
            transcripts[id] = next
            return TranscriptFetch(generation: next.generation, cursor: nil)
        }
        if let pane = transcripts[id] {
            guard !pane.loading else { return nil }
            guard newest || pane.generation > 0 else { return nil }
        } else {
            guard newest else { return nil }
        }
        var pane = transcripts[id] ?? TranscriptPane()
        if newest || pane.generation == 0 {
            transcriptGeneration &+= 1
            pane.reset(generation: transcriptGeneration, keepAutoReloaded: false)
        } else {
            pane.beginFetch()
        }
        transcripts[id] = pane
        return TranscriptFetch(generation: pane.generation, cursor: cursor)
    }

    @discardableResult
    func foldTranscriptPage(_ page: TranscriptPage, for id: String,
                            generation: UInt64) -> Bool {
        guard agents[id] != nil,
              var pane = transcripts[id],
              pane.generation == generation else {
            return false
        }
        pane.apply(page)
        transcripts[id] = pane
        return true
    }

    enum TranscriptFoldOutcome: Equatable, Sendable {
        case applied
        case dropped
        case needsReload
        case notGranted
    }

    func foldTranscriptFailure(_ failure: TranscriptFailure, for id: String,
                               generation: UInt64) -> TranscriptFoldOutcome {
        guard agents[id] != nil,
              var pane = transcripts[id],
              pane.generation == generation else {
            return .dropped
        }
        guard !failure.isStaleCursor || !pane.autoReloaded else {
            pane.apply(failure)
            transcripts[id] = pane
            return .applied
        }
        if failure.isStaleCursor {
            transcriptGeneration &+= 1
            pane.reset(generation: transcriptGeneration, keepAutoReloaded: true)
            transcripts[id] = pane
            return .needsReload
        }
        pane.apply(failure)
        transcripts[id] = pane
        return failure.isNotGranted ? .notGranted : .applied
    }

    /// Surface a local "cannot fetch" failure without ever issuing network
    /// work (for example, the device is not registered or demo has no store).
    func noteTranscriptFailure(_ failure: TranscriptFailure, for id: String) {
        guard agents[id] != nil else { return }
        var pane = transcripts[id] ?? TranscriptPane()
        if pane.generation == 0 {
            transcriptGeneration &+= 1
            pane.reset(generation: transcriptGeneration, keepAutoReloaded: false)
        }
        pane.apply(failure)
        transcripts[id] = pane
    }

    /// A cancelled page fetch must not leave the pane permanently loading:
    /// clear the in-flight mark so an explicit reload can reset the walk.
    func cancelTranscriptFetch(agent id: String, generation: UInt64) {
        guard agents[id] != nil,
              var pane = transcripts[id],
              pane.generation == generation else {
            return
        }
        pane.loading = false
        transcripts[id] = pane
    }

    /// Store the daemon's bounded tail result (lines + #167 blocks) with a
    /// small client-side defense in depth for malformed/future servers.
    func rememberTail(_ lines: [String], for id: String) {
        rememberTail(lines, blocks: [], for: id)
    }

    /// #167 overload: also fold the segmented blocks + clear the loading/
    /// error flags (the live tail is now "loaded", never a spinner).
    func rememberTail(_ lines: [String], blocks: [TranscriptBlock], for id: String) {
        let maxLines = 200
        let maxBytes = 32 * 1024
        var bounded: [String] = []
        var bytes = 0
        for line in lines.prefix(maxLines) {
            let lineBytes = line.utf8.count + (bounded.isEmpty ? 0 : 1)
            guard bytes + lineBytes <= maxBytes else { break }
            bounded.append(line)
            bytes += lineBytes
        }
        tails[id] = bounded
        var pane = tailPanes[id] ?? TailPane()
        pane.apply(blocks, lines: bounded)
        tailPanes[id] = pane
    }

    /// Mark a live-tail fetch in flight (the four-state machine's loading).
    func prepareTailFetch(agent id: String) {
        guard agents[id] != nil else { return }
        var pane = tailPanes[id] ?? TailPane()
        pane.beginFetch()
        tailPanes[id] = pane
    }

    /// Fold a live-tail failure (e.g. a hard timeout → error + Retry).
    func foldTailFailure(_ failure: TranscriptFailure, for id: String) {
        guard agents[id] != nil else { return }
        var pane = tailPanes[id] ?? TailPane()
        pane.apply(failure)
        tailPanes[id] = pane
    }

    /// #167: cleared when the fetch is cancelled so the four-state machine
    /// does not stay stuck on loading.
    func cancelTailFetch(agent id: String) {
        guard agents[id] != nil, var pane = tailPanes[id] else { return }
        pane.loading = false
        tailPanes[id] = pane
    }

    /// Remove a target immediately after a typed stale-agent refusal. The
    /// subsequent snapshot/SSE update may re-add a current identity, but the
    /// old row cannot keep rendering usable controls during the refresh.
    func removeAgent(_ id: String) {
        agents.removeValue(forKey: id)
        tails.removeValue(forKey: id)
        tailPanes.removeValue(forKey: id)
        transcripts.removeValue(forKey: id)
        previousStates.removeValue(forKey: id)
        streamSeen.removeValue(forKey: id)
        stateEnteredAt.removeValue(forKey: id)
    }

    // NOTE: the pre-D25 `blockedAgents`/`sortedAgents` accessors were
    // removed with the board rework — ordering now lives ONLY in
    // `BoardModel.ordered` (blocked > done > working > idle > unknown),
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
                // The stream callback can race disconnect/demo after the
                // frame has been decoded. Pass the connection identity into
                // the main-actor hop so a late frame cannot overwrite the
                // replacement fleet.
                self?.ingest(frame, generation: generation)
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
    nonisolated func ingest(_ frame: SSEFrame,
                            generation: Int? = nil) -> Task<Void, Never> {
        let outcome = CorraldClient.decode(frame)
        return Task { @MainActor in
            if let generation {
                guard self.streamTask != nil,
                      self.connectionGeneration == generation else { return }
            }
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
        tails = [:]
        tailPanes = [:]
        transcripts = [:]
        lastEventId = nil
        cursorBox.write(nil)
        // A reset abandons the delta base; retaining it would let a later
        // live connection resume from demo or otherwise unrelated state.
        cursorDefaults.removeObject(forKey: "fleetnotifier.lastEventId")
        previousStates = [:]
        stateEnteredAt = [:]
        connectionState = .disconnected
    }

#if DEBUG
    /// Debug-only demo mode: seed the store directly (no daemon).
    func seedDemo(agents: [String: Agent], rev: UInt64) {
        let old = self.agents
        self.agents = agents
        tails = tails.filter { agents[$0.key] != nil }
        transcripts = transcripts.filter { agents[$0.key] != nil }
        lastEventId = rev
        cursorBox.write(rev)
        updateStateEnteredAt(old: old, new: agents)
        connectionState = .disconnected
    }

    /// Demo transition: replace one agent record in place.
    func upsertDemo(_ agent: Agent) {
        var next = agents
        next[agent.agentId] = agent
        let old = agents
        agents = next
        updateStateEnteredAt(old: old, new: next)
    }
#endif

    func persistCursor() {
        if let lastEventId {
            cursorDefaults.set(String(lastEventId), forKey: "fleetnotifier.lastEventId")
        }
    }

    func restoreCursor() {
        if let raw = cursorDefaults.string(forKey: "fleetnotifier.lastEventId"),
           let rev = UInt64(raw) {
            lastEventId = rev
            cursorBox.write(rev)
        }
    }
}
