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
    let approvalEnabled: Bool
    let approvalDisabledReason: String?
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
            if waiting.kind != .crash && approvalEnabled {
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
            } else if let approvalDisabledReason {
                Text(approvalDisabledReason)
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
        .accessibilityElement(children: .contain)
        .accessibilityLabel("Blocked approval claim for \(agent.displayName ?? agent.agentId)")
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

// MARK: - Agent row and detail/action surface

/// The row is deliberately a navigation label rather than a container for
/// nested buttons. That makes the whole visible row tappable, including the
/// whitespace at its trailing edge, while keeping action buttons reliable in
/// the destination view. The destination resolves the live record again, so
/// a stale/deleted row can never dispatch from its old snapshot.
struct AgentRow: View {
    let agent: Agent

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack(spacing: 6) {
                Circle()
                    .fill(stateColor)
                    .frame(width: 10, height: 10)
                    .accessibilityHidden(true)
                Text(agent.state.displayName)
                    .font(.caption.weight(.semibold))
                    .foregroundStyle(stateColor)
                    .accessibilityLabel(agent.state.accessibilityLabel)
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
            if let waiting = agent.waitingOn, agent.isBlocked {
                HStack(alignment: .top, spacing: 6) {
                    KindBadge(kind: waiting.kind)
                    Text(waiting.prompt)
                        .font(.caption)
                        .foregroundStyle(.secondary)
                        .lineLimit(2)
                    Spacer(minLength: 0)
                }
                .accessibilityLabel("Blocked claim: \(waiting.prompt)")
            }
            Text("Open details for Tail 200, Prompt, Interrupt, and approvals")
                .font(.caption2)
                .foregroundStyle(.secondary)
        }
        .padding(.vertical, 4)
        .opacity(isDimmed ? 0.65 : 1)
        .frame(maxWidth: .infinity, alignment: .leading)
        .contentShape(Rectangle())
        .accessibilityElement(children: .combine)
        .accessibilityLabel(accessibilitySummary)
        .accessibilityHint("Double tap to open agent details and actions")
    }

    private var accessibilitySummary: String {
        let title = agent.title ?? agent.displayName ?? agent.agentId
        return "\(title), \(agent.state.displayName), agent row"
    }

    /// D28: idle/done rows dim, but their explicit state text remains.
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

/// Detail destination for one route. `currentAgent` is intentionally looked
/// up from the store on every render; a selected agent can disappear between
/// the tap and an action response.
struct AgentDetailView: View {
    let agentId: String
    @ObservedObject var model: AppModel
    @ObservedObject var drafts: PromptDrafts

    var body: some View {
        Group {
            if let agent = model.fleet.agent(agentId) {
                AgentDetailContent(agent: agent, model: model, drafts: drafts)
            } else {
                VStack(alignment: .leading, spacing: 12) {
                    Label("Agent no longer available", systemImage: "exclamationmark.triangle")
                        .font(.headline)
                    Text("This agent was deleted or migrated. Refresh the fleet before sending an action.")
                        .foregroundStyle(.secondary)
                }
                .frame(maxWidth: .infinity, alignment: .leading)
                .padding()
                .accessibilityElement(children: .combine)
                .accessibilityLabel("Agent \(agentId) is no longer available; no actions are enabled")
            }
        }
        .navigationTitle(model.fleet.agent(agentId)?.title
                         ?? model.fleet.agent(agentId)?.displayName
                         ?? agentId)
        .navigationBarTitleDisplayMode(.inline)
    }
}

private struct AgentDetailContent: View {
    let agent: Agent
    @ObservedObject var model: AppModel
    @ObservedObject var drafts: PromptDrafts

    private var grants: Set<Capability> { model.actionGrants }
    private var availability: [AgentActionAvailability] {
        BoardModel.actionAvailability(agent: agent, grants: grants)
    }
    private var driveClient: DriveClient {
        DriveClient(host: model.hostURL ?? URL(string: "http://127.0.0.1:8474")!)
    }

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 16) {
                AgentStateSummary(agent: agent)
                if let reason = agent.reason, !reason.isEmpty {
                    Text(reason)
                        .font(.subheadline)
                        .foregroundStyle(.secondary)
                        .textSelection(.enabled)
                }
                Text("Actions are checked against the current fleet record before dispatch.")
                    .font(.caption)
                    .foregroundStyle(.secondary)

                if let waiting = agent.waitingOn, agent.isBlocked,
                   let approval = availability.first(where: { $0.action == .approveDeny }) {
                    let approvalInFlight = model.isActionInFlight(agentId: agent.agentId,
                                                                  capability: .approve)
                    ClaimCard(
                        agent: agent,
                        waiting: waiting,
                        approvalEnabled: approval.isEnabled && !approvalInFlight,
                        approvalDisabledReason: approval.disabledReason
                            ?? (approvalInFlight ? "An approval action is already in progress." : nil),
                        onChoice: { choice in
                            dispatchApproval(choice, expectedPromptHash: waiting.promptHash)
                        },
                        onCanned: { action in
                            dispatchCanned(action, expectedPromptHash: waiting.promptHash)
                        })
                }

                VStack(alignment: .leading, spacing: 12) {
                    Text("Controls")
                        .font(.headline)
                    actionButton(.tail, systemImage: "text.line.first.and.arrowtriangle.forward",
                                 title: "Tail 200") {
                        dispatchTail()
                    }
                    actionButton(.interrupt, systemImage: "stop.circle",
                                 title: "Interrupt") {
                        dispatchInterrupt()
                    }
                    promptControl
                }

                if let tail = model.fleet.tail(for: agent.agentId) {
                    TailOutputView(lines: tail)
                }
            }
            .padding()
        }
        .accessibilityElement(children: .contain)
    }

    @ViewBuilder
    private func actionButton(_ action: RowAction, systemImage: String,
                              title: String, perform: @escaping () -> Void) -> some View {
        if let item = availability.first(where: { $0.action == action }) {
            VStack(alignment: .leading, spacing: 4) {
                Button {
                    perform()
                } label: {
                    Label(title, systemImage: systemImage)
                        .frame(maxWidth: .infinity, alignment: .leading)
                }
                .buttonStyle(.bordered)
                .disabled(!item.isEnabled || model.isActionInFlight(agentId: agent.agentId,
                                                                     capability: action.capability))
                .accessibilityLabel(item.isEnabled ? title : "\(title) unavailable")
                if let reason = item.disabledReason {
                    Text(reason)
                        .font(.caption)
                        .foregroundStyle(.secondary)
                        .accessibilityLabel("Why \(title) is disabled: \(reason)")
                } else if model.isActionInFlight(agentId: agent.agentId,
                                                 capability: action.capability) {
                    Text("Action in progress")
                        .font(.caption)
                        .foregroundStyle(.secondary)
                }
            }
        }
    }

    @ViewBuilder
    private var promptControl: some View {
        if let item = availability.first(where: { $0.action == .prompt }) {
            VStack(alignment: .leading, spacing: 4) {
                Text("Prompt")
                    .font(.subheadline.weight(.semibold))
                HStack {
                    TextField("Send a prompt…", text: drafts.binding(for: agent.agentId))
                        .textFieldStyle(.roundedBorder)
                        .disabled(!item.isEnabled)
                    Button("Send Prompt") {
                        dispatchPrompt()
                    }
                    .buttonStyle(.borderedProminent)
                    .disabled(!item.isEnabled
                              || drafts.drafts[agent.agentId]?.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty != false
                              || model.isActionInFlight(agentId: agent.agentId, capability: .prompt))
                }
                if let reason = item.disabledReason {
                    Text(reason)
                        .font(.caption)
                        .foregroundStyle(.secondary)
                        .accessibilityLabel("Why Prompt is disabled: \(reason)")
                }
            }
        }
    }

    private func dispatchTail() {
        guard let live = model.fleet.agent(agent.agentId) else { return }
        if model.mode == .demo {
            model.driveDemo(capability: .readTail, agent: live)
        } else {
            model.driveReadTail(agent: live, driveClient: driveClient)
        }
    }

    private func dispatchInterrupt() {
        guard let live = model.fleet.agent(agent.agentId) else { return }
        if model.mode == .demo {
            model.driveDemo(capability: .interrupt, agent: live)
        } else {
            model.driveInterrupt(agent: live, driveClient: driveClient)
        }
    }

    private func dispatchPrompt() {
        guard let live = model.fleet.agent(agent.agentId) else { return }
        let text = drafts.drafts[agent.agentId] ?? ""
        guard !text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else { return }
        drafts.clear(agent.agentId)
        if model.mode == .demo {
            model.driveDemo(capability: .prompt, agent: live, choice: text)
        } else {
            model.drivePrompt(agent: live, text: text, driveClient: driveClient)
        }
    }

    private func dispatchApproval(_ choice: String, expectedPromptHash: String) {
        guard let live = model.fleet.agent(agent.agentId) else { return }
        if model.mode == .demo {
            model.driveDemo(capability: .approve, agent: live, choice: choice)
        } else {
            model.driveApprove(agent: live, choice: choice, driveClient: driveClient,
                              expectedPromptHash: expectedPromptHash)
        }
    }

    private func dispatchCanned(_ action: CannedChoice.Action, expectedPromptHash: String) {
        if model.mode == .demo {
            guard let live = model.fleet.agent(agent.agentId), let waiting = live.waitingOn,
                  let choice = CannedChoice.choice(for: action, kind: waiting.kind,
                                                   choices: waiting.choices) else { return }
            model.driveDemo(capability: .approve, agent: live, choice: choice)
        } else {
            model.handleCannedAction(agentId: agent.agentId, action: action,
                                     driveClient: driveClient,
                                     expectedPromptHash: expectedPromptHash)
        }
    }
}

private struct AgentStateSummary: View {
    let agent: Agent

    var body: some View {
        HStack(spacing: 8) {
            Image(systemName: "circle.fill")
                .foregroundStyle(color)
                .accessibilityHidden(true)
            Text(agent.state.displayName)
                .font(.headline)
            Text(agent.tool)
                .font(.caption.monospaced())
                .foregroundStyle(.secondary)
            Spacer()
        }
        .accessibilityElement(children: .combine)
        .accessibilityLabel("\(agent.state.accessibilityLabel), \(agent.tool) agent")
    }

    private var color: Color {
        switch agent.state {
        case .blocked: return .red
        case .working: return .green
        case .idle: return .secondary
        case .done: return .blue
        case .unknown: return .gray
        }
    }
}

private struct TailOutputView: View {
    let lines: [String]

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            Text("Latest tail (up to 200 lines)")
                .font(.headline)
            if lines.isEmpty {
                Text("No output returned")
                    .font(.caption.monospaced())
                    .foregroundStyle(.secondary)
            } else {
                ScrollView([.vertical, .horizontal]) {
                    Text(lines.joined(separator: "\n"))
                        .font(.caption.monospaced())
                        .textSelection(.enabled)
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .accessibilityLabel("Latest bounded agent tail")
                }
                .frame(maxHeight: 220)
                .padding(6)
                .background(.quaternary.opacity(0.35), in: RoundedRectangle(cornerRadius: 5))
            }
        }
        .accessibilityElement(children: .contain)
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

/// Full-width disclosure control for the low-priority bucket. The visible
/// Expanded/Collapsed value complements the chevron and is also exposed as
/// the accessibility value, so the state is clear without relying on shape
/// or color.
struct IdleDoneHeader: View {
    let count: Int
    @Binding var isExpanded: Bool

    var body: some View {
        Button {
            withAnimation { isExpanded.toggle() }
        } label: {
            HStack(spacing: 8) {
                Text("Idle / done (\(count))")
                Text(isExpanded ? "Expanded" : "Collapsed")
                    .font(.caption.weight(.regular))
                    .foregroundStyle(.secondary)
                Spacer(minLength: 8)
                Image(systemName: isExpanded ? "chevron.down" : "chevron.right")
                    .font(.caption2)
                    .accessibilityHidden(true)
            }
            // A minimum hit height plus max width makes the whole pinned
            // header tappable, including trailing whitespace.
            .frame(maxWidth: .infinity, minHeight: 44, alignment: .leading)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .frame(maxWidth: .infinity, alignment: .leading)
        .contentShape(Rectangle())
        .accessibilityLabel("Idle and done agents")
        .accessibilityValue("\(count) agents, \(isExpanded ? "Expanded" : "Collapsed")")
        .accessibilityHint(isExpanded ? "Double tap to collapse" : "Double tap to expand")
    }
}

struct FleetView: View {
    @ObservedObject var model: AppModel

    var body: some View {
        NavigationStack(path: $navigationPath) {
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
                var nextSelection = selection
                nextSelection.reconcile(availableAgentIds: agentIds)
                selection = nextSelection
                navigationPath.removeAll { !agentIds.contains($0.agentId) }
            }
            .onChange(of: navigationPath) { _, path in
                var nextSelection = selection
                if let route = path.last {
                    nextSelection.select(route.agentId)
                } else {
                    nextSelection.clear()
                }
                selection = nextSelection
            }
            .navigationDestination(for: AgentRoute.self) { route in
                AgentDetailView(agentId: route.agentId, model: model, drafts: promptDrafts)
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
    @State private var idleDoneDisclosure = IdleDoneDisclosure()
    @State private var navigationPath: [AgentRoute] = []
    @State private var selection = AgentSelection()
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
            if idleDoneDisclosure.isExpanded {
                ForEach(sections.idleDone) { agent in
                    agentRow(agent)
                }
            }
        } header: {
            pinnedHeader {
                IdleDoneHeader(count: sections.idleDone.count,
                               isExpanded: $idleDoneDisclosure.isExpanded)
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
        NavigationLink(value: AgentRoute(agentId: agent.agentId)) {
            AgentRow(agent: agent)
        }
        .accessibilityHint("Double tap to open agent details and actions")
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
