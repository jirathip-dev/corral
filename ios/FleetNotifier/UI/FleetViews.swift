import SwiftUI

// MARK: - Kind badges (distinct: ApproveTool / AnswerQuestion / Menu / Crash)

enum KindBadgeStyle {
    static func color(_ kind: WaitingOnKind) -> Color {
        switch kind {
        case .approveTool: return .blue
        case .answerQuestion: return .purple
        case .menu: return .orange
        case .crash: return .red
        }
    }

    static func label(_ kind: WaitingOnKind) -> String {
        switch kind {
        case .approveTool: return "Approve tool"
        case .answerQuestion: return "Question"
        case .menu: return "Menu"
        case .crash: return "Crash"
        }
    }
}

struct KindBadge: View {
    let kind: WaitingOnKind

    var body: some View {
        Text(KindBadgeStyle.label(kind).uppercased())
            .font(.caption2.weight(.bold))
            .foregroundStyle(.white)
            .padding(.horizontal, 6)
            .padding(.vertical, 2)
            .background(KindBadgeStyle.color(kind), in: RoundedRectangle(cornerRadius: 4))
    }
}

// MARK: - Blocked claim card

struct ClaimCard: View {
    let agent: Agent
    let waiting: WaitingOn
    let grants: Set<Capability>
    var onChoice: (String) -> Void
    var onCanned: (CannedChoice.Action) -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack {
                KindBadge(kind: waiting.kind)
                Spacer()
                if waiting.kind != .crash {
                    Text("approval_id \(shortApprovalId)")
                        .font(.caption2.monospaced())
                        .foregroundStyle(.secondary)
                }
            }
            Text(waiting.prompt)
                .font(.body)
                .textSelection(.enabled)
            if waiting.kind != .crash && grants.contains(.approve) {
                if !waiting.choices.isEmpty {
                    // Menu / approve-tool with known choices: exact buttons.
                    FlowLayout(spacing: 8) {
                        ForEach(waiting.choices, id: \.self) { choice in
                            Button(choice) { onChoice(choice) }
                                .buttonStyle(.borderedProminent)
                                .tint(KindBadgeStyle.color(waiting.kind))
                        }
                    }
                } else {
                    // Free-form answer (AnswerQuestion / empty menu).
                    CannedButtons(waiting: waiting, onCanned: onCanned)
                }
            } else if waiting.kind != .crash && !grants.contains(.approve) {
                Text("The device has no `approve` grant — ask the host to promote capabilities.")
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }
        }
        .padding(10)
        .background(KindBadgeStyle.color(waiting.kind).opacity(0.12),
                    in: RoundedRectangle(cornerRadius: 8))
        .overlay(
            RoundedRectangle(cornerRadius: 8)
                .stroke(KindBadgeStyle.color(waiting.kind).opacity(0.5), lineWidth: 1)
        )
    }

    private var shortApprovalId: String {
        let id = waiting.approvalId ?? Claim.approvalId(agentId: agent.agentId, promptHash: waiting.promptHash)
        return String(id.prefix(24)) + "…"
    }
}

struct CannedButtons: View {
    let waiting: WaitingOn
    var onCanned: (CannedChoice.Action) -> Void

    var body: some View {
        HStack(spacing: 8) {
            ForEach(CannedChoice.Action.allCases, id: \.self) { action in
                if CannedChoice.choice(for: action, kind: waiting.kind, choices: waiting.choices) != nil {
                    Button(CannedChoice.title(for: action)) {
                        onCanned(action)
                    }
                    .buttonStyle(.bordered)
                }
            }
        }
    }
}

/// Simple flow layout for choice buttons.
struct FlowLayout: Layout {
    var spacing: CGFloat

    func sizeThatFits(proposal: ProposedViewSize, subviews: Subviews, cache: inout ()) -> CGSize {
        let maxWidth = proposal.width ?? .infinity
        var x: CGFloat = 0, y: CGFloat = 0, rowHeight: CGFloat = 0
        for subview in subviews {
            let size = subview.sizeThatFits(.unspecified)
            if x + size.width > maxWidth, x > 0 {
                x = 0
                y += rowHeight + spacing
                rowHeight = 0
            }
            x += size.width + spacing
            rowHeight = max(rowHeight, size.height)
        }
        return CGSize(width: maxWidth == .infinity ? x : maxWidth, height: y + rowHeight)
    }

    func placeSubviews(in bounds: CGRect, proposal: ProposedViewSize, subviews: Subviews, cache: inout ()) {
        var x = bounds.minX, y = bounds.minY, rowHeight: CGFloat = 0
        for subview in subviews {
            let size = subview.sizeThatFits(.unspecified)
            if x + size.width > bounds.maxX, x > bounds.minX {
                x = bounds.minX
                y += rowHeight + spacing
                rowHeight = 0
            }
            subview.place(at: CGPoint(x: x, y: y), proposal: ProposedViewSize(size))
            x += size.width + spacing
            rowHeight = max(rowHeight, size.height)
        }
    }
}

// MARK: - Agent row (D24 3-line dense anatomy)

/// One board row, D24 shape:
///
/// - line 1 — state dot + title (falling back to display name / id) + issue
///   chip (`⑂ #N` authoritative / `~#N` inferred, display-only per D21) +
///   CI glyph + tool badge;
/// - line 2 — repo·branch·worktree basename (D26: no nesting level) with
///   PR / dirty / `↑a↓b` trailing;
/// - line 3 — the waiting-on claim card when blocked; hidden otherwise
///   until the daemon grows `activity` (D22, out of this slice).
///
/// Idle/done rows dim and lose line 3 (D28). Row actions are limited to
/// Approve/Deny (in the claim card), Prompt, and Tail 200 — all grant-gated
/// (D30); Interrupt/Kill are deliberately NOT row actions (D29/D30).
struct AgentRow: View {
    let agent: Agent
    let grants: Set<Capability>
    var onChoice: (String) -> Void
    var onCanned: (CannedChoice.Action) -> Void
    var onReadTail: () -> Void
    var onPrompt: (String) -> Void

    @State private var promptText = ""

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack(spacing: 6) {
                Circle()
                    .fill(stateColor)
                    .frame(width: 10, height: 10)
                Text(agent.title ?? agent.displayName ?? agent.agentId)
                    .font(.subheadline.weight(.semibold))
                    .lineLimit(1)
                Spacer()
                if let chip = IssueChip.chip(for: agent) {
                    Text(chip.label)
                        .font(.caption2.monospaced())
                        .foregroundStyle(chip.isFlagged ? Color.secondary : Color.accentColor)
                }
                if let ci = agent.workspace.ciStatus {
                    CiGlyph(status: ci)
                }
                Text(agent.tool)
                    .font(.caption2.monospaced())
                    .padding(.horizontal, 6)
                    .padding(.vertical, 2)
                    .background(.quaternary, in: RoundedRectangle(cornerRadius: 4))
            }
            WorkspaceLine(agent: agent)
            if let waiting = agent.waitingOn, agent.isBlocked {
                ClaimCard(agent: agent, waiting: waiting, grants: grants,
                          onChoice: onChoice, onCanned: onCanned)
            }
            if !isDimmed || hasRowActions {
                HStack(spacing: 8) {
                    if agent.capabilities.contains(Capability.readTail.rawValue), grants.contains(.readTail) {
                        Button("Tail 200") { onReadTail() }
                            .buttonStyle(.bordered)
                            .font(.caption)
                    }
                    Spacer()
                }
            }
            if grants.contains(.prompt) && agent.capabilities.contains(Capability.prompt.rawValue) {
                HStack {
                    TextField("Send a prompt…", text: $promptText)
                        .textFieldStyle(.roundedBorder)
                        .font(.caption)
                    Button("Send") {
                        let text = promptText
                        promptText = ""
                        onPrompt(text)
                    }
                    .disabled(promptText.isEmpty)
                    .buttonStyle(.borderedProminent)
                    .font(.caption)
                }
            }
        }
        .padding(.vertical, 4)
        .opacity(isDimmed ? 0.55 : 1)
    }

    /// D28: idle/done rows dim (and have no line 3 by construction).
    private var isDimmed: Bool {
        agent.state == .idle || agent.state == .done
    }

    private var hasRowActions: Bool {
        agent.capabilities.contains(Capability.readTail.rawValue) && grants.contains(.readTail)
    }

    private var stateColor: Color {
        switch agent.state {
        case .blocked: return .red
        case .working: return .green
        case .idle: return .secondary
        case .done: return .blue
        case .unknown: return .gray
        }
    }
}

/// Line 2 (D26): repo·branch·worktree basename — no nesting level — with
/// PR / dirty / one `↑a↓b` badge trailing (D29: not separate columns).
struct WorkspaceLine: View {
    let agent: Agent

    var body: some View {
        let w = agent.workspace
        HStack(spacing: 6) {
            Text(pathLine(w))
                .font(.caption2.monospaced())
                .foregroundStyle(.secondary)
                .lineLimit(1)
                .truncationMode(.middle)
            Spacer(minLength: 4)
            if let pr = w.prNumber {
                Text("#\(pr)")
                    .font(.caption2)
                    .foregroundStyle(.secondary)
            }
            if w.dirty {
                Text("dirty").font(.caption2.weight(.semibold))
                    .foregroundStyle(.orange)
            }
            if w.ahead > 0 || w.behind > 0 {
                Text("↑\(w.ahead)↓\(w.behind)")
                    .font(.caption2.monospaced())
                    .foregroundStyle(.secondary)
            }
        }
    }

    private func pathLine(_ w: Workspace) -> String {
        var parts: [String] = []
        if let repo = w.repo { parts.append(repo) }
        if let branch = w.branch { parts.append(branch) }
        if let basename = w.worktreePath.flatMap({ $0.split(separator: "/").last }),
           String(basename) != w.branch {
            parts.append(String(basename))
        }
        return parts.isEmpty ? "—" : parts.joined(separator: " · ")
    }
}

/// Line-1 CI glyph (symbol only — the dense row has no room for a label).
struct CiGlyph: View {
    let status: CiStatus

    var body: some View {
        let (color, symbol) = switch status {
        case .success: (Color.green, "checkmark.circle.fill")
        case .failure: (Color.red, "xmark.circle.fill")
        case .pending: (Color.orange, "clock.fill")
        case .unknown: (Color.gray, "questionmark.circle")
        }
        Image(systemName: symbol)
            .font(.caption)
            .foregroundStyle(color)
            .accessibilityLabel("CI \(status.rawValue)")
    }
}

// MARK: - Fleet list

struct FleetView: View {
    @ObservedObject var model: AppModel

    var body: some View {
        NavigationStack {
            List {
                if let banner = model.banner {
                    BannerView(banner: banner) {
                        model.banner = nil
                    }
                }
                switch model.mode {
                case .needsSetup:
                    RegistrationView(model: model)
                case .demo, .live:
                    fleetList
                }
            }
            .navigationTitle("Fleet")
            .toolbar {
                ToolbarItem(placement: .topBarTrailing) {
                    Menu {
                        Button(model.mode == .demo ? "Exit demo" : "Demo mode",
                               systemImage: "sparkles") {
                            if model.mode == .demo {
                                restartLive()
                            } else {
                                model.enterDemo()
                            }
                        }
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
        }
    }

    @State private var showSettings = false
    @State private var showIdleDone = false

    /// D25 hierarchy: sticky cross-repo NEEDS YOU (always expanded — a
    /// promotion, not a filter: the same agents also appear in their repo
    /// section) → repo sections with counts → orphan bucket → collapsed
    /// IDLE/DONE.
    @ViewBuilder
    private var fleetList: some View {
        let sections = BoardModel.sections(Array(model.fleet.agents.values))
        Section {
            if sections.needsYou.isEmpty {
                Text("No blocked agents")
                    .foregroundStyle(.secondary)
            }
            ForEach(sections.needsYou) { agent in
                agentRow(agent)
            }
        } header: {
            HStack {
                Text("Needs you (\(sections.needsYou.count))")
                Spacer()
                if model.fleet.connectionState != .connected, model.mode == .live {
                    connectionLabel
                }
            }
        }
        ForEach(sections.repos, id: \.repo) { group in
            Section("\(group.repo ?? "(no repo)") (\(group.agents.count))") {
                ForEach(group.agents) { agent in
                    agentRow(agent)
                }
            }
        }
        Section {
            if showIdleDone {
                ForEach(sections.idleDone) { agent in
                    agentRow(agent)
                }
            }
        } header: {
            Button {
                withAnimation { showIdleDone.toggle() }
            } label: {
                HStack {
                    Text("Idle / done (\(sections.idleDone.count))")
                    Spacer()
                    Image(systemName: showIdleDone ? "chevron.down" : "chevron.right")
                        .font(.caption2)
                }
            }
            .buttonStyle(.plain)
        }
    }

    @ViewBuilder
    private var connectionLabel: some View {
        switch model.fleet.connectionState {
        case .connected: EmptyView()
        case .connecting: ProgressView().controlSize(.mini)
        case .disconnected: Text("offline").font(.caption2).foregroundStyle(.secondary)
        case .error(let message):
            Text("⚠ \(message)").font(.caption2).foregroundStyle(.orange).lineLimit(1)
        }
    }

    private func agentRow(_ agent: Agent) -> some View {
        let driveClient = DriveClient(host: model.hostURL ?? URL(string: "http://127.0.0.1:8474")!)
        return AgentRow(agent: agent,
                        grants: Set(model.grants.compactMap(Capability.init(rawValue:))),
                        onChoice: { choice in
                            if model.mode == .demo {
                                model.driveDemo(capability: .approve, agent: agent, choice: choice)
                            } else {
                                model.driveApprove(agent: agent, choice: choice, driveClient: driveClient)
                            }
                        },
                        onCanned: { action in
                            if model.mode == .demo {
                                let waiting = agent.waitingOn
                                let choice = waiting.flatMap { CannedChoice.choice(for: action, kind: $0.kind, choices: $0.choices) }
                                model.driveDemo(capability: .approve, agent: agent, choice: choice ?? "y")
                            } else {
                                model.handleCannedAction(agentId: agent.agentId, action: action, driveClient: driveClient)
                            }
                        },
                        onReadTail: {
                            if model.mode == .demo {
                                model.driveDemo(capability: .readTail, agent: agent)
                            } else {
                                model.driveReadTail(agent: agent, driveClient: driveClient)
                            }
                        },
                        onPrompt: { text in
                            model.drivePrompt(agent: agent, text: text, driveClient: driveClient)
                        })
    }

    private func restartLive() {
        model.mode = .live
        model.startLive()
    }
}

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
        Section("Connect") {
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
            Text("The device signs every write with its own Ed25519 key. Registration grants NOTHING: read-only until the host promotes capabilities (D13).")
                .font(.caption)
                .foregroundStyle(.secondary)
            if model.keyStorageWarning {
                Label("Keychain unavailable — the device key is stored in the plaintext in-app store. Use a device with Keychain support for production.",
                      systemImage: "exclamationmark.triangle.fill")
                    .font(.caption)
                    .foregroundStyle(.orange)
            }
        }
        Section("Demo") {
            Button("Try demo fleet (no daemon)") {
                model.enterDemo()
            }
            .font(.subheadline)
            Text("Seeded fake fleet for App Review 4.2 (minimal functionality).")
                .font(.caption)
                .foregroundStyle(.secondary)
        }
    }
}

// MARK: - Settings

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
                    if let host = model.hostURL {
                        LabeledContent("Host", value: host.absoluteString)
                    }
                }
                Section("Security") {
                    Text("Writes are Ed25519-signed by the device key. Destructive payloads require Face ID step-up (X-Step-Up-Token).")
                        .font(.caption)
                        .foregroundStyle(.secondary)
                }
                Section("Danger zone") {
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
