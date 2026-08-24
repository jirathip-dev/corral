import Combine
import Foundation
import SwiftUI

/// Thread-safe identity boundary shared by the main model and the APNs
/// delegate. The delegate is not a child of `AppModel`, so reset/demo cannot
/// cancel its task through a main-actor property alone.
final class IdentityLifecycle: @unchecked Sendable {
    enum Mode: Equatable, Sendable {
        case needsSetup
        case registering
        case live
#if DEBUG
        case demo
#endif
    }

    struct Context: Equatable, Sendable {
        let generation: Int
        let mode: Mode
        let hostURL: URL?
        let keyId: String?
        let signerPublicKeyB64: String?
    }

    static let shared = IdentityLifecycle()

    private let lock = NSLock()
    private var context = Context(generation: 0, mode: .needsSetup,
                                  hostURL: nil, keyId: nil,
                                  signerPublicKeyB64: nil)
    private var tasks: [UUID: Task<Void, Never>] = [:]

    func current() -> Context {
        lock.lock()
        defer { lock.unlock() }
        return context
    }

    func setCurrent(mode: Mode, hostURL: URL?, keyId: String?, signerPublicKeyB64: String?) {
        lock.lock()
        context = Context(generation: context.generation, mode: mode,
                          hostURL: hostURL, keyId: keyId,
                          signerPublicKeyB64: signerPublicKeyB64)
        lock.unlock()
    }

    func isCurrent(_ expected: Context) -> Bool {
        lock.lock()
        defer { lock.unlock() }
        return context == expected
    }

    /// Advance the identity boundary and cancel every task owned by the old
    /// identity. Task creation is synchronized with insertion into `tasks`,
    /// so a reset cannot miss a just-created APNs upload.
    @discardableResult
    func invalidate(mode: Mode, hostURL: URL?, keyId: String?, signerPublicKeyB64: String?) -> Context {
        lock.lock()
        context = Context(generation: context.generation &+ 1, mode: mode,
                          hostURL: hostURL, keyId: keyId,
                          signerPublicKeyB64: signerPublicKeyB64)
        let oldTasks = Array(tasks.values)
        tasks.removeAll()
        lock.unlock()
        oldTasks.forEach { $0.cancel() }
        return current()
    }

    /// Launch one task under the current identity. The operation must perform
    /// its own `isCurrent` checks around every suspension; this owner supplies
    /// cancellation on the next identity boundary.
    @discardableResult
    func launch(_ operation: @escaping @Sendable (Context) async -> Void) -> Task<Void, Never> {
        lock.lock()
        let expected = context
        let taskId = UUID()
        let task = Task { [weak self] in
            await operation(expected)
            self?.finish(taskId)
        }
        tasks[taskId] = task
        lock.unlock()
        return task
    }

    private func finish(_ taskId: UUID) {
        lock.lock()
        tasks.removeValue(forKey: taskId)
        lock.unlock()
    }
}

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
    enum Mode: Equatable, Sendable {
        case needsSetup
        case live
#if DEBUG
        case demo
#endif
    }

    /// Captures the identity a lifecycle-sensitive async operation is
    /// allowed to use. Generation handles explicit boundaries; the remaining
    /// fields catch a host/key/mode replacement that happens without a
    /// network callback returning first.
    private struct LifecycleContext: Equatable, Sendable {
        let generation: Int
        let mode: Mode
        let hostURL: URL?
        let keyId: String?
        let signerPublicKeyB64: String?
    }

    private struct RegistrationContext: Sendable {
        let lifecycle: LifecycleContext
        let sharedLifecycle: IdentityLifecycle.Context
        let requestedHostURL: URL
        let candidateSignerPublicKeyB64: String
    }

    private struct PersistedLiveIdentity {
        let hostURL: URL
        let keyId: String
        let grants: [String]
        let signer: DeviceSigner
        let storage: DeviceKeyStore.Storage
    }

    @Published var mode: Mode = .needsSetup
    @Published var fleet: FleetStore
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
    private var lifecycleGeneration = 0
    /// Notification replies, stale-agent snapshot refreshes, and grants
    /// refreshes all suspend outside the model. Track every one so a mode or
    /// identity boundary can cancel the complete set, not just the latest.
    private var lifecycleTasks: [UUID: Task<Void, Never>] = [:]
    /// Registration is also lifecycle-owned. A separate id prevents a
    /// canceled registration's cleanup from clearing a newer registration.
    private var registrationTaskId: UUID?
    /// Injectable for tests (URLProtocol-mocked session); `.shared` by
    /// default so production call sites are unchanged.
    private let session: URLSession

    private let defaults: UserDefaults
    private let identityLifecycle: IdentityLifecycle
    private let identityLoader: @Sendable () throws -> (DeviceSigner, DeviceKeyStore.Storage)
    private let loadMeta: @Sendable () -> DeviceKeyStore.DeviceMeta?
    private let saveMeta: @Sendable (DeviceKeyStore.DeviceMeta) -> Void
    private let wipeIdentity: @Sendable () -> Void

    /// #93: `fleet` is a NESTED `ObservableObject`. `@Published` fires only
    /// when the REFERENCE is reassigned — it does not forward the child's
    /// `objectWillChange`. Every view observes `AppModel` but reads
    /// `model.fleet.agents` / `model.fleet.connectionState`, so applying an
    /// SSE frame used to mutate the store without re-running any `body`.
    /// Forward the child's change notifications to this object so the board
    /// re-renders when the fleet changes.
    private var fleetChanges: AnyCancellable?

    var actionGrants: Set<Capability> {
#if DEBUG
        /// Demo mode is local and intentionally exposes the safe action set
        /// even though no daemon grant exists. Live mode always uses the
        /// device's signed grants.
        if mode == .demo {
            return [.prompt, .interrupt, .approve, .readTail]
        }
#endif
        return Set(grants.compactMap(Capability.init(rawValue:)))
    }

    /// #166 review F13: single shared DriveClient constructor for the view
    /// layer's three call sites (detail, Recent output, answer sheet). Uses
    /// the registered host URL, falling back to the documented localhost
    /// default, and the default `.shared` URLSession (the injected `session`
    /// is only for tests).
    func makeDriveClient() -> DriveClient {
        DriveClient(host: hostURL ?? URL(string: "http://127.0.0.1:8474")!)
    }

    func isActionInFlight(agentId: String, capability: Capability?) -> Bool {
        guard let capability else { return false }
        return inFlightDriveKeys.contains { $0.target == agentId && $0.capability == capability }
    }

    var inFlightDriveCount: Int { inFlightDriveKeys.count }

    private func lifecycleContext() -> LifecycleContext {
        LifecycleContext(generation: lifecycleGeneration,
                         mode: mode,
                         hostURL: hostURL,
                         keyId: keyId,
                         signerPublicKeyB64: signer?.publicKeyB64)
    }

    private func isCurrent(_ context: LifecycleContext) -> Bool {
        lifecycleGeneration == context.generation
            && mode == context.mode
            && hostURL == context.hostURL
            && keyId == context.keyId
            && signer?.publicKeyB64 == context.signerPublicKeyB64
    }

    private func sharedMode(for mode: Mode) -> IdentityLifecycle.Mode {
        switch mode {
        case .needsSetup: return .needsSetup
        case .live: return .live
#if DEBUG
        case .demo: return .demo
#endif
        }
    }

    init(session: URLSession = .shared,
         identityLifecycle: IdentityLifecycle = .shared,
         defaults: UserDefaults = .standard,
         identityLoader: @escaping @Sendable () throws -> (DeviceSigner, DeviceKeyStore.Storage) = {
             try DeviceKeyStore.loadOrCreate()
         },
         loadMeta: @escaping @Sendable () -> DeviceKeyStore.DeviceMeta? = {
             DeviceKeyStore.loadMeta()
         },
         saveMeta: @escaping @Sendable (DeviceKeyStore.DeviceMeta) -> Void = {
             DeviceKeyStore.saveMeta($0)
         },
         wipeIdentity: @escaping @Sendable () -> Void = {
             DeviceKeyStore.wipe()
         }) {
        self.session = session
        self.identityLifecycle = identityLifecycle
        self.defaults = defaults
        self.identityLoader = identityLoader
        self.loadMeta = loadMeta
        self.saveMeta = saveMeta
        self.wipeIdentity = wipeIdentity
        self.fleet = FleetStore(defaults: defaults)
        fleetChanges = fleet.objectWillChange.sink { [weak self] _ in
            self?.objectWillChange.send()
        }
        // Restore a previous registration so relaunch skips setup.
        if let identity = persistedLiveIdentity() {
            applyLiveIdentity(identity)
        } else {
            identityLifecycle.setCurrent(mode: .needsSetup, hostURL: nil,
                                         keyId: nil, signerPublicKeyB64: nil)
        }
    }

    /// Load only a complete, internally consistent persisted identity. The
    /// host is stored in both metadata and the convenience default; requiring
    /// them to agree prevents a stale host from being paired with the current
    /// key after a partial reset or interrupted registration.
    private func persistedLiveIdentity() -> PersistedLiveIdentity? {
        guard let meta = loadMeta(), !meta.keyId.isEmpty,
              let configuredHost = defaults.string(forKey: "fleetnotifier.host"),
              configuredHost == meta.host,
              let url = URL(string: meta.host),
              let scheme = url.scheme?.lowercased(),
              (scheme == "http" || scheme == "https"),
              url.host != nil,
              let (signer, storage) = try? identityLoader() else {
            return nil
        }
        return PersistedLiveIdentity(hostURL: url, keyId: meta.keyId,
                                     grants: meta.grants, signer: signer,
                                     storage: storage)
    }

    private func applyLiveIdentity(_ identity: PersistedLiveIdentity) {
        signer = identity.signer
        keyId = identity.keyId
        grants = identity.grants
        hostURL = identity.hostURL
        keyStorageWarning = (identity.storage == .insecureFallback)
        mode = .live
        identityLifecycle.setCurrent(mode: .live, hostURL: identity.hostURL,
                                     keyId: identity.keyId,
                                     signerPublicKeyB64: identity.signer.publicKeyB64)
    }

    // MARK: - Registration (R1)

    func register(host: String, token: String) async {
        guard registrationTaskId == nil else {
            banner = .info("Registration is already in progress.")
            return
        }
        guard let url = URL(string: host.hasPrefix("http") ? host : "http://\(host)") else {
            banner = .error("bad_host", "Host must be an http(s) URL or host:port")
            return
        }
        cancelLifecycleTasks()
        let baseContext = lifecycleContext()
        identityLifecycle.setCurrent(mode: .registering,
                                     hostURL: baseContext.hostURL,
                                     keyId: baseContext.keyId,
                                     signerPublicKeyB64: baseContext.signerPublicKeyB64)
        let sharedRegistrationContext = identityLifecycle.current()
        do {
            let (candidateSigner, storage) = try identityLoader()
            guard isCurrent(baseContext), identityLifecycle.isCurrent(sharedRegistrationContext) else { return }
            let registrationContext = RegistrationContext(
                lifecycle: baseContext, sharedLifecycle: sharedRegistrationContext,
                requestedHostURL: url,
                candidateSignerPublicKeyB64: candidateSigner.publicKeyB64)
            let taskId = UUID()
            registrationTaskId = taskId
            let task = Task { @MainActor [weak self] in
                guard let self else { return }
                defer {
                    self.lifecycleTasks.removeValue(forKey: taskId)
                    if self.registrationTaskId == taskId {
                        self.registrationTaskId = nil
                    }
                }
                guard self.isCurrent(registrationContext.lifecycle),
                      self.identityLifecycle.isCurrent(registrationContext.sharedLifecycle),
                      candidateSigner.publicKeyB64 == registrationContext.candidateSignerPublicKeyB64,
                      !Task.isCancelled else { return }
                do {
                    let client = DriveClient(host: registrationContext.requestedHostURL,
                                             session: self.session)
                    let response = try await client.register(token: token, signer: candidateSigner)
                    // A late /register response must not resurrect a reset or
                    // demo identity, write metadata, or start the live stream.
                    guard !Task.isCancelled,
                          self.isCurrent(registrationContext.lifecycle),
                          self.identityLifecycle.isCurrent(registrationContext.sharedLifecycle),
                          candidateSigner.publicKeyB64 == registrationContext.candidateSignerPublicKeyB64 else { return }
                    self.signer = candidateSigner
                    self.keyStorageWarning = (storage == .insecureFallback)
                    self.keyId = response.keyId
                    self.grants = response.grants
                    self.hostURL = registrationContext.requestedHostURL
                    self.saveMeta(DeviceKeyStore.DeviceMeta(
                        keyId: response.keyId, host: registrationContext.requestedHostURL.absoluteString,
                        grants: response.grants, expiryTs: response.expiryTs,
                        registeredAt: UInt64(Date().timeIntervalSince1970)))
                    self.defaults.set(registrationContext.requestedHostURL.absoluteString, forKey: "fleetnotifier.host")
                    self.fleet.restoreCursor()
                    self.mode = .live
                    self.identityLifecycle.setCurrent(
                        mode: .live, hostURL: registrationContext.requestedHostURL,
                        keyId: response.keyId,
                        signerPublicKeyB64: candidateSigner.publicKeyB64)
                    // #79 defect 1: registration used to leave .live with NO
                    // stream — the only startLive() call sites were the
                    // .active scene transition (already fired) and the demo
                    // toggle. connect() is idempotent, and notification setup
                    // is guarded once per process.
                    self.startLive()
                    self.banner = .info("Registered \(response.keyId.prefix(12))… read-only until the host grants capabilities (grants: \(response.grants.isEmpty ? "none" : response.grants.joined(separator: ", ")))")
                } catch is CancellationError {
                    return
                } catch {
                    guard !Task.isCancelled,
                          self.isCurrent(registrationContext.lifecycle),
                          self.identityLifecycle.isCurrent(registrationContext.sharedLifecycle) else { return }
                    self.identityLifecycle.setCurrent(
                        mode: self.sharedMode(for: baseContext.mode),
                        hostURL: baseContext.hostURL, keyId: baseContext.keyId,
                        signerPublicKeyB64: baseContext.signerPublicKeyB64)
                    self.banner = .error("register_failed", error.localizedDescription)
                }
            }
            lifecycleTasks[taskId] = task
            await task.value
        } catch {
            guard !Task.isCancelled, isCurrent(baseContext),
                  identityLifecycle.isCurrent(sharedRegistrationContext) else { return }
            identityLifecycle.setCurrent(mode: sharedMode(for: baseContext.mode),
                                         hostURL: baseContext.hostURL,
                                         keyId: baseContext.keyId,
                                         signerPublicKeyB64: baseContext.signerPublicKeyB64)
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
        let context = lifecycleContext()
        guard context.mode == .live,
              let hostURL = context.hostURL,
              let signer = self.signer,
              let keyId = context.keyId else {
            return
        }
        let taskId = UUID()
        let task = Task { @MainActor [weak self] in
            guard let self else { return }
            defer { self.lifecycleTasks.removeValue(forKey: taskId) }
            guard self.isCurrent(context) else { return }
            let client = DriveClient(host: hostURL, session: self.session)
            do {
                let response = try await client.fetchGrants(keyId: keyId, signer: signer)
                // The device may have been reset / re-registered while the
                // read was in flight — never apply another key's grants.
                guard !Task.isCancelled, self.isCurrent(context) else { return }
                self.grants = response.grants
                if let meta = self.loadMeta() {
                    self.saveMeta(DeviceKeyStore.DeviceMeta(
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
        lifecycleTasks[taskId] = task
        await task.value
    }

    // MARK: - Live connection

    func startLive() {
        guard let hostURL else { return }
        let client = CorraldClient(host: hostURL, session: session)
        fleet.onNewlyBlocked = { [weak self] agentId in
            self?.notifyBlocked(agentId: agentId)
        }
        fleet.onNewlyDone = { [weak self] agentId in
            self?.notifyDone(agentId: agentId)
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
        // APNs upload/retry is independent from one-time local notification
        // setup. A token callback can arrive during demo; retry it now that
        // the shared lifecycle is live, even when notificationsConfigured is
        // already true.
        AppDelegate.shared?.retryPendingDeviceTokenUpload()
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
        // This OS permission request captures no host, key, fleet, or mode;
        // it cannot apply stale lifecycle state after a boundary.
        let notifierForAuthorization = notifier
        Task { await notifierForAuthorization?.requestAuthorization() }
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
        // APNs registration is likewise an OS side effect with no captured
        // model identity or state mutation to re-apply after a boundary.
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
        let context = lifecycleContext()
        guard context.mode == .live else { return }
        let taskId = UUID()
        let task = Task { @MainActor [weak self] in
            guard let self else { return }
            defer { self.lifecycleTasks.removeValue(forKey: taskId) }
            // R-N1: bind to the CURRENT host at reply time — never a
            // client captured at startLive() time (it can be stale after
            // a device reset + re-registration).
            guard self.isCurrent(context), let hostURL = context.hostURL else { return }
            let driveClient = injectedDriveClient ?? DriveClient(host: hostURL)
            let live = await self.resolveLiveAgent(payload: payload, context: context)
            guard !Task.isCancelled, self.isCurrent(context) else { return }
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
        lifecycleTasks[taskId] = task
    }

    /// Cold-start case: the app may have been killed before the action
    /// tap, so the fleet store is empty — fetch the live snapshot once and
    /// re-validate against it.
    private func resolveLiveAgent(payload: PushPayload,
                                  context: LifecycleContext) async -> Agent? {
        guard isCurrent(context) else { return nil }
        if let agent = fleet.agent(payload.agentId) { return agent }
        guard let hostURL = context.hostURL else { return nil }
        let client = CorraldClient(host: hostURL, session: session)
        guard let snapshot = try? await client.fetchSnapshot() else { return nil }
        guard !Task.isCancelled, isCurrent(context) else { return nil }
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

    /// `read_tail` is bounded (D5): 200 lines, never prefetched. #167:
    /// the detail view auto-loads it (no tap) and auto-refreshes while open;
    /// `silent` suppresses the in-flight/again banners so the auto timer does
    /// not spam the fleet banner.
    func driveReadTail(agent: Agent, driveClient: DriveClient, silent: Bool = false) {
        guard let live = currentAgent(for: agent.agentId) else { return }
        guard let signer, let keyId else {
            if !silent { banner = .error("unregistered", "Device is not registered.") }
            return
        }
        guard authorize(.readTail, for: live, silent: silent) else { return }
        let payload = CanonicalJSON.readTailPayload(lines: 200)
        let key = DriveActionKey(capability: .readTail, target: live.agentId, identity: "tail-200")
        guard let requestId = beginDriveAction(key, silent: silent) else { return }
        fleet.prepareTailFetch(agent: live.agentId)
        drive(capability: .readTail, target: live.agentId, payload: payload,
              driveClient: driveClient, keyId: keyId, signer: signer,
              actionKey: key, requestId: requestId)
    }

    /// Outcome of `drivePrompt`. The typed result lets the sheet keep a typed
    /// draft on a real refusal and distinguish it from an in-flight dedup
    /// (same prompt already sending) without sniffing the global banner's
    /// `isError` (re-review P5).
    enum DriveOutcome: Equatable, Sendable {
        case accepted
        case alreadyInFlight
        case refused(String?)
    }

    /// #166 review F7: returns `.accepted` only when the prompt drive was
    /// actually started (all local gates passed). A refused dispatch sets the
    /// banner and returns `.refused(reason)`, so the caller can preserve a
    /// typed draft instead of clearing/dismissing it on a refusal.
    @discardableResult
    func drivePrompt(agent: Agent, text: String, driveClient: DriveClient) -> DriveOutcome {
        guard let live = currentAgent(for: agent.agentId) else {
            return .refused(banner?.message ?? "The prompt was not dispatched.")
        }
        guard let signer, let keyId else {
            banner = .error("unregistered", "Device is not registered.")
            return .refused("Device is not registered.")
        }
        guard !text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else {
            banner = .error("empty_prompt", "Prompt text cannot be empty.")
            return .refused("Prompt text cannot be empty.")
        }
        guard authorize(.prompt, for: live) else {
            return .refused(banner?.message)
        }
        let payload = CanonicalJSON.promptPayload(text: text)
        let key = DriveActionKey(capability: .prompt, target: live.agentId, identity: text)
        guard let requestId = beginDriveAction(key) else {
            return .alreadyInFlight
        }
        drive(capability: .prompt, target: live.agentId, payload: payload,
              driveClient: driveClient, keyId: keyId, signer: signer,
              actionKey: key, requestId: requestId)
        return .accepted
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

    /// `kill` uses the contract's null payload but is destructive by
    /// capability: force the same biometrics -> `/step-up` -> token path even
    /// though the null payload does not match `DestructivePatterns`.
    func driveKill(agent: Agent, driveClient: DriveClient,
                   biometrics: Biometrics = Biometrics()) {
        guard let live = currentAgent(for: agent.agentId) else { return }
        guard let signer, let keyId else {
            banner = .error("unregistered", "Device is not registered.")
            return
        }
        guard authorize(.kill, for: live) else { return }
        let payload = CanonicalJSON.killPayload()
        let key = DriveActionKey(capability: .kill, target: live.agentId,
                                 identity: "kill")
        guard let requestId = beginDriveAction(key) else { return }
        drive(capability: .kill, target: live.agentId, payload: payload,
              driveClient: driveClient, keyId: keyId, signer: signer,
              actionKey: key, requestId: requestId, forceStepUp: true,
              biometrics: biometrics)
    }

    func driveAttach(agent: Agent, driveClient: DriveClient) {
        guard let live = currentAgent(for: agent.agentId) else { return }
        guard let signer, let keyId else {
            banner = .error("unregistered", "Device is not registered.")
            return
        }
        guard authorize(.attach, for: live) else { return }
        let payload = CanonicalJSON.attachPayload()
        let key = DriveActionKey(capability: .attach, target: live.agentId,
                                 identity: "attach")
        guard let requestId = beginDriveAction(key) else { return }
        drive(capability: .attach, target: live.agentId, payload: payload,
              driveClient: driveClient, keyId: keyId, signer: signer,
              actionKey: key, requestId: requestId)
    }

    // MARK: - Older transcript pages (#142 / #64)

    /// Open the newest page. The detail control is already disabled without
    /// capability/grant; this method re-checks so a direct caller cannot
    /// bypass the gate either.
    func openTranscript(agentId: String, driveClient: DriveClient? = nil) {
        requestTranscriptPage(agentId: agentId, cursor: nil,
                              driveClient: driveClient)
    }

    func loadOlderTranscript(agentId: String, driveClient: DriveClient? = nil) {
        guard let pane = fleet.transcript(agentId), let cursor = pane.nextCursor,
              !pane.loading else { return }
        requestTranscriptPage(agentId: agentId, cursor: cursor,
                              driveClient: driveClient)
    }

    func retryTranscript(agentId: String, driveClient: DriveClient? = nil) {
        guard let pane = fleet.transcript(agentId), pane.canRetry else { return }
        requestTranscriptPage(agentId: agentId, cursor: pane.nextCursor,
                              driveClient: driveClient)
    }

    /// #167: the tappable full-width "Load earlier" divider. If no older
    /// transcript page has been fetched yet, open the newest page; otherwise
    /// continue walking the existing cursor.
    func loadEarlierOutput(agentId: String, driveClient: DriveClient? = nil) {
        guard let pane = fleet.transcript(agentId) else {
            openTranscript(agentId: agentId, driveClient: driveClient)
            return
        }
        if pane.nextCursor != nil {
            loadOlderTranscript(agentId: agentId, driveClient: driveClient)
        } else {
            openTranscript(agentId: agentId, driveClient: driveClient)
        }
    }

    private func requestTranscriptPage(agentId: String, cursor: String?,
                                       driveClient: DriveClient?,
                                       autoReload: Bool = false) {
        guard let live = fleet.agent(agentId) else {
            banner = .error("stale_agent",
                            "This agent was deleted or migrated; refresh the fleet before reading its transcript.")
            return
        }
        guard mode == .live else {
            fleet.noteTranscriptFailure(TranscriptFailure(
                kind: "demo",
                message: "Older output is live-only; demo mode does not fetch or fake transcripts.",
                candidates: []
            ), for: agentId)
            return
        }
        guard let signer, let keyId else {
            fleet.noteTranscriptFailure(TranscriptFailure(
                kind: "not_registered",
                message: "Register this device to read full chat.",
                candidates: []
            ), for: agentId)
            banner = .error("unregistered", "Device is not registered.")
            return
        }
        guard authorize(.readTail, for: live) else { return }
        guard let fetch = fleet.prepareTranscriptFetch(agent: agentId,
                                                       cursor: cursor,
                                                       newest: cursor == nil,
                                                       autoReload: autoReload) else {
            return
        }
        let context = lifecycleContext()
        let client = driveClient ?? DriveClient(host: hostURL ?? URL(string: "http://127.0.0.1:8474")!,
                                                session: session)
        let taskId = UUID()
        let task = Task { @MainActor [weak self] in
            guard let self else { return }
            defer {
                self.lifecycleTasks.removeValue(forKey: taskId)
                if Task.isCancelled {
                    self.fleet.cancelTranscriptFetch(agent: agentId,
                                                     generation: fetch.generation)
                }
            }
            guard !Task.isCancelled, self.isCurrent(context) else { return }
            let header = DriveClient.transcriptAuthHeader(keyId: keyId,
                                                          signer: signer,
                                                          target: agentId,
                                                          cursor: fetch.cursor)
            // #167 hard timeout: a stalled older page folds to error+Retry,
            // never an infinite spinner (#160).
            let op: @Sendable () async -> Result<TranscriptPage, TranscriptFailure> = {
                await client.fetchTranscript(agentId: agentId, authHeader: header)
            }
            let result = await Self.raceTimeout(seconds: Self.recentOutputTimeoutSeconds, op)
                ?? .failure(TranscriptFailure(kind: "timeout",
                                              message: "Couldn't load earlier output",
                                              candidates: []))
            guard !Task.isCancelled, self.isCurrent(context) else { return }
            switch result {
            case .success(let page):
                guard self.fleet.foldTranscriptPage(page, for: agentId,
                                                     generation: fetch.generation) else {
                    return
                }
            case .failure(let failure):
                switch self.fleet.foldTranscriptFailure(failure, for: agentId,
                                                        generation: fetch.generation) {
                case .dropped:
                    return
                case .needsReload:
                    self.requestTranscriptPage(agentId: agentId, cursor: nil,
                                               driveClient: client, autoReload: true)
                case .applied, .notGranted:
                    if failure.isNotGranted {
                        self.banner = .error("not_granted", failure.message)
                    }
                }
            }
        }
        lifecycleTasks[taskId] = task
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
    private func authorize(_ capability: Capability, for agent: Agent,
                           silent: Bool = false) -> Bool {
        guard agent.capabilities.contains(capability.rawValue) else {
            if !silent {
                banner = .error("capability_unavailable",
                                "\(capability.rawValue): not available for this agent.")
            }
            return false
        }
        guard actionGrants.contains(capability) else {
            if !silent {
                banner = .error("not_granted",
                                "requires the \(capability.rawValue) grant — ask the host.")
            }
            return false
        }
        return true
    }

    private func beginDriveAction(_ key: DriveActionKey, silent: Bool = false) -> String? {
        guard !inFlightDriveKeys.contains(key) else {
            if !silent {
                banner = .info("\(key.capability.displayName) for \(key.target) is already in progress.")
            }
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

    /// #167 hard timeout for the Recent-output surface (live tail + older
    /// transcript pages). A stalled fetch must fold to error+Retry, never a
    /// spinner (#160).
    private static let recentOutputTimeoutSeconds = 12.0

    /// Race an async operation against a hard timeout. If the operation wins
    /// we get its value; if the timeout wins we get `nil` (the running task
    /// is cancelled). The operation must be cancellable (URLSession is).
    private static func raceTimeout<T: Sendable>(
        seconds: Double,
        _ operation: @escaping @Sendable () async -> T
    ) async -> T? {
        await withTaskGroup(of: Optional<T>.self) { group in
            group.addTask { await operation() }
            group.addTask {
                try? await Task.sleep(nanoseconds: UInt64(seconds * 1_000_000_000))
                return nil
            }
            for await value in group {
                group.cancelAll()
                return value
            }
            return nil
        }
    }

    private func drive(capability: Capability, target: String, payload: CanonicalJSON.Value,
                       driveClient: DriveClient, keyId: String, signer: DeviceSigner,
                       actionKey: DriveActionKey, requestId: String,
                       forceStepUp: Bool = false,
                       biometrics: Biometrics = Biometrics()) {
        let context = lifecycleContext()
        let task = Task { @MainActor [weak self] in
            guard let self else { return }
            defer {
                if self.lifecycleGeneration == context.generation {
                    self.inFlightDriveKeys.remove(actionKey)
                    self.driveTasks.removeValue(forKey: requestId)
                }
            }
            guard !Task.isCancelled, self.isCurrent(context) else { return }
            let rev = self.fleet.lastEventId
            let result: DriveResult
            if capability == .readTail {
                // #167 hard timeout: a stalled tail must never leave the
                // Recent-output surface on an infinite spinner.
                let op: @Sendable () async -> DriveResult = {
                    await driveClient.drive(capability: capability, target: target,
                                            payload: payload, rev: rev,
                                            requestId: requestId,
                                            keyId: keyId, signer: signer,
                                            biometrics: biometrics,
                                            forceStepUp: forceStepUp)
                }
                result = await Self.raceTimeout(seconds: Self.recentOutputTimeoutSeconds, op)
                    ?? .refused(.network("Recent output timed out."))
            } else {
                result = await driveClient.drive(capability: capability, target: target,
                                                 payload: payload, rev: rev,
                                                 requestId: requestId,
                                                 keyId: keyId, signer: signer,
                                                 biometrics: biometrics,
                                                 forceStepUp: forceStepUp)
            }
            guard !Task.isCancelled, self.isCurrent(context) else { return }
            switch result {
            case .dispatched(let response):
                if response.ok {
                    if capability == .readTail {
                        let lines = response.result?.tailLines ?? []
                        let blocks = response.result?.tailBlocks ?? []
                        // #167: fold the segmented blocks; the result stays in
                        // the detail view (no hijacking fleet banner).
                        self.fleet.rememberTail(lines, blocks: blocks, for: target)
                    } else if capability == .approve {
                        self.banner = .info("Approved \(target): rev \(response.rev)")
                    } else if capability == .prompt {
                        self.banner = .info("Prompt sent to \(target): rev \(response.rev)")
                    } else if capability == .interrupt {
                        self.banner = .info("Interrupted \(target): rev \(response.rev)")
                    } else if capability == .kill {
                        self.banner = .info("Killed \(target): rev \(response.rev)")
                    } else if capability == .attach {
                        self.banner = .info("Attached \(target): rev \(response.rev)")
                    }
                } else {
                    if response.errorKind == "stale_agent" {
                        self.handleStaleAgent(target,
                                              message: response.error ?? "the agent moved or disappeared")
                    } else if capability == .readTail {
                        self.fleet.foldTailFailure(TranscriptFailure(
                            kind: response.errorKind ?? "dispatch_refused",
                            message: response.error ?? "dispatch refused (ok:false)",
                            candidates: []), for: target)
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
                    if capability == .readTail {
                        self.fleet.foldTailFailure(TranscriptFailure(
                            kind: kind, message: message, candidates: []), for: target)
                        return
                    }
                    // Read-only default: ungranted capabilities are refused
                    // with the typed banner; the UI also explains disabled
                    // controls before this path is reachable.
                    self.banner = .error(kind, "\(message) (HTTP \(status))")
                case .network(let message):
                    if capability == .readTail {
                        self.fleet.foldTailFailure(TranscriptFailure(
                            kind: "transport", message: message, candidates: []), for: target)
                    } else {
                        self.banner = .error("network", message)
                    }
                case .encoding:
                    if capability == .readTail {
                        self.fleet.foldTailFailure(TranscriptFailure(
                            kind: "encoding", message: "payload encoding failed",
                            candidates: []), for: target)
                    } else {
                        self.banner = .error("encoding", "payload encoding failed")
                    }
                }
            }
        }
        driveTasks[requestId] = task
    }

    /// Invalidate the current identity before cancelling handles. Results from
    /// cancellation races then fail the generation check even if URLSession
    /// delivers one final callback on another queue.
    private func cancelLifecycleTasks() {
        lifecycleGeneration &+= 1
        for task in driveTasks.values {
            task.cancel()
        }
        for task in lifecycleTasks.values {
            task.cancel()
        }
        driveTasks.removeAll()
        lifecycleTasks.removeAll()
        registrationTaskId = nil
        inFlightDriveKeys.removeAll()
        // Do not leave the shared delegate context live while reset/demo is
        // still clearing the model. A callback arriving on another queue must
        // be refused during this transition; register() immediately replaces
        // this with its pre-registration context.
        identityLifecycle.invalidate(mode: .registering, hostURL: nil,
                                    keyId: nil, signerPublicKeyB64: nil)
    }

    /// Stale target handling is deliberately shared by the HTTP 409 path and
    /// the narrow 200 dispatch-race path: remove controls immediately, then
    /// fetch the current read model once.
    private func handleStaleAgent(_ target: String, message: String) {
        fleet.removeAgent(target)
        banner = .error("stale_agent", "\(message) — refreshing the fleet.")
        let context = lifecycleContext()
        guard context.mode == .live, let hostURL = context.hostURL else { return }
        let client = CorraldClient(host: hostURL, session: session)
        let taskId = UUID()
        let task = Task { @MainActor [weak self] in
            guard let self else { return }
            defer { self.lifecycleTasks.removeValue(forKey: taskId) }
            guard self.isCurrent(context) else { return }
            guard let snapshot = try? await client.fetchSnapshot() else { return }
            guard !Task.isCancelled, self.isCurrent(context) else { return }
            self.fleet.apply(.snapshot(snapshot))
        }
        lifecycleTasks[taskId] = task
    }

#if DEBUG
    // MARK: - Demo mode (Debug only)

    func enterDemo() {
        cancelLifecycleTasks()
        fleet.disconnect()
        fleet.reset()
        fleet.seedDemo(agents: DemoFleet.seed(), rev: 1)
        mode = .demo
        identityLifecycle.setCurrent(mode: .demo, hostURL: hostURL,
                                    keyId: keyId,
                                    signerPublicKeyB64: signer?.publicKeyB64)
    }

    /// Leave demo through the same identity boundary as reset/registration.
    /// Demo rows and their cursor are discarded before a live identity is
    /// restored, so the next connection must receive a fresh snapshot.
    func exitDemo() {
        guard mode == .demo else { return }
        cancelLifecycleTasks()
        fleet.reset()

        guard let identity = persistedLiveIdentity() else {
            signer = nil
            keyId = nil
            grants = []
            hostURL = nil
            keyStorageWarning = false
            notificationsConfigured = false
            mode = .needsSetup
            identityLifecycle.setCurrent(mode: .needsSetup, hostURL: nil,
                                         keyId: nil, signerPublicKeyB64: nil)
            return
        }

        // This is synchronous on the main actor: local fields and the shared
        // delegate context are updated as one transition before startLive()
        // can create any new live work.
        applyLiveIdentity(identity)
        startLive()
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
                fleet.rememberTail(response.result?.tailLines ?? [],
                                   blocks: response.result?.tailBlocks ?? [],
                                   for: agent.agentId)
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

#endif

    // MARK: - Identity management

    func resetDevice() {
        cancelLifecycleTasks()
        AppDelegate.shared?.clearRetainedDeviceToken()
        stopLive()
        wipeIdentity()
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
        identityLifecycle.setCurrent(mode: .needsSetup, hostURL: nil,
                                    keyId: nil, signerPublicKeyB64: nil)
    }
}

/// Where the app stands on the wire: registered device + host.
struct DeviceRegistration: Equatable {
    var host: String
    var keyId: String
    var grants: [String]
}
