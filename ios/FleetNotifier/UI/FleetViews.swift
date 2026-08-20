import SwiftUI
import Combine

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

// MARK: - Prompt drafts (R2-B: keystrokes must not invalidate the board)

/// Per-agent prompt drafts, keyed by agent id so the NEEDS YOU row and the
/// repo-section row of the same blocked agent share one draft (R2-B).
///
/// Ownership vs observation: `FleetView` holds this object in `@State`
/// (stable identity, NO subscription — `@State` ignores `ObservableObject`
/// conformance), and each `AgentRow` observes it via `@ObservedObject`. So
/// a keystroke re-renders the rows — cheap bodies — while
/// `FleetView.body`, and with it `BoardModel.sections`, re-runs only on
/// fleet snapshot/delta changes.
@MainActor
final class PromptDrafts: ObservableObject {
    @Published private(set) var drafts: [String: String] = [:]

    /// A two-way binding into the shared draft for `agentId`.
    func binding(for agentId: String) -> Binding<String> {
        Binding(get: { [weak self] in self?.drafts[agentId] ?? "" },
                set: { [weak self] in self?.drafts[agentId] = $0 })
    }

    /// Send cleared the shared draft for both rows of the agent.
    func clear(_ agentId: String) {
        drafts[agentId] = nil
    }

    /// R2-F: drop drafts for agents that left the snapshot — one copy and
    /// one publish regardless of how many drafts are dropped.
    func prune(to agentIds: Set<String>) {
        let kept = drafts.filter { agentIds.contains($0.key) }
        if kept.count != drafts.count {
            drafts = kept
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
    /// Last successful bounded read_tail result, shown below the action.
    let tail: [String]?
    let grants: Set<Capability>
    /// The shared draft store (R2-B): the ROW observes it, so keystrokes
    /// re-render rows without touching `FleetView.body`. Rows of the same
    /// agent share one draft (a blocked agent appears in both NEEDS YOU
    /// and its repo section).
    @ObservedObject var drafts: PromptDrafts
    /// The row actions to render (D30 pin) — computed by the pure
    /// `BoardModel.rowActions(agent:grants:)`; this view renders ONLY this
    /// list, never its own capability/grants checks.
    let actions: [RowAction]
    var onChoice: (String) -> Void
    var onCanned: (CannedChoice.Action) -> Void
    var onReadTail: () -> Void
    var onPrompt: (String) -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack(spacing: 6) {
                Circle()
                    .fill(stateColor)
                    .frame(width: 10, height: 10)
                Text(agent.title ?? agent.displayName ?? agent.agentId)
                    .font(.subheadline.weight(.semibold))
                    .lineLimit(1)
                    // Line-1 emphasis (R2-E): the task TITLE wins under
                    // compression, so it gets the higher layoutPriority;
                    // the secondary identity truncates first.
                    .layoutPriority(1)
                // Session identity must survive on the row even when a
                // title is shown — two agents can share a title. Falls back
                // to the agent id when the session has no display name
                // (R2-D).
                if agent.title != nil {
                    Text(agent.displayName ?? agent.agentId)
                        .font(.caption2.monospaced())
                        .foregroundStyle(.secondary)
                        .lineLimit(1)
                }
                Spacer()
                ForEach(IssueChip.chips(for: agent), id: \.label) { chip in
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
            if actions.contains(.approveDeny), let waiting = agent.waitingOn {
                ClaimCard(agent: agent, waiting: waiting, grants: grants,
                          onChoice: onChoice, onCanned: onCanned)
            }
            if actions.contains(.tail) {
                HStack(spacing: 8) {
                    Button("Tail 200") { onReadTail() }
                        .buttonStyle(.bordered)
                        .font(.caption)
                    Spacer()
                }
            }
            if let tail {
                VStack(alignment: .leading, spacing: 4) {
                    Text("Latest tail")
                        .font(.caption.weight(.semibold))
                        .foregroundStyle(.secondary)
                    if tail.isEmpty {
                        Text("No output returned")
                            .font(.caption.monospaced())
                            .foregroundStyle(.secondary)
                    } else {
                        ScrollView([.vertical, .horizontal]) {
                            Text(tail.joined(separator: "\n"))
                                .font(.caption.monospaced())
                                .textSelection(.enabled)
                                .frame(maxWidth: .infinity, alignment: .leading)
                                .accessibilityLabel("Latest bounded agent tail")
                        }
                        .frame(maxHeight: 180)
                        .padding(6)
                        .background(.quaternary.opacity(0.35), in: RoundedRectangle(cornerRadius: 5))
                    }
                }
                .accessibilityElement(children: .contain)
            }
            if actions.contains(.prompt) {
                HStack {
                    TextField("Send a prompt…", text: drafts.binding(for: agent.agentId))
                        .textFieldStyle(.roundedBorder)
                        .font(.caption)
                    Button("Send") {
                        let text = drafts.drafts[agent.agentId] ?? ""
                        drafts.clear(agent.agentId)
                        onPrompt(text)
                    }
                    .disabled((drafts.drafts[agent.agentId] ?? "").isEmpty)
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
///
/// Each segment is its own `Text` so truncation is per-segment: the
/// identity segments (branch, worktree basename) middle-truncate within
/// their own bounds (the egui board's lesson: never middle-truncate the
/// joined line, it eats the branch — the most identifying token on the
/// row), and the basename sits in the top priority tier so a long
/// worktree name keeps head AND tail instead of collapsing to a bare
/// `…` stub (G100).
struct WorkspaceLine: View {
    let agent: Agent

    /// Per-segment truncation + compression policy (G100). Pinned here so
    /// the "who compresses first, and how" contract is unit-testable
    /// without a rendering harness (see WorkspaceLineTests). Tiers:
    /// basename + badges (2) take their ideal width first and never
    /// compress in a realistic row; repo + branch (0) share the remainder,
    /// with the branch — middle-truncating by design — absorbing the
    /// compression.
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

        /// `.middle` keeps head+tail of the identity segments. Repo and
        /// badges keep SwiftUI's default `.tail`; the view doesn't call
        /// this for them (no truncation mode modifier there) — the cases
        /// are pinned for completeness.
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
    /// branch: herdr derives worktree dirs from branch names with `/`
    /// flattened to `-`, so `g57/board-d24-d25` → `g57-board-d24-d25`
    /// carries no extra information and only forces truncation. R2-C: only
    /// equality-after-flattening (or a basename that is merely a PREFIX of
    /// the flattened branch) is suppressed — a basename that EXTENDS the
    /// branch adds tokens and must be kept.
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

// MARK: - Pinned section header (R2-A)

/// The backing every pinned `.plain`-list section header in this List gets
/// (board sections AND the registration sections share it — a pinned header
/// with no backing overlaps scrolling content). `.bar` is a translucent
/// Material, deliberately: it stays legible over cards while keeping the
/// scroll context visible; it is NOT fully opaque. `listRowInsets` is
/// zeroed so the backing spans the full row, and the horizontal padding
/// matches the default row content inset so header text aligns with row
/// content instead of sitting one notch deeper (round-1 finding F4).
struct PinnedHeader<Content: View>: View {
    @ViewBuilder var content: () -> Content

    var body: some View {
        content()
            .font(.subheadline.weight(.semibold))
            .padding(.horizontal, 20)
            .padding(.vertical, 6)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(.bar, ignoresSafeAreaEdges: [])
            .listRowInsets(EdgeInsets())
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
            // D25's "sticky NEEDS YOU": only the plain list style pins
            // section headers while scrolling (inset-grouped does not).
            .listStyle(.plain)
            .navigationTitle("Fleet")
            // R2-F: drop drafts for agents that left the snapshot. This
            // body (and the Set below) re-evaluates on fleet
            // snapshot/delta changes — the exact moments a prune can
            // matter — and not on keystrokes (see promptDrafts above).
            .onChange(of: Set(model.fleet.agents.keys)) { _, agentIds in
                promptDrafts.prune(to: agentIds)
            }
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
    /// Per-agent prompt drafts (R2-B). Held in `@State`, DELIBERATELY not
    /// `@StateObject`: `@State` keeps the object's identity across renders
    /// but does not subscribe to `objectWillChange`, so keystrokes do not
    /// re-run this body. The rows observe the object (`@ObservedObject`)
    /// and re-render themselves.
    @State private var promptDrafts = PromptDrafts()

    /// D25 hierarchy: sticky cross-repo NEEDS YOU (always expanded — a
    /// promotion, not a filter: the same agents also appear in their repo
    /// section) → repo sections with counts → orphan bucket → collapsed
    /// IDLE/DONE. Section headers pin while scrolling via the `.plain`
    /// list style set on the List (inset-grouped headers do not pin).
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
            pinnedHeader {
                HStack {
                    Text("Needs you (\(sections.needsYou.count))")
                    Spacer()
                    if model.fleet.connectionState != .connected, model.mode == .live {
                        connectionLabel
                    }
                }
            }
        }
        ForEach(sections.repos, id: \.repo) { group in
            Section {
                ForEach(group.agents) { agent in
                    agentRow(agent)
                }
            } header: {
                // R2-A: pinned headers need an opaque backing or the header
                // text overlaps scrolling rows at every section boundary.
                pinnedHeader {
                    Text("\(group.repo ?? "(no repo)") (\(group.countLabel))")
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
            pinnedHeader {
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
    }

    @ViewBuilder
    private func pinnedHeader<Content: View>(@ViewBuilder content: @escaping () -> Content) -> some View {
        PinnedHeader(content: content)
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
        let grants = Set(model.grants.compactMap(Capability.init(rawValue:)))
        return AgentRow(agent: agent,
                        tail: model.fleet.tail(for: agent.agentId),
                        grants: grants,
                        drafts: promptDrafts,
                        actions: BoardModel.rowActions(agent: agent, grants: grants),
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
                            // Send clears the SHARED draft, so both rows of
                            // the agent (NEEDS YOU + repo section) reset.
                            promptDrafts.clear(agent.agentId)
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
            Text("The device signs every write with its own Ed25519 key. Registration grants NOTHING: read-only until the host promotes capabilities (D13).")
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
        Section {
            Button("Try demo fleet (no daemon)") {
                model.enterDemo()
            }
            .font(.subheadline)
            Text("Seeded fake fleet for App Review 4.2 (minimal functionality).")
                .font(.caption)
                .foregroundStyle(.secondary)
        } header: {
            PinnedHeader { Text("Demo") }
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
