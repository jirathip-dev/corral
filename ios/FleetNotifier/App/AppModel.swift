import Combine
import Foundation
import SwiftUI
import UIKit

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
/// `kind` is the daemon's typed error (`not_granted`, `stale_agent`, …).
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

/// One logical read drive. The identity is stable across duplicated row
/// surfaces, so a double tap cannot create two signed request ids.
private struct DriveActionKey: Hashable, Sendable {
    let capability: Capability
    let target: String
    let identity: String
}

/// #364 C: a recents-sheet presentation request. Carries a monotonic id so
/// every request — including a re-tap of the SAME agent — is a distinct
/// value for the sheet's `.sheet(item:)` binding: after a dismissal the
/// request is nil again, so the next open is a real nil → request
/// transition SwiftUI always presents (the pre-#364 sticky latch compared
/// equal and swallowed first taps after dismissal).
struct RecentsRequest: Identifiable, Equatable, Sendable {
    let id: UInt64
    let agentId: String
}

/// App-level orchestration for the READ-ONLY client (#354 L2): identity,
/// registration, live connection, state-change notification hooks, the
/// signed read_tail drive shared by the recents sheet, and the deep link
/// that opens an agent's recents from a notification tap.
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

    private final class TimeoutState<T: Sendable>: @unchecked Sendable {
        private let lock = NSLock()
        private var continuation: CheckedContinuation<T?, Never>?
        private var finished = false
        private var operationTask: Task<Void, Never>?
        private var timeoutTask: Task<Void, Never>?

        func install(_ continuation: CheckedContinuation<T?, Never>) {
            lock.lock()
            guard !finished else {
                lock.unlock()
                continuation.resume(returning: nil)
                return
            }
            self.continuation = continuation
            lock.unlock()
        }

        func install(operationTask: Task<Void, Never>,
                     timeoutTask: Task<Void, Never>) {
            lock.lock()
            guard !finished else {
                lock.unlock()
                operationTask.cancel()
                timeoutTask.cancel()
                return
            }
            self.operationTask = operationTask
            self.timeoutTask = timeoutTask
            lock.unlock()
        }

        func finish(_ value: T?) {
            lock.lock()
            guard !finished else {
                lock.unlock()
                return
            }
            finished = true
            let continuation = self.continuation
            self.continuation = nil
            let operationTask = self.operationTask
            let timeoutTask = self.timeoutTask
            lock.unlock()
            operationTask?.cancel()
            timeoutTask?.cancel()
            continuation?.resume(returning: value)
        }
    }

    @Published var mode: Mode = .needsSetup
    @Published var fleet: FleetStore
    @Published var banner: DriveBanner?
    @Published var grants: [String] = []
    @Published var keyId: String?
    @Published var hostURL: URL?
    @Published var keyStorageWarning: Bool = false
    /// #364 B: the board's repo-chip selection (nil = All). Model-owned so
    /// the filter survives pull-to-refresh and tab/foreground refresh; the
    /// board reconciles it against the current fleet via
    /// `BoardModel.reconcile` (a vanished repo renders as All without
    /// losing the user's last choice).
    @Published var repoFilter: String?
    /// #364 C: the recents-sheet presentation request. Model-owned so the
    /// board's `.sheet(item:)` binds straight to it and every open request
    /// — including a re-tap of the SAME agent after a dismissal — is a
    /// brand-new value (nil → request). The previous design latched a
    /// sticky `recentsAgentId` behind an equality-guarded onChange: SwiftUI
    /// auto-cleared only the view's item binding on dismissal, so a repeat
    /// request compared equal and the first tap after a dismissal was
    /// swallowed (see `recentsSheetDismissed`).
    @Published var recentsRequest: RecentsRequest?
    /// Global notifications on/off (Settings → Notifications pairing).
    @Published var notificationsEnabled: Bool

    var signer: DeviceSigner?
    private var notifier: LocalNotifier?
    /// #364 A.2: one-shot selection haptic for discrete board actions (row
    /// tap, Done close). Injectable so tests can count ticks; never called
    /// from drag/scroll paths.
    private let hapticTick: () -> Void
    /// Monotonic counter backing `RecentsRequest.id`.
    private var recentsSerial: UInt64 = 0
    /// #79 review F4: one-shot guard for the non-idempotent half of startLive().
    private var notificationsConfigured = false
    /// Every live read gets its own task handle. A mode/device boundary must
    /// cancel all of them.
    private var driveTasks: [String: Task<Void, Never>] = [:]
    private var driveTaskKeys: [String: DriveActionKey] = [:]
    @Published private var inFlightDriveKeys: Set<DriveActionKey> = []
    private var lifecycleGeneration = 0
    /// Notification deep links and grants refreshes suspend outside the
    /// model. Track every one so a mode or identity boundary can cancel the
    /// complete set, not just the latest.
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

    static let notificationsKey = "fleetnotifier.notificationsEnabled"

    /// The device's current read grant set, decoded from the daemon's
    /// register/grants-read responses. Demo mode (Debug only) treats the
    /// seeded fleet as fully readable.
    var actionGrants: Set<Capability> {
#if DEBUG
        if mode == .demo {
            return [.readTail, .readDiff]
        }
#endif
        return Set(grants.compactMap(Capability.init(rawValue:)))
    }

    /// #388: the device is REGISTERED once it holds an identity key — set
    /// by a successful registration (or a restored live identity) and
    /// cleared by Remove device. The Connection section reads this to hide
    /// the pointless Registration-token field once paired. The published
    /// key id IS the registration truth — a DEBUG demo launch over a
    /// paired keychain keeps the registered posture without implying a
    /// live transport (mode stays .demo, so no live networking starts).
    var isRegistered: Bool { keyId != nil }

    /// #166 review F13: single shared DriveClient constructor for the view
    /// layer's read call sites. Uses the registered host URL, falling back
    /// to the documented localhost default, and the default `.shared`
    /// URLSession (the injected `session` is only for tests).
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
         },
         haptics: @escaping () -> Void = Haptics.selection) {
        self.session = session
        self.identityLifecycle = identityLifecycle
        self.defaults = defaults
        self.identityLoader = identityLoader
        self.loadMeta = loadMeta
        self.saveMeta = saveMeta
        self.wipeIdentity = wipeIdentity
        self.hapticTick = haptics
        self.notificationsEnabled = defaults.object(forKey: Self.notificationsKey) as? Bool ?? true
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
        // #365: an ALREADY-paired device re-pointing at a DIFFERENT host
        // must drop the current host's SSE stream before the new pairing —
        // FleetStore.connect() is a no-op while a stream runs, so without
        // this the registration would claim the new host while the board
        // kept streaming the old one forever.
        let switchingHost = hostURL.map { $0.absoluteString != url.absoluteString } ?? false
        cancelLifecycleTasks()
        if switchingHost {
            stopLive()
        }
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
                    // #209: the device name rides along as the cosmetic
                    // host-side label (display only — the key signs).
                    let response = try await client.register(token: token, signer: candidateSigner,
                                                            name: UIDevice.current.name)
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
                    self.banner = .info("Registered \(response.keyId.prefix(12))… read-only (grants: \(response.grants.isEmpty ? "none" : response.grants.joined(separator: ", ")))")
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
                    // #365: a FAILED host switch must not leave a paired
                    // board dead — mode and hostURL are still the
                    // pre-registration values here, so restarting the OLD
                    // host's stream (dropped above when the hosts differed)
                    // keeps the last-known board live behind the banner.
                    if baseContext.mode == .live {
                        self.startLive()
                    }
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
            // #365: same failure-restart contract as the request catch above
            // (this path covers identity-loader failures).
            if baseContext.mode == .live {
                startLive()
            }
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

    /// Issue #219: one pull/toolbar snapshot refresh in flight. Guards
    /// coalescing (repeated pulls share one request) and drives the
    /// Refresh button's spinner state; the native `.refreshable` spinner
    /// is fed by the same await.
    @Published private(set) var isRefreshingFleet = false

    func startLive() {
        guard let hostURL else { return }
        let client = CorraldClient(host: hostURL, session: session)
        // #354 L2: state-change notification hooks (started / blocked /
        // finished), fired by FleetStore on SSE deltas.
        fleet.onStarted = { [weak self] agentId in
            self?.notifyTransition(.started, agentId: agentId)
        }
        fleet.onBlocked = { [weak self] agentId in
            self?.notifyTransition(.blocked, agentId: agentId)
        }
        fleet.onFinished = { [weak self] agentId in
            self?.notifyTransition(.finished, agentId: agentId)
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
        notifier?.isEnabled = notificationsEnabled
        // This OS permission request captures no host, key, fleet, or mode;
        // it cannot apply stale lifecycle state after a boundary.
        let notifierForAuthorization = notifier
        Task { await notifierForAuthorization?.requestAuthorization() }
        // #354 L2: notification tap → deep link to the agent row's recents.
        notifier?.onOpenAgent = { [weak self] agentId in
            self?.openRecents(for: agentId)
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

    // MARK: - Fleet refresh (#219)

    /// Pull-to-refresh / foreground refresh: ONE authoritative snapshot,
    /// serialized and coalesced (a second pull while one is in flight is
    /// a no-op — no duplicate stream tasks, no reordering). The result
    /// reconciles through `FleetStore.applyRefresh`, which enforces
    /// snapshot/delta revision ordering against the live stream and
    /// resumes the stream from the newest accepted revision via the
    /// shared cursor. Failure ends the indicator and lands in the
    /// existing dismissible/retryable banner — never an endless spinner
    /// (`isRefreshingFleet` is cleared in `defer`).
    func refreshFleet() async {
        guard mode == .live, let hostURL else { return }
        guard !isRefreshingFleet else { return }
        isRefreshingFleet = true
        defer { isRefreshingFleet = false }
        let context = lifecycleContext()
        let client = CorraldClient(host: hostURL, session: session)
        do {
            let snapshot = try await client.fetchSnapshot()
            guard !Task.isCancelled, isCurrent(context) else { return }
            fleet.applyRefresh(snapshot)
        } catch {
            guard !Task.isCancelled, isCurrent(context) else { return }
            banner = .error("fleet_refresh",
                            "Fleet refresh failed — \(error.localizedDescription)")
        }
    }

    // MARK: - Notifications (#354 L2)

    /// The transition hooks. The LOCAL path fires only through the DEBUG
    /// bridge (`PushBridge`); release builds rely on APNs — the daemon
    /// pushes the same payload once the APNs provisioning checkpoint is met.
    private func notifyTransition(_ type: PushPayload.PushType, agentId: String) {
        guard let agent = fleet.agent(agentId) else { return }
        guard PushBridge.shouldPresentLocally else { return }
        let payload = PushPayload.transition(type: type, agent: agent)
        notifier?.notify(payload)
    }

    /// Notification-pairing toggle (Settings). Global on/off only — no
    /// per-agent controls, no catch-up/badge on foreground.
    func setNotificationsEnabled(_ enabled: Bool) {
        notificationsEnabled = enabled
        defaults.set(enabled, forKey: Self.notificationsKey)
        notifier?.isEnabled = enabled
    }

    /// Deep link from a tapped notification: open the agent's row recents.
    /// Live mode only — setup/demo states have no live agent to show. A
    /// deep link is not a row tap, so it plays no haptic.
    func openRecents(for agentId: String) {
        guard mode == .live else { return }
        guard fleet.agent(agentId) != nil else {
            banner = .info("This agent is no longer on the fleet — refresh the board.")
            return
        }
        requestRecents(for: agentId, haptic: false)
    }

    // MARK: - Recents sheet request lifecycle (#364 C)

    /// Every board/notification/demo open request funnels through here:
    /// the request is ALWAYS a fresh value with a monotonic id, so a
    /// re-request of the agent currently (or previously) shown is a real
    /// nil → request transition for `.sheet(item:)` — the first tap after
    /// any dismissal re-presents. `haptic: true` is reserved for real row
    /// taps (one light selection tick); programmatic opens stay silent.
    func requestRecents(for agentId: String, haptic: Bool) {
        guard fleet.agent(agentId) != nil else { return }
        if haptic { hapticTick() }
        recentsSerial += 1
        recentsRequest = RecentsRequest(id: recentsSerial, agentId: agentId)
    }

    /// The recents sheet finished dismissing (swipe-down or Done — the
    /// board's `.sheet(item:onDismiss:)` calls this). SwiftUI already
    /// wrote `recentsRequest` back to nil when the dismissal started; a
    /// request that landed DURING the dismissal (SwiftUI drops
    /// presentations issued while a dismissal transition is running) is
    /// still pending here, so re-arm it with a fresh id and the completed
    /// dismissal presents it immediately. With nothing pending the latch
    /// stays nil — the next open, same agent included, is a brand-new
    /// request that always presents.
    func recentsSheetDismissed() {
        guard let pending = recentsRequest else { return }
        recentsSerial += 1
        recentsRequest = RecentsRequest(id: recentsSerial, agentId: pending.agentId)
    }

    /// #364 A.2: the sheet's Done (close) control was tapped — one light
    /// selection tick before the dismissal starts. Swipe-down dismissals
    /// deliberately play nothing (drag gestures must never tick).
    func closeRecentsButtonTapped() {
        hapticTick()
    }

    // MARK: - Read drives (read_tail)

    /// `read_tail` is bounded (D5): the daemon's 200-line cap is the whole
    /// history (recents v1 = LIVE TAIL ONLY). `silent` suppresses the
    /// in-flight/again banners so the auto timer does not spam the fleet
    /// banner.
    func driveReadTail(agent: Agent, driveClient: DriveClient, silent: Bool = false,
                       lines: UInt32 = 200) {
        guard let live = fleet.agent(agent.agentId) else { return }
        guard let signer, let keyId else {
            if !silent { banner = .error("unregistered", "Device is not registered.") }
            return
        }
        guard authorize(.readTail, for: live, silent: silent) else { return }
        let sinceRev = fleet.tailPane(for: live.agentId)?.sourceRev
        let payload = CanonicalJSON.readTailPayload(lines: lines, sinceRev: sinceRev)
        let key = DriveActionKey(capability: .readTail, target: live.agentId,
                                 identity: "tail-\(lines)")
        guard let requestId = beginDriveAction(key, silent: silent) else { return }
        fleet.prepareTailFetch(agent: live.agentId)
        drive(capability: .readTail, target: live.agentId, payload: payload,
              driveClient: driveClient, keyId: keyId, signer: signer,
              actionKey: key, requestId: requestId)
    }

    /// Resolve a drive's target from the current read model. A recents sheet
    /// may outlive a delta deletion; in that case no signed bytes are built.
    private func currentAgent(for agentId: String) -> Agent? {
        guard let agent = fleet.agent(agentId) else {
            banner = .error("stale_agent", "This agent was deleted or migrated; refresh the fleet.")
            return nil
        }
        return agent
    }

    /// Both sides of the drive authorization contract must hold locally for
    /// a read control. The daemon remains authoritative and can still
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
                banner = .info("\(key.capability.rawValue) for \(key.target) is already in progress.")
            }
            return nil
        }
        inFlightDriveKeys.insert(key)
        return DriveClient.newRequestId()
    }

    /// #167 hard timeout for the Recent-output surface (live tail). A
    /// stalled fetch must fold to error+Retry, never a spinner (#160).
    private static let readTimeoutSeconds = 12.0

    /// Race an async operation against a hard timeout. Cancellation finishes
    /// the race immediately and cancels both child tasks, so dismissing a
    /// sheet cannot wait for a transport that ignores cancellation.
    private static func raceTimeout<T: Sendable>(
        seconds: Double,
        _ operation: @escaping @Sendable () async -> T
    ) async -> T? {
        let state = TimeoutState<T>()
        return await withTaskCancellationHandler(operation: {
            await withCheckedContinuation { (continuation: CheckedContinuation<T?, Never>) in
                state.install(continuation)
                let operationTask = Task.detached {
                    state.finish(await operation())
                }
                let timeoutTask = Task.detached {
                    do {
                        try await Task.sleep(nanoseconds: UInt64(seconds * 1_000_000_000))
                    } catch {
                        return
                    }
                    state.finish(nil)
                }
                state.install(operationTask: operationTask, timeoutTask: timeoutTask)
            }
        }, onCancel: {
            state.finish(nil)
        })
    }

    private func drive(capability: Capability, target: String, payload: CanonicalJSON.Value,
                       driveClient: DriveClient, keyId: String, signer: DeviceSigner,
                       actionKey: DriveActionKey, requestId: String) {
        let context = lifecycleContext()
        let task = Task { @MainActor [weak self] in
            guard let self else { return }
            defer {
                if self.lifecycleGeneration == context.generation,
                   self.driveTaskKeys[requestId] == actionKey {
                    self.inFlightDriveKeys.remove(actionKey)
                    self.driveTasks.removeValue(forKey: requestId)
                    self.driveTaskKeys.removeValue(forKey: requestId)
                }
            }
            guard !Task.isCancelled, self.isCurrent(context) else { return }
            let rev = self.fleet.lastEventId
            // #167: hard timeout — a stalled read must never leave the
            // recents pane on an infinite spinner.
            let op: @Sendable () async -> DriveResult = {
                await driveClient.drive(capability: capability, target: target,
                                        payload: payload, rev: rev,
                                        requestId: requestId,
                                        keyId: keyId, signer: signer)
            }
            let result = await Self.raceTimeout(seconds: Self.readTimeoutSeconds, op)
                ?? .refused(.network("Recent output timed out."))
            guard !Task.isCancelled, self.isCurrent(context) else { return }
            switch result {
            case .dispatched(let response):
                if response.ok {
                    let lines = response.result?.tailLines ?? []
                    let blocks = response.result?.tailBlocks ?? []
                    // Fold the segmented blocks; the result stays in the
                    // sheet (no hijacking fleet banner).
                    self.fleet.rememberTail(lines, blocks: blocks,
                                            sourceRev: response.result?.tailSourceRev ?? response.rev,
                                            for: target)
                } else {
                    if response.errorKind == "stale_agent" {
                        self.handleStaleAgent(target,
                                              message: response.error ?? "the agent moved or disappeared")
                    } else {
                        self.fleet.foldTailFailure(TranscriptFailure(
                            kind: response.errorKind ?? "dispatch_refused",
                            message: response.error ?? "dispatch refused (ok:false)",
                            candidates: []), for: target)
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
                    // with the typed banner.
                    self.banner = .error(kind, "\(message) (HTTP \(status))")
                case .network(let message):
                    if capability == .readTail {
                        self.fleet.foldTailFailure(TranscriptFailure(
                            kind: message == "Recent output timed out." ? "timeout" : "transport",
                            message: message, candidates: []), for: target)
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
        driveTaskKeys[requestId] = actionKey
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
        driveTaskKeys.removeAll()
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
    /// the narrow 200 dispatch-race path: remove the row immediately, then
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

    @Published var demoDetailAgentId: String?

    func enterDemo(detailAgentId: String? = nil) {
        cancelLifecycleTasks()
        fleet.disconnect()
        fleet.reset()
        fleet.seedDemo(agents: DemoFleet.seed(), rev: 1)
        mode = .demo
        demoDetailAgentId = detailAgentId
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
        demoDetailAgentId = nil

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

    /// Demo read tail: answered locally from the seeded fixture.
    func driveDemoReadTail(agent: Agent) {
        let result = DemoFleet.respond(to: .readTail, agent: agent,
                                       rev: (fleet.lastEventId ?? 1) + 1)
        if case .dispatched(let response) = result {
            fleet.seedDemo(agents: fleet.agents, rev: response.rev)
            fleet.rememberTail(response.result?.tailLines ?? [],
                               blocks: response.result?.tailBlocks ?? [],
                               for: agent.agentId)
        }
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
        // token reaches the new daemon and the deep-link path re-arms.
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
