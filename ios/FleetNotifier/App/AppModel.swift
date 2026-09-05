import Combine
import Foundation
import SwiftUI
import UIKit
import os

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
/// #400 C2/E1: the key is scoped by the composite identity's host profile,
/// so two hosts driving an EQUAL raw target id never block or collide.
private struct DriveActionKey: Hashable, Sendable {
    let capability: Capability
    let target: String
    let identity: String
    /// Owning host profile (nil = the legacy single-host runtime).
    let hostProfileID: UUID?
}

/// #364 C: a recents-sheet presentation request. Carries a monotonic id so
/// every request — including a re-tap of the SAME agent — is a distinct
/// value for the sheet's `.sheet(item:)` binding: after a dismissal the
/// request is nil again, so the next open is a real nil → request
/// transition SwiftUI always presents (the pre-#364 sticky latch compared
/// equal and swallowed first taps after dismissal).
/// #400 E1: the request carries the COMPOSITE identity (host_profile_id +
/// raw agent id) — the sheet resolves EXACTLY the owning host's store, so
/// an equal raw id on another host can never be opened or driven.
struct RecentsRequest: Identifiable, Equatable, Sendable {
    let id: UInt64
    /// Raw agent id unchanged from the wire.
    let agentId: String
    /// The profile the row belongs to (nil = legacy single-host runtime /
    /// demo, which resolve to the active store).
    let hostProfileID: UUID?

    init(id: UInt64, agentId: String, hostProfileID: UUID?) {
        self.id = id
        self.agentId = agentId
        self.hostProfileID = hostProfileID
    }
}

/// #400 E2: the recents-sheet availability of one composite target.
enum RecentsRouteState: Equatable, Sendable {
    /// The owning host is connected — reloads are permitted.
    case live
    /// The owning host disconnected while output was already loaded: the
    /// loaded content stays visible with an Offline marker and reload is
    /// disabled until reconnection.
    case offline
    /// The owning host is disconnected and nothing was loaded: show
    /// unavailable; never synthesize or load persisted content.
    case unavailable
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

    /// #399 B4: key-continuity posture of the ACTIVE pinned profile.
    /// `.pending` = `/host-key` must be re-checked before any live work;
    /// `.verified` = checked and matched since launch/foreground;
    /// `.mismatch` = the host presented a different key — everything
    /// fails closed until Remove Host + fresh pairing. `.notPinned`
    /// covers legacy single-host flows (no pin = no continuity gate).
    enum KeyContinuityState: Equatable, Sendable {
        case notPinned
        case pending
        case verified
        case mismatch
    }

    /// #399 B6/B3: the sheet request that pauses a host profile on the
    /// fingerprint confirmation. Presented by the board until answered.
    struct FingerprintConfirmationRequest: Identifiable, Equatable, Sendable {
        /// Sheet identity — a fresh value per presentation.
        let id: UUID
        let profileID: UUID
        let profileName: String
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
    /// #401 D1/D2: the board's host-chip selection (nil = All Hosts). Like
    /// `repoFilter` it is model-owned so the choice survives pull-to-refresh
    /// and tab/foreground refresh; it is SESSION-ONLY (never persisted —
    /// every fresh launch starts at All Hosts + All Repos, D1) and the
    /// board reconciles it when the selected host is removed (renders All).
    @Published var hostFilter: UUID?
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
    /// #389: the OS notification-permission posture the Settings Notifications
    /// section renders from. Refreshed when Settings appears / the app
    /// re-activates and updated by the permission-aware enable flow, so a
    /// blocked permission shows the why + 'Open iOS Settings' guidance
    /// instead of the enable toggle silently failing.
    @Published var notificationPermission: NotificationPermissionState = .notDetermined
    /// #399: ordered host profiles (B1) — the multi-host STORE alongside
    /// the single-host state. Empty until the profile store is configured
    /// (production app always configures it; unit fixtures may not).
    @Published private(set) var profiles: [HostProfile] = []
    /// #399: the profile the single-host runtime is currently bound to
    /// (V1: exactly one live stream; #400 consumes the store for N).
    @Published private(set) var activeProfileID: UUID?
    /// #399 B4: continuity posture of the active pinned profile.
    @Published private(set) var keyContinuityState: KeyContinuityState = .notPinned
    /// #399 B6: non-nil while a profile is paused awaiting fingerprint
    /// confirmation (legacy migration) — the board presents the sheet.
    @Published var fingerprintConfirmation: FingerprintConfirmationRequest?
    /// #415: the scene-scoped Add Host draft — host name, URL, the
    /// one-time registration token, and the current host-key verification
    /// phase (`prepared` non-nil = fingerprint confirmation). Owned by the
    /// MODEL (never sheet @State) so app-switch/return and sheet
    /// view-identity churn from normal scene lifecycle updates cannot wipe
    /// a partially entered pairing; the draft lives as long as the app
    /// scene. The token is TRANSIENT in-memory state — never persisted,
    /// never logged, never printed. Cleared on Cancel and only AFTER a
    /// successful profile commit.
    @Published var addHostDraft = AddHostDraft()
    /// #400: the per-host stream coordinator for every NON-active profile
    /// (the ACTIVE profile keeps the legacy single-host runtime fields
    /// below for F1 parity). Each coordinator session owns one independent
    /// stream/cursor/generation/task set (C3); nil when no profile store
    /// is configured (legacy-only fixtures).
    @Published private(set) var coordinator: HostStreamCoordinator?
    /// #397: host profiles whose APNs token clear is still pending because
    /// the host was unreachable when enrollment was disabled / the host
    /// was removed. Retried when that host reconnects (see
    /// `retryPendingPushTokenClear`); Settings surfaces the guidance.
    @Published private(set) var pendingPushTokenClears: Set<UUID> = []

    /// #397: the enrollment target of every pending token clear, captured
    /// at schedule time — a REMOVED profile can no longer be resolved
    /// through the store, but its clear/guidance needs the URL, key id,
    /// and display name.
    private struct PendingPushClearTarget: Equatable, Sendable {
        let urlString: String
        let keyId: String
        let displayName: String
    }

    private var pendingPushClearTargets: [UUID: PendingPushClearTarget] = [:]
    /// #397: per-host dedupe of APNs token uploads (the token this process
    /// retained was already enrolled with a host under this key id).
    private var enrolledTokenPerHost: [UUID: String] = [:]

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
    /// #389: permission queries/prompts for the notification enable flow.
    private let notificationPermissionProvider: NotificationPermissionProviding
    private let identityLoader: @Sendable () throws -> (DeviceSigner, DeviceKeyStore.Storage)
    private let loadMeta: @Sendable () -> DeviceKeyStore.DeviceMeta?
    private let saveMeta: @Sendable (DeviceKeyStore.DeviceMeta) -> Void
    private let wipeIdentity: @Sendable () -> Void
    /// #399 B6: consumes the legacy registration metadata after a
    /// successful migration into the profile store.
    private let removeMeta: @Sendable () -> Void
    /// #399: the host-profile store (nil = legacy single-host-only
    /// fixtures; the production app always configures one).
    private let profileStore: HostProfileStore?

    private static let activeProfileKey = "fleetnotifier.activeHostProfileID"

    /// #93: `fleet` is a NESTED `ObservableObject`. `@Published` fires only
    /// when the REFERENCE is reassigned — it does not forward the child's
    /// `objectWillChange`. Every view observes `AppModel` but reads
    /// `model.fleet.agents` / `model.fleet.connectionState`, so applying an
    /// SSE frame used to mutate the store without re-running any `body`.
    /// Forward the child's change notifications to this object so the board
    /// re-renders when the fleet changes.
    private var fleetChanges: AnyCancellable?

    static let notificationsKey = "fleetnotifier.notificationsEnabled"
    private static let log = Logger(subsystem: "com.corral.fleetnotifier", category: "host-profiles")

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
         removeMeta: @escaping @Sendable () -> Void = {
             DeviceKeyStore.removeRegistrationMetadata()
         },
         profileStore: HostProfileStore? = nil,
         haptics: @escaping () -> Void = Haptics.selection,
         notificationPermissionProvider: NotificationPermissionProviding = SystemNotificationPermissionProvider()) {
        self.session = session
        self.identityLifecycle = identityLifecycle
        self.defaults = defaults
        self.notificationPermissionProvider = notificationPermissionProvider
        self.identityLoader = identityLoader
        self.loadMeta = loadMeta
        self.saveMeta = saveMeta
        self.wipeIdentity = wipeIdentity
        self.removeMeta = removeMeta
        self.profileStore = profileStore
        self.hapticTick = haptics
        self.notificationsEnabled = defaults.object(forKey: Self.notificationsKey) as? Bool ?? true
        self.fleet = FleetStore(defaults: defaults)
        fleetChanges = fleet.objectWillChange.sink { [weak self] _ in
            self?.objectWillChange.send()
        }
        // #399 B4: the APNs upload path consults the app-wide continuity
        // gate; install the model predicate only when a profile store is
        // configured (legacy-only fixtures keep the default allow).
        // #397: the gate now reflects the ACTIVE host's OWN posture — the
        // 2+-profile blanket denial is gone (every paired host enrolls
        // independently); a token still never reaches a host whose pinned
        // identity is unverified/mismatched, and a per-host notification
        // DISABLE also stops the active host from enrolling.
        if profileStore != nil {
            KeyContinuityGate.setPushPredicate { [weak self] in
                guard let self else { return true }
                return await MainActor.run {
                    guard self.keyContinuityAllowsLiveWork else { return false }
                    return self.activeProfile?.notificationsEnabled ?? true
                }
            }
        }
        // #400 C3/E3: the per-host coordinator owns every NON-active
        // profile's stream lifecycle. Created up front (profile mutations
        // reconcile sessions through it); the ACTIVE profile is excluded
        // and keeps the legacy runtime for F1 parity.
        if let profileStore {
            let coordinator = HostStreamCoordinator(
                defaults: defaults,
                session: session,
                profileStore: profileStore,
                signerProvider: { [weak self] in self?.signer })
            coordinator.activeProfileID = activeProfileID
            coordinator.onSessionConnected = { [weak self] profileID in
                self?.retryPendingPushTokenClear(profileID: profileID)
                // #397: a coordinator host that JUST verified + opened its
                // stream may have missed the launch-time fan-out — enroll
                // the retained token now (per-host dedupe makes this a
                // no-op for already-enrolled hosts).
                self?.uploadRetainedTokenToHosts()
            }
            // #397: per-host state-change notifications fire through the
            // same model path as the ACTIVE host's store hooks — composite
            // (profile, raw agent id) payloads, per-host enable honored.
            coordinator.onAgentTransition = { [weak self] type, agentID, profileID in
                self?.notifyTransition(type, agentId: agentID, hostProfileID: profileID)
            }
            self.coordinator = coordinator
        }
        // #399 B6: when the profile store is configured, the store is the
        // source of truth for host pairing. The FIRST upgraded launch
        // migrates the legacy single-host identity into the first ordered
        // profile and pauses for fingerprint confirmation (no stream).
        // Legacy-only fixtures (profileStore == nil) keep the pre-#399
        // restore path byte-for-byte.
        if let profileStore {
            restoreProfilesFromStore(profileStore)
        } else if let identity = persistedLiveIdentity() {
            applyLiveIdentity(identity)
        } else {
            identityLifecycle.setCurrent(mode: .needsSetup, hostURL: nil,
                                         keyId: nil, signerPublicKeyB64: nil)
        }
    }

    // MARK: - #399 host profiles (B1-B7)

    /// Ordered profiles mirror of the store.
    private func reloadProfiles(from store: HostProfileStore) {
        profiles = store.orderedProfiles
        // #400: profile mutations reconcile the coordinator's sessions
        // (removal cancels only that host's tasks — E3).
        syncCoordinator(startStreams: false)
    }

    /// #400 C3/E3: reconcile the coordinator's per-host sessions against
    /// the CURRENT profile set. Sessions for removed/paused profiles are
    /// torn down (stream + tasks canceled, rows purged — E3); sessions
    /// for new connectable profiles are created. `startStreams` opens the
    /// not-yet-open sessions' streams (called by startLive — never from
    /// inside reload, where the caller is mid-mutation and startLive will
    /// follow).
    private func syncCoordinator(startStreams: Bool) {
        guard let store = profileStore, let coordinator else { return }
        coordinator.activeProfileID = activeProfileID
        coordinator.update(profiles: store.orderedProfiles,
                           startStreams: startStreams)
    }

    /// #397: per-host notification enrollment toggle (Settings host row).
    /// The GLOBAL Notifications control is retained; this flag is scoped
    /// per host profile. Disabling stops the DEBUG bridge for the host AND
    /// clears that host's enrolled APNs token best-effort (an unreachable
    /// host lands in `pendingPushTokenClears` with host-side guidance and
    /// a retry on reconnect). Re-enabling re-enrolls the retained token
    /// when this device holds one.
    func setHostNotificationsEnabled(profileID: UUID, enabled: Bool) {
        guard let store = profileStore, let profile = store.profile(id: profileID) else { return }
        guard profile.notificationsEnabled != enabled else { return }
        do {
            try store.setNotificationsEnabled(enabled, id: profileID)
        } catch {
            return
        }
        reloadProfiles(from: store)
        guard let url = URL(string: profile.urlString) else { return }
        if enabled {
            // Re-enable: enroll the retained token with THIS host again
            // (per-host dedupe lets the fan-out decide). Any pending clear
            // from an earlier disable is superseded by the new intent.
            enrolledTokenPerHost.removeValue(forKey: profileID)
            pendingPushTokenClears.remove(profileID)
            pendingPushClearTargets.removeValue(forKey: profileID)
            // The ACTIVE host's enrollment is the delegate's lifecycle
            // path — retry it too (its own dedupe suppresses repeats).
            AppDelegate.shared?.retryPendingDeviceTokenUpload()
            uploadRetainedTokenToHosts()
            if AppDelegate.shared?.retainedDeviceToken == nil {
                // Fresh process with no OS callback yet: ask again — the
                // callback re-enters the fan-out.
                Task { @MainActor in
                    UIApplication.shared.registerForRemoteNotifications()
                }
            }
        } else {
            notifier?.removeAll(forHostId: profile.hostKeyB64)
            enrolledTokenPerHost.removeValue(forKey: profileID)
            clearHostToken(profile: profile, hostURL: url)
        }
    }

    /// #397: clear ONE host's enrolled APNs token via the signed
    /// empty-token path. A reachable host clears immediately; an
    /// unreachable host lands in `pendingPushTokenClears` (target captured
    /// so a REMOVED profile's clear can still surface guidance and be
    /// superseded by a re-pair) and retries on that host's next successful
    /// connection. The pending marker is set SYNCHRONOUSLY before the
    /// attempt and only removed by a confirmed success — a clear that
    /// races an identity boundary (e.g. removing the ACTIVE host while it
    /// is unreachable) can never lose its guidance record.
    private func clearHostToken(profile: HostProfile, hostURL: URL) {
        guard let signer else { return }
        guard let keyId = profile.keyId, !keyId.isEmpty else { return }
        let target = PendingPushClearTarget(urlString: profile.urlString,
                                            keyId: keyId,
                                            displayName: profile.displayName)
        pendingPushTokenClears.insert(profile.id)
        pendingPushClearTargets[profile.id] = target
        let context = lifecycleContext()
        let taskId = UUID()
        let task = Task { @MainActor [weak self] in
            guard let self else { return }
            defer { self.lifecycleTasks.removeValue(forKey: taskId) }
            guard !Task.isCancelled, self.isCurrent(context) else { return }
            let client = DriveClient(host: hostURL, session: self.session)
            do {
                // Empty token = the daemon's documented clear path.
                _ = try await client.registerDeviceToken("", keyId: keyId, signer: signer)
                guard !Task.isCancelled, self.isCurrent(context) else { return }
                self.pendingPushTokenClears.remove(profile.id)
                self.pendingPushClearTargets.removeValue(forKey: profile.id)
            } catch {
                guard !Task.isCancelled, self.isCurrent(context) else { return }
                // Offline host: keep the clear pending — Settings shows
                // the host-side guidance and the next successful
                // connection to this host retries it.
            }
        }
        lifecycleTasks[taskId] = task
    }

    /// #397: retry one host's pending token clear when it reconnects.
    /// Existing profiles retry through the store; a REMOVED profile's
    /// pending clear stays listed (host-side guidance) until the same URL
    /// is paired again, which supersedes it.
    func retryPendingPushTokenClear(profileID: UUID) {
        guard pendingPushTokenClears.contains(profileID) else { return }
        guard let store = profileStore,
              let profile = store.profile(id: profileID),
              let url = URL(string: profile.urlString) else {
            return
        }
        clearHostToken(profile: profile, hostURL: url)
    }

    /// #397: a fresh pairing with the same URL replaces the enrollment
    /// intent — the old registration record is the SAME deterministic
    /// key_id, and the new enrollment writes the token it should hold, so
    /// any pending removal-clear for that URL is superseded.
    private func supersedePendingClear(urlString: String) {
        for (id, target) in pendingPushClearTargets where target.urlString == urlString {
            pendingPushTokenClears.remove(id)
            pendingPushClearTargets.removeValue(forKey: id)
        }
    }

    /// Settings guidance names for every pending clear — current profiles
    /// resolve live, removed profiles fall back to the captured target.
    func pendingPushClearNames() -> [String] {
        pendingPushTokenClears.sorted(by: { $0.uuidString < $1.uuidString }).compactMap { id in
            profiles.first { $0.id == id }?.displayName
                ?? pendingPushClearTargets[id]?.displayName
        }
    }

    /// #397: enroll the retained APNs token with every NON-active host
    /// whose per-host notifications are enabled — each upload is signed
    /// with THAT host profile's key id (per-host grants/expiry stay
    /// independent because each host daemon holds its own registry
    /// record). The ACTIVE host is enrolled by the AppDelegate's own
    /// lifecycle path; per-host dedupe prevents repeat posts per token.
    func uploadRetainedTokenToHosts() {
        guard mode == .live, signer != nil else { return }
        guard let hex = AppDelegate.shared?.retainedDeviceToken, !hex.isEmpty else { return }
        for profile in profiles {
            guard profile.id != activeProfileID else { continue }
            guard let url = URL(string: profile.urlString) else { continue }
            uploadRetainedToken(hex, to: profile, hostURL: url)
        }
    }

    private func uploadRetainedToken(_ hex: String, to profile: HostProfile, hostURL: URL) {
        guard mode == .live,
              profile.notificationsEnabled,
              profile.mayConnect,
              let signer,
              let keyId = profile.keyId,
              enrolledTokenPerHost[profile.id] != hex else { return }
        if profile.hostKeyB64 != nil {
            // #397 AC7: a pinned coordinator host must be VERIFIED before
            // a token may reach it (unchanged/unverified keys never
            // enroll) — posture .verifying/.mismatch denies.
            guard coordinator?.allowsLiveWork(profileID: profile.id) == true else { return }
        }
        uploadToken(hex, profileID: profile.id, hostURL: hostURL,
                    keyId: keyId, signer: signer)
    }

    private func uploadToken(_ hex: String, profileID: UUID, hostURL: URL,
                             keyId: String, signer: DeviceSigner) {
        let context = lifecycleContext()
        let taskId = UUID()
        let task = Task { @MainActor [weak self] in
            guard let self else { return }
            defer { self.lifecycleTasks.removeValue(forKey: taskId) }
            guard !Task.isCancelled, self.isCurrent(context) else { return }
            let client = DriveClient(host: hostURL, session: self.session)
            do {
                _ = try await client.registerDeviceToken(hex, keyId: keyId, signer: signer)
                guard !Task.isCancelled, self.isCurrent(context) else { return }
                self.enrolledTokenPerHost[profileID] = hex
            } catch {
                _ = error  // Offline host: the next startLive / foreground
                           // fan-out retries (per-host dedupe never recorded).
            }
        }
        lifecycleTasks[taskId] = task
    }

    /// The profile the single-host runtime is bound to: the persisted
    /// active id when it still exists, else the first ordered profile.
    var activeProfile: HostProfile? {
        if let activeProfileID,
           let profile = profiles.first(where: { $0.id == activeProfileID }) {
            return profile
        }
        return profiles.first
    }

    /// Whether the host-profile surface is active (production app always;
    /// legacy-only fixtures not). Settings renders the Hosts section only
    /// when this is true.
    var hostProfilesConfigured: Bool {
        profileStore != nil || !profiles.isEmpty
    }

    /// #401 D2/D6: the multi-host board surfaces are active with 2+
    /// configured profiles. Single-host F1 parity: with one profile the
    /// board keeps the pre-#401 layout (no host chips row, no row badges).
    var multiHostConfigured: Bool {
        profiles.count > 1
    }

    /// #401: the composite board rows across EVERY configured host (the
    /// ACTIVE host's store plus every coordinator session — #400's
    /// aggregate read model with per-row staleness/last-seen). nil keeps
    /// the single-host board on the legacy fleet-store path.
    var aggregateBoardRows: [HostBoardRow]? {
        guard multiHostConfigured, let coordinator else { return nil }
        return coordinator.aggregateRows(profiles: profiles) { [weak self] in
            self?.fleet
        }
    }

    /// The profile the host filter currently selects (nil = All Hosts).
    var hostFilterProfile: HostProfile? {
        guard let hostFilter else { return nil }
        return profiles.first { $0.id == hostFilter }
    }

    /// #401 D1: select the host chip (nil = All Hosts). The choice is
    /// session-only and reconciled when the selected host disappears.
    func selectHostFilter(_ profileID: UUID?) {
        guard profileID == nil || profiles.contains(where: { $0.id == profileID }) else {
            hostFilter = nil
            return
        }
        hostFilter = profileID
    }

    /// #401 C6: per-row state-entered tracking of a COMPOSITE target from
    /// its OWNING store (the same #166 read the single-host board uses).
    func stateEnteredAt(hostProfileID: UUID?, agentID: String) -> UInt64? {
        if let profileID = hostProfileID, profileID != activeProfileID {
            return coordinator?.store(profileID: profileID)?.stateEnteredAt[agentID]
        }
        return fleet.stateEnteredAt[agentID]
    }

    /// #401 D3/D7: runtime facts of one host profile for the host-filter
    /// chips and Settings host rows. Consumes #399/#400 state only —
    /// per-host store connection state, coordinator posture, and the
    /// active profile's continuity state.
    func hostRuntimeFacts(for profile: HostProfile) -> BoardModel.HostRuntimeFacts {
        var facts = BoardModel.HostRuntimeFacts()
        if profile.id == activeProfileID {
            facts.keyMismatch = profile.connectionState == .keyMismatch
                || keyContinuityState == .mismatch
            facts.awaitingFingerprint =
                profile.connectionState == .awaitingFingerprintConfirmation
            switch fleet.connectionState {
            case .connected: facts.isConnected = true
            case .connecting: facts.isConnecting = true
            default: break
            }
            // A pinned active host that has not re-verified since
            // launch/foreground is connecting (B4 re-check in flight).
            if profile.hostKeyB64 != nil, keyContinuityState == .pending {
                facts.isConnecting = true
            }
        } else {
            facts.keyMismatch = profile.connectionState == .keyMismatch
                || coordinator?.posture(profileID: profile.id) == .mismatch
            facts.awaitingFingerprint =
                profile.connectionState == .awaitingFingerprintConfirmation
            if let store = coordinator?.store(profileID: profile.id) {
                switch store.connectionState {
                case .connected: facts.isConnected = true
                case .connecting: facts.isConnecting = true
                default: break
                }
            }
            // A coordinator session whose verification has NOT completed
            // (`.verifying`, no stream open) reads as OFFLINE — the host is
            // not connected; only a live transport attempt (.connecting)
            // shows as connecting.
        }
        return facts
    }

    /// #401 D7: the Settings row Retry action — idempotent start of ONE
    /// host's verification/stream. The ACTIVE host re-runs its continuity
    /// gate + stream; a coordinator host starts only its own session
    /// (never another host's, never a global reconnect).
    func retryHostConnection(_ profile: HostProfile) {
        guard profile.mayConnect else { return }
        if profile.id == activeProfileID {
            guard keyContinuityState != .mismatch else { return }
            startLive()
        } else {
            coordinator?.startSessionIfNeeded(profile)
        }
    }

    /// #401 D2/D7: drag-to-reorder (Settings). Order drives the host-chip
    /// row; the store's moveProfile re-normalizes every profile's order.
    /// SwiftUI `.onMove` delivers the destination in the ORIGINAL row
    /// coordinates, so a downward move (destination after the removed row)
    /// is converted to the store's post-removal insertion index.
    func moveHosts(from source: IndexSet, to destination: Int) {
        guard let store = profileStore,
              let movedIndex = source.first,
              profiles.indices.contains(movedIndex) else { return }
        let movedID = profiles[movedIndex].id
        let insertion = destination > movedIndex ? destination - 1 : destination
        try? store.moveProfile(id: movedID, toOrder: insertion)
        reloadProfiles(from: store)
    }

    /// #401 D7: rename one host's DISPLAY NAME in place (B5 — URL/identity
    /// changes are remove-and-re-pair, never an edit). Returns the error
    /// text (duplicate/empty name) or nil on success.
    func renameHost(id: UUID, to newDisplayName: String) -> String? {
        guard let store = profileStore else { return nil }
        do {
            try store.renameProfile(id: id, to: newDisplayName)
            reloadProfiles(from: store)
            return nil
        } catch {
            return error.localizedDescription
        }
    }

    private func persistActiveProfileID(_ id: UUID?) {
        activeProfileID = id
        if let id {
            defaults.set(id.uuidString, forKey: Self.activeProfileKey)
        } else {
            defaults.removeObject(forKey: Self.activeProfileKey)
        }
    }

    /// Launch-time store restore: migrate legacy data when the store is
    /// empty (B6), then bind the active profile and pause on the
    /// fingerprint confirmation when the host key is not yet pinned.
    /// Never starts a stream here — the RootView task calls startLive(),
    /// which honors the profile's paused state.
    private func restoreProfilesFromStore(_ store: HostProfileStore) {
        reloadProfiles(from: store)
        // B6: first upgraded launch — migrate the legacy single-host
        // identity into the first profile. No token, no /register; the
        // key_id/grants/expiry ride along verbatim.
        if profiles.isEmpty,
           let legacy = loadMeta(),
           !legacy.keyId.isEmpty,
           let host = defaults.string(forKey: "fleetnotifier.host"),
           host == legacy.host {
            if store.migrateLegacy(host: legacy.host,
                                   keyId: legacy.keyId,
                                   grants: legacy.grants,
                                   expiryTs: legacy.expiryTs,
                                   registeredAt: legacy.registeredAt) != nil {
                // Consumed: legacy + profile records can never both be
                // active (B6). Removal happens only after the profile
                // document write succeeded (rollback-safe).
                removeMeta()
                defaults.removeObject(forKey: "fleetnotifier.host")
                reloadProfiles(from: store)
            }
        }
        // Bind the previously active profile (or the first ordered one)
        // and let startLive() decide whether the stream may open.
        let storedID = defaults.string(forKey: Self.activeProfileKey)
            .flatMap(UUID.init(uuidString:))
        let target = store.profile(id: storedID ?? UUID()) ?? store.orderedProfiles.first
        guard let profile = target else {
            // A clean store with no legacy data behaves like a fresh
            // install: needsSetup, nothing restored.
            mode = .needsSetup
            identityLifecycle.setCurrent(mode: .needsSetup, hostURL: nil,
                                         keyId: nil, signerPublicKeyB64: nil)
            return
        }
        persistActiveProfileID(profile.id)
        bindActiveProfile(profile)
    }

    /// Bind the single-host runtime fields to a profile (host/key/grants/
    /// cursor mirror) and pause when the profile is awaiting fingerprint
    /// confirmation (migration) — no live work happens in the paused
    /// state because startLive() checks it.
    private func bindActiveProfile(_ profile: HostProfile) {
        guard let url = URL(string: profile.urlString),
              let (signer, storage) = try? identityLoader() else {
            return
        }
        self.signer = signer
        keyId = profile.keyId
        grants = profile.grants
        hostURL = url
        keyStorageWarning = (storage == .insecureFallback)
        mode = .live
        identityLifecycle.setCurrent(mode: .live, hostURL: url,
                                     keyId: profile.keyId,
                                     signerPublicKeyB64: signer.publicKeyB64)
        // Per-host cursor mirror: the active profile's cursor is the
        // single-host cursor while it is bound.
        fleet.restoreCursor()
        if let rev = fleet.lastEventId {
            profileStore?.setCursor(rev, for: profile.id)
        }
        if profile.connectionState == .awaitingFingerprintConfirmation {
            // Legacy migration pause (B6): fetch + confirm the host key
            // before any stream may open.
            keyContinuityState = .pending
            fingerprintConfirmation = FingerprintConfirmationRequest(
                id: UUID(), profileID: profile.id, profileName: profile.displayName)
        } else if profile.hostKeyB64 != nil {
            // Pinned: re-verify `/host-key` before the stream opens (B4).
            keyContinuityState = .pending
        } else {
            // Legacy single-host pairing without a pin: no continuity
            // gate (parity with the pre-#399 Connection flow).
            keyContinuityState = .notPinned
        }
        // #400: the active profile is now excluded from the coordinator —
        // reconcile sessions for the remaining hosts.
        syncCoordinator(startStreams: false)
    }

    /// #399 B4: fail-closed gate for every live read/write route. A
    /// profile paused on fingerprint confirmation (B6) denies everything;
    /// a PINNED profile must be `.verified` since launch/reconnect;
    /// legacy unpinned hosts (parity) and unpinned flows pass.
    private var keyContinuityAllowsLiveWork: Bool {
        guard let profile = activeProfile else { return true }
        if profile.connectionState == .awaitingFingerprintConfirmation {
            return false
        }
        guard profile.hostKeyB64 != nil else { return true }
        return keyContinuityState == .verified
    }

    private func keyContinuityDeniedBanner() -> Bool {
        guard let profile = activeProfile, profile.hostKeyB64 != nil,
              keyContinuityState == .mismatch else { return false }
        banner = .error("host_key_mismatch",
                        "\(profile.displayName)'s host key changed — the board is paused. Remove the host and pair it again; Corral never auto-accepts a new key.")
        return true
    }

    /// B4: re-check `/host-key` before opening the live stream after
    /// launch/reconnect. On match: connect. On mismatch: fail closed —
    /// no stream/fetch/push-register/Recent Output reaches the
    /// replacement identity and the last safe snapshot stays stale.
    private func beginKeyContinuityCheck(for profile: HostProfile) {
        guard keyContinuityState != .mismatch else { return }
        let context = lifecycleContext()
        keyContinuityState = .pending
        guard let url = URL(string: profile.urlString),
              let pinned = profile.hostKeyB64 else { return }
        let client = CorraldClient(host: url, session: session)
        let taskId = UUID()
        let task = Task { @MainActor [weak self] in
            guard let self else { return }
            defer { self.lifecycleTasks.removeValue(forKey: taskId) }
            guard !Task.isCancelled, self.isCurrent(context) else { return }
            do {
                let response = try await client.fetchHostKey()
                guard !Task.isCancelled, self.isCurrent(context) else { return }
                if HostKeyTrust.matches(response, pinnedKeyB64: pinned) {
                    self.keyContinuityState = .verified
                    self.profileStore?.noteLastSuccessfulConnection(id: profile.id)
                    self.startLive()
                } else {
                    self.failKeyContinuity(profile)
                }
            } catch {
                guard !Task.isCancelled, self.isCurrent(context) else { return }
                // Could not reach the host to verify identity: the stream
                // stays closed (never unverified-open). The next
                // launch/foreground startLive() retries the check.
                self.banner = .error("host_key_unverified",
                                     "Could not verify \(profile.displayName)'s host key — the board stays paused until the host is reachable.")
            }
        }
        lifecycleTasks[taskId] = task
    }

    /// Fail closed on a host-key mismatch (B4): no stream, no push, no
    /// reads; the retained metadata snapshot stays stale; the profile
    /// records the mismatch. Only Remove Host + fresh pairing recovers.
    private func failKeyContinuity(_ profile: HostProfile) {
        keyContinuityState = .mismatch
        profileStore?.noteConnectionState(id: profile.id, .keyMismatch)
        fleet.acceptedHostIdentity = nil
        fleet.disconnect()
        banner = .error("host_key_mismatch",
                        "\(profile.displayName) presented a different host key than the one you paired. The board is paused and stale — Remove the host, then pair it again with a fresh token. Corral never auto-accepts a rotated key.")
    }

    /// Remove Host (B7, local unlink): purge the profile, its cursor, its
    /// durable cache and in-memory tails. The shared phone signing key and
    /// every other profile stay intact.
    /// #397: removal also clears the removed host's enrolled APNs token
    /// (signed empty-token path) when the host is reachable; an
    /// unreachable host lands in `pendingPushTokenClears` with host-side
    /// cleanup guidance, and the pending clear is superseded if the same
    /// URL is paired again. The daemon's registry entry itself remains
    /// until the host removes it. If the removed profile was the active
    /// one, the next ordered profile activates (or the app returns to
    /// setup when none remains).
    func removeHost(profileID: UUID) {
        guard let store = profileStore else { return }
        // #397: capture the profile BEFORE the unlink — the token clear
        // needs its key id/URL, and a failed clear must keep its target
        // even though the profile record is gone.
        let removedProfile = store.profile(id: profileID)
        let wasActive = activeProfile?.id == profileID
        if wasActive {
            // Removing the ACTIVE host tears down the whole single-host
            // runtime first (stream + every task), then unlinks the
            // profile and rebinds the next ordered host (or returns to
            // setup).
            cancelLifecycleTasks()
            stopLive()
        }
        if let removedProfile {
            notifier?.removeAll(forHostId: removedProfile.hostKeyB64)
            if removedProfile.keyId != nil,
               let url = URL(string: removedProfile.urlString) {
                clearHostToken(profile: removedProfile, hostURL: url)
            }
        }
        store.removeProfile(id: profileID)
        reloadProfiles(from: store)
        enrolledTokenPerHost.removeValue(forKey: profileID)
        // #401 D1: a removed host can never stay selected — the filter
        // reconciles to All Hosts (session-only choice).
        if let hostFilter, store.profile(id: hostFilter) == nil {
            self.hostFilter = nil
        }
        if wasActive {
            fleet.acceptedHostIdentity = nil
            keyContinuityState = .notPinned
            fingerprintConfirmation = nil
            // B7: the removed host's in-memory rows/tails go with it.
            fleet.reset()
            if let next = store.orderedProfiles.first {
                persistActiveProfileID(next.id)
                bindActiveProfile(next)
                startLive()
            } else {
                persistActiveProfileID(nil)
                fleet.reset()
                // B7: the shared phone signing key STAYS (only the host's
                // local state is unlinked); keyId/grants are this host's
                // registration metadata and go with the record.
                keyId = nil
                grants = []
                hostURL = nil
                keyStorageWarning = false
                notificationsConfigured = false
                mode = .needsSetup
                identityLifecycle.setCurrent(mode: .needsSetup, hostURL: nil,
                                             keyId: nil, signerPublicKeyB64: nil)
            }
        } else {
            // #400 E3: removing a NON-active host cancels ONLY that host's
            // tasks and purges ONLY that composite target's stream, tails,
            // and sheet state — every other host keeps streaming and the
            // active board is untouched (the old code canceled the whole
            // app here, freezing the active host too).
            coordinator?.remove(profileID: profileID)
            cancelHostDriveTasks(hostProfileID: profileID)
            if recentsRequest?.hostProfileID == profileID {
                recentsRequest = nil
            }
        }
    }

    /// Confirm the paused profile's fingerprint (B6/B3): pins the fetched
    /// host key and lets the stream open. Never auto-accepts — this is
    /// the single explicit confirmation point.
    func confirmFingerprint(profileID: UUID, hostKeyB64: String, fingerprint: String) {
        guard let store = profileStore,
              store.profile(id: profileID) != nil else { return }
        do {
            try store.confirmFingerprint(id: profileID,
                                         hostKeyB64: hostKeyB64,
                                         fingerprint: fingerprint)
        } catch {
            banner = .error("host_key_conflict", error.localizedDescription)
            return
        }
        fingerprintConfirmation = nil
        reloadProfiles(from: store)
        if activeProfile?.id == profileID {
            fleet.acceptedHostIdentity = hostKeyB64
            keyContinuityState = .verified
            startLive()
        }
    }

    /// Fetch the host key for the paused profile's confirmation sheet
    /// (B6): the sheet shows the fingerprint and only passes a
    /// user-confirmed pin to `confirmFingerprint`.
    func fetchHostKey(profileID: UUID) async throws -> HostKeyResponse {
        guard let store = profileStore,
              let profile = store.profile(id: profileID),
              let url = URL(string: profile.urlString) else {
            throw HostProfileError.profileNotFound
        }
        let client = CorraldClient(host: url, session: session)
        return try await client.fetchHostKey()
    }

    /// #399: fetches the paused profile's host key for review from the
    /// Settings row (re-presents the confirmation sheet after a dismiss).
    func requestFingerprintReview(profileID: UUID) {
        guard let store = profileStore,
              let profile = store.profile(id: profileID) else { return }
        fingerprintConfirmation = FingerprintConfirmationRequest(
            id: UUID(), profileID: profileID, profileName: profile.displayName)
    }

    /// Decline/dismiss the paused confirmation: the profile stays paused
    /// (no stream) and can be reviewed again from Settings.
    func deferFingerprintConfirmation() {
        fingerprintConfirmation = nil
    }

    /// #399 C5: persist the ACTIVE host's allowlisted board-metadata
    /// cache. Called on connection success and when the app backgrounds —
    /// never on read_tail content, which this DTO cannot hold.
    func persistBoardMetadata() {
        guard let store = profileStore, let profile = activeProfile else { return }
        let rows = BoardCacheDTO.snapshot(hostProfileID: profile.id,
                                          agents: fleet.agents,
                                          stateEnteredAt: fleet.stateEnteredAt,
                                          now: UInt64(Date().timeIntervalSince1970 * 1000))
        store.boardCache.save(rows, for: profile.id)
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
                    // #399: keep the profile store as the source of truth
                    // when configured. The legacy single-host registration
                    // mirrors into a fresh ACTIVE profile record; the
                    // previous active record is removed first (URL change =
                    // remove-and-re-pair, B5; other profiles untouched).
                    // Legacy flow carries NO pinned key — fingerprint
                    // pairing is the new Add Host path.
                    if let store = self.profileStore {
                        do {
                            // #397: a re-pair supersedes any pending
                            // removal-clear for the URL and carries the
                            // replaced record's per-host notification
                            // state when the URL is unchanged.
                            self.supersedePendingClear(
                                urlString: registrationContext.requestedHostURL.absoluteString)
                            let carriedNotifications =
                                self.activeProfile?.urlString
                                    == registrationContext.requestedHostURL.absoluteString
                                ? (self.activeProfile?.notificationsEnabled ?? true)
                                : true
                            if let oldActive = self.activeProfile {
                                store.removeProfile(id: oldActive.id)
                            }
                            let profile = try store.commitActivePairing(
                                displayName: HostURLForm.displayNameCandidate(
                                    for: registrationContext.requestedHostURL.absoluteString),
                                urlString: registrationContext.requestedHostURL.absoluteString,
                                hostKeyB64: nil,
                                fingerprint: nil,
                                keyId: response.keyId,
                                grants: response.grants,
                                expiryTs: response.expiryTs,
                                registeredAt: UInt64(Date().timeIntervalSince1970),
                                notificationsEnabled: carriedNotifications)
                            self.reloadProfiles(from: store)
                            self.persistActiveProfileID(profile.id)
                            self.keyContinuityState = .notPinned
                            self.fleet.acceptedHostIdentity = nil
                        } catch {
                            // Mirroring failure is best-effort on the
                            // legacy path — the daemon registration itself
                            // already succeeded; the store stays empty and
                            // the next launch repeats the legacy restore.
                            Self.log.error("host profile mirror failed: \(error.localizedDescription)")
                        }
                    }
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

    // MARK: - Add Host (#399 B3, fingerprint-verified pairing)

    /// Prepared Add Host pairing (B3 phase 1 result): name + normalized
    /// URL + the fetched host key response + derived fingerprint. Nothing
    /// is persisted and no token is sent at this point.
    struct PreparedHostPairing: Equatable, Sendable {
        var displayName: String
        var urlString: String
        var hostKey: HostKeyResponse
        var fingerprint: String
    }

    /// #415: scene-scoped Add Host sheet state (see `addHostDraft`):
    /// `prepared` non-nil = the host key was fetched and the sheet is on
    /// the fingerprint-confirmation phase; `errorMessage` is a
    /// phase-identifying, secret-free failure shown inside the sheet;
    /// `isWorking` drives the phase buttons' spinner/disabled state.
    struct AddHostDraft: Equatable {
        var name = ""
        var urlString = ""
        var token = ""
        var prepared: PreparedHostPairing?
        var errorMessage: String?
        var isWorking = false
    }

    /// #415: the phase that rejected an Add Host attempt, carrying a
    /// phase-identifying, secret-free message for the sheet.
    enum AddHostFailure: Equatable {
        /// A submit arrived while a registration was already in flight.
        case inProgress
        /// Entry/duplicate rejection (empty name, bad URL, already-paired
        /// name/URL/identity).
        case conflict(String)
        /// `/host-key` fetch failed or returned an unusable key.
        case hostKeyFetch(String)
        /// `/register` failed (rejected token, unreachable host, ...).
        case registrationFailed(String)
        /// The committed profile could not be persisted.
        case profileStore(String)

        var message: String {
            switch self {
            case .inProgress:
                return "Registration is already in progress — wait for it to finish, then retry."
            case .conflict(let detail):
                return "Could not add this host — \(detail)"
            case .hostKeyFetch(let detail):
                return "Could not verify this host's key — \(detail)"
            case .registrationFailed(let detail):
                return "Registration failed — \(detail)"
            case .profileStore(let detail):
                return "Could not save the paired host profile — \(detail)"
            }
        }
    }

    /// #415: the result of one Add Host submit attempt. `.success` means
    /// the daemon accepted registration AND exactly one profile was
    /// committed AND the scene-scoped draft was cleared — the sheet
    /// dismisses exactly once, on success only. `.failure` never clears
    /// the draft and never dismisses: every value stays available for
    /// correction/retry.
    enum AddHostOutcome: Equatable {
        case success
        case failure(AddHostFailure)
    }

    /// B3 phase 1: validate name/URL and fetch `/host-key`, validating the
    /// X25519 key form. The caller shows the derived fingerprint for
    /// explicit confirmation BEFORE any registration token is accepted.
    func prepareHostPairing(displayName: String, rawURL: String) async throws -> PreparedHostPairing {
        guard let store = profileStore else {
            throw HostProfileError.invalidURL
        }
        try store.validateCandidate(displayName: displayName, urlString: rawURL)
        guard let normalized = HostURLForm.normalized(rawURL),
              let url = URL(string: normalized) else {
            throw HostProfileError.invalidURL
        }
        let client = CorraldClient(host: url, session: session)
        let response = try await client.fetchHostKey()
        guard HostKeyTrust.isWellFormed(response),
              let fingerprint = HostKeyTrust.fingerprint(forBase64: response.publicKey) else {
            throw HostProfileError.invalidHostKeyForm
        }
        // Duplicate pinned host identity check (B3): the same key must
        // not pair twice under another URL/name.
        try store.validateCandidateIdentity(hostKeyB64: response.publicKey)
        return PreparedHostPairing(
            displayName: displayName.trimmingCharacters(in: .whitespacesAndNewlines),
            urlString: normalized,
            hostKey: response,
            fingerprint: fingerprint)
    }

    /// B3 phase 2: the user CONFIRMED the fingerprint. Register with the
    /// existing phone Ed25519 key, persist the FULL pinned key + returned
    /// grants/expiry as the ACTIVE profile record (a same-URL re-pair
    /// replaces only that URL's record; every OTHER profile — including a
    /// previously active Mac host — stays untouched, #415), then open the
    /// live stream. The key was verified moments ago, so no second
    /// `/host-key` fetch runs before the first stream; every later
    /// launch/foreground re-checks (B4).
    ///
    /// #415: `.success` = the daemon accepted registration AND exactly one
    /// profile was committed AND the scene-scoped draft was cleared (the
    /// sheet dismisses once). `.failure` never clears the draft and never
    /// dismisses — the draft carries a phase-identifying, secret-free
    /// error and every value stays available for correction/retry.
    /// Repeated submits cannot duplicate a profile: an in-flight
    /// registration returns `.failure(.inProgress)` without a second
    /// `/register`, and the store's duplicate + same-URL purge guards keep
    /// exactly one record per URL/identity.
    func completeAddHost(_ prepared: PreparedHostPairing, token: String) async -> AddHostOutcome {
        guard registrationTaskId == nil else {
            return failAddHostDraft(.inProgress)
        }
        guard let store = profileStore else {
            return failAddHostDraft(.profileStore("the host profile store is unavailable"))
        }
        guard let url = URL(string: prepared.urlString) else {
            return failAddHostDraft(.conflict("host must be an http(s) URL or host:port"))
        }
        do {
            try store.validateCandidate(displayName: prepared.displayName,
                                        urlString: prepared.urlString)
            try store.validateCandidateIdentity(hostKeyB64: prepared.hostKey.publicKey)
        } catch {
            return failAddHostDraft(.conflict(error.localizedDescription))
        }
        var outcome: AddHostOutcome = .failure(.inProgress)
        // Re-validate against the ACTIVE profile before switching (same
        // host-switch semantics as the legacy register flow, #365). The
        // ACTIVE BINDING moves to the new host; #415: its PROFILE record
        // is never removed — multi-host keeps every other host paired and
        // streaming through the coordinator.
        let switchingHost = activeProfile.map { $0.urlString != prepared.urlString } ?? false
        addHostDraft.isWorking = true
        addHostDraft.errorMessage = nil
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
            guard isCurrent(baseContext),
                  identityLifecycle.isCurrent(sharedRegistrationContext) else {
                return failAddHostDraft(.inProgress)
            }
            let taskId = UUID()
            registrationTaskId = taskId
            let task = Task { @MainActor [weak self] in
                guard let self else { return }
                defer {
                    self.lifecycleTasks.removeValue(forKey: taskId)
                    if self.registrationTaskId == taskId {
                        self.registrationTaskId = nil
                    }
                    // #415: every exit path settles the draft's working
                    // flag (success already cleared the whole draft; a
                    // superseded/cancelled registration must not leave the
                    // sheet spinning forever).
                    self.addHostDraft.isWorking = false
                }
                guard self.isCurrent(baseContext),
                      self.identityLifecycle.isCurrent(sharedRegistrationContext),
                      !Task.isCancelled else { return }
                do {
                    let client = DriveClient(host: url, session: self.session)
                    let response = try await client.register(token: token, signer: candidateSigner,
                                                             name: UIDevice.current.name)
                    guard !Task.isCancelled,
                          self.isCurrent(baseContext),
                          self.identityLifecycle.isCurrent(sharedRegistrationContext) else { return }
                    // #397: a fresh pairing supersedes any pending
                    // removal-clear for the same URL (the new enrollment
                    // writes the token the shared key record should hold).
                    self.supersedePendingClear(urlString: url.absoluteString)
                    // Persist the fingerprint-confirmed pairing (B3/B5).
                    // #415: only the SAME-URL record is replaced (the
                    // store's own purge in commitActivePairing) — no
                    // other profile is ever removed by an Add Host commit.
                    let now = UInt64(Date().timeIntervalSince1970)
                    let carriedNotifications = self.activeProfile?.urlString == url.absoluteString
                        ? (self.activeProfile?.notificationsEnabled ?? true)
                        : true
                    let profile = try store.commitActivePairing(
                        displayName: prepared.displayName,
                        urlString: prepared.urlString,
                        hostKeyB64: prepared.hostKey.publicKey,
                        fingerprint: prepared.fingerprint,
                        keyId: response.keyId,
                        grants: response.grants,
                        expiryTs: response.expiryTs,
                        registeredAt: now,
                        notificationsEnabled: carriedNotifications)
                    self.signer = candidateSigner
                    self.keyStorageWarning = (storage == .insecureFallback)
                    self.keyId = response.keyId
                    self.grants = response.grants
                    self.hostURL = url
                    self.saveMeta(DeviceKeyStore.DeviceMeta(
                        keyId: response.keyId, host: url.absoluteString,
                        grants: response.grants, expiryTs: response.expiryTs,
                        registeredAt: now))
                    self.defaults.set(url.absoluteString, forKey: "fleetnotifier.host")
                    self.reloadProfiles(from: store)
                    self.persistActiveProfileID(profile.id)
                    self.mode = .live
                    self.identityLifecycle.setCurrent(
                        mode: .live, hostURL: url, keyId: response.keyId,
                        signerPublicKeyB64: candidateSigner.publicKeyB64)
                    self.keyContinuityState = .verified
                    self.fleet.acceptedHostIdentity = profile.hostKeyB64
                    self.fleet.restoreCursor()
                    self.startLive()
                    self.banner = .info("Paired \(profile.displayName) · fingerprint confirmed")
                    // #415: the commit succeeded — ONLY now is the draft
                    // cleared (values survive every failure; the caller
                    // dismisses exactly once, on success).
                    self.addHostDraft = AddHostDraft()
                    outcome = .success
                } catch is CancellationError {
                    return
                } catch {
                    guard !Task.isCancelled,
                          self.isCurrent(baseContext),
                          self.identityLifecycle.isCurrent(sharedRegistrationContext) else { return }
                    self.identityLifecycle.setCurrent(
                        mode: self.sharedMode(for: baseContext.mode),
                        hostURL: baseContext.hostURL, keyId: baseContext.keyId,
                        signerPublicKeyB64: baseContext.signerPublicKeyB64)
                    if baseContext.mode == .live {
                        self.startLive()
                    }
                    if error is HostProfileError || error is HostProfileValidationError {
                        // The only throwers past the successful /register
                        // are the store-commit validations — the
                        // profile-store phase.
                        outcome = self.failAddHostDraft(.profileStore(
                            error.localizedDescription))
                    } else {
                        outcome = self.failAddHostDraft(.registrationFailed(
                            error.localizedDescription))
                    }
                }
            }
            lifecycleTasks[taskId] = task
            await task.value
        } catch {
            guard !Task.isCancelled, isCurrent(baseContext),
                  identityLifecycle.isCurrent(sharedRegistrationContext) else {
                return failAddHostDraft(.inProgress)
            }
            identityLifecycle.setCurrent(mode: sharedMode(for: baseContext.mode),
                                         hostURL: baseContext.hostURL,
                                         keyId: baseContext.keyId,
                                         signerPublicKeyB64: baseContext.signerPublicKeyB64)
            if baseContext.mode == .live {
                startLive()
            }
            return failAddHostDraft(.registrationFailed(error.localizedDescription))
        }
        return outcome
    }

    // MARK: - Add Host draft + outcome (#415)

    /// #415: clear the scene-scoped Add Host draft. Called on Cancel and
    /// by the SUCCESS path AFTER the profile commit — never on failure,
    /// where every draft value must stay available for correction/retry.
    func clearAddHostDraft() {
        addHostDraft = AddHostDraft()
    }

    /// #415: phase-1 "Verify host key" run against the scene-scoped
    /// draft. On success the draft moves to the fingerprint-confirmation
    /// phase (`prepared` set, name/URL/token retained); on failure the
    /// draft keeps every value and carries a phase-identifying,
    /// secret-free error. Sheet identity churn during the fetch cannot
    /// lose state — the draft is model-owned.
    func verifyAddHostDraft() async {
        guard !addHostDraft.isWorking else { return }
        addHostDraft.isWorking = true
        addHostDraft.errorMessage = nil
        defer { addHostDraft.isWorking = false }
        do {
            let pairing = try await prepareHostPairing(
                displayName: addHostDraft.name,
                rawURL: addHostDraft.urlString)
            addHostDraft.prepared = pairing
        } catch is CancellationError {
            // Flow/teardown cancelled the verify: the draft keeps its
            // values and there is nothing to report to a gone sheet.
            Self.log.info("Add Host verification cancelled")
        } catch {
            addHostDraft.errorMessage = Self.addHostPrepareFailure(for: error).message
        }
    }

    /// #415: map a phase-1 (name/URL entry + `/host-key` fetch) error to
    /// its failure phase. Entry/duplicate errors are user-correctable
    /// conflicts; transport and malformed-key failures are the fetch
    /// phase. Messages never carry secrets (tokens are never echoed).
    static func addHostPrepareFailure(for error: Error) -> AddHostFailure {
        if let validation = error as? HostProfileValidationError {
            return .conflict(validation.errorDescription ?? "duplicate host")
        }
        if let profileError = error as? HostProfileError {
            switch profileError {
            case .invalidHostKeyForm:
                return .hostKeyFetch(profileError.errorDescription ?? "malformed host key")
            default:
                return .conflict(profileError.errorDescription ?? "invalid entry")
            }
        }
        return .hostKeyFetch(error.localizedDescription)
    }

    /// #415: record a failure on the scene-scoped draft (the sheet stays
    /// open; every draft value stays editable/retryable) and return its
    /// outcome. Never clears the draft.
    private func failAddHostDraft(_ failure: AddHostFailure) -> AddHostOutcome {
        var draft = addHostDraft
        draft.errorMessage = failure.message
        draft.isWorking = false
        addHostDraft = draft
        return .failure(failure)
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
        // #399 B4: never sign a request toward a host whose pinned
        // identity is unverified or mismatched (no grants-refresh either).
        guard keyContinuityAllowsLiveWork else { return }
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
        // #400 C3: start/reconnect every OTHER configured host
        // concurrently — independent of the ACTIVE profile's gates below
        // (a paused or mismatched active host must never freeze the rest
        // of the board). Idempotent per host.
        syncCoordinator(startStreams: true)
        // #399 B6: a profile paused on fingerprint confirmation never
        // opens a stream — the board presents the confirmation sheet and
        // only confirmFingerprint() releases it.
        if let profile = activeProfile {
            if profile.connectionState == .awaitingFingerprintConfirmation {
                // Migration pause: surface the confirmation request.
                if fingerprintConfirmation == nil {
                    fingerprintConfirmation = FingerprintConfirmationRequest(
                        id: UUID(), profileID: profile.id, profileName: profile.displayName)
                }
                return
            }
            if profile.hostKeyB64 != nil && keyContinuityState != .verified {
                if keyContinuityState == .mismatch {
                    _ = keyContinuityDeniedBanner()
                    return
                }
                // B4: pinned but not yet re-verified since launch —
                // re-check /host-key before opening the stream.
                beginKeyContinuityCheck(for: profile)
                return
            }
        }
        let client = CorraldClient(host: hostURL, session: session)
        // #354 L2: state-change notification hooks (started / blocked /
        // finished), fired by FleetStore on SSE deltas. #397: every hook
        // carries the ACTIVE profile id — notifications are composite
        // (host, agent) targets from here on.
        fleet.onStarted = { [weak self] agentId in
            self?.notifyTransition(.started, agentId: agentId,
                                   hostProfileID: self?.activeProfile?.id)
        }
        fleet.onBlocked = { [weak self] agentId in
            self?.notifyTransition(.blocked, agentId: agentId,
                                   hostProfileID: self?.activeProfile?.id)
        }
        fleet.onFinished = { [weak self] agentId in
            self?.notifyTransition(.finished, agentId: agentId,
                                   hostProfileID: self?.activeProfile?.id)
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
        // #399 B4: a frame stamped with a DIFFERENT host identity than
        // the pinned one was rejected by the store — fail the stream
        // closed here (the stale snapshot stays untouched).
        fleet.onHostIntegrityMismatch = { [weak self] in
            guard let self, let profile = self.activeProfile else { return }
            self.failKeyContinuity(profile)
        }
        // Review F2: once the stream re-establishes a 200 the connection is
        // healthy again — drop a stale stream_connection banner (an idle
        // fleet emits no frames, so apply() alone would never clear it).
        // #399 C6: stamp last-successful-connection + persist the
        // allowlisted board metadata cache on connection success.
        // #400 F2: an ACTIVE-host reconnect also retries any pending
        // empty-token clear left by the multi-host safety gate.
        fleet.onConnected = { [weak self] in
            guard let self else { return }
            if self.banner?.kind == "stream_connection" {
                self.banner = nil
            }
            if let profile = self.activeProfile {
                self.profileStore?.noteLastSuccessfulConnection(id: profile.id)
                self.persistBoardMetadata()
                self.retryPendingPushTokenClear(profileID: profile.id)
            }
        }
        // #399 B4/C1: only accept frames stamped with the pinned host
        // identity once a key is pinned (host-less records from a
        // transitional daemon still pass).
        fleet.acceptedHostIdentity = activeProfile?.hostKeyB64
        fleet.connect(client: client)
        // APNs upload/retry is independent from one-time local notification
        // setup. A token callback can arrive during demo; retry it now that
        // the shared lifecycle is live, even when notificationsConfigured is
        // already true.
        AppDelegate.shared?.retryPendingDeviceTokenUpload()
        // #397: the delegate only enrolls the ACTIVE host; every OTHER
        // paired host with per-host notifications enabled gets the same
        // retained token under ITS OWN key id here (signed per-profile
        // device registration record). The hook covers a mid-session OS
        // callback; the direct fan-out covers launch/foreground/pairing.
        if let delegate = AppDelegate.shared {
            delegate.onDeviceTokenReceived = { [weak self] hex in
                guard let self else { return }
                Task { @MainActor in
                    guard self.mode == .live else { return }
                    for profile in self.profiles where profile.id != self.activeProfileID {
                        guard let url = URL(string: profile.urlString) else { continue }
                        self.uploadRetainedToken(hex, to: profile, hostURL: url)
                    }
                }
            }
        }
        uploadRetainedTokenToHosts()
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
        // #397: the tap carries the payload's host identity — the model
        // resolves EXACTLY one host profile (never a guess from a bare
        // agent id).
        notifier?.onOpenAgent = { [weak self] agentId, hostID in
            self?.openNotification(agentId: agentId, hostKeyB64: hostID)
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
        // #400 C3: cancel every coordinator host's stream/tasks when the
        // app backgrounds; persist their per-host cursors + allowlisted
        // caches before the session ends (C5/C6).
        if let coordinator, let store = profileStore {
            let profiles = store.orderedProfiles
            coordinator.persistCursors(profiles: profiles)
            coordinator.persistAllMetadata(profiles: profiles)
            coordinator.stopAll()
        }
        fleet.disconnect()
        fleet.persistCursor()
        // #397: push clear/enroll lifecycle tasks must not outlive the
        // live session that owns their URLSession — cancel them here so a
        // task queued behind teardown bails on Task.isCancelled instead of
        // touching a session that may already be invalidated. Their
        // per-host pending markers were set synchronously before launch,
        // so a cancelled attempt simply stays pending until the next
        // successful connection retries it.
        for task in lifecycleTasks.values {
            task.cancel()
        }
        lifecycleTasks.removeAll()
        // #399: mirror the active profile's cursor into the profile store
        // and persist the allowlisted board metadata cache (background/
        // host-switch boundaries).
        if let profile = activeProfile, let rev = fleet.lastEventId {
            profileStore?.setCursor(rev, for: profile.id)
        }
        persistBoardMetadata()
        // #399 B4: a background/foreground cycle is a reconnect boundary —
        // the pinned key must be re-checked before the next stream opens.
        // A confirmed mismatch stays failed closed (Remove Host only).
        if activeProfile?.hostKeyB64 != nil, keyContinuityState != .mismatch {
            keyContinuityState = .pending
        }
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
        // #399 B4: no fetch may reach a host whose pinned identity is
        // unverified or mismatched — the pull stays silent while the
        // continuity check runs and surfaces the mismatch banner when
        // the host key changed.
        guard keyContinuityAllowsLiveWork else {
            _ = keyContinuityDeniedBanner()
            return
        }
        guard !isRefreshingFleet else { return }
        isRefreshingFleet = true
        defer { isRefreshingFleet = false }
        let context = lifecycleContext()
        // #400 C3: pull-to-refresh fans out to every configured host
        // CONCURRENTLY. The ACTIVE host's fetch keeps its exact legacy
        // behavior (banner on failure); every coordinator host refreshes
        // in parallel and applies its own result — a failing host never
        // blocks or erases the hosts that succeeded.
        let client = CorraldClient(host: hostURL, session: session)
        let activeRefresh = Task { @MainActor [weak self] in
            guard let self else { return }
            do {
                let snapshot = try await client.fetchSnapshot()
                guard !Task.isCancelled, self.isCurrent(context) else { return }
                self.fleet.applyRefresh(snapshot)
                self.persistBoardMetadata()
            } catch {
                guard !Task.isCancelled, self.isCurrent(context) else { return }
                self.banner = .error("fleet_refresh",
                                     "Fleet refresh failed — \(error.localizedDescription)")
            }
        }
        let coordinatorRefresh = Task { @MainActor [weak self] in
            guard let self, let store = self.profileStore else { return }
            _ = await self.coordinator?.refreshAll(profiles: store.orderedProfiles)
        }
        await activeRefresh.value
        _ = await coordinatorRefresh.value
    }

    // MARK: - Notifications (#354 L2)

    /// The transition hooks. The LOCAL path fires only through the DEBUG
    /// bridge (`PushBridge`); release builds rely on APNs — the daemon
    /// pushes the same payload once the APNs provisioning checkpoint is met.
    /// #397: notifications are COMPOSITE — the owning host profile rides
    /// every event (the ACTIVE store hooks and every coordinator session's
    /// hooks land here), the payload carries that host's pinned key as
    /// `host_id`, and per-host + global enable are both enforced.
    private func notifyTransition(_ type: PushPayload.PushType, agentId: String,
                                  hostProfileID: UUID?) {
        guard PushBridge.shouldPresentLocally else { return }
        guard let profile = hostProfileID.flatMap({ profileStore?.profile(id: $0) })
            ?? activeProfile else { return }
        guard profile.notificationsEnabled else { return }
        guard let agent = owningAgent(hostProfileID: profile.id, agentID: agentId) else { return }
        let payload = PushPayload.transition(type: type, agent: agent,
                                              hostId: profile.hostKeyB64)
        notifier?.notify(payload)
    }

    /// #397: the agent of a composite transition from EXACTLY the owning
    /// store (the same resolution the read routes use — an equal raw agent
    /// id on another host never satisfies this event).
    private func owningAgent(hostProfileID: UUID?, agentID: String) -> Agent? {
        if let hostProfileID, hostProfileID != activeProfileID {
            return coordinator?.agent(profileID: hostProfileID, agentID: agentID)
        }
        return fleet.agent(agentID)
    }

    /// Notification-pairing toggle (Settings). Global on/off only — no
    /// per-agent controls, no catch-up/badge on foreground.
    ///
    /// #389: turning notifications ON is permission-aware — a blocked
    /// permission must never be a silent no-op:
    /// - .granted → enable immediately.
    /// - .denied / .restricted → keep the toggle OFF and publish the state;
    ///   the Settings section then shows WHY + an 'Open iOS Settings'
    ///   action (the OS will not deliver anything until the user allows it
    ///   there).
    /// - .notDetermined → prompt first (alert + sound, the same options the
    ///   first-live requestAuthorization uses); only a grant enables.
    /// Disabling stays instant and unconditional.
    func setNotificationsEnabled(_ enabled: Bool) {
        guard enabled else {
            applyNotificationsEnabled(false)
            return
        }
        guard !notificationsEnabled else { return }
        Task { @MainActor [weak self] in
            guard let self else { return }
            let permission = await self.notificationPermissionProvider.currentPermission()
            switch permission {
            case .granted:
                self.notificationPermission = .granted
                self.applyNotificationsEnabled(true)
            case .denied, .restricted:
                self.notificationPermission = permission
            case .notDetermined:
                let granted = await self.notificationPermissionProvider.requestAuthorization()
                self.notificationPermission = granted ? .granted : .denied
                if granted {
                    self.applyNotificationsEnabled(true)
                }
            }
        }
    }

    /// #389: re-read the OS notification permission so the Settings
    /// Notifications section reflects reality — called when Settings
    /// appears and when the app re-activates (after the user returns from
    /// iOS Settings, the grant/deny decision is visible immediately).
    func refreshNotificationPermission() async {
#if DEBUG
        // #389 evidence: the denied-state driver forces the blocked posture
        // in demo mode (a simulator cannot be denied notifications through
        // simctl privacy — the service list has no notifications entry, and
        // the OS alert cannot be answered without touch injection).
        if mode == .demo,
           CorralDemoLaunch.wantsDeniedNotificationsEvidence(arguments: CommandLine.arguments) {
            notificationPermission = .denied
            return
        }
#endif
        notificationPermission = await notificationPermissionProvider.currentPermission()
    }

    /// Shared write path for the toggle value: publishes, persists, and
    /// mirrors into the notifier (both the DEBUG local bridge and the APNs
    /// delivery path honor `isEnabled`).
    private func applyNotificationsEnabled(_ enabled: Bool) {
        notificationsEnabled = enabled
        defaults.set(enabled, forKey: Self.notificationsKey)
        notifier?.isEnabled = enabled
    }

    /// Deep link from a tapped notification: open the agent's row recents.
    /// Live mode only — setup/demo states have no live agent to show. A
    /// deep link is not a row tap, so it plays no haptic.
    /// #397: the payload's `host_id` (the pinned X25519 key of the owning
    /// daemon) resolves EXACTLY one host profile; a host-less legacy
    /// payload may route to the SOLE configured host (F1 back-compat);
    /// anything else — 2+ hosts without a matching host id, an unknown or
    /// removed host, or a mismatched key — is NON-ACTIONABLE with a
    /// bounded diagnostic: the app never guesses another host's lane and a
    /// removed host's alert can never recreate its profile or cached lane.
    func openNotification(agentId: String, hostKeyB64: String?) {
        guard mode == .live else { return }
        if profileStore == nil {
            // Pure legacy single-host runtime (no profile store): the
            // fleet store is the only surface — pre-#397 route unchanged.
            guard fleet.agent(agentId) != nil else {
                banner = .info("This agent is no longer on the fleet — refresh the board.")
                return
            }
            requestRecents(for: agentId, hostProfileID: nil, haptic: false)
            return
        }
        if let hostKeyB64,
           let profile = profiles.first(where: { $0.hostKeyB64 == hostKeyB64 }) {
            routeNotification(to: profile, agentId: agentId)
            return
        }
        // Legacy host-less payload / an identity no profile pins. Exactly
        // one configured host keeps the pre-#397 route for host-less
        // payloads (and unpinned legacy hosts cannot be verified anyway).
        if profiles.count == 1, let only = profiles.first,
           hostKeyB64 == nil || only.hostKeyB64 == nil {
            routeNotification(to: only, agentId: agentId)
            return
        }
        // 2+ hosts (or a pinned sole host receiving a foreign payload):
        // non-actionable — bounded diagnostic, never a cross-host guess.
        guard !profiles.isEmpty else { return }
        banner = .error(
            "notification_host_unknown",
            "This alert doesn't match a paired host, so Corral won't guess which lane to open. Re-pair the host or dismiss the alert.")
    }

    /// #397: the routed half of a notification tap — the owning profile's
    /// recents route (agent-existence, continuity, and posture gates all
    /// live inside `requestRecents`).
    private func routeNotification(to profile: HostProfile, agentId: String) {
        requestRecents(for: agentId, hostProfileID: profile.id, haptic: false)
    }

    // MARK: - Recents sheet request lifecycle (#364 C)

    /// Every board/notification/demo open request funnels through here:
    /// the request is ALWAYS a fresh value with a monotonic id, so a
    /// re-request of the agent currently (or previously) shown is a real
    /// nil → request transition for `.sheet(item:)` — the first tap after
    /// any dismissal re-presents. `haptic: true` is reserved for real row
    /// taps (one light selection tick); programmatic opens stay silent.
    func requestRecents(for agentId: String, haptic: Bool) {
        requestRecents(for: agentId, hostProfileID: activeProfile?.id, haptic: haptic)
    }

    /// #400 E1: open the recents sheet for a COMPOSITE target — the row's
    /// host profile rides in the request. EXACTLY one profile is resolved
    /// (the legacy/active store for nil/active ids, the coordinator
    /// session's store otherwise); NO other host is searched, so an equal
    /// raw agent id on another host never satisfies this request.
    func requestRecents(for agentId: String, hostProfileID: UUID?, haptic: Bool) {
        let profileID = hostProfileID ?? activeProfile?.id
        guard let profileID else {
            // No profile store: the pure legacy/demo runtime — the fleet
            // store is the only surface (unchanged single-host behavior).
            guard fleet.agent(agentId) != nil else { return }
            // #399 B4: no Recent Output route while the pinned host
            // identity is unverified or mismatched — the sheet stays
            // closed.
            guard keyContinuityAllowsLiveWork else {
                _ = keyContinuityDeniedBanner()
                return
            }
            presentRecents(agentId: agentId, hostProfileID: nil, haptic: haptic)
            return
        }
        guard let profile = profileStore?.profile(id: profileID) else { return }
        if profileID == activeProfileID {
            guard fleet.agent(agentId) != nil else { return }
            guard keyContinuityAllowsLiveWork else {
                _ = keyContinuityDeniedBanner()
                return
            }
            presentRecents(agentId: agentId, hostProfileID: profileID, haptic: haptic)
            return
        }
        // Coordinator-owned host (E1): the owning session store must be
        // connectable and must hold the row — never another host's.
        guard profile.mayConnect else { return }
        guard coordinator?.allowsLiveWork(profileID: profileID) == true else { return }
        guard coordinator?.agent(profileID: profileID, agentID: agentId) != nil else { return }
        presentRecents(agentId: agentId, hostProfileID: profileID, haptic: haptic)
    }

    private func presentRecents(agentId: String, hostProfileID: UUID?, haptic: Bool) {
        if haptic { hapticTick() }
        recentsSerial += 1
        recentsRequest = RecentsRequest(id: recentsSerial, agentId: agentId,
                                        hostProfileID: hostProfileID)
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
        recentsRequest = RecentsRequest(id: recentsSerial, agentId: pending.agentId,
                                        hostProfileID: pending.hostProfileID)
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
    /// #400 E1: the route is COMPOSITE — `hostProfileID` names the owning
    /// profile; signing uses THAT profile's key id/grants + the shared
    /// phone signer against that profile URL with the untouched raw agent
    /// id. The route NEVER searches or retries another host.
    func driveReadTail(agent: Agent, hostProfileID: UUID? = nil,
                       driveClient: DriveClient, silent: Bool = false,
                       lines: UInt32 = 200) {
        let profileID = hostProfileID ?? activeProfile?.id
        guard let route = readRoute(hostProfileID: profileID, silent: silent) else { return }
        // E1: the target row must exist in the OWNING store — an equal raw
        // agent id on another host never satisfies this route.
        guard let live = route.store.agent(agent.agentId) else { return }
        // #400 E2: reload is disabled while the owning host is not
        // connected; any already-loaded output stays visible (memory-only)
        // and nothing is synthesized or loaded from the durable cache.
        guard route.store.connectionState == .connected else {
            if !silent {
                banner = .error("host_offline",
                                "Recent output is unavailable while the host is offline.")
            }
            return
        }
        guard authorize(.readTail, for: live, grants: route.grants,
                        silent: silent) else { return }
        // E1: sign against THAT profile's URL — for coordinator-owned
        // hosts the caller's (active-host) client is replaced by a client
        // bound to the owning profile, so a signed read can never cross
        // hosts even when the view hands the model a stale client.
        let client: DriveClient
        if let profile = route.profile, profile.id != activeProfileID,
           let url = URL(string: profile.urlString) {
            client = DriveClient(host: url, session: session)
        } else {
            client = driveClient
        }
        let sinceRev = route.store.tailPane(for: live.agentId)?.sourceRev
        let payload = CanonicalJSON.readTailPayload(lines: lines, sinceRev: sinceRev)
        let key = DriveActionKey(capability: .readTail, target: live.agentId,
                                 identity: "tail-\(lines)", hostProfileID: profileID)
        guard let requestId = beginDriveAction(key, silent: silent) else { return }
        route.store.prepareTailFetch(agent: live.agentId)
        drive(capability: .readTail, target: live.agentId, payload: payload,
              driveClient: client, keyId: route.keyId, signer: route.signer,
              actionKey: key, requestId: requestId,
              store: route.store, profile: route.profile)
    }

    /// The signed read route of one host profile (E1): exactly one profile
    /// resolves to exactly one store/key-id/grants set. `nil`/active ids
    /// resolve to the legacy single-host runtime (F1 parity); coordinator
    /// profiles resolve to their OWN session store only. A paused or
    /// key-mismatched profile has no route (fail closed — nothing reaches
    /// the replacement identity and no other host is searched).
    private func readRoute(hostProfileID: UUID?, silent: Bool) -> ReadRoute? {
        let profile = hostProfileID.flatMap { profileStore?.profile(id: $0) }
        if let profile {
            guard profile.mayConnect else { return nil }
            let store: FleetStore
            if profile.id == activeProfileID {
                guard keyContinuityAllowsLiveWork else {
                    if !silent { _ = keyContinuityDeniedBanner() }
                    return nil
                }
                store = fleet
            } else {
                guard let sessionStore = coordinator?.store(profileID: profile.id),
                      coordinator?.allowsLiveWork(profileID: profile.id) == true else {
                    return nil
                }
                store = sessionStore
            }
            guard let keyId = profile.keyId, let signer else { return nil }
            return ReadRoute(profile: profile, store: store, keyId: keyId,
                             grants: Set(profile.grants.compactMap(Capability.init(rawValue:))),
                             signer: signer)
        }
        // Legacy single-host runtime / demo (no profile store, or a nil
        // target id): the pre-#400 route, unchanged.
        guard keyContinuityAllowsLiveWork else {
            if !silent { _ = keyContinuityDeniedBanner() }
            return nil
        }
        guard let signer, let keyId else {
            if !silent { banner = .error("unregistered", "Device is not registered.") }
            return nil
        }
        return ReadRoute(profile: nil, store: fleet, keyId: keyId,
                         grants: actionGrants, signer: signer)
    }

    private struct ReadRoute {
        let profile: HostProfile?
        let store: FleetStore
        let keyId: String
        let grants: Set<Capability>
        let signer: DeviceSigner
    }

    /// #400 E1: the agent of a composite target from EXACTLY the owning
    /// store. nil/active ids resolve to the legacy fleet store; any other
    /// profile resolves its coordinator session store. Equal raw ids on
    /// other hosts are unreachable here.
    func fleetAgent(hostProfileID: UUID?, agentID: String) -> Agent? {
        if let profileID = hostProfileID, profileID != activeProfileID {
            return coordinator?.agent(profileID: profileID, agentID: agentID)
        }
        return fleet.agent(agentID)
    }

    /// #400 E1: the tail pane of a composite target from its OWNING store.
    func fleetTailPane(hostProfileID: UUID?, agentID: String) -> TailPane? {
        if let profileID = hostProfileID, profileID != activeProfileID {
            return coordinator?.tailPane(profileID: profileID, agentID: agentID)
        }
        return fleet.tailPane(for: agentID)
    }

    /// #400 E2: Recent Output availability of a composite target. Reload is
    /// only permitted while the owning host is connected; loaded output
    /// stays visible as `.offline`, nothing loaded + disconnected is
    /// `.unavailable` (never synthesized, never loaded from durable
    /// storage). #401 rev N2: a target whose host was REMOVED (or paused) is
    /// `.unavailable` — it can never fall through to the ACTIVE fleet store
    /// and resolve an equal raw agent id on another host.
    func recentsRouteState(hostProfileID: UUID?, agentID: String) -> RecentsRouteState {
        let store: FleetStore?
        if let profileID = hostProfileID {
            // Removed host: no profile, no session — render unavailable
            // (E3 purge already cleared an open sheet; this closes the race
            // for a sheet still dismissing or a stale route consumer).
            guard let profile = profileStore?.profile(id: profileID) else {
                return .unavailable
            }
            guard profile.mayConnect else { return .unavailable }
            if profileID == activeProfileID {
                store = fleet
            } else if let sessionStore = coordinator?.store(profileID: profileID) {
                store = sessionStore
            } else {
                return .unavailable
            }
        } else {
            store = fleet
        }
        guard let store else { return .unavailable }
        guard store.connectionState == .connected else {
            let pane = store.tailPane(for: agentID)
            return (pane?.isEmpty ?? true) ? .unavailable : .offline
        }
        return .live
    }

    /// Both sides of the drive authorization contract must hold locally for
    /// a read control. The daemon remains authoritative and can still
    /// return a typed refusal, which the common drive path surfaces.
    /// #400 E1: composite routes authorize against the OWNING profile's
    /// grants (`grants`); the legacy runtime passes its global set.
    private func authorize(_ capability: Capability, for agent: Agent,
                           grants: Set<Capability>? = nil,
                           silent: Bool = false) -> Bool {
        guard agent.capabilities.contains(capability.rawValue) else {
            if !silent {
                banner = .error("capability_unavailable",
                                "\(capability.rawValue): not available for this agent.")
            }
            return false
        }
        guard (grants ?? actionGrants).contains(capability) else {
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

    /// #400 E1/E3: one signed read drive. `store` is the composite target's
    /// OWNING store — results, tail panes, failures, and stale removals
    /// land there, never in another host's namespace. Cancellation is
    /// scoped per host (`DriveActionKey.hostProfileID`), so removing one
    /// host terminates exactly its read tasks.
    private func drive(capability: Capability, target: String, payload: CanonicalJSON.Value,
                       driveClient: DriveClient, keyId: String, signer: DeviceSigner,
                       actionKey: DriveActionKey, requestId: String,
                       store: FleetStore, profile: HostProfile?) {
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
            let rev = store.lastEventId
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
                    store.rememberTail(lines, blocks: blocks,
                                       sourceRev: response.result?.tailSourceRev ?? response.rev,
                                       for: target)
                } else {
                    if response.errorKind == "stale_agent" {
                        self.handleStaleAgent(target,
                                              message: response.error ?? "the agent moved or disappeared",
                                              store: store, profile: profile)
                    } else {
                        store.foldTailFailure(TranscriptFailure(
                            kind: response.errorKind ?? "dispatch_refused",
                            message: response.error ?? "dispatch refused (ok:false)",
                            candidates: []), for: target)
                    }
                }
            case .refused(let error):
                switch error {
                case .server(let status, let kind, let message, _):
                    if kind == "stale_agent" {
                        self.handleStaleAgent(target, message: message,
                                              store: store, profile: profile)
                        return
                    }
                    if capability == .readTail {
                        store.foldTailFailure(TranscriptFailure(
                            kind: kind, message: message, candidates: []), for: target)
                        return
                    }
                    // Read-only default: ungranted capabilities are refused
                    // with the typed banner.
                    self.banner = .error(kind, "\(message) (HTTP \(status))")
                case .network(let message):
                    if capability == .readTail {
                        store.foldTailFailure(TranscriptFailure(
                            kind: message == "Recent output timed out." ? "timeout" : "transport",
                            message: message, candidates: []), for: target)
                    } else {
                        self.banner = .error("network", message)
                    }
                case .encoding:
                    if capability == .readTail {
                        store.foldTailFailure(TranscriptFailure(
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

    /// #400 E3: cancel every in-flight read task owned by ONE host profile
    /// (host removal). Other hosts' drives are untouched — removal never
    /// tears down the rest of the board.
    private func cancelHostDriveTasks(hostProfileID: UUID) {
        let doomed = driveTaskKeys.filter { $0.value.hostProfileID == hostProfileID }
            .map(\.key)
        for requestId in doomed {
            driveTasks.removeValue(forKey: requestId)?.cancel()
            if let key = driveTaskKeys.removeValue(forKey: requestId) {
                inFlightDriveKeys.remove(key)
            }
        }
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
    /// fetch the current read model once. #400 E3: the removal + refetch are
    /// scoped to the COMPOSITE target's own store/host — the row is purged
    /// from exactly the owning host (its tails + sheet state go with it) and
    /// the reconciliation snapshot comes from that same host. An equal raw
    /// id on another host is untouched.
    private func handleStaleAgent(_ target: String, message: String,
                                  store: FleetStore, profile: HostProfile?) {
        store.removeAgent(target)
        banner = .error("stale_agent", "\(message) — refreshing the fleet.")
        let context = lifecycleContext()
        guard context.mode == .live else { return }
        // The reconciliation fetch is a live read of THE OWNING host.
        let hostURL: URL?
        if let profile, let url = URL(string: profile.urlString) {
            hostURL = url
        } else {
            hostURL = context.hostURL
            guard keyContinuityAllowsLiveWork else { return }
        }
        guard let hostURL else { return }
        let client = CorraldClient(host: hostURL, session: session)
        let taskId = UUID()
        let task = Task { @MainActor [weak self] in
            guard let self else { return }
            defer { self.lifecycleTasks.removeValue(forKey: taskId) }
            guard self.isCurrent(context) else { return }
            guard let snapshot = try? await client.fetchSnapshot() else { return }
            guard !Task.isCancelled, self.isCurrent(context) else { return }
            store.apply(.snapshot(snapshot))
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
        // #399: demo has no host-key confirmation flow — a paused
        // migration confirmation is deferred until live mode restores.
        fingerprintConfirmation = nil
        identityLifecycle.setCurrent(mode: .demo, hostURL: hostURL,
                                    keyId: keyId,
                                    signerPublicKeyB64: signer?.publicKeyB64)
    }

    /// #401 evidence: deterministic MULTI-HOST demo state (DEBUG only, fresh
    /// simulators): three synthetic profiles — Host A LIVE (the active
    /// store, seeded with rows), Host B OFFLINE (its coordinator session
    /// keeps retained STALE rows, last-seen ~6m), Host C KEY MISMATCH
    /// (paused, fails closed, no session). Everything routes through the
    /// #399/#400 public seams (profile store + coordinator sessions' own
    /// stores) — no network, no stream internals.
    func enterMultiHostDemo() {
        guard let store = profileStore, coordinator != nil else {
            enterDemo()
            return
        }
        cancelLifecycleTasks()
        fleet.disconnect()
        let now = UInt64(Date().timeIntervalSince1970 * 1000)
        let nowSeconds = UInt64(Date().timeIntervalSince1970)
        store.removeAll()
        reloadProfiles(from: store)
        let profileA = try? store.addProfile(
            displayName: "Host A",
            urlString: DemoFleet.DemoHosts.urls[0],
            hostKeyB64: DemoFleet.DemoHosts.hostAKey,
            fingerprint: "FINGER-A",
            keyId: "dev_demo_a",
            grants: ["read_tail"],
            expiryTs: nowSeconds + 90 * 86_400,
            registeredAt: 1)
        let profileB = try? store.addProfile(
            displayName: "Host B",
            urlString: DemoFleet.DemoHosts.urls[1],
            hostKeyB64: DemoFleet.DemoHosts.hostBKey,
            fingerprint: "FINGER-B",
            keyId: "dev_demo_b",
            grants: ["read_tail"],
            expiryTs: nowSeconds + 30 * 86_400,
            registeredAt: 1)
        let profileC = try? store.addProfile(
            displayName: "Host C",
            urlString: DemoFleet.DemoHosts.urls[2],
            hostKeyB64: DemoFleet.DemoHosts.hostCKey,
            fingerprint: "FINGER-C",
            keyId: "dev_demo_c",
            grants: ["read_tail"],
            expiryTs: nil,
            registeredAt: 1)
        if let profileC {
            // B4: C presents a different key — paused, fails closed.
            store.noteConnectionState(id: profileC.id, .keyMismatch)
        }
        guard let profileA, let profileB else {
            store.removeAll()
            reloadProfiles(from: store)
            enterDemo()
            return
        }
        persistActiveProfileID(profileA.id)
        reloadProfiles(from: store)
        // Active host A: live seeded rows.
        fleet.reset()
        fleet.seedDemo(agents: DemoFleet.multiHostSeedA(now: now), rev: 1)
        fleet.noteConnected()
        // Host B: retained STALE rows in its coordinator session store.
        if let storeB = coordinator?.store(profileID: profileB.id) {
            storeB.seedDemo(agents: DemoFleet.multiHostSeedB(now: now), rev: 1)
            storeB.noteConnectionError("host unreachable")
        }
        store.noteLastSuccessfulConnection(id: profileA.id,
                                           at: now - 2 * 60 * 1000)
        store.noteLastSuccessfulConnection(id: profileB.id,
                                           at: now - 6 * 60 * 1000)
        mode = .demo
        demoDetailAgentId = nil
        fingerprintConfirmation = nil
        hostFilter = nil
        repoFilter = nil
        keyContinuityState = .notPinned
        identityLifecycle.setCurrent(mode: .demo, hostURL: nil,
                                    keyId: nil,
                                    signerPublicKeyB64: signer?.publicKeyB64)
    }

    /// #415 evidence: seeds the Add Host lifecycle evidence state — ONE
    /// pre-existing "Mac" profile (the original host that must survive an
    /// Add Host commit) and no live fleet. The bg-return / failed-submit
    /// evidence drivers never reach a live stream (mode stays .demo); the
    /// successful-commit driver runs the REAL prepare/register/commit flow
    /// against a DEBUG fixture URLSession (AddHostCommitEvidenceURLProtocol)
    /// and transitions to .live at the end — the OS notification
    /// authorization prompt is pre-suppressed on the evidence sim so the
    /// captured frames show the pairing result, not the permission alert.
    func enterAddHostEvidenceSeed() {
        guard let store = profileStore, coordinator != nil else { return }
        cancelLifecycleTasks()
        fleet.disconnect()
        store.removeAll()
        reloadProfiles(from: store)
        let nowSeconds = UInt64(Date().timeIntervalSince1970)
        if let mac = try? store.addProfile(
            displayName: "Mac",
            urlString: Self.addHostEvidenceMacURL,
            hostKeyB64: Self.addHostEvidenceMacKey,
            fingerprint: HostKeyTrust.fingerprint(forBase64: Self.addHostEvidenceMacKey)
                ?? "FINGER-MAC",
            keyId: "dev_evidence_mac",
            grants: ["read_tail"],
            expiryTs: nowSeconds + 90 * 86_400,
            registeredAt: 1) {
            store.noteLastSuccessfulConnection(id: mac.id)
        }
        reloadProfiles(from: store)
        if let mac = store.orderedProfiles.first {
            persistActiveProfileID(mac.id)
        }
        mode = .demo
        demoDetailAgentId = nil
        fingerprintConfirmation = nil
        hostFilter = nil
        repoFilter = nil
        keyContinuityState = .notPinned
        identityLifecycle.setCurrent(mode: .demo, hostURL: nil,
                                     keyId: nil,
                                     signerPublicKeyB64: signer?.publicKeyB64)
        // #415 evidence (c): completeAddHost's success path calls
        // startLive(); mark the one-shot notification setup done so the
        // evidence sim never presents the OS authorization alert over the
        // frames (DEBUG fixture transport; no daemon, no push).
        notificationsConfigured = true
    }

    /// #415 evidence: fixture URL of the Mac host (bg-return / failed /
    /// commit drivers seed this profile; see AddHostCommitEvidenceURLProtocol).
    static let addHostEvidenceMacURL = "https://mac-evidence.tail0000.ts.net"
    /// #415 evidence: fixture URL of the NEW host the commit driver pairs.
    static let addHostEvidenceNewHostURL = "https://g415-bazzite.tail0000.ts.net"
    /// #415 evidence: the original Mac host's fixture X25519 key (32-byte
    /// fill — synthetic, never a real key).
    static let addHostEvidenceMacKey = Data(repeating: 21, count: 32).base64EncodedString()
    /// #415 evidence: fixture registration token for the commit driver.
    /// Synthetic + DEBUG-only; the register body is consumed by the
    /// fixture URLProtocol and never logged or persisted.
    static let addHostEvidenceToken = "g415-evidence-registration-token"

    /// Leave demo through the same identity boundary as reset/registration.
    /// Demo rows and their cursor are discarded before a live identity is
    /// restored, so the next connection must receive a fresh snapshot.
    func exitDemo() {
        guard mode == .demo else { return }
        cancelLifecycleTasks()
        fleet.reset()
        demoDetailAgentId = nil

        // #399: restore the profile store's active profile when one
        // exists (migration consumed the legacy keys, so the legacy path
        // below would otherwise see nothing and drop to setup).
        if let store = profileStore, let profile = store.orderedProfiles.first {
            fleet.acceptedHostIdentity = nil
            keyContinuityState = .notPinned
            fingerprintConfirmation = nil
            persistActiveProfileID(profile.id)
            bindActiveProfile(profile)
            startLive()
            return
        }

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
        defaults.removeObject(forKey: Self.activeProfileKey)
        // #399: Remove device removes the shared phone identity — every
        // host profile's LOCAL state (profile/cursor/cache) goes with it.
        profileStore?.removeAll()
        profiles = []
        activeProfileID = nil
        // #401 D1: a device reset leaves no host selected (session filter).
        hostFilter = nil
        // #400 E3: tear down every coordinator session (stream/tasks/rows
        // purged).
        syncCoordinator(startStreams: false)
        // #397: a device reset retires every per-host enrollment artifact —
        // pending clears, dedupe state, and the shared retained token
        // (cleared above).
        pendingPushTokenClears.removeAll()
        pendingPushClearTargets.removeAll()
        enrolledTokenPerHost.removeAll()
        notifier?.removeAll()
        keyContinuityState = .notPinned
        fingerprintConfirmation = nil
        fleet.acceptedHostIdentity = nil
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
