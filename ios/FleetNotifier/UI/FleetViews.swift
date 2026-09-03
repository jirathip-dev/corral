import SwiftUI
import Combine

// MARK: - #354 L2 read-only FleetNotifier
//
// Surfaces after the client cut:
// - Board (home): repo groups with raw herdr state chips; blocked agents
//   pinned on top; row = agent name · repo · state · time-in-state · branch
//   + small pane ref. NO search, NO repo filter chips, NO actions.
// - Recents: tap a row → bottom sheet with the LIVE tail (auto-scroll,
//   ≤200-line daemon cap). No load-earlier, no Conversation/Harness
//   partition, no composer.
// - Settings: connection + notification pairing only.
// Removed: Issues browser, Terminal, Diff, approval/prompt/attach/kill
// controls, device/grant admin.

// MARK: - Row state chrome

/// Self-ticking "· 4s" duration chip. It owns its own clock so a 1 Hz tick
/// re-renders only this small view, not the whole board.
private struct TimeInStateLabel: View {
    let agent: Agent
    var stateEnteredAt: UInt64? = nil
    @State private var now: UInt64 = UInt64(Date().timeIntervalSince1970 * 1000)

    var body: some View {
        if let durationText {
            Text("· \(durationText)")
                .font(.caption)
                .foregroundStyle(.secondary)
                .accessibilityHidden(true)
                .task(id: tickInterval) {
                    while !Task.isCancelled {
                        now = UInt64(Date().timeIntervalSince1970 * 1000)
                        do {
                            try await Task.sleep(nanoseconds: tickInterval)
                        } catch {
                            break
                        }
                    }
                }
        }
    }

    private var durationText: String? {
        TimeInState.milliseconds(for: agent, stateEnteredAt: stateEnteredAt, now: now)
            .map(RelativeTime.duration(milliseconds:))
    }

    /// Nanoseconds; 1s while the agent is under a minute in state, else 30s.
    private var tickInterval: UInt64 {
        let entered = stateEnteredAt ?? agent.ts
        guard entered > 0 else {
            return 1_000_000_000
        }
        let age = now >= entered ? now - entered : 0
        return (age < 60_000 ? 1 : 30) * 1_000_000_000
    }
}

/// The board row (#354 L2): raw state chip + time-in-state, agent name,
/// repo · branch line, and the small pane reference (debug aid). The whole
/// row is a read-only tap target that opens the agent's recents sheet —
/// there are no action controls anywhere.
struct AgentRow: View {
    let agent: Agent
    /// #166 review F2: client-side state-entered wall clock, passed down
    /// from `FleetStore.stateEnteredAt` so a reason/title churn does not
    /// reset the duration. `nil` falls back to `agent.ts`.
    var stateEnteredAt: UInt64? = nil
    @Environment(\.dynamicTypeSize) private var dynamicTypeSize
    @ScaledMetric(relativeTo: .caption) private var badgeMinWidth: CGFloat = 84

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            if isAccessibilitySize {
                // Dynamic Type: stack the trailing chips under the title.
                VStack(alignment: .leading, spacing: 4) {
                    HStack(spacing: 6) {
                        statusDot
                        stateBadge
                        titleText
                    }
                    trailingChips
                }
            } else {
                HStack(spacing: 6) {
                    statusDot
                    stateBadge
                    titleText
                    Spacer(minLength: 0)
                    trailingChips
                }
            }
            WorkspaceLine(agent: agent)
        }
        .padding(.vertical, 2)
        .opacity(agent.state == .idle ? 0.65 : 1)
        .frame(maxWidth: .infinity, alignment: .leading)
    }

    // MARK: - Row subviews

    @ViewBuilder
    private var statusDot: some View {
        Circle()
            .fill(stateStyle.isRing ? Color.clear : stateStyle.color)
            .overlay(Circle().stroke(stateStyle.color, lineWidth: 1))
            .frame(width: 12, height: 12)
            .accessibilityHidden(true)
    }

    /// Fixed-width state badge: raw herdr token + duration. A real
    /// `minWidth` keeps the badges in one column even as durations roll
    /// over; `.lineLimit(1)` preserves the no-mid-word-wrap rule.
    @ViewBuilder
    private var stateBadge: some View {
        HStack(spacing: 3) {
            Text(stateStyle.glyph)
                .font(.caption.weight(.bold))
                .foregroundStyle(stateStyle.color)
                .accessibilityHidden(true)
            Text(stateStyle.label)
                .font(.caption.weight(.semibold))
                .foregroundStyle(stateStyle.color)
                .accessibilityLabel(stateStyle.accessibilityLabel)
            TimeInStateLabel(agent: agent, stateEnteredAt: stateEnteredAt)
        }
        .lineLimit(1)
        .frame(minWidth: badgeMinWidth, alignment: .leading)
        .fixedSize(horizontal: true, vertical: false)
        .layoutPriority(2)
    }

    @ViewBuilder
    private var titleText: some View {
        Text(agent.title ?? agent.displayName ?? agent.agentId)
            .font(.subheadline.weight(.semibold))
            .lineLimit(1)
            .layoutPriority(1)
    }

    /// Trailing chips: the small pane reference (debug aid) and the tool.
    @ViewBuilder
    private var trailingChips: some View {
        HStack(spacing: 6) {
            if let reference = paneReference {
                Text(reference)
                    .font(.caption2.monospaced())
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
                    .fixedSize(horizontal: true, vertical: false)
                    .accessibilityLabel("Pane \(reference)")
            }
            Text(agent.tool)
                .font(.caption2.monospaced())
                .lineLimit(1)
                .fixedSize(horizontal: true, vertical: false)
                .padding(.horizontal, 6)
                .padding(.vertical, 2)
                .background(.quaternary, in: RoundedRectangle(cornerRadius: 4))
        }
    }

    /// The debug-aid pane reference, shortened to its final path segment
    /// when the source prefixes it (`herdr:pane:w21:p1` → `w21:p1`).
    private var paneReference: String? {
        guard let reference = agent.attachment?.reference else { return nil }
        if let last = reference.split(separator: ":").suffix(2).joined(separator: ":") as String?,
           last.contains(":") {
            return last
        }
        return reference
    }

    private var isAccessibilitySize: Bool {
        dynamicTypeSize >= .accessibility1
    }

    private var stateStyle: StateStyle {
        StateStyle.style(for: agent.state)
    }
}

// MARK: - Row accessibility

/// One VoiceOver summary for an agent row: name, raw state, repo, branch,
/// pane. Used as the row button's label so the whole row is a single named,
/// activatable element.
private func rowSummary(_ agent: Agent) -> String {
    var parts: [String] = [
        agent.title ?? agent.displayName ?? agent.agentId,
        "State: \(StateStyle.style(for: agent.state).label)",
    ]
    if let repo = agent.workspace.repo { parts.append(repo) }
    if let branch = agent.workspace.branch { parts.append(branch) }
    if let reference = agent.attachment?.reference { parts.append("Pane \(reference)") }
    return parts.joined(separator: ", ")
}

/// Line 2 (D26): repo·branch·worktree basename — no nesting level — with
/// PR / dirty / one `↑a↓b` badge trailing (D29: not separate columns).
///
/// Each segment is its own `Text` so truncation is per-segment: the
/// identity segments (branch, worktree basename) middle-truncate within
/// their own bounds, and the basename sits in the top priority tier so a
/// long worktree name keeps head AND tail instead of collapsing to a bare
/// `…` stub (G100).
struct WorkspaceLine: View {
    let agent: Agent

    /// Per-segment truncation + compression policy (G100).
    enum SegmentPolicy {
        case repo
        case branch
        case basename
        case badge

        static func priority(for segment: SegmentPolicy) -> Double {
            switch segment {
            case .basename, .badge: return 2
            case .repo, .branch: return 0
            }
        }

        static func truncationMode(for segment: SegmentPolicy) -> Text.TruncationMode {
            switch segment {
            case .branch, .basename: return .middle
            case .repo, .badge: return .tail
            }
        }
    }

    var body: some View {
        let w = agent.workspace
        HStack(spacing: 4) {
            if let repo = w.repo {
                Text(repo)
                    .font(.caption2.monospaced())
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
                    .layoutPriority(SegmentPolicy.priority(for: .repo))
            }
            if let branch = w.branch {
                if w.repo != nil {
                    Text("·").font(.caption2).foregroundStyle(.secondary)
                }
                Text(branch)
                    .font(.caption2.monospaced())
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
                    .truncationMode(SegmentPolicy.truncationMode(for: .branch))
                    .layoutPriority(SegmentPolicy.priority(for: .branch))
            }
            if let basename = Self.worktreeBasename(w) {
                Text("·").font(.caption2).foregroundStyle(.secondary)
                Text(basename)
                    .font(.caption2.monospaced())
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
                    .truncationMode(SegmentPolicy.truncationMode(for: .basename))
                    .layoutPriority(SegmentPolicy.priority(for: .basename))
            }
            if w.repo == nil && w.branch == nil && Self.worktreeBasename(w) == nil {
                Text("—").font(.caption2).foregroundStyle(.secondary)
            }
            Spacer(minLength: 4)
            if let pr = w.prNumber {
                Text("#\(pr)")
                    .font(.caption2)
                    .foregroundStyle(.secondary)
                    .layoutPriority(SegmentPolicy.priority(for: .badge))
            }
            if w.dirty {
                Text("dirty").font(.caption2.weight(.semibold))
                    .foregroundStyle(.orange)
                    .layoutPriority(SegmentPolicy.priority(for: .badge))
            }
            if w.ahead > 0 || w.behind > 0 {
                Text("↑\(w.ahead)↓\(w.behind)")
                    .font(.caption2.monospaced())
                    .foregroundStyle(.secondary)
                    .layoutPriority(SegmentPolicy.priority(for: .badge))
            }
        }
    }

    /// The worktree basename (D26), suppressed when it just restates the
    /// branch (R2-C).
    static func worktreeBasename(_ w: Workspace) -> String? {
        guard let raw = w.worktreePath?.split(separator: "/").last else {
            return nil
        }
        let basename = String(raw)
        guard let branch = w.branch else { return basename }
        let flattened = branch.replacingOccurrences(of: "/", with: "-")
        if basename == branch || basename == flattened
            || flattened.hasPrefix(basename) {
            return nil
        }
        return basename
    }
}

// MARK: - Pinned section header (R2-A)

/// The backing every pinned `.plain`-list section header gets. `.bar` is a
/// translucent Material, deliberately: it stays legible over cards while
/// keeping the scroll context visible. `listRowInsets` is zeroed so the
/// backing spans the full row.
struct PinnedHeader<Content: View>: View {
    let fillsInteractiveWidth: Bool
    @ViewBuilder var content: () -> Content

    init(fillsInteractiveWidth: Bool = false,
         @ViewBuilder content: @escaping () -> Content) {
        self.fillsInteractiveWidth = fillsInteractiveWidth
        self.content = content
    }

    @ViewBuilder
    var body: some View {
        if fillsInteractiveWidth {
            content()
                .font(.subheadline.weight(.semibold))
                .frame(maxWidth: .infinity, alignment: .leading)
                .background(.bar, ignoresSafeAreaEdges: [])
                .listRowInsets(EdgeInsets())
        } else {
            content()
                .font(.subheadline.weight(.semibold))
                .padding(.horizontal, 20)
                .padding(.vertical, 3)
                .frame(maxWidth: .infinity, alignment: .leading)
                .background(.bar, ignoresSafeAreaEdges: [])
                .listRowInsets(EdgeInsets())
        }
    }
}

// MARK: - Fleet board (home)

/// Identifiable carrier for the recents bottom sheet target.
private struct RecentsTarget: Identifiable {
    let agentId: String
    var id: String { agentId }
}

struct FleetView: View {
    @ObservedObject var model: AppModel
    @State private var showSettings = false
    @State private var recentsTarget: RecentsTarget?

    var body: some View {
        let agents = Array(model.fleet.agents.values)
        let sections = BoardModel.sections(agents)
        return List {
            // Issue #219: the board chrome is the FIRST section of the same
            // physical scroll surface (a pinned header) instead of a
            // `.safeAreaInset` outside the list. During the pull gesture
            // the chrome, section headers, and rows translate as one unit.
            if model.mode != .needsSetup {
                Section {
                    if model.fleet.agents.isEmpty {
                        Color.clear
                            .frame(height: 0)
                            .listRowInsets(EdgeInsets())
                            .listRowBackground(Color.clear)
                            .listRowSeparator(.hidden)
                    }
                } header: {
                    PinnedHeader(fillsInteractiveWidth: true) {
                        boardChrome
                    }
                }
            }
            if let banner = model.banner {
                BannerView(banner: banner) {
                    model.banner = nil
                }
            }
            switch model.mode {
            case .needsSetup:
                RegistrationView(model: model)
#if DEBUG
            case .demo:
                boardSections(sections: sections)
#endif
            case .live:
                boardSections(sections: sections)
            }
        }
        .listStyle(.plain)
        .listSectionSpacing(.compact)
        // Issue #219: native pull-to-refresh on the one physical scroll
        // surface. `refreshFleet` is coalesced and never touches the SSE
        // stream task.
        .refreshable {
            await model.refreshFleet()
        }
        .navigationTitle("Fleet")
        .toolbar {
            ToolbarItem(placement: .topBarTrailing) {
                Menu {
#if DEBUG
                    Button(model.mode == .demo ? "Exit demo" : "Demo mode",
                           systemImage: "sparkles") {
                        if model.mode == .demo {
                            model.exitDemo()
                        } else {
                            model.enterDemo()
                        }
                    }
#endif
                    Button("Settings", systemImage: "gearshape") {
                        showSettings = true
                    }
                } label: {
                    Image(systemName: "slider.horizontal.3")
                }
            }
        }
        .sheet(isPresented: $showSettings) {
            SettingsView(model: model)
        }
        // Recents bottom sheet: row taps AND notification deep links land
        // here through the same model-owned target.
        .sheet(item: $recentsTarget) { target in
            RecentOutputSheet(agentId: target.agentId, model: model)
        }
        .onChange(of: model.recentsAgentId) { _, agentId in
            guard let agentId, recentsTarget?.agentId != agentId else { return }
            recentsTarget = RecentsTarget(agentId: agentId)
        }
#if DEBUG
        .onChange(of: model.mode) { _, _ in
            applyDemoRouteIfNeeded()
        }
        .task {
            applyDemoRouteIfNeeded()
        }
#endif
    }

    /// The #354 L2 board: repo groups with raw status chips; blocked agents
    /// pinned on top (a promotion — the same agents also sit first in their
    /// repo section); every agent of a repo listed in attention order so an
    /// idle (finished-fallback) agent stays until replaced.
    @ViewBuilder
    private func boardSections(sections: BoardModel.Sections) -> some View {
        if !sections.blocked.isEmpty {
            Section {
                ForEach(sections.blocked) { agent in
                    agentRow(agent)
                }
            } header: {
                PinnedHeader {
                    Text("blocked (\(sections.blocked.count))")
                        .accessibilityLabel("blocked (\(sections.blocked.count))")
                }
            }
        }
        ForEach(sections.repos) { repo in
            Section {
                ForEach(repo.agents) { agent in
                    agentRow(agent)
                }
            } header: {
                PinnedHeader {
                    Text(repo.header)
                        .accessibilityLabel(repo.header)
                }
            }
        }
    }

    @ViewBuilder
    private func agentRow(_ agent: Agent) -> some View {
        Button {
            model.recentsAgentId = agent.agentId
        } label: {
            AgentRow(agent: agent,
                     stateEnteredAt: model.fleet.stateEnteredAt[agent.agentId])
        }
        .buttonStyle(.plain)
        .accessibilityElement(children: .combine)
        .accessibilityLabel(rowSummary(agent))
        .accessibilityHint("Double tap to open recent output")
    }

    /// Board chrome: connection status (live) + the pull-to-refresh hint.
    /// No search field, no filter chips, no manual refresh button.
    @ViewBuilder
    private var boardChrome: some View {
        VStack(alignment: .leading, spacing: 0) {
            if model.mode == .live {
                connectionStatusLine
            }
            HStack(spacing: 4) {
                Image(systemName: "arrow.down")
                    .font(.caption2.weight(.semibold))
                    .foregroundStyle(Color.accentColor)
                    .accessibilityHidden(true)
                Text("pull to refresh · updates stream in automatically")
                    .font(.caption2)
                    .foregroundStyle(.secondary)
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            .padding(.horizontal, 20)
            .padding(.top, 2)
            .padding(.bottom, 3)
            .accessibilityElement(children: .combine)
            .accessibilityLabel("Pull to refresh. Updates stream in automatically.")
        }
    }

    /// Connection indicator line, modeled by `BoardModel.connectionStatus`
    /// so the label/spinner is a testable pure projection. When offline the
    /// board keeps showing the LAST-KNOWN fleet with the daemon-offline
    /// banner (spec: last-known board + offline banner).
    @ViewBuilder
    private var connectionStatusLine: some View {
        let status = BoardModel.connectionStatus(for: model.fleet.connectionState)
        switch status {
        case .connected:
            EmptyView()
        case .connecting:
            HStack(spacing: 4) {
                ProgressView().controlSize(.mini)
                Text("connecting")
                    .font(.caption2)
                    .foregroundStyle(.secondary)
            }
            .padding(.horizontal, 20)
            .padding(.top, 4)
        case .offline:
            Label("daemon offline — showing last-known board", systemImage: "wifi.slash")
                .font(.caption2)
                .foregroundStyle(.orange)
                .frame(maxWidth: .infinity, alignment: .leading)
                .padding(.horizontal, 20)
                .padding(.top, 4)
                .accessibilityLabel("daemon offline — showing last known board")
        case .error(let message):
            Text("⚠ \(message)")
                .font(.caption2)
                .foregroundStyle(.orange)
                .lineLimit(1)
                .frame(maxWidth: .infinity, alignment: .leading)
                .padding(.horizontal, 20)
                .padding(.top, 4)
        }
    }

#if DEBUG
    /// Deterministic evidence route: `-corralDemoDetail` opens the recents
    /// sheet for the featured demo agent right after seeding (simctl cannot
    /// inject the tap).
    private func applyDemoRouteIfNeeded() {
        guard model.mode == .demo,
              let agentId = model.demoDetailAgentId,
              model.fleet.agent(agentId) != nil,
              recentsTarget?.agentId != agentId else { return }
        Task { @MainActor in
            try? await Task.sleep(for: .milliseconds(500))
            guard model.mode == .demo,
                  model.fleet.agent(agentId) != nil else { return }
            model.recentsAgentId = agentId
        }
    }
#endif
}

// MARK: - Banner

struct BannerView: View {
    let banner: DriveBanner
    var dismiss: () -> Void

    var body: some View {
        HStack(spacing: 8) {
            Image(systemName: banner.isError ? "exclamationmark.triangle.fill" : "info.circle.fill")
                .foregroundStyle(banner.isError ? Color.red : Color.blue)
            Text("[\(banner.kind)] \(banner.message)")
                .font(.caption)
                .textSelection(.enabled)
            Spacer()
            Button { dismiss() } label: {
                Image(systemName: "xmark.circle.fill")
            }
            .buttonStyle(.plain)
            .foregroundStyle(.secondary)
        }
        .padding(8)
        .background(banner.isError ? Color.red.opacity(0.12) : Color.blue.opacity(0.12),
                    in: RoundedRectangle(cornerRadius: 8))
    }
}

// MARK: - Registration

struct RegistrationView: View {
    @ObservedObject var model: AppModel

    @State private var host = "127.0.0.1:8474"
    @State private var token = ""
    @State private var registering = false

    var body: some View {
        Section {
            TextField("Host (Tailscale host or loopback)", text: $host)
                .textFieldStyle(.roundedBorder)
                .textInputAutocapitalization(.never)
                .autocorrectionDisabled()
            SecureField("Registration token", text: $token)
                .textFieldStyle(.roundedBorder)
            Button {
                registering = true
                Task {
                    await model.register(host: host, token: token)
                    registering = false
                }
            } label: {
                if registering {
                    ProgressView().controlSize(.small)
                } else {
                    Text("Register device (read-only)")
                }
            }
            .disabled(host.isEmpty || token.isEmpty || registering)
            Text("The device signs every read with its own Ed25519 key. Registration grants NOTHING: the host provisions the read_tail grant out-of-band.")
                .font(.caption)
                .foregroundStyle(.secondary)
            if model.keyStorageWarning {
                Label("Keychain unavailable — the device key is stored in the plaintext in-app store. Use a device with Keychain support for production.",
                      systemImage: "exclamationmark.triangle.fill")
                    .font(.caption)
                    .foregroundStyle(.orange)
            }
        } header: {
            PinnedHeader { Text("Connect") }
        }
#if DEBUG
        Section {
            Button("Try demo fleet (Debug only; no daemon)") {
                model.enterDemo()
            }
            .font(.subheadline)
            Text("Seeded fake read-only fleet for local Debug/simulator testing only.")
                .font(.caption)
                .foregroundStyle(.secondary)
        } header: {
            PinnedHeader { Text("Demo") }
        }
#endif
    }
}

// MARK: - Settings (connection + notification pairing)

struct SettingsView: View {
    @ObservedObject var model: AppModel
    @Environment(\.dismiss) private var dismiss

    var body: some View {
        NavigationStack {
            Form {
                Section("Device") {
                    LabeledContent("Key id", value: String((model.keyId ?? "—").prefix(16)))
                    LabeledContent("Key storage", value: DeviceKeyStore.storageLocation == .keychain ? "Keychain" : "in-app store (⚠️ insecure)")
                    LabeledContent("Grants", value: model.grants.isEmpty ? "none (read-only)" : model.grants.joined(separator: ", "))
                    LabeledContent("Name", value: UIDevice.current.name)
                    if let host = model.hostURL {
                        LabeledContent("Host", value: host.absoluteString)
                    }
                }
                Section("Notifications") {
                    Toggle("State-change notifications",
                           isOn: Binding(
                            get: { model.notificationsEnabled },
                            set: { model.setNotificationsEnabled($0) }))
                    Text("Alerts when an agent starts, blocks, or finishes. No badges or catch-up.")
                        .font(.caption)
                        .foregroundStyle(.secondary)
                }
                Section("Reset") {
                    Button("Reset device identity", role: .destructive) {
                        model.resetDevice()
                        dismiss()
                    }
                }
            }
            .navigationTitle("Settings")
        }
    }
}

// MARK: - Recents bottom sheet (#354 L2 recents v1)

/// Read-only recents: LIVE TAIL ONLY. The sheet auto-loads the agent's
/// bounded tail (≤200 lines, daemon cap), auto-refreshes while open, and
/// auto-scrolls to the newest row. Renders ONE continuous chronological
/// rail of raw output (#361): no load-earlier paging, no partition, no
/// composer, and no divider/card/role-label chrome.
struct RecentOutputSheet: View {
    let agentId: String
    @ObservedObject var model: AppModel
    @Environment(\.dismiss) private var dismiss

    private var agent: Agent? { model.fleet.agent(agentId) }
    private var tail: TailPane? { model.fleet.tailPane(for: agentId) }
    private var driveClient: DriveClient { model.makeDriveClient() }

    var body: some View {
        NavigationStack {
            VStack(alignment: .leading, spacing: 0) {
                header
                Divider()
                content
            }
            .navigationTitle(agent?.title ?? agent?.displayName ?? agentId)
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .confirmationAction) {
                    Button("Done") { dismiss() }
                }
            }
            .task {
                refresh()
                while !Task.isCancelled {
                    try? await Task.sleep(nanoseconds: 5_000_000_000)
                    guard !Task.isCancelled else { return }
                    refresh()
                }
            }
        }
        .presentationDetents([.medium, .large])
        .presentationDragIndicator(.visible)
    }

    @ViewBuilder
    private var header: some View {
        VStack(alignment: .leading, spacing: 4) {
            if let agent {
                let style = StateStyle.style(for: agent.state)
                HStack(spacing: 6) {
                    Circle()
                        .fill(style.isRing ? Color.clear : style.color)
                        .overlay(Circle().stroke(style.color, lineWidth: 1))
                        .frame(width: 10, height: 10)
                        .accessibilityHidden(true)
                    Text(style.label)
                        .font(.caption.weight(.semibold))
                        .foregroundStyle(style.color)
                        .accessibilityLabel(style.accessibilityLabel)
                    if let repo = agent.workspace.repo {
                        Text(repo)
                            .font(.caption2.monospaced())
                            .foregroundStyle(.secondary)
                    }
                    if let branch = agent.workspace.branch {
                        Text(branch)
                            .font(.caption2.monospaced())
                            .foregroundStyle(.secondary)
                            .lineLimit(1)
                            .truncationMode(.middle)
                    }
                    Spacer()
                    if let reference = agent.attachment?.reference {
                        Text(reference)
                            .font(.caption2.monospaced())
                            .foregroundStyle(.tertiary)
                            .accessibilityLabel("Pane \(reference)")
                    }
                    if showLiveIndicator {
                        Circle()
                            .fill(RecentOutputPalette.accent)
                            .frame(width: 6, height: 6)
                            .accessibilityHidden(true)
                        Text("live")
                            .font(.caption2.weight(.bold))
                            .foregroundStyle(RecentOutputPalette.accent)
                    }
                }
            } else {
                Label("Agent no longer available", systemImage: "exclamationmark.triangle")
                    .font(.subheadline)
                    .foregroundStyle(.secondary)
            }
        }
        .padding(.horizontal, 16)
        .padding(.vertical, 10)
        .background(RecentOutputPalette.bg)
        .environment(\.colorScheme, .dark)
    }

    private var showLiveIndicator: Bool {
        RecentOutputModel.shouldShowLiveIndicator(
            isLiveMode: model.mode == .live,
            hasFreshNonErrorTail: RecentOutputModel.hasFreshNonErrorTail(tail))
    }

    @ViewBuilder
    private var content: some View {
        Group {
            switch RecentOutputModel.phase(for: tail) {
            case .loading:
                HStack(spacing: 8) {
                    ProgressView()
                        .controlSize(.small)
                        .tint(RecentOutputPalette.accent)
                    Text("Loading recent output…")
                        .font(.caption)
                        .foregroundStyle(RecentOutputPalette.muted)
                }
                .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
                .padding(16)
            case .empty:
                Text("No output yet.")
                    .font(.caption)
                    .foregroundStyle(RecentOutputPalette.muted)
                    .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
                    .padding(16)
            case .error(let failure):
                VStack(alignment: .leading, spacing: 8) {
                    Label(TranscriptText.errorText(failure), systemImage: "exclamationmark.triangle")
                        .font(.caption)
                        .foregroundStyle(RecentOutputPalette.diffDel)
                        .accessibilityLabel(TranscriptText.errorText(failure))
                    Button("Retry") {
                        refresh()
                    }
                    .buttonStyle(.bordered)
                    .tint(RecentOutputPalette.accent)
                    .accessibilityLabel("Retry recent output")
                }
                .padding(16)
                .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
            case .loaded:
                tailStream
            }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .background(RecentOutputPalette.bg)
        .environment(\.colorScheme, .dark)
    }

    /// The live tail: ONE continuous chronological rail of the agent's raw
    /// output (#361), auto-scrolled to the newest row. The row model
    /// already dropped divider-only rows and only marks semantic role
    /// transitions, so the stack renders plain rows — no divider rules, no
    /// cards, no role text.
    private var tailStream: some View {
        let rows = RecentOutputModel.railRows(from: tail)
        return ScrollViewReader { proxy in
            ScrollView(.vertical) {
                LazyVStack(alignment: .leading, spacing: 8) {
                    ForEach(rows) { row in
                        RecentRailRowView(row: row)
                    }
                    Color.clear
                        .frame(height: 1)
                        .id("recents-bottom")
                }
                .padding(.horizontal, 14)
                .padding(.vertical, 6)
            }
            .onAppear {
                scrollToBottom(proxy)
            }
            .onChange(of: rows.last?.block.text) { _, _ in
                scrollToBottom(proxy)
            }
        }
    }

    private func scrollToBottom(_ proxy: ScrollViewProxy) {
        DispatchQueue.main.async {
            withAnimation {
                proxy.scrollTo("recents-bottom", anchor: .bottom)
            }
        }
    }

    private func refresh() {
        guard let agent else { return }
#if DEBUG
        if model.mode == .demo {
            model.driveDemoReadTail(agent: agent)
            return
        }
#endif
        model.driveReadTail(agent: agent, driveClient: driveClient, silent: true)
    }
}

// MARK: - Rail row renderer (recents sheet rows)

/// One recents rail row (#361): raw output text in a continuous,
/// chrome-free stream — no divider rules, no cards, no role labels. A role
/// transition marker (circle / diamond / square in the role's locked token
/// color) appears in the left gutter ONLY on the first row of a role run;
/// continuation rows carry no marker and no label. Attributed content stays
/// attributable via the row's accessibility label.
private struct RecentRailRowView: View {
    let row: RecentOutputModel.RailRow

    private var block: TranscriptBlock { row.block }

    var body: some View {
        HStack(alignment: .top, spacing: 10) {
            markerGutter
            content
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .accessibilityElement(children: .combine)
        .accessibilityLabel(RecentOutputRender.accessibilityLabel(block))
    }

    private var markerGutter: some View {
        Group {
            if row.showsTransitionMarker {
                transitionMarker
                    .frame(width: 9, height: 9)
            } else {
                Color.clear
            }
        }
        .frame(width: 18, alignment: .topLeading)
        .padding(.top, 4)
        .accessibilityHidden(true)
    }

    @ViewBuilder
    private var transitionMarker: some View {
        // `railRows` only marks rows whose kind has a locked marker, so the
        // nil arm is unreachable in practice.
        switch RecentOutputModel.marker(for: block.kind) {
        case .circle: Circle().stroke(roleColor, lineWidth: 1.5)
        case .diamond: RecentDiamond().stroke(roleColor, lineWidth: 1.5)
        case .square: Rectangle().stroke(roleColor, lineWidth: 1.5)
        case nil: Color.clear
        }
    }

    private var roleColor: Color {
        switch block.kind {
        case .user: return RecentOutputPalette.workingBlue
        case .agent: return RecentOutputPalette.accent
        case .tool: return RecentOutputPalette.toolGold
        case .system, .unknown: return RecentOutputPalette.muted
        }
    }

    /// The block's raw output. Code/diff lines keep their inline syntax
    /// colors and numbered gutters; the card container is gone (#361).
    private var content: some View {
        Group {
            if rendersHighlighted {
                codeContent
            } else {
                proseContent
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .textSelection(.enabled)
    }

    private var rendersHighlighted: Bool {
        RecentOutputRender.codeLines(for: block).contains { $0.isHighlighted }
    }

    private var codeContent: some View {
        VStack(alignment: .leading, spacing: 2) {
            ForEach(Array(RecentOutputRender.codeLines(for: block).enumerated()),
                    id: \.offset) { item in
                RecentCodeLineView(line: item.element)
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }

    private var proseContent: some View {
        VStack(alignment: .leading, spacing: 3) {
            ForEach(Array(RecentOutputRender.messageLines(block.text).enumerated()),
                    id: \.offset) { item in
                Text(item.element)
                    .font(.subheadline)
                    .foregroundStyle(RecentOutputPalette.ink)
                    .frame(maxWidth: .infinity, alignment: .leading)
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }
}

/// Diamond outline: the You transition marker (#361). Drawn as a path so
/// the stroke stays uniform.
private struct RecentDiamond: Shape {
    func path(in rect: CGRect) -> Path {
        var path = Path()
        path.move(to: CGPoint(x: rect.midX, y: rect.minY))
        path.addLine(to: CGPoint(x: rect.maxX, y: rect.midY))
        path.addLine(to: CGPoint(x: rect.midX, y: rect.maxY))
        path.addLine(to: CGPoint(x: rect.minX, y: rect.midY))
        path.closeSubpath()
        return path
    }
}

private struct RecentCodeLineView: View {
    let line: RecentCodeLine

    var body: some View {
        HStack(alignment: .top, spacing: 8) {
            if let number = line.number {
                Text("\(number)")
                    .font(.caption2.monospaced())
                    .foregroundStyle(RecentOutputPalette.muted)
                    .frame(width: 24, alignment: .trailing)
                    .accessibilityHidden(true)
            }
            highlightedText
                .font(.caption2.monospaced())
                .foregroundStyle(RecentOutputPalette.ink)
                .fixedSize(horizontal: false, vertical: true)
        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }

    private var highlightedText: Text {
        line.segments.reduce(Text("")) { partial, segment in
            partial + Text(segment.text)
                .foregroundStyle(color(for: segment.kind))
        }
    }

    private func color(for kind: RecentCodeSegmentKind) -> Color {
        switch kind {
        case .plain: return RecentOutputPalette.ink
        case .keyword: return RecentOutputPalette.keyword
        case .string: return RecentOutputPalette.string
        case .addition: return RecentOutputPalette.diffAdd
        case .deletion: return RecentOutputPalette.diffDel
        case .comment: return RecentOutputPalette.comment
        }
    }
}

// MARK: - Recent-output palette (dark, prototype-locked)

enum RecentOutputPalette {
    static let panelCornerRadius: CGFloat = 8
    static let bg = Color(red: 13 / 255, green: 17 / 255, blue: 23 / 255)
    static let ink = Color(red: 230 / 255, green: 237 / 255, blue: 243 / 255)
    static let muted = Color(red: 139 / 255, green: 148 / 255, blue: 158 / 255)
    static let accent = Color(red: 45 / 255, green: 212 / 255, blue: 191 / 255)
    // #361 DESIGN LOCK role tokens (the #316 evidence palette): Assistant =
    // accent, You = working blue, Tool = gold. Used ONLY by the transition
    // markers — never repeated per row and never as role text.
    static let workingBlue = Color(red: 88 / 255, green: 166 / 255, blue: 255 / 255)
    static let toolGold = Color(red: 210 / 255, green: 153 / 255, blue: 34 / 255)
    static let diffAdd = Color(red: 63 / 255, green: 185 / 255, blue: 80 / 255)
    static let diffDel = Color(red: 248 / 255, green: 81 / 255, blue: 73 / 255)
    static let string = Color(red: 165 / 255, green: 214 / 255, blue: 255 / 255)
    static let keyword = Color(red: 255 / 255, green: 123 / 255, blue: 114 / 255)
    static let comment = Color(red: 139 / 255, green: 148 / 255, blue: 158 / 255)
}
