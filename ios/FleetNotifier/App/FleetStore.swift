import Foundation
import os
import Observation

/// Thread-safe cursor for the SSE reconnect closure (the stream task reads
/// it off the main actor).
struct SSECursor: Sendable {
    let rev: UInt64
    let epoch: String?
}

final class CursorBox: @unchecked Sendable {
    private let lock = NSLock()
    private var value: SSECursor?

    func read() -> SSECursor? {
        lock.lock()
        defer { lock.unlock() }
        return value
    }

    func write(rev: UInt64?, epoch: String?) {
        lock.lock()
        defer { lock.unlock() }
        value = rev.map { SSECursor(rev: $0, epoch: epoch) }
    }
}

/// #425: the client-side liveness policy for ONE host's SSE stream — an
/// explicit, documented inactivity budget plus the (bounded) watchdog poll
/// cadence.
///
/// Why a client-side budget exists at all: the daemon already emits an SSE
/// keep-alive COMMENT every 15s on an idle stream (`src/api/mod.rs`
/// `KEEPALIVE`), and comment lines are never framed, so frames alone cannot
/// tell a healthy idle stream from a silently wedged (half-open) one. The
/// ONLY honest liveness signal is received BYTES. The budget is half of the
/// transport's own configured request-inactivity timeout
/// (`URLSessionConfiguration.timeoutIntervalForRequest` — 60s for the app's
/// session), i.e. **30s by default: two missed 15s keep-alives**. The
/// transport timeout itself is unchanged and remains the outer layer (Apple
/// documents it as reset by received data); the watchdog is what preempts
/// it, so a silent connection is detected and replaced deterministically
/// instead of lingering as `.connected`.
struct StreamLiveness: Sendable {
    /// Fraction of the transport's configured inactivity timeout that the
    /// client tolerates before declaring a stream stale.
    static let budgetRatio: Double = 0.5
    /// Upper bound on the watchdog poll cadence: with the app's defaults
    /// (budget 30s) the watchdog polls every 5s, so staleness is exposed and
    /// replaced within budget + 5s — always before the transport's own 60s
    /// inactivity timeout fires.
    static let maxTickInterval: TimeInterval = 5
    /// Floors so a degenerate (zero/negative) configured timeout can never
    /// produce a busy-loop cadence.
    static let minBudget: TimeInterval = 0.05
    static let minTickInterval: TimeInterval = 0.01

    let inactivityBudget: TimeInterval
    let tickInterval: TimeInterval

    /// Explicit form — the owner (or a test) may pin both values.
    init(inactivityBudget: TimeInterval, tickInterval: TimeInterval) {
        self.inactivityBudget = max(Self.minBudget, inactivityBudget)
        self.tickInterval = max(Self.minTickInterval, tickInterval)
    }

    /// Derive the budget from the transport's configured request-inactivity
    /// timeout (see the type doc for the ratio and its rationale).
    init(transportTimeout: TimeInterval) {
        let budget = max(Self.minBudget, transportTimeout * Self.budgetRatio)
        self.init(inactivityBudget: budget,
                  tickInterval: min(Self.maxTickInterval, budget / 3))
    }
}

/// #425: thread-safe "when did this host's transport last prove itself"
/// clock. The byte path (off the main actor) marks it; the main actor reads
/// the age to decide staleness. Monotonic (`DispatchTime`), never
/// wall-clock, so a clock change can never fabricate liveness or staleness.
final class TransportHeartbeat: @unchecked Sendable {
    private let lock = NSLock()
    private var lastSignal: DispatchTime

    init(now: DispatchTime = .now()) {
        lastSignal = now
    }

    /// Record a transport signal: received bytes, a 200 stream ack, or the
    /// dispatch of a new stream attempt.
    func mark(now: DispatchTime = .now()) {
        lock.lock()
        defer { lock.unlock() }
        lastSignal = now
    }

    /// Seconds since the last transport signal.
    func age(now: DispatchTime = .now()) -> TimeInterval {
        lock.lock()
        defer { lock.unlock() }
        return TimeInterval(now.uptimeNanoseconds &- lastSignal.uptimeNanoseconds) / 1.0e9
    }
}

/// Fleet read-model store: applies snapshot/delta events from the SSE stream
/// (R2) and reports agent state transitions for the notification hooks.
///
/// #354 L2 notification transitions (herdr 0.8.2 vocabulary, spec amendment
/// 09-02):
/// - started: episode begins — a delta moves the agent INTO `working` from
///   idle/unknown/done (or first sight). Blocked→working is a resume inside
///   the same episode and fires nothing.
/// - blocked: a delta moves the agent INTO `blocked` (deduped while it stays
///   blocked).
/// - finished: the episode ends — a delta moves the agent OUT of an active
///   state into `idle` (working→idle = the v2 "done", fires ONCE per episode,
///   deduped until the agent starts again). A wire `done` (transitional
///   daemon) is treated as the same episode end.
/// Full snapshots (cold start / stale-cursor recovery) seed the shadow state
/// and NEVER fire — no catch-up storm, no badge-on-foreground.
@MainActor
final class FleetStore: ObservableObject {
    @Published private(set) var agents: [String: Agent] = [:]
    /// Last successful bounded read_tail result per agent. This is deliberately
    /// client-side display state, not part of the SSE read model.
    @Published private(set) var tails: [String: [String]] = [:]
    /// #167: per-agent live-tail pane (blocks + four-state machine). The
    /// daemon now serves `{lines, blocks}` additively; the recents sheet
    /// reads this pane, the legacy text surface reads `tails`.
    @Published private(set) var tailPanes: [String: TailPane] = [:]
    @Published private(set) var lastEventId: UInt64?
    /// Daemon lifetime that owns `lastEventId`; nil is a legacy daemon.
    private(set) var lastEventEpoch: String?
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

    /// Fired when an agent episode starts (into working; spec "start").
    var onStarted: (@MainActor @Sendable (String) -> Void)?
    /// Fired when an agent enters blocked (spec "blocked").
    var onBlocked: (@MainActor @Sendable (String) -> Void)?
    /// Fired ONCE when an active agent's episode ends (working→idle; spec
    /// "done", deduped until the agent starts again).
    var onFinished: (@MainActor @Sendable (String) -> Void)?

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

    /// #399 B4/C1: when a pinned host identity is set, any agent record in
    /// an applied frame whose NON-NIL host differs from the pin is a
    /// feed-integrity failure — the frame is REJECTED (fail closed) and
    /// the hook fires so the owner can surface the mismatch and stop the
    /// stream. Host-less records (a transitional pre-#399 daemon) are
    /// tolerated; the URL-level `/host-key` check is the continuity gate.
    /// Fired only when `acceptedHostIdentity` is non-nil.
    var onHostIntegrityMismatch: (@MainActor @Sendable () -> Void)?
    /// #397 follow-up: fired AFTER an accepted frame mutated the read
    /// model (snapshot or delta — stream, refresh, or seed). This is the
    /// moment a deferred notification tap can learn that its target agent
    /// appeared (or that the board settled without it). Rejected frames
    /// never fire it.
    var onAgentsChanged: (@MainActor () -> Void)?
    /// The pinned host identity the live feed must conform to (nil =
    /// legacy single-host flows without a pin).
    var acceptedHostIdentity: String?

    private static let log = Logger(subsystem: "com.corral.fleetnotifier", category: "stream")

    private var streamTask: Task<Void, Never>?
    /// #425: this host's transport-liveness policy. Re-derived from the
    /// connecting client's own session configuration on every `connect()`
    /// (see `StreamLiveness`); the owner may override it afterwards to pin a
    /// scenario (tests shrink the budget, or suppress the watchdog with a
    /// large tick, to exercise one recovery path deterministically). A
    /// change re-arms the watchdog, so the new cadence takes effect for the
    /// NEXT tick instead of only after the one already scheduled.
    var streamLiveness = StreamLiveness(transportTimeout: 60) {
        didSet {
            guard streamTask != nil else { return }
            startWatchdog(generation: connectionGeneration)
        }
    }
    /// #425: last transport signal for THIS host's stream (bytes, a 200 ack,
    /// or an attempt dispatch). Shared with the byte path off the main actor.
    private let heartbeat = TransportHeartbeat()
    /// #425: the per-host liveness watchdog. Alive only while a stream task
    /// is owned; cancelled by every teardown path (`disconnect`, `reset`,
    /// background stop, host removal).
    private var watchdogTask: Task<Void, Never>?
    /// #425: the client the watchdog replaces a stale stream with (the same
    /// host/session/cursor the owner connected with).
    private var streamClient: CorraldClient?
    /// #425: one replacement per stale episode. A replacement is started by
    /// the refresh path or the watchdog; until it produces its own transport
    /// signal (200 ack / clean end / error) — or its freshness window
    /// expires — neither path stacks a second replacement on top of it.
    private var replacementInFlight = false
    /// Review F3: bumped on every `connect()`. Hop closures capture it, so
    /// a report from a PREVIOUS stream cannot land after `disconnect()` —
    /// or worse, after a NEW connect — and flip the state back.
    /// #400: read externally so the coordinator's per-host reconnect
    /// generations are observable (one independent generation per host).
    private(set) var connectionGeneration = 0

    /// #400: whether a stream task is live (the coordinator uses this to
    /// start exactly one stream per host and to prove host-removal
    /// cancellation terminates that host's stream).
    var isStreaming: Bool { streamTask != nil }
    /// Review F4: last reported connection-error reason. The retry ladder
    /// re-raises every attempt (≤30s cadence); report only on change so a
    /// user-dismissed banner is not re-asserted forever and the log does
    /// not spam.
    private var lastConnectionErrorReason: String?
    private let cursorBox = CursorBox()
    private let cursorDefaults: UserDefaults
    /// Shadow of last-seen agent states for transition detection.
    private var previousStates: [String: AgentState] = [:]
    /// Agents currently inside a work episode (working or blocked).
    private var activeAgents: Set<String> = []

    init(defaults: UserDefaults = .standard) {
        self.cursorDefaults = defaults
    }

    // MARK: - Application

    private func accepts(_ event: FleetEvent, allowsEpochReset: Bool) -> Bool {
        let current = lastEventId ?? 0
        switch event {
        case .snapshot(let snapshot):
            guard let incomingEpoch = snapshot.epoch else {
                // Pre-#450 daemon: retain revision-only compatibility.
                return snapshot.rev >= current
            }
            guard let lastEventEpoch else {
                // A stream snapshot establishes epoch authority even when an
                // upgraded client retained a higher legacy revision. Pull
                // snapshots stay monotonic until the stream anchors it.
                return allowsEpochReset || snapshot.rev >= current
            }
            return incomingEpoch == lastEventEpoch
                ? snapshot.rev >= current
                : allowsEpochReset
        case .delta(let delta):
            if let incomingEpoch = delta.epoch {
                // A delta can never establish or cross an epoch: only a full
                // stream snapshot is an authoritative replacement boundary.
                guard incomingEpoch == lastEventEpoch else { return false }
            }
            // A duplicate/late delta must not mutate records even if its
            // cursor is already behind the current state.
            return delta.rev > current
        }
    }

    /// Generic frame application (stream-independent entry point): monotonic
    /// ONLY. #450 F1: epoch-reset authority is a property of the live STREAM
    /// (`applyStreamFrame`, entered via `ingest`) — an HTTP pull frame (the
    /// stale-agent reconciliation `fetchSnapshot`, a refresh) must never
    /// cross a daemon lifetime, or a late response from a DEAD lifetime
    /// replaces newer stream state and wedges convergence behind a healthy
    /// connection.
    ///
    /// #166 review F2: every apply path must track `stateEnteredAt`. The
    /// snapshot/refresh path (AppModel → `fleet.apply`) and the streaming
    /// path (`ingest`) both converge here, so the client-side state clock
    /// is seeded on first sight and re-stamped on state change regardless
    /// of which entry point delivered the event.
    func apply(_ event: FleetEvent) {
        apply(withoutDiff: event, marksConnected: true, allowsEpochReset: false)
    }

    /// #450: the live-stream frame path. A full snapshot that RODE the SSE
    /// stream is the ONE authoritative epoch boundary — a restarted
    /// daemon's lower/equal-rev snapshot replaces the retained state. Only
    /// `ingest` reaches this method.
    private func applyStreamFrame(_ event: FleetEvent) {
        apply(withoutDiff: event, marksConnected: true, allowsEpochReset: true)
    }

    /// Issue #219: authoritative pull/toolbar refresh application.
    /// Same revision ordering as stream frames (`accepts` — a stale
    /// snapshot response cannot reorder a newer delta), plus the
    /// transition tracking stream frames use, so a refresh that reveals a
    /// newly started agent behaves exactly like a frame.
    /// The SSE stream task is deliberately NOT touched here: it keeps
    /// running and resumes from the newest accepted revision via the
    /// shared cursor at the next reconnect — no duplicate stream tasks.
    /// #425: a refresh snapshot is DATA, never a liveness claim — unlike a
    /// frame that rode the live stream, it must not mark the store
    /// `.connected`. Only the stream's own transport ack (`noteConnected`,
    /// fired on every successful `/events` 200) may, so a snapshot-only
    /// success can never render a dead/offline host as live.
    func applyRefresh(_ snapshot: Snapshot) {
        apply(withoutDiff: .snapshot(snapshot), marksConnected: false,
              allowsEpochReset: false)
    }

    private func apply(withoutDiff event: FleetEvent, marksConnected: Bool,
                       allowsEpochReset: Bool) {
        // #399 B4/C1: fail closed when the frame carries records from a
        // DIFFERENT host identity than the one this store was pinned to.
        // The prior (stale) snapshot stays untouched — nothing is applied.
        if let pinned = acceptedHostIdentity,
           !Self.conformsToPinnedHost(event, pin: pinned) {
            Self.log.error(
                "frame rejected: agent host does not match pinned host identity"
            )
            connectionState = .error("host_identity_mismatch")
            onHostIntegrityMismatch?()
            return
        }
        guard accepts(event, allowsEpochReset: allowsEpochReset) else { return }
        switch event {
        case .snapshot(let snapshot):
            let old = agents
            agents = snapshot.agents
            tails = tails.filter { snapshot.agents[$0.key] != nil }
            lastEventId = snapshot.rev
            lastEventEpoch = snapshot.epoch
            cursorBox.write(rev: snapshot.rev, epoch: snapshot.epoch)
            updateStateEnteredAt(old: old, new: snapshot.agents)
            trackTransitions(.snapshot(snapshot))
        case .delta(let delta):
            var next = agents
            for agent in delta.upd { next[agent.agentId] = agent }
            for id in delta.del { next.removeValue(forKey: id) }
            for id in delta.del {
                tails.removeValue(forKey: id)
            }
            let old = agents
            agents = next
            lastEventId = delta.rev
            lastEventEpoch = delta.epoch
            cursorBox.write(rev: delta.rev, epoch: delta.epoch)
            updateStateEnteredAt(old: old, new: next)
            trackTransitions(.delta(delta))
        }
        // #425: only events that RODE the live stream (ingest frames, and
        // direct apply() seeds that mirror them) may mark the store
        // `.connected`. A pull-refresh snapshot (`applyRefresh`) is applied
        // with marksConnected: false — liveness belongs to the stream's own
        // 200 ack (`noteConnected`), never to a snapshot fetch.
        if marksConnected {
            connectionState = .connected
        }
        // #397 follow-up: an accepted frame mutated the read model — the
        // owning model replays any deferred notification tap whose target
        // may now be present (or may now be provably absent).
        onAgentsChanged?()
    }

    // MARK: - Notification transition tracking (#354 L2)

    private func trackTransitions(_ event: FleetEvent) {
        switch event {
        case .snapshot(let snapshot):
            // Seed only — a full replay must never fire (cold-start rule).
            for (id, agent) in snapshot.agents {
                seedShadow(id, agent.state)
            }
        case .delta(let delta):
            for agent in delta.upd {
                observe(agent)
            }
            for id in delta.del {
                dropShadow(id)
            }
        }
    }

    private func seedShadow(_ id: String, _ state: AgentState) {
        previousStates[id] = state
        if state == .working || state == .blocked {
            activeAgents.insert(id)
        } else {
            activeAgents.remove(id)
        }
    }

    private func dropShadow(_ id: String) {
        previousStates.removeValue(forKey: id)
        activeAgents.remove(id)
    }

    private func observe(_ agent: Agent) {
        let id = agent.agentId
        let previous = previousStates[id]
        if let previous, previous == agent.state { return }

        switch agent.state {
        case .working:
            // Episode start: first sight, or a return from idle/unknown/done.
            // Blocked→working is a resume inside the same episode.
            if previous == nil || !activeAgents.contains(id) {
                onStarted?(id)
            }
            activeAgents.insert(id)
        case .blocked:
            // Entering blocked (from anything else, incl. first sight).
            if previous == nil || previous != .blocked {
                onBlocked?(id)
            }
            activeAgents.insert(id)
        case .idle:
            // Episode end: active → idle fires ONCE (wasActive is cleared,
            // so repeated idle re-upserts never re-fire until a new start).
            if previous != nil, activeAgents.contains(id) {
                onFinished?(id)
            }
            activeAgents.remove(id)
        case .done:
            // Transitional daemon state (herdr 0.8.2 never emits it;
            // finished panes fall back to idle). Same episode-end treatment.
            if previous != nil, activeAgents.contains(id) {
                onFinished?(id)
            }
            activeAgents.remove(id)
        case .unknown:
            activeAgents.remove(id)
        }
        previousStates[id] = agent.state
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

    /// #399 B4/C1: does every agent record in this frame conform to the
    /// pinned host identity? Records WITHOUT a host (transitional
    /// pre-#399 daemon) pass; a record stamped with a DIFFERENT host
    /// fails the whole frame closed.
    static func conformsToPinnedHost(_ event: FleetEvent, pin: String) -> Bool {
        func conforms(_ agent: Agent) -> Bool {
            guard let host = agent.host else { return true }
            return host == pin
        }
        switch event {
        case .snapshot(let snapshot):
            return snapshot.agents.values.allSatisfy(conforms)
        case .delta(let delta):
            return delta.upd.allSatisfy(conforms)
        }
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

    /// Store the daemon's bounded tail result (lines + #167 blocks) with a
    /// small client-side defense in depth for malformed/future servers.
    func rememberTail(_ lines: [String], for id: String) {
        rememberTail(lines, blocks: [], for: id)
    }

    /// #167 overload: also fold the segmented blocks + clear the loading/
    /// error flags (the live tail is now "loaded", never a spinner).
    func rememberTail(_ lines: [String], blocks: [TranscriptBlock], sourceRev: UInt64? = nil, for id: String) {
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
        pane.sourceRev = sourceRev ?? pane.sourceRev
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

    /// Remove a target immediately (stale-agent refresh reconciliation). The
    /// subsequent snapshot/SSE update may re-add a current identity, but the
    /// old row cannot keep rendering during the refresh.
    func removeAgent(_ id: String) {
        agents.removeValue(forKey: id)
        tails.removeValue(forKey: id)
        tailPanes.removeValue(forKey: id)
        previousStates.removeValue(forKey: id)
        activeAgents.remove(id)
        stateEnteredAt.removeValue(forKey: id)
    }

    // MARK: - Streaming

    /// Start (or resume) the SSE stream from the last seen rev. The daemon
    /// responds with a full snapshot when the cursor is too old (R2).
    /// #425: the task's OWN exit clears the handle — when the async stream
    /// owner finishes (for any reason other than `disconnect()`), the
    /// non-nil `streamTask` must not keep refusing later reconnects.
    /// #425: also derives this host's liveness policy from the connecting
    /// transport, marks the attempt as the first heartbeat of its window,
    /// and arms the per-host staleness watchdog.
    func connect(client: CorraldClient) {
        guard streamTask == nil else { return }
        connectionState = .connecting
        connectionGeneration += 1
        lastConnectionErrorReason = nil
        // #425: the policy follows the connecting transport's own configured
        // request-inactivity timeout, and the attempt dispatch is this
        // window's first liveness signal (a replacement therefore always
        // gets a full budget before it can be judged silent).
        streamLiveness = StreamLiveness(
            transportTimeout: client.session.configuration.timeoutIntervalForRequest)
        streamClient = client
        heartbeat.mark()
        // #91: a cursor is only valid while the store holds the state it is
        // a delta-base for — resetDevice() wipes the map but NOT the
        // persisted cursor, so an EMPTY store must not resume one (the
        // daemon would answer deltas-only and the board would stay empty).
        if agents.isEmpty && lastEventId != nil {
            lastEventId = nil
            lastEventEpoch = nil
        }
        cursorBox.write(rev: lastEventId, epoch: lastEventEpoch)
        let generation = connectionGeneration
        // CorraldClient.stream() is a nonisolated async operation on a
        // Sendable value, so its URLSession transport, retry loop, and
        // callbacks are not MainActor-isolated. The store state transitions
        // below still return to MainActor explicitly.
        let cursorBox = self.cursorBox
        streamTask = Task { [weak self, cursorBox] in
            await client.stream(lastEventId: {
                cursorBox.read()
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
            }, onStreamEnded: { [weak self] in
                // #425: a clean server EOF ends the live posture NOW (the
                // backoff wait before the retry is not a live connection).
                // Same generation guard as the other hops, so a late report
                // from an obsolete stream can never revive it.
                Task { @MainActor in
                    guard let self, self.streamTask != nil,
                          self.connectionGeneration == generation else { return }
                    self.noteStreamEnded()
                }
            }, onActivity: { [heartbeat] in
                // #425: received bytes — comment keep-alives included — are
                // THE liveness signal the watchdog measures. Runs off the
                // main actor on the transport's read path; the clock box is
                // lock-guarded.
                heartbeat.mark()
            })
            // #425: the stream owner EXITED. Clear the stale task handle
            // on the main actor so the next refresh/foreground reconnect is
            // not refused by `connect()`'s non-nil guard, and report the
            // honest non-live state (a stream that ended without
            // disconnect() must never leave the store looking connected).
            // Generation-guarded: a NEWER connection (or a deliberate
            // disconnect that already nil-ed the handle) is never
            // disturbed by this late cleanup.
            Task { @MainActor [weak self] in
                guard let self, self.connectionGeneration == generation else { return }
                self.streamTask = nil
                if self.connectionState != .disconnected {
                    self.connectionState = .disconnected
                }
            }
        }
        startWatchdog(generation: generation)
    }

    /// #425: pull-to-refresh stream recovery. `applyRefresh` never claims
    /// liveness, so a successful snapshot over an ended/wedged stream
    /// leaves the host honestly offline. This clears the stale task
    /// ownership (cancel + handle) and starts EXACTLY ONE replacement
    /// stream from the shared cursor whenever the stream is NOT provably
    /// live. A stream is left alone only while it is BOTH `.connected` AND
    /// fresh (a transport signal within the documented inactivity budget) —
    /// so refreshing a healthy host stays idempotent (no duplicate SSE
    /// tasks), while a half-open stream whose last handshake succeeded but
    /// whose bytes stopped arriving is replaced exactly once.
    /// Fire-and-forget: the pull indicator is governed by the caller's own
    /// awaits, never by the (re)connect.
    func reconnectIfNeeded(client: CorraldClient) {
        if connectionState == .connected, !isTransportStale { return }
        // #425: a replacement that is still inside its own freshness window
        // is already the one recovery attempt for this stale episode — do
        // not stack a second one on top of it (a replacement that never
        // answers falls out of this guard once its window expires).
        if replacementInFlight, !isTransportStale { return }
        replaceStream(client: client)
    }

    /// #425: tear down whatever stream is owned and start EXACTLY ONE
    /// replacement from the shared cursor. Every replacement path (refresh
    /// and watchdog) funnels through here so a stale episode can never end
    /// with two competing stream tasks.
    private func replaceStream(client: CorraldClient?) {
        guard let client else { return }
        disconnect()
        connect(client: client)
        replacementInFlight = true
    }

    /// #425: has THIS host's transport gone silent past the documented
    /// inactivity budget? A stream attempt is born fresh (its dispatch
    /// marks the clock), so only real silence — no bytes, no ack — can make
    /// this true. An idle-but-healthy fleet keeps sending 15s comment
    /// keep-alives, which mark the clock and keep this false.
    var isTransportStale: Bool {
        heartbeat.age() > streamLiveness.inactivityBudget
    }

    /// #425: per-host liveness watchdog. Bounded poll cadence (never a tight
    /// loop), armed only while a stream task is owned, and cancelled by
    /// every teardown path (`disconnect`, `reset`, background stop, host
    /// removal). Generation-guarded, so a tick belonging to a replaced
    /// stream can never touch its successor's state.
    private func startWatchdog(generation: Int) {
        watchdogTask?.cancel()
        watchdogTask = Task { [weak self] in
            while !Task.isCancelled {
                let tick = self?.streamLiveness.tickInterval ?? StreamLiveness.maxTickInterval
                try? await Task.sleep(nanoseconds: UInt64(tick * 1_000_000_000))
                guard !Task.isCancelled, let self else { return }
                self.watchdogTick(generation: generation)
            }
        }
    }

    private func watchdogTick(generation: Int) {
        guard connectionGeneration == generation, streamTask != nil else { return }
        // A stream that ACKED live and then went silent is a half-open
        // socket the daemon (and the user) still believe is live — it must
        // leave the live posture and be replaced. An attempt that never
        // acked is deliberately NOT touched here: the #436 pull contract
        // keeps a pull able to restart it, and the transport's own 60s
        // inactivity timeout backs it up, so one wedge never triggers two
        // independent replacement paths.
        guard connectionState == .connected, isTransportStale else { return }
        let budget = streamLiveness.inactivityBudget
        Self.log.error(
            "stream heartbeat silent past the \(budget, privacy: .public)s budget — replacing the wedged stream"
        )
        replaceStream(client: streamClient)
    }

    /// #425: the transport reported a CLEAN server EOF (the daemon closed
    /// the stream; the client is between attempts on its backoff ladder).
    /// A continuously-live posture must end IMMEDIATELY — a host that
    /// stopped delivering must never keep rendering live while the retry
    /// waits — and a later 200 ack (`noteConnected`) clears the retry
    /// posture safely. This is the transport's normal reconnect path, not a
    /// failure: it gets a log line, never an error banner.
    func noteStreamEnded() {
        replacementInFlight = false
        guard connectionState == .connected else { return }
        Self.log.info("stream ended cleanly — retry pending")
        connectionState = .connecting
    }

    /// #92: visible + diagnosable connection-failure state (never a silent
    /// spinner). Mirrors `noteDecodeFailure`: os.Logger (retrievable from a
    /// detached TestFlight build via Console/sysdiagnose — print is not),
    /// the `.error` connection state, and the callback the owner routes to
    /// the full-width, copyable banner. A later good frame's `apply()`
    /// returns the state to `.connected`, so a transient failure is visible
    /// but not fatal.
    func noteConnectionError(_ reason: String) {
        // #425: the failing attempt's episode is over — a later refresh may
        // start a fresh replacement.
        replacementInFlight = false
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
    /// #425: the 200 is a transport liveness signal (it refreshes the
    /// watchdog's clock) and it settles any in-flight replacement, so a
    /// later pull sees a healthy stream and stays snapshot-only.
    func noteConnected() {
        lastConnectionErrorReason = nil
        replacementInFlight = false
        heartbeat.mark()
        connectionState = .connected
        onConnected?()
    }

    /// #427 evidence seeding: put a store in the CONNECTING posture without
    /// starting a stream (the demo filter-header frames need a host whose
    /// sheet/banner text reads `connecting`). Mirrors the exact `.connecting`
    /// transition `connect()` makes and clears the error-dedupe reason like
    /// `noteConnected()`; it deliberately touches NO stream task, cursor, or
    /// callback state — only the published connection posture changes.
    func noteConnecting() {
        lastConnectionErrorReason = nil
        connectionState = .connecting
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
                self.applyStreamFrame(event)
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
    /// owner, so `connect` resumes without a full snapshot when fresh. The
    /// cancelled task is returned so teardown callers can await termination
    /// before invalidating the session that owns its transport.
    /// #425: teardown also ends the per-host watchdog (background,
    /// host removal, reset — one cancellation point per host) and the
    /// replacement episode, so no timer or late callback can revive
    /// obsolete state.
    @discardableResult
    func disconnect() -> Task<Void, Never>? {
        let task = streamTask
        task?.cancel()
        streamTask = nil
        watchdogTask?.cancel()
        watchdogTask = nil
        streamClient = nil
        replacementInFlight = false
        connectionState = .disconnected
        return task
    }

    func reset() {
        disconnect()
        agents = [:]
        tails = [:]
        tailPanes = [:]
        lastEventId = nil
        lastEventEpoch = nil
        cursorBox.write(rev: nil, epoch: nil)
        // A reset abandons the delta base; retaining it would let a later
        // live connection resume from demo or otherwise unrelated state.
        cursorDefaults.removeObject(forKey: "fleetnotifier.lastEventId")
        cursorDefaults.removeObject(forKey: "fleetnotifier.lastEventEpoch")
        previousStates = [:]
        activeAgents = []
        stateEnteredAt = [:]
        connectionState = .disconnected
    }

#if DEBUG
    /// Debug-only demo mode: seed the store directly (no daemon).
    func seedDemo(agents: [String: Agent], rev: UInt64) {
        let old = self.agents
        self.agents = agents
        tails = tails.filter { agents[$0.key] != nil }
        lastEventId = rev
        lastEventEpoch = nil
        cursorBox.write(rev: rev, epoch: nil)
        updateStateEnteredAt(old: old, new: agents)
        // Seed the transition shadows so demo seeding never fires hooks.
        for (id, agent) in agents {
            seedShadow(id, agent.state)
        }
        connectionState = .disconnected
    }

    /// Demo transition: replace one agent record in place.
    func upsertDemo(_ agent: Agent) {
        var next = agents
        next[agent.agentId] = agent
        let old = agents
        agents = next
        updateStateEnteredAt(old: old, new: next)
        observe(agent)
    }
#endif

    func persistCursor() {
        if let lastEventId {
            cursorDefaults.set(String(lastEventId), forKey: "fleetnotifier.lastEventId")
            if let lastEventEpoch {
                cursorDefaults.set(lastEventEpoch, forKey: "fleetnotifier.lastEventEpoch")
            } else {
                cursorDefaults.removeObject(forKey: "fleetnotifier.lastEventEpoch")
            }
        } else {
            cursorDefaults.removeObject(forKey: "fleetnotifier.lastEventId")
            cursorDefaults.removeObject(forKey: "fleetnotifier.lastEventEpoch")
        }
    }

    func restoreCursor() {
        if let raw = cursorDefaults.string(forKey: "fleetnotifier.lastEventId"),
           let rev = UInt64(raw) {
            lastEventId = rev
            lastEventEpoch = cursorDefaults.string(forKey: "fleetnotifier.lastEventEpoch")
            cursorBox.write(rev: rev, epoch: lastEventEpoch)
        }
    }

    /// #400: restore a HOST-SCOPED cursor (the per-profile cursor of the
    /// stream coordinator), never the legacy single-host default. The
    /// active single-host store keeps the legacy key for parity; every
    /// coordinator session store restores/advances its own profile cursor.
    func restoreCursor(rev: UInt64?, epoch: String? = nil) {
        lastEventId = rev
        lastEventEpoch = rev == nil ? nil : epoch
        cursorBox.write(rev: rev, epoch: lastEventEpoch)
    }

    /// #400 (E3): purge one host's in-memory read state — rows, tails,
    /// tail panes, transition shadows, and the state clock — WITHOUT
    /// touching the shared legacy cursor default. `reset()` removes the
    /// legacy `fleetnotifier.lastEventId` key, which belongs to the
    /// ACTIVE host's mirror; coordinator sessions persist their cursors
    /// through the profile store, so their purge must leave that key
    /// alone or removing one host would erase another host's cursor.
    /// Stream cancellation stays the CALLER's job (`disconnect()`), so
    /// host-removal teardown has exactly one cancellation point.
    func purgeState() {
        agents = [:]
        tails = [:]
        tailPanes = [:]
        lastEventId = nil
        lastEventEpoch = nil
        cursorBox.write(rev: nil, epoch: nil)
        previousStates = [:]
        activeAgents = []
        stateEnteredAt = [:]
    }
}
