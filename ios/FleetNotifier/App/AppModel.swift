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
    private var driveTask: Task<Void, Never>?
    /// In-flight notification-reply validation (cold-start snapshot fetch).
    private var notificationTask: Task<Void, Never>?

    private let defaults = UserDefaults.standard

    init() {
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
            let client = DriveClient(host: url)
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
            banner = .info("Registered \(response.keyId.prefix(12))… read-only until the host grants capabilities (grants: \(response.grants.isEmpty ? "none" : response.grants.joined(separator: ", ")))")
        } catch {
            banner = .error("register_failed", error.localizedDescription)
        }
    }

    // MARK: - Live connection

    func startLive() {
        guard let hostURL else { return }
        let client = CorraldClient(host: hostURL)
        let driveClient = DriveClient(host: hostURL)
        fleet.onNewlyBlocked = { [weak self] agentId in
            Task { @MainActor in self?.notifyBlocked(agentId: agentId) }
        }
        fleet.onNewlyDone = { [weak self] agentId in
            Task { @MainActor in self?.notifyDone(agentId: agentId) }
        }
        fleet.connect(client: client)
        notifier = LocalNotifier()
        Task { await notifier?.requestAuthorization() }
        notifier?.registerCategories()
        notifier?.onReply = { [weak self] payload, action in
            self?.handleNotificationReply(payload: payload, action: action, driveClient: driveClient)
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
    func handleCannedAction(agentId: String, action: CannedChoice.Action, driveClient: DriveClient) {
        guard let agent = fleet.agent(agentId), let waiting = agent.waitingOn else {
            banner = .error("no_waiting_approval", "The agent is no longer waiting; the claim is stale.")
            return
        }
        guard let choice = CannedChoice.choice(for: action, kind: waiting.kind, choices: waiting.choices) else {
            banner = .error("cannot_approve_kind", "This waiting state cannot be answered with \(action.rawValue).")
            return
        }
        driveApprove(agent: agent, choice: choice, driveClient: driveClient)
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
                                 driveClient: DriveClient) {
        let task = Task { @MainActor [weak self] in
            guard let self else { return }
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
        let approvalId = payload.approvalId ?? Claim.approvalId(agentId: payload.agentId,
                                                                promptHash: promptHash)
        let approvePayload = CanonicalJSON.approvePayload(approvalId: approvalId,
                                                          promptHash: promptHash,
                                                          choice: choice)
        drive(capability: .approve, target: payload.agentId, payload: approvePayload,
              driveClient: driveClient, keyId: keyId, signer: signer)
    }

    // MARK: - Drive flows (D8 claims, byte-for-byte from the snapshot)

    /// The approval claim is echoed EXACTLY from the snapshot's `waiting_on`
    /// (approval_id + prompt_hash); never re-derived from pane text.
    func driveApprove(agent: Agent, choice: String, driveClient: DriveClient) {
        guard let signer, let keyId else {
            banner = .error("unregistered", "Device is not registered.")
            return
        }
        guard let waiting = agent.waitingOn else {
            banner = .error("no_waiting_approval", "Agent is not waiting on an approval.")
            return
        }
        let approvalId = waiting.approvalId ?? Claim.approvalId(agentId: agent.agentId, promptHash: waiting.promptHash)
        let payload = CanonicalJSON.approvePayload(approvalId: approvalId,
                                                   promptHash: waiting.promptHash,
                                                   choice: choice)
        drive(capability: .approve, target: agent.agentId, payload: payload,
              driveClient: driveClient, keyId: keyId, signer: signer)
    }

    /// `read_tail` is bounded (D5): 200 lines, never prefetched.
    func driveReadTail(agent: Agent, driveClient: DriveClient) {
        guard let signer, let keyId else {
            banner = .error("unregistered", "Device is not registered.")
            return
        }
        let payload = CanonicalJSON.readTailPayload(lines: 200)
        drive(capability: .readTail, target: agent.agentId, payload: payload,
              driveClient: driveClient, keyId: keyId, signer: signer)
    }

    func drivePrompt(agent: Agent, text: String, driveClient: DriveClient) {
        guard let signer, let keyId else {
            banner = .error("unregistered", "Device is not registered.")
            return
        }
        let payload = CanonicalJSON.promptPayload(text: text)
        drive(capability: .prompt, target: agent.agentId, payload: payload,
              driveClient: driveClient, keyId: keyId, signer: signer)
    }

    // NOTE: `driveCommand` (the interrupt/kill wrapper) was removed with the
    // D30 board rework — row actions are Approve/Deny/Prompt/Tail only, and
    // kill-on-phone is D29-cut. `DriveClient.drive` still supports every
    // capability for any future, non-row surface.

    private func drive(capability: Capability, target: String, payload: CanonicalJSON.Value,
                       driveClient: DriveClient, keyId: String, signer: DeviceSigner) {
        driveTask?.cancel()
        driveTask = Task { [weak self] in
            guard let self else { return }
            let result = await driveClient.drive(capability: capability, target: target,
                                                 payload: payload, rev: self.fleet.lastEventId,
                                                 keyId: keyId, signer: signer)
            switch result {
            case .dispatched(let response):
                if response.ok {
                    if capability == .approve {
                        self.banner = .info("Approved \(target): rev \(response.rev)")
                    }
                } else {
                    self.banner = .error("dispatch_refused",
                                         response.error ?? "dispatch refused (ok:false)")
                }
            case .refused(let error):
                switch error {
                case .server(let status, let kind, let message, _):
                    // Read-only default: ungranted capabilities are refused
                    // with the typed banner; the UI also hides the buttons.
                    self.banner = .error(kind, "\(message) (HTTP \(status))")
                case .network(let message):
                    self.banner = .error("network", message)
                case .encoding:
                    self.banner = .error("encoding", "payload encoding failed")
                }
            }
        }
    }

    // MARK: - Demo mode (App Review 4.2)

    func enterDemo() {
        driveTask?.cancel()
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
        stopLive()
        DeviceKeyStore.wipe()
        defaults.removeObject(forKey: "fleetnotifier.host")
        fleet.reset()
        signer = nil
        keyId = nil
        grants = []
        hostURL = nil
        mode = .needsSetup
    }
}

/// Where the app stands on the wire: registered device + host.
struct DeviceRegistration: Equatable {
    var host: String
    var keyId: String
    var grants: [String]
}
