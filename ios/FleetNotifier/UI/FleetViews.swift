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
///
/// #166 item 3: a blocked row additionally surfaces the pending question
/// (≤2 lines) inline plus a borderless "Answer" affordance. `onAnswer` is
/// wired to FleetView's focused prompt sheet, NOT to navigation — the row
/// still opens the detail surface when the summary (not the button) is
/// tapped.
//
/// Self-ticking "· 4s" duration chip (re-review P1/P4/P6). It owns its own
/// clock so a 1 Hz tick re-renders only this small view, not the whole board
/// (the `FleetView`-level timer that used to drive `AgentRow` is removed).
/// The interval follows the same 1s-while-sub-minute / 30s rule.
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

struct AgentRow: View {
    let agent: Agent
    var onAnswer: (() -> Void)?
    /// #166 review F2: client-side state-entered wall clock, passed down
    /// from `FleetStore.stateEnteredAt` so a reason/title churn does not
    /// reset the duration. `nil` falls back to `agent.ts` (pre-tracking
    /// callers / pure tests).
    var stateEnteredAt: UInt64? = nil
    @Environment(\.dynamicTypeSize) private var dynamicTypeSize
    @ScaledMetric(relativeTo: .caption) private var badgeMinWidth: CGFloat = 84

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            if isAccessibilitySize {
                // Dynamic Type: stack the trailing chips (issue/CI/tool)
                // under the title instead of clipping them at the edge.
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
                    if agent.title != nil { identityText }
                    Spacer(minLength: 0)
                    trailingChips
                }
            }
            WorkspaceLine(agent: agent)
            if let waiting = agent.waitingOn, agent.isBlocked {
                waitingLine(waiting)
            }
            if let activity = BoardModel.supervisionActivity(for: agent) {
                supervisionLine(activity)
            }
        }
        .padding(.vertical, 2)
        .opacity(isDimmed ? 0.65 : 1)
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

    /// Fixed-width state badge (glyph + label + duration). A real
    /// `minWidth` keeps the badges in one column even as durations roll
    /// over; `.lineLimit(1)` preserves the #166 item-1 no-mid-word-wrap rule.
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
        // Round-4: the fixed-size content must be honoured even when the
        // row is tight. `.fixedSize` applied AFTER `.frame` makes the whole
        // badge report its natural width (max of content and minWidth) and
        // refuse to compress, so its duration is never painted over. The
        // priority keeps it above the title so the title truncates after.
        .fixedSize(horizontal: true, vertical: false)
        .layoutPriority(2)
    }

    @ViewBuilder
    private var titleText: some View {
        Text(agent.title ?? agent.displayName ?? agent.agentId)
            .font(.subheadline.weight(.semibold))
            .lineLimit(1)
            // Line-1 emphasis (R2-E): the task TITLE wins under
            // compression, so it gets a higher layoutPriority than the
            // identity, but below the state badge (round-4) so the
            // fixed-size duration is never clipped/overlapped.
            .layoutPriority(1)
    }

    /// Session identity must survive on the row even when a title is shown —
    /// two agents can share a title. Falls back to the agent id when the
    /// session has no display name (R2-D).
    @ViewBuilder
    private var identityText: some View {
        let role = BoardModel.role(for: agent)
        let identity = agent.displayName ?? agent.agentId
        Text(role == .unknown ? identity : "\(role.displayName) · \(identity)")
            .font(.caption2.monospaced())
            .foregroundStyle(.secondary)
            .lineLimit(1)
            .layoutPriority(0)
    }

    @ViewBuilder
    private var trailingChips: some View {
        HStack(spacing: 6) {
            ForEach(IssueChip.chips(for: agent), id: \.label) { chip in
                Text(chip.label)
                    .font(.caption2.monospaced())
                    .foregroundStyle(chip.isFlagged ? Color.secondary : Color.accentColor)
                    .lineLimit(1)
                    .fixedSize(horizontal: true, vertical: false)
            }
            if let ci = agent.workspace.ciStatus {
                CiGlyph(status: ci)
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

    @ViewBuilder
    private func waitingLine(_ waiting: WaitingOn) -> some View {
        HStack(alignment: .top, spacing: 6) {
            KindBadge(kind: waiting.kind)
                .accessibilityHidden(true)
            Text(waiting.prompt)
                .font(.caption)
                .foregroundStyle(.secondary)
                .lineLimit(2)
                .accessibilityLabel("Blocked claim: \(waiting.prompt)")
            Spacer(minLength: 6)
            if let onAnswer {
                Button("Answer", action: onAnswer)
                    .buttonStyle(.borderless)
                    .font(.caption.weight(.semibold))
                    .accessibilityLabel("Answer \(waiting.kind.rawValue) for \(agent.title ?? agent.agentId)")
            }
        }
    }

    @ViewBuilder
    private func supervisionLine(_ activity: SupervisionActivity) -> some View {
        Label(activity.summary, systemImage: "arrow.triangle.2.circlepath")
            .font(.caption2.monospaced())
            .foregroundStyle(.tint)
            .lineLimit(1)
            .accessibilityElement(children: .ignore)
            .accessibilityLabel(activity.accessibilityLabel)
    }

    private var isAccessibilitySize: Bool {
        dynamicTypeSize >= .accessibility1
    }

    /// Finished rows dim, but a supervising orchestrator remains active in
    /// the presentation even though its canonical state is `done`.
    private var isDimmed: Bool {
        agent.state == .idle
            || (agent.state == .done && BoardModel.supervisionActivity(for: agent) == nil)
    }

    private var stateStyle: StateStyle {
        StateStyle.style(for: agent.state)
    }
}

// MARK: - Row accessibility (review F6)

/// One VoiceOver summary for an agent row (review F6): title/session identity,
/// state label, repo, branch. Used as the NavigationLink container's label so
/// the whole row is a single, named, navigable element.
private func rowSummary(_ agent: Agent) -> String {
    var parts: [String] = [
        agent.title ?? agent.displayName ?? agent.agentId,
        "Role: \(BoardModel.role(for: agent).displayName)",
        "State: \(StateStyle.style(for: agent.state).label)",
    ]
    if let activity = BoardModel.supervisionActivity(for: agent) {
        parts.append(activity.accessibilityLabel)
    } else {
        parts.append("Activity: none")
    }
    if let repo = agent.workspace.repo { parts.append(repo) }
    if let branch = agent.workspace.branch { parts.append(branch) }
    return parts.joined(separator: ", ")
}

/// A row is one VoiceOver element with a summary label and, for blocked
/// rows, a custom "Answer" action. Children stay individually reachable
/// (`.contain`), so the Answer button and the claimed-prompt text are not
/// swallowed, while the container no longer fragments every glyph/label into
/// its own element. Applied to the NavigationLink container so the row keeps
/// one named, navigable element with the Answer as a custom action.
private extension View {
    @ViewBuilder
    func rowAccessibility(summary: String, answerAction: (() -> Void)?) -> some View {
        self
            // re-review P3: `.combine` keeps the row as ONE activatable
            // VoiceOver element (double-tap opens detail; "Answer" is the
            // custom action below) instead of a `.contain` container that can
            // fragment the link into non-activatable children.
            .accessibilityElement(children: .combine)
            .accessibilityLabel(summary)
            .modifier(AnswerAccessibilityAction(action: answerAction))
    }
}

private struct AnswerAccessibilityAction: ViewModifier {
    let action: (() -> Void)?

    func body(content: Content) -> some View {
        if let action {
            content.accessibilityAction(named: "Answer") { action() }
        } else {
            content
        }
    }
}

private struct SupervisionEvidenceView: View {
    let activity: SupervisionActivity

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            Text("Session status")
                .font(.caption.weight(.bold))
                .foregroundStyle(.secondary)
            HStack(spacing: 8) {
                Text("State: Finished")
                    .font(.caption.weight(.semibold))
                Text("Activity: Supervising")
                    .font(.caption.weight(.semibold))
                    .foregroundStyle(.tint)
            }
            Text(activity.summary)
                .font(.caption2.monospaced())
                .foregroundStyle(.tint)
            if let queuedWork = activity.queuedWork {
                Text("Queued work: \(queuedWork)")
                    .font(.caption2.monospaced())
                    .foregroundStyle(.secondary)
            }
            Text("Current command redacted")
                .font(.caption2)
                .foregroundStyle(.secondary)
        }
        .padding(.leading, 34)
        .padding(.vertical, 6)
        .accessibilityElement(children: .ignore)
        .accessibilityLabel(
            "Expanded Session status. State: Finished, "
                + activity.accessibilityLabel)
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
    @State private var showKillConfirm = false
    /// #232: the worktree-diff sheet (lazy paged, grant/capability-gated).
    @State private var diffPresented = false
    @State private var terminalPresented = false
    @State private var devicesGrantsPresented = false
    @FocusState private var focusPrompt: Bool

    private var grants: Set<Capability> { model.actionGrants }
    private var availability: [AgentActionAvailability] {
        BoardModel.actionAvailability(agent: agent, grants: grants)
    }
    private var driveClient: DriveClient {
        model.makeDriveClient()
    }

    private var terminalAvailability: AgentActionAvailability {
#if DEBUG
        let demoRegistered = model.mode == .demo
#else
        let demoRegistered = false
#endif
        return BoardModel.terminalAvailability(
            agent: agent,
            grants: grants,
            isRegistered: demoRegistered ||
                (model.hostURL != nil && model.keyId != nil && model.signer != nil))
    }

    private var terminalClient: TerminalAttachClient? {
        guard let signer = model.signer,
              let keyId = model.keyId,
              let host = model.hostURL else { return nil }
        return TerminalAttachClient(host: host, keyId: keyId, signer: signer)
    }

    private var terminalWorktree: CorralWorktree? {
        guard agent.capabilities.contains(Capability.attach.rawValue),
              let path = agent.workspace.worktreePath, !path.isEmpty,
              model.hostURL != nil, model.keyId != nil, model.signer != nil,
              grants.contains(.attach) else { return nil }
        return CorralWorktree(repo: agent.workspace.repo ?? "—",
                              branch: agent.workspace.branch ?? agent.title ?? agent.agentId,
                              path: path, workspaceId: agent.agentId, paneId: nil,
                              isPrunable: false, dirty: agent.workspace.dirty,
                              agentAttached: true, currentFocus: true)
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            // #246 Variant 2: the summary hugs its content — no ScrollView
            // wrapper, so the dead black gap between the summary and the
            // Recent-output panel is gone and the panel (which owns its own
            // scroll cage) fills the remaining sheet height down to the
            // pinned toolbar + composer.
            VStack(alignment: .leading, spacing: 16) {
                AgentStateSummary(agent: agent,
                                  stateEnteredAt: model.fleet.stateEnteredAt[agent.agentId])
                presentationStatus
                if let diff = availability.first(where: { $0.action == .diff }),
                   !diff.isEnabled {
                    Text(diff.disabledReason ?? "Diff unavailable")
                        .font(.caption)
                        .foregroundStyle(.secondary)
                        .accessibilityLabel(diff.disabledReason ?? "Diff unavailable")
                }

                if !terminalAvailability.isEnabled,
                   let reason = terminalAvailability.disabledReason {
                    VStack(alignment: .leading, spacing: 6) {
                        Label(reason, systemImage: "terminal")
                            .font(.caption)
                            .foregroundStyle(.secondary)
                            .accessibilityLabel(reason)
                        Button("Open Devices & Grants", systemImage: "lock.shield") {
                            devicesGrantsPresented = true
                        }
                        .font(.caption.weight(.semibold))
                        .accessibilityHint("Navigation only; grants are not changed")
                    }
                }

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
            }
            .padding()

            // The Recent-output view owns its viewport and the composer is a
            // sibling below it, so scrolling history never moves the prompt
            // controls.
#if DEBUG
            if model.mode == .demo && model.demoPresentation == .before {
                RecentOutputBeforeView(
                    agent: agent,
                    model: model,
                    drafts: drafts,
                    focusPrompt: $focusPrompt,
                    onSend: dispatchPrompt,
                    toolbar: nil)
                    .frame(minHeight: 260, maxHeight: .infinity)
            } else {
                RecentOutputView(
                    agent: agent,
                    model: model,
                    drafts: drafts,
                    focusPrompt: $focusPrompt,
                    onSend: dispatchPrompt,
                    toolbar: nil)
                    .frame(minHeight: 260, maxHeight: .infinity)
            }
#else
            RecentOutputView(
                agent: agent,
                model: model,
                drafts: drafts,
                focusPrompt: $focusPrompt,
                onSend: dispatchPrompt,
                toolbar: nil)
                .frame(minHeight: 260, maxHeight: .infinity)
#endif
        }
        .accessibilityElement(children: .contain)
        .confirmationDialog("Kill Agent?",
                            isPresented: $showKillConfirm,
                            titleVisibility: .visible) {
            Button("Kill \(agent.title ?? agent.agentId)", role: .destructive) {
                dispatchKill()
            }
            Button("Cancel", role: .cancel) {}
        } message: {
            Text("This sends the kill capability to \(agent.tool) on \(agent.workspace.repo ?? "—"). The agent stops; any in-progress work is lost.")
        }
        .sheet(isPresented: $diffPresented) {
            AgentDiffSheet(agent: agent, model: model)
        }
        .sheet(isPresented: $terminalPresented) {
            TerminalAttachView(client: terminalClient, worktree: terminalWorktree)
        }
        .sheet(isPresented: $devicesGrantsPresented) {
            DevicesGrantsView(model: model)
        }
        .toolbar {
            ToolbarItemGroup(placement: .topBarTrailing) {
                toolbarPrimaryControl
                toolbarDiffButton
                overflowMenu
            }
        }
    }

    @ViewBuilder
    private var presentationStatus: some View {
        let style = StateStyle.style(for: agent.state)
        let activity = BoardModel.supervisionActivity(for: agent)
        VStack(alignment: .leading, spacing: 4) {
            Text("State: \(style.label)")
                .font(.subheadline.weight(.semibold))
                .foregroundStyle(style.color)
            if let activity {
                Text(activity.summary)
                    .font(.caption.monospaced())
                    .foregroundStyle(.tint)
                if let queuedWork = activity.queuedWork {
                    Text("Queued work: \(queuedWork)")
                        .font(.caption2.monospaced())
                        .foregroundStyle(.secondary)
                }
                Text("Current command redacted")
                    .font(.caption2)
                    .foregroundStyle(.secondary)
            } else {
                Text("Activity: none")
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }
        }
        .accessibilityElement(children: .ignore)
        .accessibilityLabel(
            "State: \(style.label), "
                + (activity?.accessibilityLabel ?? "Activity: none"))
    }

    /// #308 V1: compact trailing top action group. Interrupt is destructive;
    /// Diff is evidence/review; More contains secondary actions and recovery.
    private var actionToolbar: some View {
        HStack(spacing: 8) {
            toolbarPrimaryControl
            toolbarDiffButton
            overflowMenu
        }
        .frame(minHeight: 44)
        .accessibilityElement(children: .contain)
        .accessibilityLabel("Agent actions")
    }

    /// Issue #166 item 3: ONE primary action chosen by state — blocked →
    /// answer, working → interrupt, done → attach/PR, idle/unknown → none.
    /// The remaining actions live in `overflowMenu`.
    @ViewBuilder
    private var toolbarPrimaryControl: some View {
        let primary = BoardModel.primaryAction(for: agent)
        switch primary {
        case .answer:
            toolbarButton("Answer", systemImage: "bubble.left.and.bubble.right.fill",
                          item: availability.first(where: { $0.action == .prompt }),
                          inFlight: false) {
                focusPrompt = true
            }
        case .interrupt:
            toolbarButton("Interrupt", systemImage: "stop.circle.fill",
                          item: availability.first(where: { $0.action == .interrupt }),
                          inFlight: model.isActionInFlight(agentId: agent.agentId,
                                                           capability: .interrupt)) {
                dispatchInterrupt()
            }
        case .attach:
            toolbarButton("Attach", systemImage: "paperclip.fill",
                          item: availability.first(where: { $0.action == .attach }),
                          inFlight: model.isActionInFlight(agentId: agent.agentId,
                                                           capability: .attach)) {
                dispatchAttach()
            }
        case .none:
            EmptyView()
        }
    }

    /// Compact toolbar button for the state primary. The disabled-reason
    /// caption that used to sit under the full-width pill is now carried by
    /// the VoiceOver label (and the More overflow), so no dead space remains.
    @ViewBuilder
    private func toolbarButton(_ title: String, systemImage: String,
                               item: AgentActionAvailability?, inFlight: Bool,
                               perform: @escaping () -> Void) -> some View {
        let disabled = !(item?.isEnabled ?? false) || inFlight
        Button {
            perform()
        } label: {
            Label(title, systemImage: systemImage)
                .lineLimit(1)
                .fixedSize(horizontal: true, vertical: false)
        }
        .buttonStyle(.bordered)
        .controlSize(.small)
        .tint(title == "Interrupt" ? .red : .accentColor)
        .frame(minWidth: 44, minHeight: 44)
        .disabled(disabled)
        .accessibilityLabel(disabled
                            ? "\(title) unavailable — \(item?.disabledReason ?? "action in progress")"
                            : title)
    }

    /// ± Diff — one of the two diff access points for #232 (the agent-row
    /// dirty chip is the other). Grant/capability-gated like read_tail;
    /// presents the diff sheet, which lazy-pages the daemon's bounded pages.
    private var toolbarDiffButton: some View {
        let item = availability.first(where: { $0.action == .diff })
        let inFlight = model.isActionInFlight(agentId: agent.agentId,
                                              capability: .readDiff)
        let disabled = !(item?.isEnabled ?? false) || inFlight
        return Button {
            diffPresented = true
        } label: {
            Label("Diff", systemImage: "doc.text.magnifyingglass")
                .lineLimit(1)
                .fixedSize(horizontal: true, vertical: false)
        }
        .buttonStyle(.bordered)
        .controlSize(.small)
        .frame(minWidth: 44, minHeight: 44)
        .disabled(disabled)
        .accessibilityLabel(disabled
                            ? "Diff unavailable — \(item?.disabledReason ?? "diff fetch in progress")"
                            : "Diff")
    }

    /// Issue #166 item 4: Kill leaves the peer button stack and lives in the
    /// overflow menu as `.destructive`, guarded by a confirmation dialog.
    /// #246: the full kill-grant sentence (demoted from the Controls
    /// caption) lives here as a read-only hint when Kill is disabled.
    @ViewBuilder
    private var overflowMenu: some View {
        let primary = BoardModel.primaryAction(for: agent)
        Menu {
            if primary != .answer,
               let item = availability.first(where: { $0.action == .prompt }) {
                Button {
                    focusPrompt = true
                } label: {
                    Label("Prompt", systemImage: "text.bubble")
                }
                .disabled(!item.isEnabled)
            }
            if primary != .interrupt,
               let item = availability.first(where: { $0.action == .interrupt }) {
                Button {
                    dispatchInterrupt()
                } label: {
                    Label("Interrupt", systemImage: "stop.circle")
                }
                .disabled(!item.isEnabled)
            }
            if primary != .attach,
               let item = availability.first(where: { $0.action == .attach }) {
                Button {
                    dispatchAttach()
                } label: {
                    Label("Attach", systemImage: "paperclip")
                }
                .disabled(!item.isEnabled)
            }
            let terminal = terminalAvailability
            Button {
                terminalPresented = true
            } label: {
                Label("Terminal", systemImage: "terminal")
            }
            .disabled(!terminal.isEnabled)
            .accessibilityLabel(terminal.isEnabled ? "Terminal" : "Terminal unavailable")
            .accessibilityHint(terminal.disabledReason ?? "Open the attached worktree terminal")
            if let reason = terminal.disabledReason {
                Button {
                    // Read-only recovery route; it never changes grants.
                    devicesGrantsPresented = true
                } label: {
                    Text(reason)
                }
                .disabled(true)
                Button("Open Devices & Grants", systemImage: "lock.shield") {
                    devicesGrantsPresented = true
                }
                .accessibilityHint("Navigation only; grants are not changed")
            }
            if let item = availability.first(where: { $0.action == .kill }) {
                Button(role: .destructive) {
                    showKillConfirm = true
                } label: {
                    Label("Kill", systemImage: "xmark.circle")
                }
                .disabled(!item.isEnabled)
            }
            if let killItem = availability.first(where: { $0.action == .kill }),
               !killItem.isEnabled, let reason = killItem.disabledReason {
                Button {
                    // Read-only hint, not an action.
                } label: {
                    Text(reason)
                }
                .disabled(true)
            }
        } label: {
            Label("More", systemImage: "ellipsis.circle")
        }
        .buttonStyle(.bordered)
        .controlSize(.small)
        .frame(minWidth: 44, minHeight: 44)
        .accessibilityLabel("More actions")
    }

    private func dispatchInterrupt() {
        guard let live = model.fleet.agent(agent.agentId) else { return }
#if DEBUG
        if model.mode == .demo {
            model.driveDemo(capability: .interrupt, agent: live)
            return
        }
#endif
        model.driveInterrupt(agent: live, driveClient: driveClient)
    }

    private func dispatchKill() {
        guard let live = model.fleet.agent(agent.agentId) else { return }
#if DEBUG
        if model.mode == .demo {
            model.banner = .info("(demo) Kill is not faked; use a live registered device.")
            return
        }
#endif
        model.driveKill(agent: live, driveClient: driveClient)
    }

    private func dispatchAttach() {
        guard let live = model.fleet.agent(agent.agentId) else { return }
#if DEBUG
        if model.mode == .demo {
            model.banner = .info("(demo) Attach is not faked; use a live registered device.")
            return
        }
#endif
        model.driveAttach(agent: live, driveClient: driveClient)
    }

    private func dispatchPrompt() {
        guard let live = model.fleet.agent(agent.agentId) else { return }
        let text = drafts.drafts[agent.agentId] ?? ""
        guard !text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else { return }
#if DEBUG
        if model.mode == .demo {
            model.driveDemo(capability: .prompt, agent: live, choice: text)
            drafts.clear(agent.agentId)
            return
        }
#endif
        // Dispatch FIRST; clear only when the drive was accepted. A refused
        // dispatch keeps the typed draft on the detail surface (review F7).
        if model.drivePrompt(agent: live, text: text, driveClient: driveClient) == .accepted {
            drafts.clear(agent.agentId)
        }
    }

    private func dispatchApproval(_ choice: String, expectedPromptHash: String) {
        guard let live = model.fleet.agent(agent.agentId) else { return }
#if DEBUG
        if model.mode == .demo {
            model.driveDemo(capability: .approve, agent: live, choice: choice)
            return
        }
#endif
        model.driveApprove(agent: live, choice: choice, driveClient: driveClient,
                           expectedPromptHash: expectedPromptHash)
    }

    private func dispatchCanned(_ action: CannedChoice.Action, expectedPromptHash: String) {
#if DEBUG
        if model.mode == .demo {
            guard let live = model.fleet.agent(agent.agentId), let waiting = live.waitingOn,
                  let choice = CannedChoice.choice(for: action, kind: waiting.kind,
                                                   choices: waiting.choices) else { return }
            model.driveDemo(capability: .approve, agent: live, choice: choice)
            return
        }
#endif
        model.handleCannedAction(agentId: agent.agentId, action: action,
                                 driveClient: driveClient,
                                 expectedPromptHash: expectedPromptHash)
    }
}

private struct AgentStateSummary: View {
    let agent: Agent
    /// #166 review F2: client-side state-entered time (see `.stateEnteredAt`
    /// on `FleetStore`). `nil` falls back to `agent.ts`.
    var stateEnteredAt: UInt64? = nil

    var body: some View {
        HStack(spacing: 8) {
            // Honor the StateStyle mark (finding 8a): working renders as an
            // open ring, every other state as a filled dot — identical to
            // the row. Never hard-code `circle.fill` for all states.
            Circle()
                .fill(stateStyle.isRing ? Color.clear : stateStyle.color)
                .overlay(Circle().stroke(stateStyle.color, lineWidth: 1))
                .frame(width: 14, height: 14)
                .accessibilityHidden(true)
            Text(stateStyle.glyph)
                .font(.headline)
                .foregroundStyle(stateStyle.color)
                .accessibilityHidden(true)
            Text(stateStyle.label)
                .font(.headline)
            TimeInStateLabel(agent: agent, stateEnteredAt: stateEnteredAt)
            Text(agent.tool)
                .font(.caption.monospaced())
                .foregroundStyle(.secondary)
            Spacer()
        }
        .accessibilityElement(children: .combine)
        .accessibilityLabel("\(stateStyle.accessibilityLabel), \(agent.tool) agent")
    }

    private var stateStyle: StateStyle {
        StateStyle.style(for: agent.state)
    }

}

/// Debug-only baseline used by the design gate. It intentionally renders the
/// pre-#205 terminal-shaped concatenation as one text payload, while the
/// production path below renders semantic blocks and disclosures.
#if DEBUG
private struct RecentOutputBeforeView: View {
    let agent: Agent
    @ObservedObject var model: AppModel
    let drafts: PromptDrafts
    let focusPrompt: FocusState<Bool>.Binding
    let onSend: () -> Void
    /// #246 Variant 2: compact action toolbar pinned above the composer
    /// (built by AgentDetailContent, which owns availability + dispatch).
    let toolbar: AnyView?

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            HStack(spacing: 8) {
                Text("Recent output")
                    .font(.headline)
                    .foregroundStyle(RecentOutputPalette.ink)
                Spacer()
                Text("legacy")
                    .font(.caption2.weight(.bold))
                    .foregroundStyle(RecentOutputPalette.muted)
            }
            .padding(.horizontal, 12)
            .padding(.top, 10)
            .padding(.bottom, 4)

            ScrollView(.vertical) {
                Text(DemoFleet.monotoneOutput(for: agent))
                    .font(.subheadline.monospaced())
                    .foregroundStyle(RecentOutputPalette.ink)
                    .textSelection(.enabled)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .padding(12)
            }
            .frame(maxHeight: .infinity)

            if let toolbar {
                toolbar
            }

            RecentPromptComposer(
                agent: agent,
                model: model,
                drafts: drafts,
                focusPrompt: focusPrompt,
                onSend: onSend)
        }
        .background(RecentOutputPalette.bg,
                    in: RoundedRectangle(cornerRadius: RecentOutputPalette.panelCornerRadius))
        .overlay(
            RoundedRectangle(cornerRadius: RecentOutputPalette.panelCornerRadius)
                .stroke(RecentOutputPalette.line, lineWidth: 1))
        .clipShape(RoundedRectangle(cornerRadius: RecentOutputPalette.panelCornerRadius))
        .environment(\.colorScheme, .dark)
        .accessibilityElement(children: .contain)
    }
}
#endif

/// Exact color literals from the approved Recent-output prototype. Keeping
/// this table next to the native palette gives tests a drift guard for every
/// prototype hex, including literals that are not currently painted by the
/// native surface.
enum RecentOutputPrototypeTokens {
    static let hexes: [String: String] = [
        "body": "#05070a",
        "bg": "#0d1117",
        "panel": "#10151c",
        "panel2": "#161b22",
        "panel3": "#1c2128",
        "line": "#30363d",
        "ink": "#e6edf3",
        "muted": "#8b949e",
        "accent": "#2dd4bf",
        "blocked": "#f85149",
        "done": "#d29922",
        "working": "#58a6ff",
        "idle": "#8b949e",
        "unknown": "#6e7681",
        "user-tint": "#12263f",
        "code-bg": "#0d1117",
        "code-line": "#21262d",
        "code-ink": "#e6edf3",
        "syn-diff-add": "#3fb950",
        "syn-diff-del": "#f85149",
        "syn-str": "#a5d6ff",
        "syn-kw": "#ff7b72",
        "syn-com": "#8b949e",
        "phone-border": "#2a2f37",
        "notch": "#000",
        "send-ink": "#052420",
        "user-blue": "#6ea8ff"
    ]
}

enum RecentOutputAccessibility {
    static func modelLabel(_ value: String) -> String { "Model: \(value)" }
    static func effortLabel(_ value: String) -> String { "Effort: \(value)" }
    static func worktreeLabel(_ value: String) -> String { "Worktree: \(value)" }
}

/// #316 V3 layout constants for the stacked iOS Recent-output surface.
enum RecentOutputLayout {
    /// The conversation viewport is height-capped so Session status, Harness
    /// activity, and the pinned composer below always stay reachable — the
    /// stacked mirror of the egui narrow-layout bound.
    static let conversationViewportHeight: CGFloat = 320
}

enum RecentOutputPalette {
    static let panelCornerRadius: CGFloat = 8
    /// The approved prototype is a dark-only surface. Explicitly injecting
    /// `.dark` below also keeps system controls such as the TextField aligned
    /// with the charcoal tokens when the containing app uses light mode.
    static let colorSchemePolicy = "forced-dark"
    static let forcesDarkSurface = true
    static let panel = Color(red: 16 / 255, green: 21 / 255, blue: 28 / 255)
    static let bg = Color(red: 13 / 255, green: 17 / 255, blue: 23 / 255)
    static let panel2 = Color(red: 22 / 255, green: 27 / 255, blue: 34 / 255)
    static let panel3 = Color(red: 28 / 255, green: 33 / 255, blue: 40 / 255)
    static let line = Color(red: 48 / 255, green: 54 / 255, blue: 61 / 255)
    static let ink = Color(red: 230 / 255, green: 237 / 255, blue: 243 / 255)
    static let muted = Color(red: 139 / 255, green: 148 / 255, blue: 158 / 255)
    static let accent = Color(red: 45 / 255, green: 212 / 255, blue: 191 / 255)
    static let sendInk = Color(red: 5 / 255, green: 36 / 255, blue: 32 / 255)
    static let sendInkHex = "#052420"
    static let userBlue = Color(red: 110 / 255, green: 168 / 255, blue: 255 / 255)
    static let userBlueHex = "#6ea8ff"
    static let userTint = Color(red: 18 / 255, green: 38 / 255, blue: 63 / 255)
    static let codeBg = Color(red: 13 / 255, green: 17 / 255, blue: 23 / 255)
    static let codeLine = Color(red: 33 / 255, green: 38 / 255, blue: 45 / 255)
    static let diffAdd = Color(red: 63 / 255, green: 185 / 255, blue: 80 / 255)
    static let diffDel = Color(red: 248 / 255, green: 81 / 255, blue: 73 / 255)
    static let string = Color(red: 165 / 255, green: 214 / 255, blue: 255 / 255)
    static let keyword = Color(red: 255 / 255, green: 123 / 255, blue: 114 / 255)
    static let comment = Color(red: 139 / 255, green: 148 / 255, blue: 158 / 255)
}

private struct RecentSendButtonStyle: ButtonStyle {
    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .font(.caption.weight(.semibold))
            .foregroundStyle(RecentOutputPalette.sendInk)
            .padding(.horizontal, 10)
            .padding(.vertical, 8)
            .background(RecentOutputPalette.accent,
                        in: RoundedRectangle(cornerRadius: 8))
            .opacity(configuration.isPressed ? 0.8 : 1)
    }
}

private struct RecentOutputView: View {
    let agent: Agent
    @ObservedObject var model: AppModel
    let drafts: PromptDrafts
    let focusPrompt: FocusState<Bool>.Binding
    let onSend: () -> Void
    /// #246 Variant 2: compact action toolbar pinned above the composer
    /// (built by AgentDetailContent, which owns availability + dispatch).
    let toolbar: AnyView?
    @State private var paginationAnchor: String?
    /// #316 V3: Harness activity is collapsed by default; its content stays
    /// outside the conversation viewport whether expanded or not.
    @State private var harnessExpanded = false

    private var driveClient: DriveClient {
        model.makeDriveClient()
    }

    private var tail: TailPane? { model.fleet.tailPane(for: agent.agentId) }

    private var availability: AgentActionAvailability? {
        BoardModel.actionAvailability(agent: agent, grants: model.actionGrants)
            .first { $0.action == .tail }
    }

    var body: some View {
        let snapshot = RecentOutputModel.snapshot(tail: tail)
        VStack(alignment: .leading, spacing: 0) {
            header
            if let availability, !availability.isEnabled {
                Label(availability.disabledReason ?? "Recent output unavailable",
                      systemImage: "exclamationmark.triangle")
                    .font(.caption)
                    .foregroundStyle(RecentOutputPalette.muted)
                    .padding(.horizontal, 12)
                    .padding(.vertical, 8)
                    .accessibilityLabel(availability.disabledReason ?? "Recent output unavailable")
            } else {
                sessionStatus(snapshot: snapshot)
                historyBar(snapshot: snapshot)
                content(snapshot: snapshot)
            }
            if let toolbar {
                toolbar
            }
            composer
        }
        // The approved #205 prototype supersedes the earlier unbounded
        // detail-scroll decision: this is the one bounded Recent-output
        // scroll cage, with the composer kept outside it. Use the same
        // rounded shape
        // for the fill, stroke, and clip so the border remains continuous.
        .background(RecentOutputPalette.bg,
                    in: RoundedRectangle(cornerRadius: RecentOutputPalette.panelCornerRadius))
        .overlay(
            RoundedRectangle(cornerRadius: RecentOutputPalette.panelCornerRadius)
                .stroke(RecentOutputPalette.line, lineWidth: 1))
        .clipShape(RoundedRectangle(cornerRadius: RecentOutputPalette.panelCornerRadius))
        .environment(\.colorScheme, .dark)
        .task {
            refresh()
            guard model.mode == .live else { return }
            while !Task.isCancelled {
                try? await Task.sleep(nanoseconds: 5_000_000_000)
                guard !Task.isCancelled else { return }
                refresh()
            }
        }
        .accessibilityElement(children: .contain)
    }

    private var header: some View {
        let showLive = RecentOutputModel.shouldShowLiveIndicator(
            isLiveMode: model.mode == .live,
            hasFreshNonErrorTail: RecentOutputModel.hasFreshNonErrorTail(tail))
        let indicatorColor = showLive ? RecentOutputPalette.accent : RecentOutputPalette.muted
        return HStack(spacing: 8) {
            Text("Recent output")
                .font(.headline)
                .foregroundStyle(RecentOutputPalette.ink)
            Spacer()
            Circle()
                .fill(indicatorColor)
                .frame(width: 6, height: 6)
                .accessibilityHidden(true)
            Text(showLive ? "live" : "paused")
                .font(.caption2.weight(.bold))
                .foregroundStyle(indicatorColor)
        }
        .padding(.horizontal, 12)
        .padding(.top, 10)
        .padding(.bottom, 4)
    }

    /// #316 V3 "Session status": OUTSIDE the conversation, structured
    /// read-model values only (state/freshness, session identity, tool or
    /// model metadata, effort, worktree). Unavailable values are omitted —
    /// nothing is inferred from output prose.
    @ViewBuilder
    private func sessionStatus(snapshot: RecentOutputSnapshot) -> some View {
        let fresh = RecentOutputModel.hasFreshNonErrorTail(tail)
        let status = RecentSessionStatusModel.status(
            agent: agent, tail: tail, fresh: fresh,
            metadata: snapshot.render.metadata)
        VStack(alignment: .leading, spacing: 4) {
            Text("Session status")
                .font(.caption.weight(.bold))
                .foregroundStyle(RecentOutputPalette.muted)
            // Keep the metadata chips (the already-supported canonical
            // `model effort · path` extraction) as the value carriers so the
            // #205/#315 contract is preserved; state/session are new
            // structured rows.
            HStack(spacing: 6) {
                Text(status.state)
                    .font(.caption2.weight(.semibold))
                    .foregroundStyle(RecentOutputPalette.ink)
                    .accessibilityLabel("State: \(status.state)")
                if let session = status.session {
                    metadataChip(session)
                        .accessibilityLabel("Session: \(session)")
                }
                if let tool = status.tool {
                    metadataChip(tool, color: RecentOutputPalette.accent)
                        .accessibilityLabel(RecentOutputAccessibility.modelLabel(tool))
                }
                if let effort = status.effort {
                    metadataChip(effort)
                        .accessibilityLabel(RecentOutputAccessibility.effortLabel(effort))
                }
                if let worktree = status.worktree {
                    Text(worktree)
                        .font(.caption2.monospaced())
                        .foregroundStyle(RecentOutputPalette.muted)
                        .lineLimit(1)
                        .truncationMode(.middle)
                        .padding(.horizontal, 7)
                        .padding(.vertical, 3)
                        .background(RecentOutputPalette.panel3,
                                    in: Capsule())
                        .overlay(Capsule().stroke(RecentOutputPalette.line, lineWidth: 1))
                        .accessibilityLabel(RecentOutputAccessibility.worktreeLabel(worktree))
                }
            }
        }
        .padding(.horizontal, 12)
        .padding(.bottom, 8)
        .frame(maxWidth: .infinity, alignment: .leading)
        .accessibilityElement(children: .contain)
        .accessibilityLabel("Session status")
    }

    private func metadataChip(_ text: String, color: Color = RecentOutputPalette.ink) -> some View {
        Text(text)
            .font(.caption2.monospaced())
            .foregroundStyle(color)
            .lineLimit(1)
            .padding(.horizontal, 7)
            .padding(.vertical, 3)
            .background(RecentOutputPalette.panel3,
                        in: Capsule())
            .overlay(Capsule().stroke(RecentOutputPalette.line, lineWidth: 1))
    }

    @ViewBuilder
    private func content(snapshot: RecentOutputSnapshot) -> some View {
        switch snapshot.render.phase {
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
            .padding(12)
        case .empty:
            Text("No output yet.")
                .font(.caption)
                .foregroundStyle(RecentOutputPalette.muted)
                .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
                .padding(12)
        case .error(let failure):
            VStack(alignment: .leading, spacing: 6) {
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
            .padding(12)
            .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
        case .loaded:
            // #316 V3: the canonical stream is partitioned once through the
            // production read path; Conversation is a bounded viewport (height
            // cap) so Harness activity and the pinned composer below always
            // stay reachable, and the conversation rows/counts come from the
            // Conversation partition only.
            let sections = RecentOutputSections.displaySections(from: snapshot.visibleRows)
            ScrollViewReader { proxy in
                ScrollView(.vertical) {
                    LazyVStack(alignment: .leading, spacing: 8) {
                        Text("Conversation")
                            .font(.caption.weight(.bold))
                            .foregroundStyle(RecentOutputPalette.muted)
                            .frame(maxWidth: .infinity, alignment: .leading)
                            .accessibilityLabel("Conversation")
                        ForEach(Array(identifiedConversationRows(sections).enumerated()), id: \.element.id) {
                            index, identified in
                            RecentOutputRowView(
                                row: identified.row,
                                model: model,
                                agent: agent,
                                previousBlock: previousBlock(in: sections.conversation, at: index))
                        }
                        Color.clear
                            .frame(height: 1)
                            .id("recent-output-bottom")
                    }
                    .padding(.horizontal, 12)
                    .padding(.vertical, 6)
                }
                .frame(maxHeight: RecentOutputLayout.conversationViewportHeight)
                .onAppear {
                    paginationAnchor = nil
                    scrollToBottom(proxy, animated: false)
                }
                .onChange(of: snapshot.render.rows) { oldRows, newRows in
                    if let anchor = paginationAnchor {
                        paginationAnchor = nil
                        if RecentOutputModel.shouldPreservePaginationAnchor(
                            anchor,
                            from: oldRows,
                            to: newRows) {
                            DispatchQueue.main.async {
                                proxy.scrollTo(anchor, anchor: .top)
                            }
                        } else if RecentOutputModel.shouldFollowLatest(
                            from: oldRows,
                            to: newRows) {
                            scrollToBottom(proxy, animated: true)
                        }
                    } else if RecentOutputModel.shouldFollowLatest(from: oldRows, to: newRows) {
                        scrollToBottom(proxy, animated: true)
                    }
                }
            }
            harnessActivity(sections: sections)
        }
    }

    /// #316 V3 "Harness activity": canonical System/Unknown blocks, outside
    /// the conversation viewport, collapsible, order preserved, content
    /// complete (Diagnostic / Unknown activity identity), never dropped.
    @ViewBuilder
    private func harnessActivity(sections: RecentOutputSections) -> some View {
        if !sections.harness.isEmpty {
            VStack(alignment: .leading, spacing: 0) {
                DisclosureGroup(isExpanded: $harnessExpanded) {
                    VStack(alignment: .leading, spacing: 8) {
                        ForEach(Array(RecentOutputModel.identifiedRows(
                            for: sections.harness.map(RecentOutputRow.block))
                            .enumerated()), id: \.element.id) { index, identified in
                            if case .block(let block) = identified.row {
                                RecentBlockRow(
                                    block: block,
                                    showSpeaker: true,
                                    showTimestamp: true)
                            }
                        }
                    }
                    .padding(.horizontal, 12)
                    .padding(.vertical, 6)
                } label: {
                    Text("Harness activity · \(sections.harness.count) outside conversation")
                        .font(.caption.weight(.bold))
                        .foregroundStyle(RecentOutputPalette.muted)
                }
                .accessibilityLabel("Harness activity · \(sections.harness.count) outside conversation")
            }
        }
    }

    private func identifiedConversationRows(
        _ sections: RecentOutputSections
    ) -> [RecentOutputIdentifiedRow] {
        RecentOutputModel.identifiedRows(for: sections.conversation.map(RecentOutputRow.block))
    }

    @ViewBuilder
    private func historyBar(snapshot: RecentOutputSnapshot) -> some View {
        if snapshot.render.canLoadOlder
            || snapshot.render.rows.contains(where: {
                if case .loadEarlier = $0 { return true }
                return false
            }) {
            Button { loadEarlier(snapshot: snapshot) } label: {
                HStack(spacing: 6) {
                    Image(systemName: "chevron.up")
                    if let earlierCount = snapshot.render.rows.compactMap({ row -> UInt32? in
                        if case .loadEarlier(let count) = row { return count }
                        return nil
                    }).first, earlierCount > 0 {
                        Text("Load earlier (\(earlierCount) lines)")
                    } else {
                        Text("Load earlier")
                    }
                    Spacer()
                    Text("history")
                        .font(.caption2)
                        .foregroundStyle(RecentOutputPalette.muted)
                }
                .font(.caption.weight(.medium))
                .foregroundStyle(RecentOutputPalette.accent)
                .padding(.horizontal, 12)
                .padding(.vertical, 7)
                .frame(maxWidth: .infinity, alignment: .leading)
                .background(RecentOutputPalette.panel2)
                .overlay(Rectangle().stroke(RecentOutputPalette.line, lineWidth: 1))
            }
            .buttonStyle(.plain)
            .accessibilityLabel(snapshot.render.rows.compactMap { row -> UInt32? in
                if case .loadEarlier(let count) = row { return count }
                return nil
            }.first.map {
                "Load earlier, \($0) lines omitted"
            } ?? "Load earlier")
        }
    }

    private func previousBlock(in rows: [TranscriptBlock], at index: Int) -> TranscriptBlock? {
        guard index > 0 else { return nil }
        return rows[index - 1]
    }

    private func scrollToBottom(_ proxy: ScrollViewProxy, animated: Bool) {
        DispatchQueue.main.async {
            if animated {
                withAnimation {
                    proxy.scrollTo("recent-output-bottom", anchor: .bottom)
                }
            } else {
                proxy.scrollTo("recent-output-bottom", anchor: .bottom)
            }
        }
    }

    private func loadEarlier(snapshot: RecentOutputSnapshot) {
        guard snapshot.render.canLoadOlder else { return }
        // Anchor on the first CONVERSATION block row: a history prepend is
        // measured against the same identified sequence the conversation
        // renders (the anchor never points into the harness partition).
        let anchor = RecentOutputModel.identifiedRows(
            for: RecentOutputSections.partition(snapshot.visibleRows.compactMap { row -> TranscriptBlock? in
                if case .block(let block) = row { return block }
                return nil
            }).conversation.map(RecentOutputRow.block))
            .first?.id
        guard model.loadEarlierOutput(agentId: agent.agentId) else {
            paginationAnchor = nil
            return
        }
        paginationAnchor = anchor
    }

    private var composer: some View {
        RecentPromptComposer(
            agent: agent,
            model: model,
            drafts: drafts,
            focusPrompt: focusPrompt,
            onSend: onSend)
    }

    private func refresh() {
        guard let live = model.fleet.agent(agent.agentId) else { return }
#if DEBUG
        if model.mode == .demo {
            model.driveDemo(capability: .readTail, agent: live)
            return
        }
#endif
        model.driveReadTail(agent: live, driveClient: driveClient, silent: true)
    }
}

private struct RecentPromptComposer: View {
    let agent: Agent
    @ObservedObject var model: AppModel
    @ObservedObject var drafts: PromptDrafts
    let focusPrompt: FocusState<Bool>.Binding
    let onSend: () -> Void

    private var promptAvailability: AgentActionAvailability? {
        BoardModel.actionAvailability(agent: agent, grants: model.actionGrants)
            .first { $0.action == .prompt }
    }

    var body: some View {
        if let item = promptAvailability {
            let draftIsEmpty = drafts.drafts[agent.agentId]?
                .trimmingCharacters(in: .whitespacesAndNewlines)
                .isEmpty != false
            VStack(alignment: .leading, spacing: 4) {
                HStack(spacing: 8) {
                    TextField("Reply to agent…", text: drafts.binding(for: agent.agentId))
                        .textFieldStyle(.roundedBorder)
                        .focused(focusPrompt)
                        .disabled(!item.isEnabled)
                    Button("Send", action: onSend)
                        .buttonStyle(RecentSendButtonStyle())
                        .disabled(!item.isEnabled
                                  || draftIsEmpty
                                  || model.isActionInFlight(agentId: agent.agentId,
                                                            capability: .prompt))
                }
                if let reason = item.disabledReason {
                    Text(reason)
                        .font(.caption2)
                        .foregroundStyle(RecentOutputPalette.muted)
                        .accessibilityLabel("Why Prompt is disabled: \(reason)")
                }
            }
            .padding(.horizontal, 12)
            .padding(.top, 8)
            .padding(.bottom, 10)
            .background(RecentOutputPalette.bg)
            .overlay(alignment: .top) {
                Rectangle()
                    .fill(RecentOutputPalette.line)
                    .frame(height: 1)
            }
            .accessibilityElement(children: .contain)
            .accessibilityLabel("Pinned prompt composer")
        }
    }
}

private struct RecentOutputRowView: View {
    let row: RecentOutputRow
    @ObservedObject var model: AppModel
    let agent: Agent
    let previousBlock: TranscriptBlock?

    var body: some View {
        switch row {
        case .block(let block):
            if RecentOutputRender.isDividerRun(block.text) {
                // #253 fallback: residual TUI furniture renders as a real
                // divider (a thin rule), not as dash-run text.
                Divider()
                    .overlay(RecentOutputPalette.line)
                    .padding(.vertical, 2)
                    .accessibilityHidden(true)
            } else {
                RecentBlockRow(
                    block: block,
                    showSpeaker: previousBlock?.kind != block.kind,
                    showTimestamp: RecentOutputRender.isBoundary(
                        previous: previousBlock,
                        current: block))
            }
        case .loadEarlier:
            EmptyView()
        case .error:
            HStack {
                Text("Couldn’t load earlier output")
                    .font(.caption)
                    .foregroundStyle(RecentOutputPalette.muted)
                Spacer()
                Button("Retry") {
                    model.loadEarlierOutput(agentId: agent.agentId)
                }
                .font(.caption)
                .foregroundStyle(RecentOutputPalette.accent)
                .accessibilityLabel("Retry loading earlier output")
            }
            .accessibilityElement(children: .combine)
        case .loading:
            HStack(spacing: 6) {
                ProgressView()
                    .controlSize(.small)
                    .tint(RecentOutputPalette.accent)
                Text("Loading earlier…")
                    .font(.caption)
                    .foregroundStyle(RecentOutputPalette.muted)
            }
        }
    }
}

private struct RecentBlockRow: View {
    let block: TranscriptBlock
    let showSpeaker: Bool
    let showTimestamp: Bool
    @State private var expanded: Bool

    init(block: TranscriptBlock, showSpeaker: Bool, showTimestamp: Bool) {
        self.block = block
        self.showSpeaker = showSpeaker
        self.showTimestamp = showTimestamp
        _expanded = State(initialValue: block.kind == .tool
                          && RecentOutputRender.isCodeOrDiff(block.text))
    }

    var body: some View {
        HStack(alignment: .top, spacing: 8) {
            speakerRail
                .frame(width: 58, alignment: .topLeading)
            Group {
                if rendersHighlighted {
                    highlightedMessage
                        .accessibilityElement(children: .combine)
                        .accessibilityLabel(RecentOutputRender.accessibilityLabel(block))
                } else {
                    switch block.kind {
                    case .user:
                        userMessage
                            .accessibilityElement(children: .combine)
                            .accessibilityLabel(RecentOutputRender.accessibilityLabel(block))
                    case .agent:
                        agentMessage
                            .accessibilityElement(children: .combine)
                            .accessibilityLabel(RecentOutputRender.accessibilityLabel(block))
                    case .tool, .system, .unknown:
                        toolMessage
                    }
                }
            }
            .frame(maxWidth: .infinity, alignment: .leading)
        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }

    private var speakerRail: some View {
        VStack(alignment: .leading, spacing: 2) {
            Circle()
                .fill(roleColor)
                .frame(width: 7, height: 7)
                .padding(.top, 5)
            if showSpeaker {
                Text(block.kind == .tool ? "▸" : roleLabel)
                    .font(.caption2.weight(.bold))
                    .foregroundStyle(roleColor)
                    .lineLimit(1)
            }
        }
    }

    private var rendersHighlighted: Bool {
        RecentOutputRender.codeLines(for: block).contains { $0.isHighlighted }
    }

    private var highlightedMessage: some View {
        VStack(alignment: .leading, spacing: 2) {
            ForEach(Array(RecentOutputRender.codeLines(for: block).enumerated()),
                    id: \.offset) { item in
                RecentCodeLineView(line: item.element)
            }
        }
        .padding(8)
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(RecentOutputPalette.codeBg,
                    in: RoundedRectangle(cornerRadius: 6))
        .overlay(
            RoundedRectangle(cornerRadius: 6)
                .stroke(RecentOutputPalette.codeLine, lineWidth: 1))
        .textSelection(.enabled)
    }

    private var userMessage: some View {
        HStack {
            Spacer(minLength: 24)
            messageLines
                .padding(10)
                .background(RecentOutputPalette.userTint,
                            in: RoundedRectangle(cornerRadius: 10))
                .textSelection(.enabled)
        }
    }

    private var agentMessage: some View {
        messageLines
            .textSelection(.enabled)
    }

    private var toolMessage: some View {
        DisclosureGroup(isExpanded: $expanded) {
            VStack(alignment: .leading, spacing: 2) {
                ForEach(Array(RecentOutputRender.codeLines(for: block).enumerated()),
                        id: \.offset) { item in
                    RecentCodeLineView(line: item.element)
                }
            }
            .padding(8)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(RecentOutputPalette.codeBg,
                        in: RoundedRectangle(cornerRadius: 6))
            .overlay(
                RoundedRectangle(cornerRadius: 6)
                    .stroke(RecentOutputPalette.codeLine, lineWidth: 1))
            .textSelection(.enabled)
        } label: {
            HStack(spacing: 6) {
                Text(RecentOutputRender.toolSummary(block.text))
                    .font(.caption2.monospaced())
                    .foregroundStyle(RecentOutputPalette.ink)
                    .lineLimit(1)
                Spacer()
                if showTimestamp, let at = block.at {
                    Text(RecentOutputRender.timestamp(at))
                        .font(.caption2.monospaced())
                        .foregroundStyle(RecentOutputPalette.muted)
                        .accessibilityHidden(true)
                }
            }
        }
        .padding(8)
        .background(RecentOutputPalette.panel2,
                    in: RoundedRectangle(cornerRadius: 8))
        .overlay(
            RoundedRectangle(cornerRadius: 8)
                .stroke(RecentOutputPalette.line, lineWidth: 1))
        // This label is for the disclosure control only; its value/hint
        // preserve the expanded/collapsed toggle semantics.
        .accessibilityLabel(RecentOutputRender.disclosureAccessibilityLabel(block))
        .accessibilityValue(expanded ? "Expanded" : "Collapsed")
        .accessibilityHint(RecentOutputRender.disclosureAccessibilityHint)
    }

    private var roleHeader: some View {
        HStack(spacing: 6) {
            Text(roleLabel)
                .font(.caption2.weight(.bold))
                .foregroundStyle(roleColor)
            Spacer()
            if showTimestamp, let at = block.at {
                Text(RecentOutputRender.timestamp(at))
                    .font(.caption2.monospaced())
                    .foregroundStyle(RecentOutputPalette.muted)
                    .accessibilityHidden(true)
            }
        }
    }

    private var messageLines: some View {
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

    private var roleLabel: String {
        switch block.kind {
        case .user: return "you"
        case .agent: return "assistant"
        case .tool: return "tool"
        case .system: return "diagnostic"
        case .unknown: return "unknown activity"
        }
    }

    private var roleColor: Color {
        switch block.kind {
        case .user: return RecentOutputPalette.userBlue
        case .agent: return RecentOutputPalette.ink
        case .tool: return RecentOutputPalette.accent
        case .system: return RecentOutputPalette.muted
        case .unknown: return RecentOutputPalette.muted
        }
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
            // Interactive content owns its own insets. Keeping padding out
            // here makes the rendered pinned header area identical to the
            // Button hit target instead of leaving dead outer margins.
            content()
                .font(.subheadline.weight(.semibold))
                .frame(maxWidth: .infinity, alignment: .leading)
                .background(.bar, ignoresSafeAreaEdges: [])
                .listRowInsets(EdgeInsets())
        } else {
            content()
                .font(.subheadline.weight(.semibold))
                .padding(.horizontal, 20)
                // #245 (approved Variant 1): header→first-row padding cut
                // roughly in half.
                .padding(.vertical, 3)
                .frame(maxWidth: .infinity, alignment: .leading)
                .background(.bar, ignoresSafeAreaEdges: [])
                .listRowInsets(EdgeInsets())
        }
    }
}

// MARK: - Fleet list

/// Full-width disclosure control for the low-priority bucket. The visible
/// Expanded/Collapsed value complements the chevron and is also exposed as
/// the accessibility value, so the state is clear without relying on shape
/// or color.
enum IdleDoneHeaderLayout {
    static let horizontalPadding: CGFloat = 20
    /// #245 (approved Variant 1): tightened vertical spacing; the 44 pt
    /// minimum hit height below still keeps the disclosure tappable.
    static let verticalPadding: CGFloat = 4
    static let minimumHitHeight: CGFloat = 44
}

struct IdleDoneHeader: View {
    let count: Int
    let isExpanded: Bool
    let onToggle: () -> Void

    var body: some View {
        Button {
            withAnimation { onToggle() }
        } label: {
            HStack(spacing: 8) {
                // #245: lowercase section headers (approved Variant 1).
                Text("idle / finished (\(count))")
                Text(isExpanded ? "Expanded" : "Collapsed")
                    .font(.caption.weight(.regular))
                    .foregroundStyle(.secondary)
                Spacer(minLength: 8)
                Image(systemName: isExpanded ? "chevron.down" : "chevron.right")
                    .font(.caption2)
                    .accessibilityHidden(true)
            }
            .padding(.horizontal, IdleDoneHeaderLayout.horizontalPadding)
            .padding(.vertical, IdleDoneHeaderLayout.verticalPadding)
            // This frame is the entire interactive pinned-header surface;
            // PinnedHeader does not add an external padded margin for it.
            .frame(maxWidth: .infinity, minHeight: IdleDoneHeaderLayout.minimumHitHeight,
                   alignment: .leading)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .frame(maxWidth: .infinity, alignment: .leading)
        .contentShape(Rectangle())
        .accessibilityLabel("Idle and finished agents")
        .accessibilityValue("\(count) agents, \(isExpanded ? "Expanded" : "Collapsed")")
        .accessibilityHint(isExpanded ? "Double tap to collapse" : "Double tap to expand")
    }
}

struct FleetView: View {
    @ObservedObject var model: AppModel

    var body: some View {
        // Compute the filter-chip list and agent projection once per body
        // evaluation; consumed by the top bar, the `.onChange` chip
        // reconciliation, and the sectioned list (review N7).
        let agents = Array(model.fleet.agents.values)
        let chips = BoardFilter.chips(for: agents)
        return NavigationStack(path: $viewState.navigationPath) {
            List {
                // Issue #219: the filter chrome is now the FIRST section of
                // the same physical scroll surface (a pinned header) instead
                // of a `.safeAreaInset` outside the list. During the pull
                // gesture the chrome, section headers, and rows therefore
                // translate as one unit — no stranded repo header and no
                // black gap. Normal scrolling keeps the chrome pinned under
                // the navigation bar.
                // #245 (approved Variant 1): the zero-height row is needed
                // only on a zero-agent board (the plain List would drop the
                // empty section's pinned header there). With agents present
                // the row is omitted — iOS 26 gives even a 0-height row its
                // minimum row height (a ~57 pt phantom band).
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
                        pinnedHeader(fillsInteractiveWidth: true) {
                            fleetChrome(chips: chips)
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
                    fleetList(agents: agents)
#endif
                case .live:
                    fleetList(agents: agents)
                }
            }
            // R2 status sections use the plain list style so each native
            // status header pins while scrolling.
            .listStyle(.plain)
            // #245 (approved Variant 1): inter-section gaps tightened from
            // the iOS-26 default spacing to the compact 12 pt rhythm.
            .listSectionSpacing(.compact)
            // Issue #219: native pull-to-refresh on the one physical
            // scroll surface. `refreshFleet` is coalesced and never
            // touches the SSE stream task.
            .refreshable {
                await model.refreshFleet()
            }
            .navigationTitle("Fleet")
#if DEBUG
            .task {
                applyDebugDemoRoute()
#if DEBUG
                if model.mode == .demo && CommandLine.arguments.contains("-corralTerminalPreview") {
                    try? await Task.sleep(for: .milliseconds(750))
                    terminalDemoPresented = true
                }
#endif
            }
            .onChange(of: model.demoDetailAgentId) { _, _ in
                applyDebugDemoRoute()
            }
            .onChange(of: model.mode) { _, _ in
                applyDebugDemoRoute()
            }
#endif
            .modifier(FleetSearchable(mode: model.mode, text: $searchText))
            // Issue #219: the chrome now lives INSIDE the list (first
            // pinned section) — see the List body above. No top
            // `.safeAreaInset`, so the pull gesture cannot strand the
            // filter-chip strip against a fixed surface.
            // R2-F: drop drafts for agents that left the snapshot. This
            // body (and the Set below) re-evaluates on fleet
            // snapshot/delta changes — the exact moments a prune can
            // matter — and not on keystrokes (see promptDrafts above).
            .onChange(of: Set(model.fleet.agents.keys)) { _, agentIds in
                promptDrafts.prune(to: agentIds)
                viewState.reconcile(availableAgentIds: agentIds)
            }
            // #166 review F9: reconcile the active chip against the derived
            // list — if the selected repo chip is no longer present, fall
            // back to `.all` instead of showing an empty board under a
            // selected-but-vanished chip.
            .onChange(of: chips) { _, chips in
                if !chips.contains(filterChip) {
                    filterChip = .all
                }
            }
            .navigationDestination(for: FleetRoute.self) { route in
                switch route {
                case .agent(let agentId):
                    AgentDetailView(agentId: agentId, model: model, drafts: promptDrafts)
                case .issues:
                    IssuesBrowserView(model: model)
                }
            }
            .toolbar {
                ToolbarItem(placement: .topBarTrailing) {
                    HStack(spacing: 10) {
                        // #267 approved entry point: a teal Issues button
                        // (icon + label) next to the slider menu. Gated by
                        // the device ledger grant (read_issues, default-
                        // empty) — the browser is never shown to an
                        // ungranted device.
                        if model.actionGrants.contains(.readIssues) {
                            Button {
                                viewState.openIssues()
                            } label: {
                                // Explicit HStack: SwiftUI toolbar items
                                // collapse a Label to its icon, which would
                                // drop the approved "Issues" label.
                                HStack(spacing: 4) {
                                    Image(systemName: "list.bullet.rectangle")
                                    Text("Issues")
                                }
                                .font(.caption.weight(.semibold))
                                .padding(.horizontal, 8)
                                .padding(.vertical, 5)
                                .background(Color.accentColor.opacity(0.15), in: Capsule())
                                .foregroundStyle(Color.accentColor)
                            }
                            .buttonStyle(.plain)
                            .accessibilityLabel("Issues")
                        }
#if DEBUG
                        if model.mode == .demo && model.actionGrants.contains(.attach) {
                            Button {
                                terminalDemoPresented = true
                            } label: {
                                Label("Terminal", systemImage: "terminal")
                            }
                            .accessibilityLabel("Terminal preview")
                        }
#endif
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
                        // #245 (approved Variant 1): manual refresh is
                        // removed — SSE + pull-to-refresh only. See the
                        // refresh hint in the chrome.
                        Button("Settings", systemImage: "gearshape") {
                            showSettings = true
                        }
                    } label: {
                        Image(systemName: "slider.horizontal.3")
                    }
                    }
                }
            }
            .sheet(isPresented: $showSettings) {
                SettingsView(model: model)
            }
#if DEBUG
            .sheet(isPresented: $terminalDemoPresented) {
                NavigationStack { TerminalAttachDemoView() }
            }
#endif
            .sheet(item: $answerTarget) { target in
                AnswerPromptSheet(agentId: target.agentId, model: model, drafts: promptDrafts)
            }
        }
    }

    @State private var showSettings = false
#if DEBUG
    @State private var terminalDemoPresented = false
#endif
    @State private var viewState = FleetViewState()
    @State private var expandedSupervisionIds: Set<String> = []
    /// #166 item 5: the active filter chip and the `.searchable` query over
    /// repo / branch / title / issue. Pure logic lives in `BoardFilter`.
    @State private var filterChip: BoardFilterChip = .all
    @State private var searchText = ""
    /// #166 item 3: agent id for the focused, keyboard-up answer sheet.
    @State private var answerTarget: AgentAnswerTarget?
    /// Per-agent prompt drafts (R2-B). Held in `@State`, DELIBERATELY not
    /// `@StateObject`: `@State` keeps the object's identity across renders
    /// but does not subscribe to `objectWillChange`, so keystrokes do not
    /// re-run this body. The rows observe the object (`@ObservedObject`)
    /// and re-render themselves.
    @State private var promptDrafts = PromptDrafts()

#if DEBUG
    /// Apply the opt-in launch route after the demo task has seeded the store.
    /// This is deliberately state-driven rather than a hidden tap target, so
    /// the design-gate command can reproduce the same detail from a clean app
    /// launch and normal users never enter it accidentally.
    private func applyDebugDemoRoute() {
        guard model.mode == .demo else { return }
        if CommandLine.arguments.contains("-corralTerminalPreview") {
            terminalDemoPresented = true
            return
        }
        // #267: the issues evidence route wins (it is the shallowest demo
        // destination; the detail route below targets an agent row).
        if model.demoOpenIssues {
            viewState.openIssues()
            return
        }
        guard let agentId = model.demoDetailAgentId,
              model.fleet.agent(agentId) != nil else { return }
        viewState.open(agentId: agentId)
        if promptDrafts.drafts[agentId]
            .map({ !$0.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty }) != true {
            promptDrafts.binding(for: agentId).wrappedValue = "Please verify the diff too."
        }
    }
#endif

    /// R2 status hierarchy: Needs you → Working → Supervising → Finished
    /// → Idle. Each non-empty group is a native List Section with a pinned
    /// header and count; query/filter mode remains a flat result section.
    /// The filter-chip row is no longer a header here — it lives in the
    /// List's first pinned section header (`fleetChrome`). The list is a
    /// plain `Group` of Sections so pinned headers and `.swipeActions`
    /// stay direct List children; durations tick themselves via
    /// `TimeInStateLabel` so the board is not re-rendered at 1 Hz
    /// (re-review P4).
    @ViewBuilder
    private func fleetList(agents: [Agent]) -> some View {
        if queryActive {
            filteredSection(agents: agents)
        } else {
            standardSections(agents: agents)
        }
    }

    /// #166 item 5: flat results while a chip is active or a search is typed.
    @ViewBuilder
    private func filteredSection(agents: [Agent]) -> some View {
        let filtered = BoardModel.ordered(BoardFilter.filtered(agents, chip: filterChip, query: searchText))
        Section {
            ForEach(filtered) { agent in
                agentRow(agent)
            }
        } header: {
            pinnedHeader {
                Text("\(filterHeaderLabel) (\(filtered.count))")
                    .accessibilityLabel("\(filterHeaderLabel) (\(filtered.count))")
            }
        }
    }

    /// Normal R2 status hierarchy. Empty groups are omitted so section counts
    /// describe exactly the rows currently on the board.
    @ViewBuilder
    private func standardSections(agents: [Agent]) -> some View {
        ForEach(BoardModel.presentationSections(for: agents)) { section in
            Section {
                ForEach(section.agents) { agent in
                    agentRow(agent)
                }
            } header: {
                pinnedHeader {
                    Text(section.header)
                        .accessibilityLabel(section.header)
                }
            }
        }
    }

    /// The filter-chip row (`All · Needs you · repo₁…repoₙ`). Rendered inside
    /// the List's first pinned section header (`fleetChrome`) — a pinned
    /// plain-list header, so it truly stays on screen while scrolling yet
    /// moves with the same scroll surface during the pull gesture (issue
    /// #219).
    @ViewBuilder
    private func filterChipRow(chips: [BoardFilterChip]) -> some View {
        ScrollView(.horizontal, showsIndicators: false) {
            HStack(spacing: 6) {
                ForEach(chips, id: \.self) { chip in
                    Button {
                        withAnimation { filterChip = chip }
                    } label: {
                        Text(chip.label)
                            .font(.caption.weight(.semibold))
                            .padding(.horizontal, 10)
                            .padding(.vertical, 5)
                            .background(chip == filterChip
                                        ? Color.accentColor
                                        : Color.secondary.opacity(0.12),
                                        in: Capsule())
                            .foregroundStyle(chip == filterChip ? Color.white : Color.primary)
                    }
                    .buttonStyle(.plain)
                    .accessibilityLabel(chip.label)
                    .accessibilityAddTraits(chip == filterChip ? .isSelected : [])
                }
            }
            .padding(.horizontal, 20)
        }
    }

    /// True when the flat search/filter projection should replace sections.
    private var queryActive: Bool {
        filterChip != .all || !searchText.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
    }

    /// #166 item 5: flat results while a chip is active or a search is typed.
    /// #245: the section header follows the approved lowercase style.
    private var filterHeaderLabel: String {
        if !searchText.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
            return "results"
        }
        // The chip label stays Title Case on the chip; its section header is
        // lowercase per the approved Variant 1. Repo names are data — never
        // transformed.
        return filterChip == .needsYou ? "needs you" : filterChip.label
    }

    @ViewBuilder
    private func pinnedHeader<Content: View>(fillsInteractiveWidth: Bool = false,
                                             @ViewBuilder content: @escaping () -> Content) -> some View {
        PinnedHeader(fillsInteractiveWidth: fillsInteractiveWidth, content: content)
    }

    /// Issue #219: the persistent filter chrome, now the first pinned
    /// section header of the List. In live mode it always shows the
    /// connection status (even with zero blocked agents, active filters,
    /// or search), then the always-visible filter-chip row plus the
    /// #245 pull-to-refresh hint (the manual refresh button was removed —
    /// SSE + pull-to-refresh only). Demo mode skips the connection line
    /// (there is no stream) but keeps chips and the hint. PinnedHeader
    /// supplies the `.bar` background, so the strip stays legible over
    /// scrolling rows.
    @ViewBuilder
    private func fleetChrome(chips: [BoardFilterChip]) -> some View {
#if DEBUG
        if CommandLine.arguments.contains("-corralStatusPresentation") {
            statusPresentationChrome
        } else {
            standardFleetChrome(chips: chips)
        }
#else
        standardFleetChrome(chips: chips)
#endif
    }

    @ViewBuilder
    private func standardFleetChrome(chips: [BoardFilterChip]) -> some View {
        VStack(alignment: .leading, spacing: 0) {
            if model.mode == .live {
                connectionStatusLine
            }
            filterChipRow(chips: chips)
            refreshHintLine
        }
    }

#if DEBUG
    /// Compact synthetic R2 capture chrome mirrors the approved status chips
    /// without changing the live repo-filter controls.
    @ViewBuilder
    private var statusPresentationChrome: some View {
        let sections = BoardModel.presentationSections(for: Array(model.fleet.agents.values))
        let groups: [PresentationGroup] = [.needsYou, .supervising, .finished]
        ScrollView(.horizontal, showsIndicators: false) {
            HStack(spacing: 6) {
                Text("All \(model.fleet.agents.count)")
                    .font(.caption.weight(.semibold))
                    .padding(.horizontal, 10)
                    .padding(.vertical, 5)
                    .background(Color.accentColor, in: Capsule())
                    .foregroundStyle(Color.white)
                    .accessibilityLabel("All \(model.fleet.agents.count)")
                ForEach(groups, id: \.self) { group in
                    if let section = sections.first(where: { $0.group == group }) {
                        Text("\(group.rawValue) \(section.agents.count)")
                            .font(.caption.weight(.semibold))
                            .padding(.horizontal, 10)
                            .padding(.vertical, 5)
                            .background(Color.secondary.opacity(0.12), in: Capsule())
                            .foregroundStyle(Color.primary)
                            .accessibilityLabel("\(group.rawValue) \(section.agents.count)")
                    }
                }
            }
            .padding(.horizontal, 20)
        }
    }
#endif

    /// #245 (approved Variant 1): the manual refresh button (chrome ⟳ and
    /// toolbar menu item) is removed. The pull gesture is the only refresh
    /// affordance; this hint line replaces the button and explains it.
    @ViewBuilder
    private var refreshHintLine: some View {
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

    /// Connection indicator line, modeled by `BoardModel.connectionStatus` so
    /// the label/spinner is a testable pure projection, independent of
    /// section emptiness, filters, and search.
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
            Text("offline")
                .font(.caption2)
                .foregroundStyle(.secondary)
                .frame(maxWidth: .infinity, alignment: .leading)
                .padding(.horizontal, 20)
                .padding(.top, 4)
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

    /// R2: supervising rows retain their native NavigationLink and gain an
    /// accessible DisclosureGroup for the structured activity evidence.
    @ViewBuilder
    private func agentRow(_ agent: Agent) -> some View {
        let answerAvailable = agent.isBlocked && promptAvailable(agent)
        let answerAction: (() -> Void)? = answerAvailable
            ? { answerTarget = AgentAnswerTarget(agentId: agent.agentId) }
            : nil
        if let activity = BoardModel.supervisionActivity(for: agent) {
            DisclosureGroup(isExpanded: supervisionBinding(for: agent.agentId)) {
                SupervisionEvidenceView(activity: activity)
            } label: {
                agentNavigationLink(agent, answerAvailable: answerAvailable, answerAction: answerAction)
            }
        } else {
            agentNavigationLink(agent, answerAvailable: answerAvailable, answerAction: answerAction)
        }
    }

    @ViewBuilder
    private func agentNavigationLink(
        _ agent: Agent,
        answerAvailable: Bool,
        answerAction: (() -> Void)?) -> some View {
        NavigationLink(value: FleetRoute.agent(agentId: agent.agentId)) {
            AgentRow(agent: agent,
                     onAnswer: answerAction,
                     stateEnteredAt: model.fleet.stateEnteredAt[agent.agentId])
        }
        .rowAccessibility(summary: rowSummary(agent), answerAction: answerAction)
        .accessibilityHint("Double tap to open agent details and actions")
        .swipeActions(edge: .leading, allowsFullSwipe: answerAvailable) {
            if answerAvailable {
                Button {
                    answerTarget = AgentAnswerTarget(agentId: agent.agentId)
                } label: {
                    Label("Answer", systemImage: "bubble.left.fill")
                }
                .tint(.blue)
                .accessibilityLabel("Answer the pending question")
            }
        }
    }

    private func supervisionBinding(for agentId: String) -> Binding<Bool> {
        Binding(
            get: { expandedSupervisionIds.contains(agentId) },
            set: { expanded in
                if expanded {
                    expandedSupervisionIds.insert(agentId)
                } else {
                    expandedSupervisionIds.remove(agentId)
                }
            })
    }

    /// #166 review F7: the row Answer affordance (button + leading swipe) is
    /// offered only when the device grant + agent capability allow `.prompt`.
    private func promptAvailable(_ agent: Agent) -> Bool {
        BoardModel.actionAvailability(agent: agent, grants: model.actionGrants)
            .first { $0.action == .prompt }?.isEnabled ?? false
    }

}

// MARK: - Conditional searchable (review F11)

/// `.searchable` is attached only for live/demo fleets, never on the
/// registration screen (an unregistered device sees no search bar).
private struct FleetSearchable: ViewModifier {
    let mode: AppModel.Mode
    @Binding var text: String

    @ViewBuilder
    func body(content: Content) -> some View {
        if mode == .needsSetup {
            content
        } else {
            content.searchable(text: $text,
                               placement: .navigationBarDrawer(displayMode: .always),
                               prompt: "Search repo / branch / issue…")
        }
    }
}

// MARK: - Focused answer sheet (#166 item 3)

/// Identifiable carrier for the answer sheet. FleetView sets this when the
/// row's "Answer" affordance is tapped, so the #166 sheet can be presented
/// with `sheet(item:)` (keyboard-up on the focused TextField).
private struct AgentAnswerTarget: Identifiable {
    let agentId: String
    var id: String { agentId }
}

/// The focused, keyboard-up prompt field for a blocked agent. Reuses the
/// existing `PromptDrafts` plumbing (the same per-agent draft shared by the
/// detail surface) and dispatches through `AppModel.drivePrompt` / the demo
/// path, exactly like the detail view.
private struct AnswerPromptSheet: View {
    let agentId: String
    @ObservedObject var model: AppModel
    @ObservedObject var drafts: PromptDrafts
    @Environment(\.dismiss) private var dismiss
    @FocusState private var focused: Bool
    @State private var refusalMessage: String?

    private var driveClient: DriveClient {
        model.makeDriveClient()
    }

    /// #166 review F7: the same availability gate the row uses. When the
    /// device has no `.prompt` grant (or the agent lacks the capability) the
    /// field/send are disabled and the reason is shown; if a refusal still
    /// happens, the typed draft is preserved.
    private var promptItem: AgentActionAvailability? {
        guard let agent = model.fleet.agent(agentId) else { return nil }
        return BoardModel.actionAvailability(agent: agent, grants: model.actionGrants)
            .first { $0.action == .prompt }
    }

    var body: some View {
        NavigationStack {
            VStack(alignment: .leading, spacing: 12) {
                if let agent = model.fleet.agent(agentId) {
                    Text("Answer \(agent.title ?? agent.displayName ?? agent.agentId)")
                        .font(.subheadline)
                        .foregroundStyle(.secondary)
                    if let waiting = agent.waitingOn {
                        Text(waiting.prompt)
                            .font(.body)
                            .padding(10)
                            .frame(maxWidth: .infinity, alignment: .leading)
                            .background(Color.secondary.opacity(0.08),
                                        in: RoundedRectangle(cornerRadius: 8))
                    }
                    TextField("Answer…", text: drafts.binding(for: agentId))
                        .textFieldStyle(.roundedBorder)
                        .focused($focused)
                        .disabled(promptItem?.isEnabled != true)
                        .onSubmit(send)
                    Button("Send Answer", action: send)
                        .buttonStyle(.borderedProminent)
                        .disabled(promptItem?.isEnabled != true
                                  || drafts.drafts[agentId]?
                                    .trimmingCharacters(in: .whitespacesAndNewlines).isEmpty != false)
                    if let promptItem, !promptItem.isEnabled {
                        Text(promptItem.disabledReason ?? "Prompt is not available.")
                            .font(.caption)
                            .foregroundStyle(.secondary)
                            .accessibilityLabel("Why answering is not available: \(promptItem.disabledReason ?? "Prompt is not available.")")
                    }
                    if let refusalMessage {
                        Text(refusalMessage)
                            .font(.caption)
                            .foregroundStyle(.orange)
                    }
                } else {
                    Label("Agent no longer available", systemImage: "exclamationmark.triangle")
                        .font(.headline)
                    Text("This agent was deleted or migrated. Refresh the fleet before sending an action.")
                        .foregroundStyle(.secondary)
                        .font(.subheadline)
                }
                Spacer()
            }
            .padding()
            .navigationTitle("Answer")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("Cancel") { dismiss() }
                }
            }
            .onAppear { focused = true }
        }
    }

    private func send() {
        let text = drafts.drafts[agentId] ?? ""
        guard !text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty else { return }
        guard let agent = model.fleet.agent(agentId) else { return }
        guard promptItem?.isEnabled == true else {
            refusalMessage = promptItem?.disabledReason
                ?? "This prompt is not available for this agent."
            return
        }
#if DEBUG
        if model.mode == .demo {
            model.driveDemo(capability: .prompt, agent: agent, choice: text)
            drafts.clear(agentId)
            dismiss()
            return
        }
#endif
        // Dispatch FIRST; clear + dismiss only when the drive was accepted.
        // A refused dispatch keeps the typed draft on the sheet (review F7).
        switch model.drivePrompt(agent: agent, text: text, driveClient: driveClient) {
        case .accepted:
            drafts.clear(agentId)
            dismiss()
        case .alreadyInFlight:
            refusalMessage = "Already sending this answer. Your draft was kept."
        case .refused(let reason):
            refusalMessage = "Not dispatched — \(reason ?? "The prompt was not dispatched."). Your draft was kept."
        }
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
#if DEBUG
        Section {
            Button("Try demo fleet (Debug only; no daemon)") {
                model.enterDemo()
            }
            .font(.subheadline)
            Text("Seeded fake fleet for local Debug/simulator testing only.")
                .font(.caption)
                .foregroundStyle(.secondary)
        } header: {
            PinnedHeader { Text("Demo") }
        }
#endif
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
                    LabeledContent("Name", value: UIDevice.current.name)
                    if let host = model.hostURL {
                        LabeledContent("Host", value: host.absoluteString)
                    }
                }
                DeviceAccessSection(model: model)
                Section("Security") {
                    Text("Writes are Ed25519-signed by the device key. Destructive payloads require Face ID step-up (X-Step-Up-Token).")
                        .font(.caption)
                        .foregroundStyle(.secondary)
                    Text("The host admin token below authorizes the Devices & Grants read/write surface only. It is never sent on the signed drive path.")
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

/// The #209 Devices / Grants surface on iOS: groups THIS DEVICE vs REMOTE
/// DEVICES (other machines), per-capability toggles that apply immediately,
/// and Revoke/Re-grant for remote devices — mirroring the approved #250
/// mockup's grouping and labels. Same host ledger as the board.
/// Navigation-only wrapper for the recovery destination. Mutations remain
/// inside the existing admin surface and are never triggered by this route.
struct DevicesGrantsView: View {
    @ObservedObject var model: AppModel

    var body: some View {
        NavigationStack {
            Form {
                DeviceAccessSection(model: model)
            }
            .navigationTitle("Devices & Grants")
            .navigationBarTitleDisplayMode(.inline)
        }
    }
}

struct DeviceAccessSection: View {
    @ObservedObject var model: AppModel
    @State private var adminTokenInput = ""

    var body: some View {
        Section {
            adminTokenRow
            if model.hasAdminToken {
                Button {
                    Task { await model.loadAdminDevices() }
                } label: {
                    Label("Refresh device list", systemImage: "arrow.clockwise")
                }
                .disabled(model.grantsLoading || model.grantsSaving)
                if model.grantsLoading || model.grantsSaving {
                    ProgressView().controlSize(.small)
                }
                if let notice = model.grantsNotice {
                    Text(notice).font(.caption).foregroundStyle(.red)
                }
                if let own = model.thisDevice {
                    DisclosureGroup {
                        deviceDetail(own, isSelf: true)
                    } label: {
                        deviceLabel(own, isSelf: true)
                    }
                }
                if !model.remoteDevices.isEmpty {
                    DisclosureGroup {
                        ForEach(model.remoteDevices) { device in
                            deviceDetail(device, isSelf: false)
                            Divider()
                        }
                    } label: {
                        Text("REMOTE DEVICES (other machines)")
                            .font(.caption.bold())
                    }
                } else {
                    Text("No remote devices registered.")
                        .font(.caption)
                        .foregroundStyle(.secondary)
                }
            }
        } header: {
            PinnedHeader { Text("Device access") }
        }
    }

    @ViewBuilder
    private var adminTokenRow: some View {
        if model.hasAdminToken {
            LabeledContent("Admin token", value: "✓ (Keychain)")
            Button("Clear admin token", role: .destructive) {
                model.clearAdminToken()
            }
            .font(.subheadline)
        } else {
            SecureField("Host admin token", text: $adminTokenInput)
                .textContentType(.password)
                .autocorrectionDisabled()
            Button {
                model.saveAdminToken(adminTokenInput)
                adminTokenInput = ""
                Task { await model.loadAdminDevices() }
            } label: {
                Text("Save token (Keychain)")
            }
            .disabled(adminTokenInput.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
            Text("Lists every registered device and lets you grant/revoke capabilities (host admin — same path as corrald-grant.sh).")
                .font(.caption)
                .foregroundStyle(.secondary)
        }
    }

    private func deviceLabel(_ device: AdminGrantDevice, isSelf: Bool) -> some View {
        HStack {
            VStack(alignment: .leading, spacing: 2) {
                HStack(spacing: 6) {
                    Text(deviceTitle(device)).font(.subheadline.bold())
                    if isSelf {
                        Text("THIS DEVICE").font(.caption2.bold()).foregroundStyle(.teal)
                    }
                    if device.revoked {
                        Text("REVOKED").font(.caption2.bold()).foregroundStyle(.red)
                    }
                }
                Text(shortKey(device.keyId)).font(.caption.monospaced()).foregroundStyle(.secondary)
            }
            Spacer()
            Text("\(device.grants.count) caps").font(.caption).foregroundStyle(.secondary)
        }
    }

    @ViewBuilder
    private func deviceDetail(_ device: AdminGrantDevice, isSelf: Bool) -> some View {
        VStack(alignment: .leading, spacing: 6) {
            Text("CAPABILITIES · \(device.grants.count) of \(Capability.allCases.count) granted")
                .font(.caption.bold())
            ForEach(Capability.allCases, id: \.self) { capability in
                HStack {
                    VStack(alignment: .leading, spacing: 1) {
                        Text(capability.rawValue).font(.caption.monospaced().bold())
                        Text(capability.grantDescription).font(.caption2).foregroundStyle(.secondary)
                    }
                    Spacer()
                    Toggle("", isOn: grantBinding(device, capability))
                        .labelsHidden()
                        .disabled(device.revoked || model.grantsSaving)
                }
            }
            if isSelf {
                Button("Refresh grants") {
                    Task { await model.refreshGrants() }
                }
                .font(.subheadline)
            } else if device.revoked {
                Button("Re-grant device") {
                    Task { await model.setDeviceRevoked(device.keyId, revoked: false) }
                }
                .font(.subheadline)
            } else {
                Button("Revoke device", role: .destructive) {
                    Task { await model.setDeviceRevoked(device.keyId, revoked: true) }
                }
                .font(.subheadline)
            }
        }
        .padding(.vertical, 2)
    }

    private func grantBinding(_ device: AdminGrantDevice, _ capability: Capability) -> Binding<Bool> {
        Binding(
            get: {
                model.adminDevices.first(where: { $0.keyId == device.keyId })?
                    .grants.contains(capability.rawValue) ?? false
            },
            set: { on in
                Task { await model.setDeviceCapability(capability.rawValue, enabled: on, deviceId: device.keyId) }
            }
        )
    }

    private func deviceTitle(_ device: AdminGrantDevice) -> String {
        if let name = device.name, !name.isEmpty { return name }
        return shortKey(device.keyId)
    }

    private func shortKey(_ keyId: String) -> String {
        let bare = keyId.hasPrefix("dev_") ? String(keyId.dropFirst(4)) : keyId
        if bare.count > 12 {
            return "dev_\(bare.prefix(8))…\(bare.suffix(4))"
        }
        return "dev_\(bare)"
    }
}

/// #232: the worktree-diff sheet (approved 232-diff-page layout: header
/// (repo · branch, agent, state chip) → one-line diffstat → changed-files
/// list → paged unified diff, lazy-loaded like the transcript). The first
/// page loads on appear; "Load next 200 lines" appends the daemon's next
/// bounded page. Read-only: there is no write path in this sheet.
struct AgentDiffSheet: View {
    let agent: Agent
    @ObservedObject var model: AppModel
    @Environment(\.dismiss) private var dismiss

    private var pane: DiffPane? { model.fleet.diffs[agent.agentId] }

    var body: some View {
        NavigationStack {
            Group {
                if let pane, pane.hasLoadedContent {
                    content(pane)
                } else {
                    ProgressView("Loading diff…")
                        .accessibilityLabel("Loading the worktree diff")
                        .onAppear {
                            // Explicit tap only — no auto-prefetch elsewhere.
                            model.driveReadDiff(agent: agent,
                                                driveClient: model.makeDriveClient())
                        }
                }
            }
            .navigationTitle(agent.workspace.branch ?? agent.agentId)
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("Done") {
                        model.cancelReadDiff(agentId: agent.agentId)
                        dismiss()
                    }
                }
            }
        }
        .presentationDetents([.large])
        .onDisappear {
            model.cancelReadDiff(agentId: agent.agentId)
        }
    }

    @ViewBuilder
    private func content(_ pane: DiffPane) -> some View {
        List {
            Section {
                VStack(alignment: .leading, spacing: 6) {
                    LabeledContent("Agent", value: agent.title ?? agent.agentId)
                    if let repo = pane.repo ?? agent.workspace.repo {
                        LabeledContent("Repo", value: repo)
                    }
                    if let branch = pane.branch ?? agent.workspace.branch {
                        LabeledContent("Branch", value: branch)
                    }
                    LabeledContent("Diffstat", value: diffstatText(pane.stats))
                }
                .font(.subheadline)
            }
            if !pane.files.isEmpty {
                Section("Changed files") {
                    ForEach(Array(pane.files.enumerated()), id: \.offset) { _, file in
                        HStack {
                            Text(file.path)
                                .font(.system(.footnote, design: .monospaced))
                                .truncationMode(.middle)
                                .lineLimit(1)
                            Spacer()
                            Text("+\(file.adds)/−\(file.dels)")
                                .font(.system(.caption2, design: .monospaced))
                                .foregroundStyle(.secondary)
                        }
                    }
                    if pane.filesTruncated {
                        Text("… more files")
                            .font(.footnote)
                            .foregroundStyle(.secondary)
                    }
                }
            }
            Section("Diff") {
                if let error = pane.error {
                    VStack(alignment: .leading, spacing: 8) {
                        Text(diffErrorText(error: error, kind: pane.errorKind,
                                           status: pane.errorStatus))
                            .font(.footnote)
                            .foregroundStyle(.red)
                            .accessibilityLabel("Diff failed: \(error)")
                        Button("Retry") {
                            model.driveReadDiff(agent: agent,
                                                driveClient: model.makeDriveClient())
                        }
                        .font(.footnote.weight(.semibold))
                    }
                }
                if pane.lines.isEmpty && pane.error == nil {
                    Text("No changes in this worktree.")
                        .font(.footnote)
                        .foregroundStyle(.secondary)
                }
                ForEach(Array(pane.lines.enumerated()), id: \.offset) { _, line in
                    Text(line)
                        .font(.system(size: 11, design: .monospaced))
                        .foregroundStyle(diffColor(line))
                        .textSelection(.enabled)
                }
                if pane.hasMore, let next = pane.nextOffset {
                    Button {
                        model.driveReadDiffNext(agent: agent,
                                                driveClient: model.makeDriveClient())
                    } label: {
                        if pane.isLoading {
                            ProgressView()
                                .controlSize(.small)
                        } else {
                            Text("Load next 200 lines (offset \(next))")
                        }
                    }
                    .disabled(pane.isLoading)
                    .accessibilityLabel("Load the next 200 diff lines from offset \(next)")
                }
            }
        }
        .listStyle(.plain)
        .refreshable {
            model.driveReadDiff(agent: agent, driveClient: model.makeDriveClient())
        }
    }

    private func diffstatText(_ stats: DiffStatsWire) -> String {
        "+\(stats.adds)/−\(stats.dels) · \(stats.files) files"
    }

    private func diffErrorText(error: String, kind: String?, status: Int?) -> String {
        var text = kind.map { "\($0): " } ?? ""
        text += error
        if let status { text += " (HTTP \(status))" }
        return text
    }

    private func diffColor(_ line: String) -> Color {
        if line.hasPrefix("+") {
            return .green
        } else if line.hasPrefix("-") {
            return .red
        } else if line.hasPrefix("@") || line.hasPrefix("diff --git") || line.hasPrefix("index ") {
            return .secondary
        } else {
            return .primary.opacity(0.85)
        }
    }
}

private extension DiffPane {
    /// Distinct from `isEmpty`: a stored pane that already has content (or a
    /// failure) should render immediately instead of refetching on every
    /// appear.
    var hasLoadedContent: Bool {
        hasLoaded || !lines.isEmpty || !files.isEmpty || error != nil
    }
}

// MARK: - #267 read-only issue browser (approved V3: flat list + chips + inline detail)

/// The read-only GitHub issues browser: flat list (newest-first), open/
/// closed chip filter in the pinned chrome (open by default), and a
/// tap-to-expand INLINE detail (state + label pills, repo meta, body,
/// lazy comment reveal with the transcript-style divider, `▴ collapse`).
/// Grant-gated by the device ledger (read_issues, default-empty): the entry
/// button lives on the fleet screen toolbar.
struct IssuesBrowserView: View {
    @ObservedObject var model: AppModel

    @State private var filter: IssueFilter = .open
    @State private var expandedKey: IssueBrowserKey?
    /// #267 lazy comment reveal: per issue, how many of the daemon's
    /// newest-first window comments are shown (revealed in `commentChunk`
    /// chunks on "Load earlier").
    @State private var revealedComments: [IssueBrowserKey: Int] = [:]

    /// Identities one browser row (repo + number).
    struct IssueBrowserKey: Hashable {
        let repo: String
        let number: UInt64
    }

    private struct BrowserRowItem: Identifiable {
        let key: IssueBrowserKey
        let issue: GhIssueRef
        var id: String { "\(key.repo)#\(key.number)" }
    }

    private func key(_ issue: GhIssueRef) -> IssueBrowserKey {
        IssueBrowserKey(repo: issue.repo, number: issue.number)
    }

    var body: some View {
        let pane = model.fleet.issuesBrowser
        let rows = IssueBrowser.rows(pane.repos.flatMap { $0.value }, filter: filter)
        List {
            Section {
                if pane.isLoading && !pane.isEmpty {
                    HStack(spacing: 8) {
                        ProgressView()
                        Text("loading issues…")
                            .font(.caption)
                            .foregroundStyle(.secondary)
                    }
                    .listRowBackground(Color.clear)
                }
                if let error = pane.error {
                    HStack(spacing: 8) {
                        Image(systemName: "exclamationmark.triangle")
                            .foregroundStyle(.orange)
                        Text(error)
                            .font(.caption)
                            .foregroundStyle(.secondary)
                        Button("Retry") { model.driveReadIssues(driveClient: model.makeDriveClient()) }
                            .font(.caption.weight(.semibold))
                            .tint(Color.accentColor)
                    }
                    .listRowBackground(Color.clear)
                }
                if !pane.isLoading, pane.error == nil, rows.isEmpty {
                    Text(emptyCopy)
                        .font(.caption)
                        .foregroundStyle(.secondary)
                        .listRowBackground(Color.clear)
                }
                ForEach(rows.map { BrowserRowItem(key: key($0), issue: $0) }) { item in
                    IssueBrowserRow(issue: item.issue,
                                    isExpanded: expandedKey == item.key,
                                    revealed: revealedComments[item.key] ?? 0,
                                    onToggle: {
                                        withAnimation {
                                            if expandedKey == item.key {
                                                expandedKey = nil
                                            } else {
                                                expandedKey = item.key
                                            }
                                        }
                                    },
                                    onLoadEarlier: {
                                        let newCount = (revealedComments[item.key] ?? 0)
                                            + IssueBrowser.commentChunk
                                        revealedComments[item.key] = newCount
                                    })
                }
            } header: {
                // #219-grade chrome: the filter chips ride the SAME scroll
                // surface (pinned header), so the pull gesture never strands
                // the chips against a fixed surface.
                PinnedHeader(fillsInteractiveWidth: true) {
                    browserChrome
                }
            }
        }
        .listStyle(.plain)
        .listSectionSpacing(.compact)
        .navigationTitle("Issues")
        .navigationBarTitleDisplayMode(.inline)
        .refreshable {
            model.driveReadIssues(driveClient: model.makeDriveClient())
        }
        .task {
            // First open: fetch once (refreshable + retry cover the rest).
            if model.fleet.issuesBrowser.isEmpty {
                model.driveReadIssues(driveClient: model.makeDriveClient())
            }
            // #267 DEBUG evidence route: auto-expand the requested issue's
            // inline detail (with one reveal chunk) so the design-gate
            // command can reproduce the annotated proof. Runs AFTER the
            // (synchronous demo) seed above.
#if DEBUG
            applyDemoIssueRoute()
#endif
        }
    }

#if DEBUG
    /// #267: auto-expand `demoOpenIssueNumber`'s row (no-op in live mode
    /// where the flag is nil). Hoisted into a method: the Release swiftc
    /// pass cannot resolve the observed-object dynamic member from a
    /// nested escaping closure.
    private func applyDemoIssueRoute() {
        guard let number = model.demoOpenIssueNumber else { return }
        let seeded = IssueBrowser.rows(model.fleet.issuesBrowser.repos.flatMap { $0.value },
                                       filter: filter)
        guard let issue = seeded.first(where: { $0.number == number }) else { return }
        expandedKey = key(issue)
        revealedComments[key(issue)] = IssueBrowser.commentChunk
    }
#endif

    private var emptyCopy: String {
        filter == .open ? "no open issues — all clear"
                        : "no closed issues in this view"
    }

    /// Pinned chrome: open/closed chips + the read-only subline (approved
    /// V3 copy, device-neutral).
    private var browserChrome: some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack(spacing: 6) {
                ForEach(IssueFilter.allCases, id: \.self) { candidate in
                    Button {
                        withAnimation { filter = candidate }
                    } label: {
                        Text(candidate.label)
                            .font(.caption.weight(.semibold))
                            .padding(.horizontal, 10)
                            .padding(.vertical, 5)
                            .background(candidate == filter
                                        ? Color.accentColor
                                        : Color.secondary.opacity(0.12),
                                        in: Capsule())
                            .foregroundStyle(candidate == filter ? Color.white : Color.primary)
                    }
                    .buttonStyle(.plain)
                    .accessibilityLabel(candidate.label)
                    .accessibilityAddTraits(candidate == filter ? .isSelected : [])
                }
            }
            .padding(.horizontal, 20)
            Label("read-only · tap a row to expand · no mutations from this device",
                  systemImage: "info.circle")
                .font(.caption2)
                .foregroundStyle(.secondary)
                .labelStyle(.titleAndIcon)
                .padding(.horizontal, 20)
        }
        .padding(.top, 2)
        .padding(.bottom, 6)
    }
}

/// One browser row: `#N  title  STATE-pill  ›` — or, when expanded, the
/// inline detail panel (label + state pills, repo meta, body, lazy
/// comments, `▴ collapse`) with the approved `--panel2` backing.
private struct IssueBrowserRow: View {
    let issue: GhIssueRef
    let isExpanded: Bool
    let revealed: Int
    let onToggle: () -> Void
    let onLoadEarlier: () -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            header
            if isExpanded {
                detail
                    .padding(.top, 6)
            }
        }
        .padding(.vertical, 8)
        .listRowBackground(isExpanded ? Color.secondary.opacity(0.16) : Color.clear)
        .listRowSeparator(isExpanded ? .hidden : .automatic)
    }

    private var header: some View {
        Button(action: onToggle) {
            HStack(spacing: 8) {
                Text("#\(issue.number)")
                    .font(.subheadline.weight(.bold))
                    .foregroundStyle(Color.accentColor)
                Text(issue.title)
                    .font(.subheadline)
                    .foregroundStyle(.primary)
                    .lineLimit(1)
                Spacer(minLength: 4)
                statePill
                chevron
            }
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .accessibilityLabel("Issue \(issue.number): \(issue.title)")
        .accessibilityAddTraits(isExpanded ? .isSelected : [])
    }

    private var chevron: some View {
        Image(systemName: "chevron.right")
            .font(.caption2)
            .foregroundStyle(isExpanded ? Color.accentColor : Color.secondary)
            .rotationEffect(.degrees(isExpanded ? 90 : 0))
    }

    private var statePill: some View {
        let open = issue.state.lowercased() == "open"
        let color = open ? Color.accentColor : Color.secondary
        return Text(open ? "OPEN" : "CLOSED")
            .font(.caption2.weight(.bold))
            .padding(.horizontal, 7)
            .padding(.vertical, 3)
            .overlay(Capsule().stroke(color, lineWidth: 1))
            .foregroundStyle(color)
            .opacity(open ? 1 : 0.65)
    }

    private var detail: some View {
        VStack(alignment: .leading, spacing: 8) {
            labelRow
            Text("\(issue.repo) · #\(issue.number)")
                .font(.caption)
                .foregroundStyle(.secondary)
            if let body = issue.body, !body.isEmpty {
                Text(body)
                    .font(.callout)
                    .foregroundStyle(.primary)
                    .textSelection(.enabled)
                    .fixedSize(horizontal: false, vertical: true)
            }
            commentSection
        }
        .padding(.horizontal, 10)
        .padding(.bottom, 2)
    }

    private var labelRow: some View {
        HStack(spacing: 6) {
            statePill
            ForEach(issue.labels, id: \.name) { label in
                issueLabelPill(label)
            }
        }
    }

    private func issueLabelPill(_ label: IssueLabel) -> some View {
        let color = Color(uiColor: UIColor(hex: "#" + label.color))
        return Text(label.name)
            .font(.caption2.weight(.semibold))
            .padding(.horizontal, 7)
            .padding(.vertical, 3)
            .background(color.opacity(0.25), in: Capsule())
            .foregroundStyle(color)
    }

    /// The transcript-style lazy comment paging (approved V3): the newest
    /// comments render first; `──── N earlier comments · Load earlier ────`
    /// reveals the rest of the daemon's bounded window chunk by chunk. When
    /// the window is exhausted the divider stays informative (no fake link).
    @ViewBuilder
    private var commentSection: some View {
        let earlier = IssueBrowser.earlierCount(issue, revealed: revealed)
        let comments = IssueBrowser.visibleComments(issue, revealed: revealed)
        if earlier > 0 {
            earlierDivider(earlier: earlier)
        }
        ForEach(comments, id: \.self) { comment in
            commentBlock(comment)
        }
        if !comments.isEmpty || earlier > 0 {
            collapseButton
        }
    }

    private func earlierDivider(earlier: Int) -> some View {
        HStack(spacing: 6) {
            dividerLine
            if IssueBrowser.canRevealMore(issue, revealed: revealed) {
                Button(action: onLoadEarlier) {
                    Text("\(earlier) earlier comments · Load earlier")
                        .font(.caption)
                        .foregroundStyle(Color.accentColor)
                }
                .buttonStyle(.plain)
            } else {
                Text("\(earlier) earlier comments")
                    .font(.caption)
                    .foregroundStyle(.secondary)
            }
            dividerLine
        }
        .padding(.vertical, 2)
    }

    private func commentBlock(_ comment: IssueComment) -> some View {
        VStack(alignment: .leading, spacing: 3) {
            HStack(spacing: 6) {
                Text(comment.author ?? "someone")
                    .font(.caption.weight(.semibold))
                Text(comment.createdAt.map(shortTime) ?? "")
                    .font(.caption2)
                    .foregroundStyle(.secondary)
            }
            Text(comment.body)
                .font(.caption)
                .foregroundStyle(.primary)
                .fixedSize(horizontal: false, vertical: true)
        }
    }

    private var collapseButton: some View {
        Button(action: onToggle) {
            Label("collapse", systemImage: "chevron.up")
                .font(.caption)
                .foregroundStyle(Color.accentColor)
        }
        .buttonStyle(.plain)
        .padding(.top, 2)
    }

    private var dividerLine: some View {
        Rectangle()
            .fill(Color.secondary.opacity(0.35))
            .frame(height: 1)
    }

    /// `2026-08-28T14:02:00Z` → `28 Aug, 14:02` (display-only).
    private func shortTime(_ iso: String) -> String {
        let formatter = ISO8601DateFormatter()
        guard let date = formatter.date(from: iso) else { return iso }
        let time = DateFormatter()
        time.dateFormat = "d MMM, HH:mm"
        time.timeZone = TimeZone(identifier: "UTC")
        return time.string(from: date)
    }
}
