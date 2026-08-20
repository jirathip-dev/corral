import Combine
import Foundation
import SwiftUI

/// One typed refusal surfaced to the UI (D13 read-only default enforcement):
/// `kind` is the daemon's typed error (`not_granted`, `stale_approval`,
/// `hash_mismatch`, `choice_not_in_menu`, `step_up_required`, …).
struct DriveBanner: Equatable, Sendable {
    var kind: String
    var message: String
    var isError: Bool

    static func error(_ kind: String, _ message: String) -> DriveBanner {
        DriveBanner(kind: kind, message: message, isError: true)
    }

    static func info(_ message: String) -> DriveBanner {
        DriveBanner(kind: "info", message: message, isError: false)
    }
}

/// One logical drive action. The identity is stable across duplicated row
/// surfaces (for example NEEDS YOU plus its repo section), so a double tap
/// cannot create two signed request ids for the same operation.
private struct DriveActionKey: Hashable, Sendable {
    let capability: Capability
    let target: String
    let identity: String
}

/// App-level orchestration: identity, registration, live connection,
/// notification hook, and the signed drive flows shared by the UI and the
/// notification-action path.
@MainActor
final class AppModel: ObservableObject {
    enum Mode: Equatable {
        case needsSetup
        case live
        case demo
    }

    @Published var mode: Mode = .needsSetup
    @Published var fleet = FleetStore()
    @Published var banner: DriveBanner?
    @Published var grants: [String] = []
    @Published var keyId: String?
    @Published var hostURL: URL?
    @Published var keyStorageWarning: Bool = false

    var signer: DeviceSigner?
    private var notifier: LocalNotifier?
    /// #79 review F4: one-shot guard for the non-idempotent half of startLive().
    private var notificationsConfigured = false
    /// Every live drive gets its own task handle. A mode/device boundary must
    /// cancel all of them: retaining only the latest handle lets an earlier
    /// Tail/Prompt/Interrupt finish against the old identity after reset.
    private var driveTasks: [String: Task<Void, Never>] = [:]
    @Published private var inFlightDriveKeys: Set<DriveActionKey> = []
    private var driveGeneration = 0
    /// In-flight notification-reply validation (cold-start snapshot fetch).
    private var notificationTask: Task<Void, Never>?
    /// Injectable for tests (URLProtocol-mocked session); `.shared` by
    /// default so production call sites are unchanged.
    private let session: URLSession

    private let defaults = UserDefaults.standard

    /// #93: `fleet` is a NESTED `ObservableObject`. `@Published` fires only
    /// when the REFERENCE is reassigned — it does not forward the child's
    /// `objectWillChange`. Every view observes `AppModel` but reads
    /// `model.fleet.agents` / `model.fleet.connectionState`, so applying an
    /// SSE frame used to mutate the store without re-running any `body`.
    /// Forward the child's change notifications to this object so the board
    /// re-renders when the fleet changes.
    private var fleetChanges: AnyCancellable?

    /// Demo mode is local and intentionally exposes the safe action set even
    /// though no daemon grant exists. Live mode always uses the device's
    /// signed grants.
    var actionGrants: Set<Capability> {
        if mode == .demo {
            return [.prompt, .interrupt, .approve, .readTail]
        }
        return Set(grants.compactMap(Capability.init(rawValue:)))
    }

    func isActionInFlight(agentId: String, capability: Capability?) -> Bool {
        guard let capability else { return false }
        return inFlightDriveKeys.contains { $0.target == agentId && $0.capability == capability }
    }

    var inFlightDriveCount: Int { inFlightDriveKeys.count }

    init(session: URLSession = .shared) {
        self.session = session
        fleetChanges = fleet.objectWillChange.sink { [weak self] _ in
            self?.objectWillChange.send()
        }
        // Restore a previous registration so relaunch skips setup.
        if let meta = DeviceKeyStore.loadMeta(),
           let url = URL(string: defaults.string(forKey: "fleetnotifier.host") ?? "") {
            if let (s, storage) = try? DeviceKeyStore.loadOrCreate() {
                signer = s
                keyId = meta.keyId
                grants = meta.grants
                hostURL = url
                keyStorageWarning = (storage == .insecureFallback)
                mode = .live
            }
        }
    }

    // MARK: - Registration (R1)

    func register(host: String, token: String) async {
        guard let url = URL(string: host.hasPrefix("http") ? host : "http://\(host)") else {
            banner = .error("bad_host", "Host must be an http(s) URL or host:port")
            return
        }
        do {
            let (signer, storage) = try DeviceKeyStore.loadOrCreate()
            self.signer = signer
            keyStorageWarning = (storage == .insecureFallback)
            let client = DriveClient(host: url, session: session)
            let response = try await client.register(token: token, signer: signer)
            keyId = response.keyId
            grants = response.grants
            hostURL = url
            DeviceKeyStore.saveMeta(DeviceKeyStore.DeviceMeta(keyId: response.keyId, host: url.absoluteString,
                                                              grants: response.grants, expiryTs: response.expiryTs,
                                                              registeredAt: UInt64(Date().timeIntervalSince1970)))
            defaults.set(url.absoluteString, forKey: "fleetnotifier.host")
            fleet.restoreCursor()
            mode = .live
            // #79 defect 1: registration used to leave .live with NO
            // stream — the only startLive() call sites were the .active
            // scene transition (already fired) and the demo toggle.
            // connect() is idempotent (streamTask guard), so a
            // scene-driven connect cannot double-stream; startLive()'s
            // notification half is guarded once-per-process (review F4).
            startLive()
            banner = .info("Registered \(response.keyId.prefix(12))… read-only until the host grants capabilities (grants: \(response.grants.isEmpty ? "none" : response.grants.joined(separator: ", ")))")
        } catch {
            banner = .error("register_failed", error.localizedDescription)
        }
    }

    // MARK: - Grants refresh (#101)

    /// Signed self-service grants read: re-fetches THIS key's CURRENT
    /// grants + expiry from the daemon so a host-side promotion reaches the
    /// phone without a device reset. Idempotent and non-blocking: callers
    /// wrap it in a fire-and-forget `Task`, and it never touches the live
    /// stream. On ANY failure the cached grants are kept — a stale cached
    /// set is strictly better than a broken board, so grants are never
    /// cleared by a network error.
    @MainActor
    func refreshGrants() async {
        guard let hostURL, let signer, let keyId else {
            return
        }
        let client = DriveClient(host: hostURL, session: session)
        let currentKeyId = keyId
        do {
            let response = try await client.fetchGrants(keyId: currentKeyId, signer: signer)
            // The device may have been reset / re-registered while the
            // read was in flight — never apply another key's grants.
            guard self.keyId == currentKeyId else { return }
            grants = response.grants
            if let meta = DeviceKeyStore.loadMeta() {
                DeviceKeyStore.saveMeta(DeviceKeyStore.DeviceMeta(
                    keyId: meta.keyId,
                    host: meta.host,
                    grants: response.grants,
                    expiryTs: response.expiryTs,
                    registeredAt: meta.registeredAt))
            }
        } catch {
            // Silent by design: stale cached grants beat a broken board.
        }
    }

    // MARK: - Live connection

    func startLive() {
        guard let hostURL else { return }
        let client = CorraldClient(host: hostURL)
        fleet.onNewlyBlocked = { [weak self] agentId in
            Task { @MainActor in self?.notifyBlocked(agentId: agentId) }
        }
        fleet.onNewlyDone = { [weak self] agentId in
            Task { @MainActor in self?.notifyDone(agentId: agentId) }
        }
        // #79 review F2: a decode failure lands in the full-width,
        // dismissible, text-selectable banner — readable/copyable on
        // device, where diagnosis actually happens — in addition to the
        // .error connection state and the os.Logger line.
        fleet.onDecodeFailure = { [weak self] reason in
            self?.banner = .error("stream_decode", reason)
        }
        // #92: a connection failure lands in the same full-width,
        // dismissible, text-selectable banner as decode failures —
        // readable/copyable on device — in addition to the .error
        // connection state and the os.Logger line.
        fleet.onConnectionError = { [weak self] reason in
            self?.banner = .error("stream_connection", reason)
        }
        // Review F2: once the stream re-establishes a 200 the connection is
        // healthy again — drop a stale stream_connection banner (an idle
        // fleet emits no frames, so apply() alone would never clear it).
        fleet.onConnected = { [weak self] in
            if self?.banner?.kind == "stream_connection" {
                self?.banner = nil
            }
        }
        fleet.connect(client: client)
        // #79 review F4: only connect() is idempotent by itself (its
        // streamTask guard). The notification/APNs setup below is NOT —
        // re-running it allocates a fresh LocalNotifier (dropping the
        // installed delegate) and re-fires APNs registration + a signed
        // /device-token post — and startLive() now legitimately runs
        // more than once per launch (RootView task, .active transition,
        // register()). Guard it to once per process.
        guard !notificationsConfigured else { return }
        notificationsConfigured = true
        notifier = LocalNotifier()
        Task { await notifier?.requestAuthorization() }
        notifier?.registerCategories()
        // R-N1: the reply handler must NOT capture a host-bound client —
        // after "Reset device identity" + re-registration this closure
        // survives (once-per-process guard), and a captured client would
        // send a SIGNED drive to the PREVIOUS host. It resolves the
        // current hostURL at reply time instead.
        notifier?.onReply = { [weak self] payload, action in
            self?.handleNotificationReply(payload: payload, action: action)
        }
        // APNs registration (D16): the token is sent to the daemon by the
        // AppDelegate; on the simulator this fails and the DEBUG local
        // bridge (PushBridge) stays active.
        Task { @MainActor in
            UIApplication.shared.registerForRemoteNotifications()
        }
    }

    func stopLive() {
        fleet.disconnect()
        fleet.persistCursor()
    }

    // MARK: - Notifications (D16)

    /// Blocked hook. The local path fires ONLY through the DEBUG bridge
    /// (`PushBridge`); release builds rely on APNs — the daemon pushes the
    /// same payload the bridge would have.
    private func notifyBlocked(agentId: String) {
        guard let agent = fleet.agent(agentId), let waiting = agent.waitingOn else { return }
        guard PushBridge.shouldPresentLocally else { return }
        let payload = PushPayload.blocked(agent: agent, waiting: waiting)
        notifier?.notifyBlocked(payload,
                                title: agent.displayName ?? agent.agentId,
                                prompt: waiting.prompt)
    }

    /// Done hook (completion notification, no reply surface — D16).
    private func notifyDone(agentId: String) {
        guard let agent = fleet.agent(agentId) else { return }
        guard PushBridge.shouldPresentLocally else { return }
        let payload = PushPayload.done(agentId: agent.agentId)
        notifier?.notifyDone(payload)
    }

    /// In-app canned reply (UI row buttons): the LIVE claim is authoritative
    /// here — the row is rendered from the live agent record. (The lock
    /// screen uses [`handleNotificationReply`], which binds to the
    /// notification's OWN prompt_hash instead.)
    func handleCannedAction(agentId: String, action: CannedChoice.Action,
                            driveClient: DriveClient,
                            expectedPromptHash: String? = nil) {
        guard let agent = fleet.agent(agentId), let waiting = agent.waitingOn else {
            banner = .error("no_waiting_approval", "The agent is no longer waiting; the claim is stale.")
            return
        }
        if let expectedPromptHash, waiting.promptHash != expectedPromptHash {
            banner = .error("stale_approval",
                             "This approval is stale: the agent is now waiting on a different prompt.")
            return
        }
        guard let choice = CannedChoice.choice(for: action, kind: waiting.kind, choices: waiting.choices) else {
            banner = .error("cannot_approve_kind", "This waiting state cannot be answered with \(action.rawValue).")
            return
        }
        driveApprove(agent: agent, choice: choice, driveClient: driveClient,
                     expectedPromptHash: waiting.promptHash)
    }

    /// The lock-screen reply path: bound to the notification's OWN
    /// `prompt_hash`. The reply is validated against the LIVE claim before
    /// any signed bytes leave the phone — a stale notification (agent
    /// moved on, or the prompt changed) surfaces a typed refusal here, and
    /// the daemon would refuse it anyway (`stale_approval` /
    /// `hash_mismatch`). Simple approve/deny/continue replies never carry
    /// free text, so the lock-screen surface cannot trip the destructive
    /// step-up gate; destructive drives happen in-app where Face ID runs.
    func handleNotificationReply(payload: PushPayload, action: CannedChoice.Action,
                                 driveClient injectedDriveClient: DriveClient? = nil) {
        let task = Task { @MainActor [weak self] in
            guard let self else { return }
            // R-N1: bind to the CURRENT host at reply time — never a
            // client captured at startLive() time (it can be stale after
            // a device reset + re-registration).
            guard let hostURL = self.hostURL else { return }
            let driveClient = injectedDriveClient ?? DriveClient(host: hostURL)
            let live = await self.resolveLiveAgent(payload: payload)
            switch NotificationReplyValidator.validate(payload: payload, liveAgent: live) {
            case .failure(.stale):
                self.banner = .error("stale_approval",
                                     "This notification is stale: the agent is no longer waiting on an approval.")
            case .failure(.hashMismatch):
                self.banner = .error("hash_mismatch",
                                     "Stale notification: the agent is now waiting on a different prompt (prompt_hash mismatch).")
            case .success(let waiting):
                guard let choice = CannedChoice.choice(for: action, kind: waiting.kind,
                                                       choices: waiting.choices) else {
                    self.banner = .error("cannot_approve_kind",
                                         "This waiting state cannot be answered with \(action.rawValue).")
                    return
                }
                self.driveApproveClaim(payload: payload, choice: choice, driveClient: driveClient)
            }
        }
        notificationTask = task
    }

    /// Cold-start case: the app may have been killed before the action
    /// tap, so the fleet store is empty — fetch the live snapshot once and
    /// re-validate against it.
    private func resolveLiveAgent(payload: PushPayload) async -> Agent? {
        if let agent = fleet.agent(payload.agentId) { return agent }
        guard let hostURL else { return nil }
        let client = CorraldClient(host: hostURL)
        guard let snapshot = try? await client.fetchSnapshot() else { return nil }
        fleet.apply(.snapshot(snapshot))
        return fleet.agent(payload.agentId)
    }

    /// Drive the approve with the NOTIFICATION's claim (approval_id +
    /// prompt_hash from the payload — equal to the live claim post-check;
    /// the binding stays explicit). Canned approve payloads are never
    /// destructive, so no biometric step-up is prompted from the lock
    /// screen (D13); the drive path still enforces it if the daemon
    /// disagrees.
    private func driveApproveClaim(payload: PushPayload, choice: String,
                                   driveClient: DriveClient) {
        guard let signer, let keyId else {
            banner = .error("unregistered", "Device is not registered.")
            return
        }
        guard let promptHash = payload.promptHash else {
            banner = .error("bad_payload", "Notification carried no prompt_hash.")
            return
        }
        guard let live = currentAgent(for: payload.agentId),
              let waiting = live.waitingOn,
              live.isBlocked,
              waiting.promptHash == promptHash else {
            banner = .error("stale_approval",
                             "This notification is stale: the agent is no longer waiting on that prompt.")
            return
        }
        guard authorize(.approve, for: live) else { return }
        let approvalId = payload.approvalId ?? Claim.approvalId(agentId: payload.agentId,
                                                                promptHash: promptHash)
        let approvePayload = CanonicalJSON.approvePayload(approvalId: approvalId,
                                                          promptHash: promptHash,
                                                          choice: choice)
        let key = approvalActionKey(agentId: payload.agentId, approvalId: approvalId,
                                   promptHash: promptHash)
        guard let requestId = beginDriveAction(key) else { return }
        drive(capability: .approve, target: payload.agentId, payload: approvePayload,
              driveClient: driveClient, keyId: keyId, signer: signer,
              actionKey: key, requestId: requestId)
    }

    // MARK: - Drive flows (D8 claims, byte-for-byte from the snapshot)

    /// The approval claim is echoed EXACTLY from the snapshot's `waiting_on`
    /// (approval_id + prompt_hash); never re-derived from pane text.
    func driveApprove(agent: Agent, choice: String, driveClient: DriveClient,
                      expectedPromptHash: String? = nil) {
        guard let live = currentAgent(for: agent.agentId) else { return }
        guard let signer, let keyId else {
            banner = .error("unregistered", "Device is not registered.")
            return
        }
        guard live.isBlocked, let waiting = live.waitingOn else {
            banner = .error("no_waiting_approval", "Agent is not waiting on an approval.")
            return
        }
        if waiting.kind == .crash {
            banner = .error("cannot_approve_kind", "Crash states do not accept approval replies.")
            return
        }
        if let expectedPromptHash, waiting.promptHash != expectedPromptHash {
            banner = .error("stale_approval",
                             "This approval is stale: the agent is now waiting on a different prompt.")
            return
        }
        if let renderedWaiting = agent.waitingOn,
           renderedWaiting.promptHash != waiting.promptHash {
            banner = .error("stale_approval",
                             "This approval is stale: the agent is now waiting on a different prompt.")
            return
        }
        guard authorize(.approve, for: live) else { return }
        let approvalId = waiting.approvalId ?? Claim.approvalId(agentId: agent.agentId, promptHash: waiting.promptHash)
        let payload = CanonicalJSON.approvePayload(approvalId: approvalId,
                                                   promptHash: waiting.promptHash,
                                                   choice: choice)
        let key = approvalActionKey(agentId: live.agentId, approvalId: approvalId,
                                   promptHash: waiting.promptHash)
        guard let requestId = beginDriveAction(key) else { return }
        drive(capability: .approve, target: agent.agentId, payload: payload,
              driveClient: driveClient, keyId: keyId, signer: signer,
              actionKey: key, requestId: requestId)
    }

    /// `read_tail` is bounded (D5): 200 lines, never prefetched.
    func driveReadTail(agent: Agent, driveClient: DriveClient) {
        guard let live = currentAgent(for: agent.agentId) else { return }
        guard let signer, let keyId else {
            banner = .error("unregistered", "Device is not registered.")
            return
        }
        guard authorize(.readTail, for: live) else { return }
        let payload = CanonicalJSON.readTailPayload(lines: 200)
        let key = DriveActionKey(capability: .readTail, target: live.agentId, identity: "tail-200")
        guard let requestId = beginDriveAction(key) else { return }
        drive(capability: .readTail, target: live.agentId, payload: payload,
              driveClient: driveClient, keyId: keyId, signer: signer,
              actionKey: key, requestId: requestId)
    }

    func drivePrompt(agent: Agent, text: String, driveClient: DriveClient) {
        guard let live = currentAgent(for: agent.agentId) else { return }
        guard let signer, let keyId else {
            banner = .error("unregistered", "Device is not registered.")
            return
        }
        guard !text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else {
            banner = .error("empty_prompt", "Prompt text cannot be empty.")
            return
        }
        guard authorize(.prompt, for: live) else { return }
        let payload = CanonicalJSON.promptPayload(text: text)
        let key = DriveActionKey(capability: .prompt, target: live.agentId, identity: text)
        guard let requestId = beginDriveAction(key) else { return }
        drive(capability: .prompt, target: live.agentId, payload: payload,
              driveClient: driveClient, keyId: keyId, signer: signer,
              actionKey: key, requestId: requestId)
    }

    /// Interrupt takes the contract's null payload and is grant/capability
    /// gated at the same boundary as Tail, Prompt, and Approval.
    func driveInterrupt(agent: Agent, driveClient: DriveClient) {
        guard let live = currentAgent(for: agent.agentId) else { return }
        guard let signer, let keyId else {
            banner = .error("unregistered", "Device is not registered.")
            return
        }
        guard authorize(.interrupt, for: live) else { return }
        let payload = CanonicalJSON.interruptPayload()
        let key = DriveActionKey(capability: .interrupt, target: live.agentId,
                                 identity: "interrupt")
        guard let requestId = beginDriveAction(key) else { return }
        drive(capability: .interrupt, target: live.agentId, payload: payload,
              driveClient: driveClient, keyId: keyId, signer: signer,
              actionKey: key, requestId: requestId)
    }

    /// Resolve an action's target from the current read model. A detail view
    /// may outlive a delta deletion; in that case no signed bytes are built.
    private func currentAgent(for agentId: String) -> Agent? {
        guard let agent = fleet.agent(agentId) else {
            banner = .error("stale_agent", "This agent was deleted or migrated; refresh the fleet before acting.")
            return nil
        }
        return agent
    }

    /// Both sides of the drive authorization contract must hold locally for
    /// an actionable control. The daemon remains authoritative and can still
    /// return a typed refusal, which the common drive path surfaces.
    private func authorize(_ capability: Capability, for agent: Agent) -> Bool {
        guard agent.capabilities.contains(capability.rawValue) else {
            banner = .error("capability_unavailable",
                            "This agent does not advertise the `\(capability.rawValue)` capability.")
            return false
        }
        guard actionGrants.contains(capability) else {
            banner = .error("not_granted",
                            "The device has no `\(capability.rawValue)` grant — ask the host to promote capabilities.")
            return false
        }
        return true
    }

    private func beginDriveAction(_ key: DriveActionKey) -> String? {
        guard !inFlightDriveKeys.contains(key) else {
            banner = .info("\(key.capability.displayName) for \(key.target) is already in progress.")
            return nil
        }
        inFlightDriveKeys.insert(key)
        return DriveClient.newRequestId()
    }

    /// Approval identity is the live claim, not the selected choice. Approve
    /// and Deny must share one in-flight key so two surfaces cannot answer the
    /// same claim concurrently with different choices.
    private func approvalActionKey(agentId: String, approvalId: String,
                                   promptHash: String) -> DriveActionKey {
        DriveActionKey(capability: .approve, target: agentId,
                        identity: "\(approvalId)|\(promptHash)")
    }

    private func drive(capability: Capability, target: String, payload: CanonicalJSON.Value,
                       driveClient: DriveClient, keyId: String, signer: DeviceSigner,
                       actionKey: DriveActionKey, requestId: String) {
        let generation = driveGeneration
        let task = Task { @MainActor [weak self] in
            guard let self else { return }
            defer {
                if self.driveGeneration == generation {
                    self.inFlightDriveKeys.remove(actionKey)
                    self.driveTasks.removeValue(forKey: requestId)
                }
            }
            guard !Task.isCancelled else { return }
            let result = await driveClient.drive(capability: capability, target: target,
                                                 payload: payload, rev: self.fleet.lastEventId,
                                                 requestId: requestId,
                                                 keyId: keyId, signer: signer)
            guard !Task.isCancelled, self.driveGeneration == generation else { return }
            switch result {
            case .dispatched(let response):
                if response.ok {
                    if capability == .readTail {
                        let lines = response.result?.tailLines ?? []
                        self.fleet.rememberTail(lines, for: target)
                        self.banner = .info("Tail \(target): \(lines.count) lines")
                    } else if capability == .approve {
                        self.banner = .info("Approved \(target): rev \(response.rev)")
                    } else if capability == .prompt {
                        self.banner = .info("Prompt sent to \(target): rev \(response.rev)")
                    } else if capability == .interrupt {
                        self.banner = .info("Interrupted \(target): rev \(response.rev)")
                    }
                } else {
                    if response.errorKind == "stale_agent" {
                        self.handleStaleAgent(target,
                                              message: response.error ?? "the agent moved or disappeared")
                    } else {
                        self.banner = .error("dispatch_refused",
                                             response.error ?? "dispatch refused (ok:false)")
                    }
                }
            case .refused(let error):
                switch error {
                case .server(let status, let kind, let message, _):
                    if kind == "stale_agent" {
                        self.handleStaleAgent(target, message: message)
                        return
                    }
                    // Read-only default: ungranted capabilities are refused
                    // with the typed banner; the UI also explains disabled
                    // controls before this path is reachable.
                    self.banner = .error(kind, "\(message) (HTTP \(status))")
                case .network(let message):
                    self.banner = .error("network", message)
                case .encoding:
                    self.banner = .error("encoding", "payload encoding failed")
                }
            }
        }
        driveTasks[requestId] = task
    }

    /// Invalidate the current identity before cancelling handles. Results from
    /// cancellation races then fail the generation check even if URLSession
    /// delivers one final callback on another queue.
    private func cancelDriveTasks() {
        driveGeneration &+= 1
        for task in driveTasks.values {
            task.cancel()
        }
        driveTasks.removeAll()
        inFlightDriveKeys.removeAll()
    }

    /// Stale target handling is deliberately shared by the HTTP 409 path and
    /// the narrow 200 dispatch-race path: remove controls immediately, then
    /// fetch the current read model once.
    private func handleStaleAgent(_ target: String, message: String) {
        fleet.removeAgent(target)
        banner = .error("stale_agent", "\(message) — refreshing the fleet.")
        guard let hostURL else { return }
        let client = CorraldClient(host: hostURL, session: session)
        let expectedHost = hostURL
        Task { @MainActor [weak self] in
            guard let self, let snapshot = try? await client.fetchSnapshot(),
                  self.hostURL == expectedHost else { return }
            self.fleet.apply(.snapshot(snapshot))
        }
    }

    // MARK: - Demo mode (App Review 4.2)

    func enterDemo() {
        cancelDriveTasks()
        notificationTask?.cancel()
        fleet.disconnect()
        fleet.reset()
        fleet.seedDemo(agents: DemoFleet.seed(), rev: 1)
        mode = .demo
    }

    /// Demo drive: answered locally; approve un-blocks the seeded agent.
    func driveDemo(capability: Capability, agent: Agent, choice: String? = nil) {
        var payload: CanonicalJSON.Value = .null
        switch capability {
        case .approve:
            guard let waiting = agent.waitingOn else { return }
            let approvalId = waiting.approvalId ?? Claim.approvalId(agentId: agent.agentId, promptHash: waiting.promptHash)
            payload = CanonicalJSON.approvePayload(approvalId: approvalId,
                                                   promptHash: waiting.promptHash,
                                                   choice: choice ?? "y")
        case .readTail:
            payload = CanonicalJSON.readTailPayload(lines: 200)
        case .prompt:
            payload = CanonicalJSON.promptPayload(text: choice ?? "(demo)")
        default:
            break
        }
        let result = DemoFleet.respond(to: capability, payload: payload, agent: agent,
                                       rev: (fleet.lastEventId ?? 1) + 1)
        if case .dispatched(let response) = result {
            fleet.seedDemo(agents: fleet.agents, rev: response.rev)
            if capability == .readTail {
                fleet.rememberTail(response.result?.tailLines ?? [], for: agent.agentId)
            }
            if capability == .approve, agent.isBlocked {
                simulateUnblock(agentId: agent.agentId)
            }
            banner = .info("(demo) \(capability.displayName) \(agent.agentId) → rev \(response.rev)")
        }
    }

    /// Demo transition: the approval dispatched, the agent resumes work.
    private func simulateUnblock(agentId: String) {
        guard var agent = fleet.agent(agentId) else { return }
        agent.state = .working
        agent.reason = "approved from the lock screen (demo)"
        agent.waitingOn = nil
        agent.ts = UInt64(Date().timeIntervalSince1970 * 1000)
        fleet.upsertDemo(agent)
    }

    // MARK: - Identity management

    func resetDevice() {
        cancelDriveTasks()
        stopLive()
        DeviceKeyStore.wipe()
        defaults.removeObject(forKey: "fleetnotifier.host")
        fleet.reset()
        signer = nil
        keyId = nil
        grants = []
        hostURL = nil
        // R-N1: the next registration is (potentially) a NEW host — the
        // notification/APNs half of startLive() must re-run so the APNs
        // token reaches the new daemon and the reply path re-arms.
        notificationsConfigured = false
        mode = .needsSetup
    }
}

/// Where the app stands on the wire: registered device + host.
struct DeviceRegistration: Equatable {
    var host: String
    var keyId: String
    var grants: [String]
}
