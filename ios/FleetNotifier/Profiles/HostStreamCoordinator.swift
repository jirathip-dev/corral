import Combine
import Foundation
import os

// MARK: - #400 per-host stream coordinator (C2-C4, C6/C7, E1/E3 runtime)

/// Composite client identity (C2): the unified client key for every
/// cross-host state surface is `(host_profile_id, raw_agent_id)`. The raw
/// agent id is preserved UNCHANGED for requests to that host; the profile
/// id is what keeps an equal raw id on another host unreachable. Rows,
/// recents requests, tails, cleanup, and the #401 board all route through
/// this identity — never the display name or URL.
struct CompositeAgentID: Hashable, Sendable, CustomStringConvertible {
    let hostProfileID: UUID
    /// Raw agent id exactly as the wire delivered it (never namespaced).
    let agentID: String

    var description: String {
        BoardCacheDTO.composite(hostProfileID: hostProfileID, agentID: agentID)
    }

    init(hostProfileID: UUID, agentID: String) {
        self.hostProfileID = hostProfileID
        self.agentID = agentID
    }

    /// Parse the canonical `host_profile_id::agent_id` key. Agent ids may
    /// themselves contain ":" (e.g. "herdr:demo"), so the split anchors on
    /// the FIXED 36-character UUID prefix — never on the first separator.
    init?(string: String) {
        guard string.count > 38 else { return nil }
        let uuidText = String(string.prefix(36))
        guard let uuid = UUID(uuidString: uuidText) else { return nil }
        let tailStart = string.index(string.startIndex, offsetBy: 36)
        let tail = string[tailStart...]
        guard tail.hasPrefix("::") else { return nil }
        let raw = String(tail.dropFirst(2))
        guard !raw.isEmpty else { return nil }
        hostProfileID = uuid
        agentID = raw
    }
}

/// One board row of the composite read model (C6/C7): the retained
/// metadata of a lane under its composite identity, plus the staleness and
/// last-seen facts the stale/offline rendering (#401) consumes. `agent`
/// keeps the lane's LAST REPORTED herdr state — a stale Blocked lane stays
/// Blocked; it is never recast as live urgency or as Unknown.
struct HostBoardRow: Equatable, Identifiable, Sendable {
    let identity: CompositeAgentID
    let agent: Agent
    /// True while the owning host has no live connection (C6): the row is
    /// a RETAINED snapshot (last snapshot or the durable allowlisted cache)
    /// kept until an authoritative reconnect or host removal.
    let isStale: Bool
    /// Epoch millis of the last time this lane was seen on a live feed —
    /// the last-seen age source for stale rows (C6).
    let lastSeen: UInt64

    var id: String { identity.description }
}

/// Pure projection from one host's read model + durable allowlisted cache
/// into composite board rows (C6/C7). Live rows come from the host's store;
/// a host that is NOT connected keeps its last snapshot rows as STALE rows
/// and fills never-connected gaps from the durable cache — the board never
/// freezes or erases because one host failed. Tail/transcript content
/// cannot reach this surface: the cache DTO has no such fields and this
/// projection reads metadata only.
enum HostBoardProjection {
    /// Rows for one host: every live store row (stale-marked when the host
    /// is not connected) plus cached rows the store has not seen yet (a
    /// never-connected or pre-reconnect host renders its durable last-known
    /// metadata). `lastSeen` for a retained row is the cache's stamp when
    /// one exists (the cache is written on successful connections), else
    /// the row's own ts. MainActor: reads the store's live read model.
    @MainActor
    static func boardRows(hostProfileID: UUID,
                          store: FleetStore,
                          cached: [BoardCacheRow]?,
                          connected: Bool) -> [HostBoardRow] {
        let cacheByID = cached.map { rows in
            Dictionary(rows.map { ($0.agentID, $0) }, uniquingKeysWith: { first, _ in first })
        } ?? [:]
        let storeIDs = Set(store.agents.keys)
        var rows: [HostBoardRow] = []
        for (agentID, agent) in store.agents {
            let retainedLastSeen = cacheByID[agentID]?.lastSeen ?? agent.ts
            rows.append(HostBoardRow(
                identity: CompositeAgentID(hostProfileID: hostProfileID, agentID: agentID),
                agent: agent,
                isStale: !connected,
                lastSeen: connected ? agent.ts : retainedLastSeen))
        }
        guard !connected else { return rows }
        // Cached rows the store has never seen: retained durable metadata
        // (C5/C6). The state token is preserved verbatim — never recast.
        // The cache stores only the worktree BASENAME (privacy, C5) and no
        // full path exists to synthesize; board metadata (repo/branch/
        // display fields) is all the DTO holds.
        for (agentID, row) in cacheByID where !storeIDs.contains(agentID) {
            guard let state = AgentState(rawValue: row.state) else { continue }
            let workspace = Workspace(repo: row.repo, branch: row.branch)
            let attachment = row.paneReference.map {
                Attachment(kind: "herdr-pane", reference: $0)
            }
            let agent = Agent(agentId: row.agentID, source: "herdr", tool: row.tool ?? "claude",
                              state: state, reason: row.reason, seq: 0, ts: row.ts,
                              capabilities: [], host: nil, workspace: workspace,
                              attachment: attachment,
                              displayName: row.displayName, title: row.title)
            rows.append(HostBoardRow(
                identity: CompositeAgentID(hostProfileID: hostProfileID, agentID: agentID),
                agent: agent, isStale: true, lastSeen: row.lastSeen))
        }
        return rows
    }

    /// C7 stale ranking: rows arrive in canonical board order (state rank,
    /// then ts desc, then id — the `BoardModel.ordered` vocabulary) and
    /// keep that order INSIDE each (state, repo) partition while LIVE rows
    /// sort before STALE rows within the same bucket. Partitions preserve
    /// the existing timestamp/id ordering on both sides; the state token is
    /// never recast.
    static func liveFirst(_ rows: [HostBoardRow]) -> [HostBoardRow] {
        var partitionOrder: [String] = []
        var live: [String: [HostBoardRow]] = [:]
        var stale: [String: [HostBoardRow]] = [:]
        for row in rows {
            let key = partitionKey(of: row)
            if live[key] == nil && stale[key] == nil {
                partitionOrder.append(key)
            }
            if row.isStale {
                stale[key, default: []].append(row)
            } else {
                live[key, default: []].append(row)
            }
        }
        return partitionOrder.flatMap { key in
            (live[key] ?? []) + (stale[key] ?? [])
        }
    }

    /// Canonical board input order for `liveFirst`: BoardModel's attention
    /// rank, ts desc, composite id — the same ordering the single-host
    /// board uses, with the composite id as the deterministic tiebreak.
    static func canonicallyOrdered(_ rows: [HostBoardRow]) -> [HostBoardRow] {
        rows.sorted { a, b in
            let ra = BoardModel.stateRank(a.agent.state), rb = BoardModel.stateRank(b.agent.state)
            if ra != rb { return ra < rb }
            if a.agent.ts != b.agent.ts { return a.agent.ts > b.agent.ts }
            return a.identity.description < b.identity.description
        }
    }

    /// (state, repo) partition key — repo buckets mirror BoardModel's
    /// subgroup semantics (missing/empty/Other repo → the Other bucket).
    private static func partitionKey(of row: HostBoardRow) -> String {
        let repo = BoardModel.repoKey(of: row.agent) ?? BoardModel.otherRepoLabel
        return "\(row.agent.state.rawValue)|\(repo)"
    }
}

/// #451: bounded RATE policy for a host's `/host-key` preflight ladder.
///
/// One ladder runs per host and per path (never two — see
/// `HostStreamCoordinator.startSessionIfNeeded` and
/// `AppModel.beginKeyContinuityCheck`). The delay before each retry grows
/// geometrically from `baseInterval` up to `maxInterval` and then SATURATES:
/// while the ladder is owned (app in the foreground, host not removed), an
/// unreachable host keeps being retried at the capped cadence — so an
/// initially offline host becomes live automatically WHEN it becomes
/// reachable, not merely inside a fixed attempt window. What is bounded is
/// the RATE (at most one `/host-key` attempt per `baseInterval`, never
/// further apart than `maxInterval`, saturating/overflow-free delay math),
/// never a finite availability window. Cancellation (background, host
/// removal) and a key MISMATCH stay terminal — a mismatch is never retried
/// or repaired. Jitter and any stable-session reset policy belong to #452.
struct HostPreflightRetryPolicy: Equatable, Sendable {
    /// Delay before attempt 2; every following delay doubles up to
    /// `maxInterval` and then stays there.
    var baseInterval: TimeInterval
    /// Ceiling for the retry spacing — the steady-state cadence of a host
    /// that stays unreachable while the ladder is still owned.
    var maxInterval: TimeInterval

    /// Production ladder: 3 s, doubling to a 30 s steady-state cadence
    /// (attempts at roughly 0 s, 3 s, 9 s, 21 s, 45 s, 75 s … then every
    /// 30 s while the ladder stays owned). A host that becomes reachable at
    /// ANY later point in the foreground session recovers with no
    /// foreground/manual retry/restart; backgrounding or removing the host
    /// ends the retries.
    static let `default` = HostPreflightRetryPolicy(baseInterval: 3,
                                                    maxInterval: 30)

    /// Delay before attempt number `nextAttempt` (1-based — attempt 1 runs
    /// immediately). Monotonically non-decreasing and saturated at
    /// `maxInterval`; computed iteratively so an unbounded attempt number
    /// can never overflow or busy-loop the computation.
    func delay(beforeAttempt nextAttempt: Int) -> TimeInterval {
        guard nextAttempt > 1 else { return 0 }
        var delay = baseInterval
        var attempt = 2
        while attempt < nextAttempt, delay < maxInterval {
            delay = min(delay * 2, maxInterval)
            attempt += 1
        }
        return min(delay, maxInterval)
    }

    /// The sleep interval for one ladder step, clamped so the nanoseconds
    /// conversion can never trap (`Task.sleep` takes `UInt64`).
    func delayNanoseconds(beforeAttempt nextAttempt: Int) -> UInt64 {
        let seconds = max(0, min(delay(beforeAttempt: nextAttempt), 86_400))
        return UInt64(seconds * 1_000_000_000)
    }
}

/// Per-host stream coordinator (C3/C4/C6/C7/E3): one independent revision,
/// cursor, connection generation, retry ladder, error, and task set per
/// host profile. Every profile owns a dedicated `FleetStore` whose raw
/// agent-id namespace can never touch an equal raw id on another host —
/// cross-host surfaces key by `CompositeAgentID`.
///
/// The ACTIVE single-host profile (the one AppModel binds its legacy
/// fields to) is excluded from the coordinator: AppModel owns that stream
/// exactly as before (#399 F1 parity). This coordinator runs every OTHER
/// configured host concurrently and owns their stream-task/tail lifecycle
/// + cleanup (#400 QA ownership).
@MainActor
final class HostStreamCoordinator: ObservableObject {
    /// Per-profile key-continuity posture of a coordinator session (B4,
    /// consuming #399's trust helpers): a pinned host must re-verify
    /// `/host-key` before ITS stream opens; a mismatch fails closed and no
    /// stream/read ever reaches the replacement identity. Exposed for #401
    /// to render; behavior is enforced here.
    enum KeyPosture: Equatable, Sendable {
        case unpinned
        case verifying
        case verified
        case mismatch
    }

    /// One host's live runtime: the profile id, its dedicated raw-namespaced
    /// store, and every coordinator-owned task handle for that host.
    final class Session {
        let profileID: UUID
        let store: FleetStore
        /// Set only by the coordinator (same file) — sessions expose the
        /// posture for #401/tests.
        fileprivate(set) var posture: KeyPosture = .unpinned
        var continuityTask: Task<Void, Never>?
        /// #451: monotonic ladder identity. A finishing preflight ladder
        /// clears the task handle it OWNS only — a background stop plus a
        /// foreground re-arm may already have installed a successor.
        var continuityGeneration = 0
        /// The host's in-flight pull-refresh task (per-host task set; its
        /// outcome is a String? failure reason).
        var refreshTask: Task<String?, Never>?
        /// Last pull-refresh failure for this host (state for #401; a
        /// failed host never freezes the others).
        fileprivate(set) var refreshFailure: String?

        init(profileID: UUID, store: FleetStore) {
            self.profileID = profileID
            self.store = store
        }

        func noteRefreshFailure(_ reason: String?) {
            refreshFailure = reason
        }
    }

    private static let log = Logger(subsystem: "com.corral.fleetnotifier", category: "host-coordinator")

    private let defaults: UserDefaults
    private let urlSession: URLSession
    private let profileStore: HostProfileStore?
    /// #451: pacing/attempt bound for every per-host preflight ladder.
    /// Injectable so tests run the SAME ladder semantics at tiny intervals.
    private let preflightPolicy: HostPreflightRetryPolicy
    private let signerProvider: @MainActor () -> DeviceSigner?
    /// The profile whose stream AppModel owns directly (nil = this
    /// coordinator manages every profile — used by standalone tests).
    var activeProfileID: UUID?
    private(set) var sessions: [UUID: Session] = [:]
    private var changeSinks: [UUID: AnyCancellable] = [:]

    /// Fired when one of this coordinator's hosts establishes a successful
    /// HTTP/SSE connection (C6 last-seen + F2 pending-clear retry).
    var onSessionConnected: (@MainActor (UUID) -> Void)?

    /// #397: fired when one of this coordinator's host stores observes a
    /// state-change transition (started / blocked / finished) for an agent
    /// — the composite (host profile, raw agent id) notification surface
    /// for the DEBUG local bridge. AppModel consumes it per host; equal
    /// raw agent ids on different hosts fire independently.
    var onAgentTransition: (@MainActor (PushPayload.PushType, String, UUID) -> Void)?

    /// #397 follow-up: fired when one of this coordinator's host stores
    /// applied frame data (rows changed) — AppModel replays deferred
    /// notification taps whose target may now be available on that host.
    var onSessionAgentsChanged: (@MainActor (UUID) -> Void)?

    init(defaults: UserDefaults = .standard,
         session: URLSession = .shared,
         profileStore: HostProfileStore? = nil,
         preflightPolicy: HostPreflightRetryPolicy = .default,
         signerProvider: @escaping @MainActor () -> DeviceSigner? = { nil }) {
        self.defaults = defaults
        self.urlSession = session
        self.profileStore = profileStore
        self.preflightPolicy = preflightPolicy
        self.signerProvider = signerProvider
    }

    // MARK: - Session lifecycle (C3/E3)

    /// Reconcile sessions against the CURRENT configured profiles: drop
    /// (and fully clean up) sessions whose profile vanished or can no
    /// longer connect, create sessions for new connectable profiles, and —
    /// when `start` — open every not-yet-open session's stream.
    func update(profiles: [HostProfile], startStreams: Bool) {
        var desired: [HostProfile] = []
        for profile in profiles where profile.id != activeProfileID && profile.mayConnect {
            desired.append(profile)
        }
        let desiredIDs = Set(desired.map(\.id))
        for id in Array(sessions.keys) where !desiredIDs.contains(id) {
            remove(profileID: id)
        }
        for profile in desired where sessions[profile.id] == nil {
            let store = FleetStore(defaults: defaults)
            // Per-host cursor (B1/C3): resume from THIS host's persisted
            // cursor only. The legacy `fleetnotifier.lastEventId` default
            // is the ACTIVE host's mirror and is never read here.
            store.restoreCursor(rev: profileStore?.cursor(for: profile.id),
                                epoch: profileStore?.cursorEpoch(for: profile.id))
            store.acceptedHostIdentity = profile.hostKeyB64
            let session = Session(profileID: profile.id, store: store)
            // A pinned host has no continuity contract until its /host-key
            // re-check passes (startSessionIfNeeded) — posture starts
            // .verifying, so no live work can leak through pre-verification.
            session.posture = profile.hostKeyB64 == nil ? .unpinned : .verifying
            sessions[profile.id] = session
            let sink = store.objectWillChange.sink { [weak self] _ in
                self?.objectWillChange.send()
            }
            changeSinks[profile.id] = sink
            objectWillChange.send()
        }
        if startStreams {
            for profile in desired {
                startSessionIfNeeded(profile)
            }
        }
    }

    /// Start one host's stream when it is not already open (idempotent —
    /// repeated startLive() calls are no-ops per host). A PINNED host must
    /// re-verify `/host-key` before its stream opens (B4 fail-closed); an
    /// unpinned legacy host opens directly (parity with the active flow).
    /// #451: a transient preflight failure (unreachable/timeout/server
    /// error) retries at a bounded RATE — a cancellable ladder that lives
    /// inside THIS session's single continuity task, so ownership,
    /// cancellation (background/removal) and re-verification all stay
    /// one-per-host, and a host that becomes reachable at any later point
    /// while the ladder is owned recovers automatically. A key MISMATCH
    /// stays terminal — never re-polled, never repaired.
    func startSessionIfNeeded(_ profile: HostProfile) {
        guard profile.id != activeProfileID, profile.mayConnect,
              let session = sessions[profile.id] else { return }
        if session.store.isStreaming { return }
        if session.continuityTask != nil { return }
        guard session.posture != .mismatch else { return }
        guard let url = URL(string: profile.urlString) else { return }
        let client = CorraldClient(host: url, session: urlSession)
        guard let pinned = profile.hostKeyB64 else {
            session.posture = .unpinned
            openStream(profile: profile, session: session, client: client)
            return
        }
        session.posture = .verifying
        session.continuityGeneration += 1
        let ladder = session.continuityGeneration
        session.continuityTask = Task { @MainActor [weak self] in
            guard let self else { return }
            // The handle stays set for the WHOLE ladder (the one-owner
            // guard above must hold across its entire lifetime), and only
            // the ladder that still owns this session may clear it.
            defer {
                if session.continuityGeneration == ladder {
                    session.continuityTask = nil
                }
            }
            var attempt = 1
            while true {
                guard !Task.isCancelled,
                      self.sessions[profile.id] === session else { return }
                do {
                    let response = try await client.fetchHostKey()
                    guard !Task.isCancelled,
                          self.sessions[profile.id] === session else { return }
                    if HostKeyTrust.matches(response, pinnedKeyB64: pinned) {
                        session.posture = .verified
                        self.openStream(profile: profile, session: session, client: client)
                    } else {
                        Self.log.error("host \(profile.displayName, privacy: .public): pinned key mismatch — stream stays closed")
                        session.posture = .mismatch
                        session.store.noteConnectionError("host key mismatch — pairing must be re-done")
                        self.objectWillChange.send()
                    }
                    return
                } catch {
                    guard !Task.isCancelled,
                          self.sessions[profile.id] === session else { return }
                    // Unreachable host: B4 keeps the stream closed (never
                    // unverified-open). The ladder keeps its bounded RATE
                    // for as long as this session owns it — a host that
                    // becomes reachable LATER in the foreground session
                    // still recovers automatically (cancellation at the
                    // background/removal boundary is what ends retries, not
                    // a fixed attempt window). The failing attempt's reason
                    // is published on the host's store so Settings/the board
                    // show a truthful reason instead of a silent verify.
                    attempt += 1
                    session.store.noteConnectionError(
                        "host unreachable — retrying host-key verification (attempt \(attempt))")
                    self.objectWillChange.send()
                    do {
                        try await Task.sleep(
                            nanoseconds: self.preflightPolicy.delayNanoseconds(beforeAttempt: attempt))
                    } catch {
                        return  // cancelled (background/removal): retries end
                    }
                }
            }
        }
    }

    private func openStream(profile: HostProfile, session: Session, client: CorraldClient) {
        guard sessions[profile.id] === session else { return }
        // Pinned-feed acceptance (C1) per host; nil-pin hosts keep the
        // transitional-daemon pass of the active flow.
        session.store.acceptedHostIdentity = profile.hostKeyB64
        session.store.onHostIntegrityMismatch = { @MainActor [weak self] in
            guard let self, let session = self.sessions[profile.id] else { return }
            session.posture = .mismatch
            session.store.disconnect()
            self.objectWillChange.send()
        }
        session.store.onConnected = { @MainActor [weak self] in
            guard let self, let session = self.sessions[profile.id] else { return }
            // C6: stamp + persist on connection success (not only on data
            // events) and fold the allowlisted cache for offline rendering.
            self.profileStore?.noteLastSuccessfulConnection(id: profile.id)
            self.persistMetadata(for: profile, session: session)
            self.onSessionConnected?(profile.id)
            self.objectWillChange.send()
        }
        // #397: per-host state-change notification hooks — each session's
        // store owns its own transition shadow, so an equal raw agent id
        // on another host fires as an independent composite event.
        session.store.onStarted = { @MainActor [weak self] agentID in
            self?.onAgentTransition?(.started, agentID, profile.id)
        }
        session.store.onBlocked = { @MainActor [weak self] agentID in
            self?.onAgentTransition?(.blocked, agentID, profile.id)
        }
        session.store.onFinished = { @MainActor [weak self] agentID in
            self?.onAgentTransition?(.finished, agentID, profile.id)
        }
        // #397 follow-up: rows applied on this host — replay deferred
        // notification taps whose target may now be present here.
        session.store.onAgentsChanged = { @MainActor [weak self] in
            self?.onSessionAgentsChanged?(profile.id)
        }
        session.store.connect(client: client)
        objectWillChange.send()
    }

    /// Stop every coordinator host (app background, C3: cancel all when
    /// the app backgrounds). Rows stay retained in each store for the
    /// next foreground resume.
    func stopAll() {
        for session in sessions.values {
            session.continuityTask?.cancel()
            session.continuityTask = nil
            session.refreshTask?.cancel()
            session.refreshTask = nil
            session.store.disconnect()
        }
        objectWillChange.send()
    }

    /// E3: remove ONE host — cancel that host's stream/tail/refresh tasks
    /// and purge ONLY that host's in-memory rows/tails/sheet state. Every
    /// other host's stream keeps running (no orphaned task, no cross-host
    /// tail survives).
    func remove(profileID: UUID) {
        guard let session = sessions.removeValue(forKey: profileID) else { return }
        changeSinks.removeValue(forKey: profileID)
        session.continuityTask?.cancel()
        session.continuityTask = nil
        session.refreshTask?.cancel()
        session.refreshTask = nil
        session.store.disconnect()
        // Memory purge scoped to this host (never the shared legacy
        // cursor default — see FleetStore.purgeState).
        session.store.purgeState()
        objectWillChange.send()
    }

    // MARK: - Read-model access

    func store(profileID: UUID) -> FleetStore? {
        sessions[profileID]?.store
    }

    func posture(profileID: UUID) -> KeyPosture? {
        sessions[profileID]?.posture
    }

    /// Resolve an agent under a composite identity from EXACTLY the owning
    /// host's store (E1). Never searches another host's rows — an equal
    /// raw id elsewhere is unreachable by construction.
    func agent(profileID: UUID, agentID: String) -> Agent? {
        sessions[profileID]?.store.agent(agentID)
    }

    func tailPane(profileID: UUID, agentID: String) -> TailPane? {
        sessions[profileID]?.store.tailPane(for: agentID)
    }

    /// E1: is this coordinator host allowed to serve signed reads right
    /// now? Pinned hosts must be `.verified` since launch/reconnect; a
    /// mismatch (or an unverified pause) fails closed.
    func allowsLiveWork(profileID: UUID) -> Bool {
        guard let session = sessions[profileID] else { return false }
        switch session.posture {
        case .unpinned, .verified: return true
        case .verifying, .mismatch: return false
        }
    }

    // MARK: - Pull-to-refresh fan-out (C3)

    /// Refresh every coordinator host CONCURRENTLY; each host's outcome is
    /// independent, so a failing host never blocks or erases the results of
    /// the hosts that succeeded. Returns per-profile failure reasons (nil =
    /// applied). The active profile's refresh stays AppModel-owned; this
    /// runs alongside it.
    @discardableResult
    func refreshAll(profiles: [HostProfile]) async -> [UUID: String?] {
        var launched: [(UUID, Task<String?, Never>)] = []
        for profile in profiles where profile.id != activeProfileID && profile.mayConnect {
            guard let session = sessions[profile.id] else { continue }
            guard let url = URL(string: profile.urlString) else { continue }
            session.refreshTask?.cancel()
            let client = CorraldClient(host: url, session: urlSession)
            let task: Task<String?, Never> = Task { @MainActor [weak self] in
                guard let self, let session = self.sessions[profile.id] else {
                    return "host removed during refresh"
                }
                do {
                    let snapshot = try await client.fetchSnapshot()
                    guard !Task.isCancelled,
                          self.sessions[profile.id] === session else {
                        return "host removed during refresh"
                    }
                    session.store.applyRefresh(snapshot)
                    session.noteRefreshFailure(nil)
                    self.persistMetadata(for: profile, session: session)
                    // #451 (AC5 parity): a successful snapshot proves the
                    // host reachable NOW — a session that never verified its
                    // key gets an IMMEDIATE fresh preflight attempt instead
                    // of waiting for the running ladder's next capped tick
                    // (the pull is an explicit "try now"). Still one owner:
                    // the previous ladder is cancelled and a fresh
                    // generation takes over; a streaming or already-verified
                    // host is untouched, and this never replaces a stream
                    // (#425 owns active-host stream replacement).
                    if session.posture == .verifying {
                        session.continuityTask?.cancel()
                        session.continuityTask = nil
                        self.startSessionIfNeeded(profile)
                    }
                    self.objectWillChange.send()
                    return nil
                } catch {
                    guard !Task.isCancelled,
                          self.sessions[profile.id] === session else {
                        return "host removed during refresh"
                    }
                    session.noteRefreshFailure(error.localizedDescription)
                    self.objectWillChange.send()
                    return error.localizedDescription
                }
            }
            session.refreshTask = task
            launched.append((profile.id, task))
        }
        var outcomes: [UUID: String?] = [:]
        for (profileID, task) in launched {
            outcomes[profileID] = await task.value
            sessions[profileID]?.refreshTask = nil
        }
        return outcomes
    }

    // MARK: - Composite board projection (C6/C7, state for #401)

    /// The composite board across the given profiles: every host's rows key
    /// by `(host_profile_id, raw_agent_id)` and stale/live facts per C6.
    /// The active profile's store comes from `activeStoreProvider`.
    func aggregateRows(profiles: [HostProfile],
                       activeStoreProvider: @MainActor () -> FleetStore?,
                       now: UInt64 = UInt64(Date().timeIntervalSince1970 * 1000)) -> [HostBoardRow] {
        var rows: [HostBoardRow] = []
        for profile in profiles {
            let store: FleetStore
            if profile.id == activeProfileID {
                guard let active = activeStoreProvider() else { continue }
                store = active
            } else {
                guard let session = sessions[profile.id] else { continue }
                store = session.store
            }
            let cached = profileStore?.boardCache.load(for: profile.id)
            let connected = store.connectionState == .connected
            rows.append(contentsOf: HostBoardProjection.boardRows(
                hostProfileID: profile.id, store: store,
                cached: cached, connected: connected))
        }
        // C7: canonical board order with live rows before stale rows inside
        // each status/repo bucket.
        return HostBoardProjection.liveFirst(HostBoardProjection.canonicallyOrdered(rows))
    }

    // MARK: - Durable allowlisted metadata (C5 consumption, per host)

    /// Fold one host's current read model into its allowlisted board-cache
    /// file (via #399's BoardCache APIs). Called on connection success and
    /// after authoritative refresh application — never on read_tail
    /// content, which the DTO cannot hold.
    func persistMetadata(for profile: HostProfile, session: Session) {
        guard let profileStore else { return }
        guard !session.store.agents.isEmpty else { return }
        let rows = BoardCacheDTO.snapshot(hostProfileID: profile.id,
                                          agents: session.store.agents,
                                          stateEnteredAt: session.store.stateEnteredAt,
                                          now: UInt64(Date().timeIntervalSince1970 * 1000))
        profileStore.boardCache.save(rows, for: profile.id)
    }

    /// Fold every coordinator host's current read model into its own cache
    /// file (background boundary). Never touches the ACTIVE profile's cache
    /// — AppModel owns that mirror.
    func persistAllMetadata(profiles: [HostProfile]) {
        for profile in profiles where profile.id != activeProfileID {
            guard let session = sessions[profile.id] else { continue }
            persistMetadata(for: profile, session: session)
        }
    }

    /// Persist each coordinator host's live revision into ITS OWN
    /// per-profile cursor (B1/C3) at the background boundary. The legacy
    /// single-host cursor default stays the ACTIVE profile's mirror and is
    /// never written here.
    func persistCursors(profiles: [HostProfile]) {
        guard let profileStore else { return }
        for profile in profiles where profile.id != activeProfileID {
            guard let session = sessions[profile.id] else { continue }
            profileStore.setCursor(session.store.lastEventId,
                                   epoch: session.store.lastEventEpoch,
                                   for: profile.id)
        }
    }
}
