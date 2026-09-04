import SwiftUI
import Combine

// MARK: - #354/#371 L2 read-only FleetNotifier
//
// Surfaces after the client cut:
// - Board (home): raw-herdr-status sections in the locked attention order
//   (blocked → working → idle → unknown; done only when herdr reports it),
//   each section grouping its rows into always-open REPO SUBGROUPS (#371:
//   alphabetical, Other last). Row = agent name · state chip · time-in-state
//   · repo chip · branch + small pane ref. NO search, NO actions.
// - Recents: tap a row → bottom sheet with the LIVE tail (auto-scroll,
//   ≤200-line daemon cap). No load-earlier, no Conversation/Harness
//   partition, no composer.
// - Settings: Appearance + connection pairing, device identity read-out,
//   notifications, and the How-to-connect sheet ('?' entry; auto-presented
//   on an unpaired first launch).
// Removed: Issues browser, Terminal, Diff, approval/prompt/attach/kill
// controls, device/grant admin.
//
// #372: every color below resolves through the environment `ThemeStore`
// (Catppuccin tokens). NO legacy GitHub-dark hex literals exist in this
// layer (audit gate in the #372 report).
// #371: state chips are tinted per state through the shared stateColor
// mapping; the working chip breathes three 4 pt squares (Reduce Motion =
// static teal dot); repo hues come from the shared RepoHue fnv1a32 % 8
// function on the filter-chip repo set.

// MARK: - Row state chrome

/// Self-ticking "· 4s" duration chip. It owns its own clock so a 1 Hz tick
/// re-renders only this small view, not the whole board.
private struct TimeInStateLabel: View {
    let agent: Agent
    var stateEnteredAt: UInt64? = nil
    @State private var now: UInt64 = UInt64(Date().timeIntervalSince1970 * 1000)
    @EnvironmentObject private var theme: ThemeStore

    var body: some View {
        if let durationText {
            Text("· \(durationText)")
                .font(.caption)
                .foregroundStyle(theme.subtext1)
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

// MARK: - #371 working-motion glyph + repo label chip

/// The approved working heartbeat as pure math: three 4 pt squares breathe
/// on a 1.2 s cycle — opacity 0.34 → 1 and scale 0.78 → 1, peaking at
/// 42 % of the cycle, then easing back — staggered 160 ms per square (the
/// prototype's `@keyframes breathe` verbatim). No rotation, translation,
/// or color change: a heartbeat, never a spinner. Testable in isolation so
/// the stagger + cycle cannot regress silently behind the view.
enum WorkingMotion {
    static let cycle: TimeInterval = 1.2
    static let stagger: TimeInterval = 0.16
    static let squareCount = 3
    /// Fraction of the cycle at the peak (0.42 in the design's keyframes).
    static let peakPhase = 0.42
    static let minOpacity = 0.34
    static let minScale: CGFloat = 0.78

    static func delay(for square: Int) -> TimeInterval {
        Double(square) * stagger
    }

    /// Normalized progress (0…<1) of one square's own cycle at `time`.
    static func phase(at time: TimeInterval, square: Int) -> Double {
        let shifted = time - delay(for: square)
        let wrapped = shifted.truncatingRemainder(dividingBy: cycle)
        return (wrapped >= 0 ? wrapped : wrapped + cycle) / cycle
    }

    /// The shared up-down ramp: 0 at rest, 1 at the cycle peak (42 %),
    /// 0 again at cycle end.
    static func ramp(at time: TimeInterval, square: Int) -> Double {
        let p = phase(at: time, square: square)
        return p <= peakPhase ? p / peakPhase : (1 - p) / (1 - peakPhase)
    }

    static func opacity(at time: TimeInterval, square: Int) -> Double {
        minOpacity + (1 - minOpacity) * ramp(at: time, square: square)
    }

    static func scale(at time: TimeInterval, square: Int) -> CGFloat {
        minScale + (1 - minScale) * CGFloat(ramp(at: time, square: square))
    }
}

/// The working-state chip glyph: three tiny squares breathing in stagger
/// (~1.2 s cycle) — or the static teal dot when Reduce Motion is on
/// (design lock: the squares are REMOVED, not paused, and no spinner
/// anywhere on the board).
struct WorkingMotionGlyph: View {
    let reduceMotion: Bool
    @EnvironmentObject private var theme: ThemeStore

    var body: some View {
        if reduceMotion {
            Circle()
                .fill(theme.stateColor(for: .working))
                .frame(width: 7, height: 7)
        } else {
            TimelineView(.animation(minimumInterval: 1.0 / 30.0)) { timeline in
                let t = timeline.date.timeIntervalSinceReferenceDate
                HStack(spacing: 3) {
                    ForEach(0..<WorkingMotion.squareCount, id: \.self) { index in
                        RoundedRectangle(cornerRadius: 1)
                            .fill(theme.stateColor(for: .working))
                            .frame(width: 4, height: 4)
                            .opacity(WorkingMotion.opacity(at: t, square: index))
                            .scaleEffect(WorkingMotion.scale(at: t, square: index))
                    }
                }
            }
        }
    }
}

/// The row's repo label chip (#371): a small capsule whose dot + border
/// echo the repo's deterministic hue (subgroup header uses the same hue),
/// with the repo name ALWAYS present (color is never the only channel).
/// The `Other` subgroup's rows carry an Other chip in the surface2 gray.
struct RepoLabelChip: View {
    /// `nil` renders the Other chip (no repo / unknown repo).
    let repo: String?
    /// The fleet repo set for deterministic hue assignment (the same list
    /// the filter chips and subgroup headers resolve against).
    var repos: [String] = []
    @EnvironmentObject private var theme: ThemeStore

    var body: some View {
        let name = repo ?? BoardModel.otherRepoLabel
        // An empty key is never in the fleet repo set, so the Other chip
        // resolves through the surface2 fallback — never an accent ring
        // hue (Other = gray, design lock).
        let hue = theme.repoHue(for: repo ?? "", among: repos)
        HStack(spacing: 5) {
            RoundedRectangle(cornerRadius: 2)
                .fill(theme.color(hue))
                .frame(width: 6, height: 6)
                .accessibilityHidden(true)
            Text(name)
                .font(.caption2.weight(.bold))
                .foregroundStyle(theme.repoInk(for: hue))
                .lineLimit(1)
        }
        .padding(.leading, 5)
        .padding(.trailing, 7)
        .padding(.vertical, 2)
        .background(theme.repoChipFill(for: hue), in: Capsule())
        .overlay(Capsule().stroke(theme.repoChipBorder(for: hue), lineWidth: 1))
        .accessibilityElement(children: .ignore)
        .accessibilityLabel(name)
    }
}


/// The board row (#354 L2, #371 board v2): a tinted per-state chip with the
/// raw state token + time-in-state, the agent name, the trailing small pane
/// reference + tool chip, and the repo · branch · basename line under it
/// (the repo renders as a colored label chip echoing its subgroup hue).
/// The whole row is a read-only tap target that opens the agent's recents
/// sheet — there are no action controls anywhere.
struct AgentRow: View {
    let agent: Agent
    /// #166 review F2: client-side state-entered wall clock, passed down
    /// from `FleetStore.stateEnteredAt` so a reason/title churn does not
    /// reset the duration. `nil` falls back to `agent.ts`.
    var stateEnteredAt: UInt64? = nil
    /// #371: the fleet repo set for deterministic row-chip hues (same list
    /// the filter chips + subgroup headers resolve against).
    var repos: [String] = []
    @Environment(\.dynamicTypeSize) private var dynamicTypeSize
    @ScaledMetric(relativeTo: .caption) private var badgeMinWidth: CGFloat = 84
    @EnvironmentObject private var theme: ThemeStore

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            if isAccessibilitySize {
                // Dynamic Type: stack the trailing chips under the title.
                VStack(alignment: .leading, spacing: 4) {
                    HStack(spacing: 6) {
                        stateChip
                        titleText
                    }
                    trailingChips
                }
            } else {
                HStack(spacing: 6) {
                    stateChip
                    titleText
                    Spacer(minLength: 0)
                    trailingChips
                }
            }
            WorkspaceLine(agent: agent, repos: repos)
        }
        .padding(.vertical, 2)
        .opacity(agent.state == .idle ? 0.65 : 1)
        .frame(maxWidth: .infinity, alignment: .leading)
    }

    // MARK: - Row subviews

    /// #371: the tinted per-state chip (#354 state chip — raw token glyph +
    /// label + time-in-state — now on the state's chip fill/border mix:
    /// working=teal, blocked=red, done=green, idle=subtext0,
    /// unknown=surface2 through the shared `stateColor` mapping; the chip
    /// surfaces resolve through the same single mapping).
    @ViewBuilder
    private var stateChip: some View {
        let stateColor = theme.stateColor(for: agent.state)
        HStack(spacing: 3) {
            stateGlyph(stateColor: stateColor)
            Text(stateStyle.label)
                .font(.caption.weight(.semibold))
                .foregroundStyle(stateColor)
                .accessibilityLabel(stateStyle.accessibilityLabel)
            TimeInStateLabel(agent: agent, stateEnteredAt: stateEnteredAt)
        }
        .lineLimit(1)
        .padding(.horizontal, 6)
        .padding(.vertical, 3)
        .frame(minWidth: badgeMinWidth, alignment: .leading)
        .background(theme.stateChipFill(for: agent.state),
                    in: RoundedRectangle(cornerRadius: 7))
        .overlay(RoundedRectangle(cornerRadius: 7)
            .stroke(theme.stateChipBorder(for: agent.state), lineWidth: 1))
        .fixedSize(horizontal: true, vertical: false)
        .layoutPriority(2)
    }

    /// The chip glyph: the shared raw-state mark for every state EXCEPT
    /// working, which shows the #371 heartbeat (three breathing squares, or
    /// the static teal dot under Reduce Motion — never a spinner).
    @ViewBuilder
    private func stateGlyph(stateColor: Color) -> some View {
        switch agent.state {
        case .working:
            WorkingMotionGlyph(reduceMotion: theme.reduceMotion)
        default:
            Text(stateStyle.glyph)
                .font(.caption.weight(.bold))
                .foregroundStyle(stateColor)
        }
    }

    @ViewBuilder
    private var titleText: some View {
        Text(agent.title ?? agent.displayName ?? agent.agentId)
            .font(.subheadline.weight(.semibold))
            .foregroundStyle(theme.text)
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
                    .foregroundStyle(theme.subtext1)
                    .lineLimit(1)
                    .fixedSize(horizontal: true, vertical: false)
                    .accessibilityLabel("Pane \(reference)")
            }
            Text(agent.tool)
                .font(.caption2.monospaced())
                .foregroundStyle(theme.subtext1)
                .lineLimit(1)
                .fixedSize(horizontal: true, vertical: false)
                .padding(.horizontal, 6)
                .padding(.vertical, 2)
                .background(theme.surface1.opacity(0.65),
                            in: RoundedRectangle(cornerRadius: 4))
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
    /// #371: the fleet repo set for deterministic hue assignment (the same
    /// list the filter chips + subgroup headers resolve against). The repo
    /// renders as a colored label chip on the line; orphan agents (repo
    /// nil) show the gray Other chip.
    var repos: [String] = []
    @EnvironmentObject private var theme: ThemeStore

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
            // #371: the repo is a colored label chip (hue dot + name,
            // echoing the subgroup header); orphans carry the Other chip —
            // the repo identity is never color-only.
            RepoLabelChip(repo: w.repo, repos: repos)
                .layoutPriority(SegmentPolicy.priority(for: .repo))
            if let branch = w.branch {
                Text("·").font(.caption2).foregroundStyle(theme.subtext1)
                Text(branch)
                    .font(.caption2.monospaced())
                    .foregroundStyle(theme.subtext1)
                    .lineLimit(1)
                    .truncationMode(SegmentPolicy.truncationMode(for: .branch))
                    .layoutPriority(SegmentPolicy.priority(for: .branch))
            }
            if let basename = Self.worktreeBasename(w) {
                Text("·").font(.caption2).foregroundStyle(theme.subtext1)
                Text(basename)
                    .font(.caption2.monospaced())
                    .foregroundStyle(theme.subtext1)
                    .lineLimit(1)
                    .truncationMode(SegmentPolicy.truncationMode(for: .basename))
                    .layoutPriority(SegmentPolicy.priority(for: .basename))
            }
            Spacer(minLength: 4)
            if let pr = w.prNumber {
                Text("#\(pr)")
                    .font(.caption2)
                    .foregroundStyle(theme.subtext1)
                    .layoutPriority(SegmentPolicy.priority(for: .badge))
            }
            if w.dirty {
                Text("dirty").font(.caption2.weight(.semibold))
                    .foregroundStyle(theme.peach)
                    .layoutPriority(SegmentPolicy.priority(for: .badge))
            }
            if w.ahead > 0 || w.behind > 0 {
                Text("↑\(w.ahead)↓\(w.behind)")
                    .font(.caption2.monospaced())
                    .foregroundStyle(theme.subtext1)
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

// MARK: - #364 A touch feedback + haptics

/// Light selection haptics for discrete board actions (agent row tap,
/// Done close). Deliberately NOT called from drag/scroll paths, so
/// repeated drags never tick. Simulators play nothing audible; the
/// on-device feel is a human verification.
enum Haptics {
    static func selection() {
        UISelectionFeedbackGenerator().selectionChanged()
    }
}

/// Immediate pressed state (dim + slight shrink) for board controls whose
/// plain buttons otherwise give no touch-down feedback: agent rows, repo
/// chips, the banner close. `isPressed` is true from touch-down to
/// up/cancel, so the feedback releases exactly when the touch ends.
struct BoardPressStyle: ButtonStyle {
    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .scaleEffect(configuration.isPressed ? 0.985 : 1)
            .opacity(configuration.isPressed ? 0.55 : 1)
            .contentShape(Rectangle())
    }
}

// MARK: - Pinned section header (R2-A)

/// The backing every pinned `.plain`-list section header gets. The chrome
/// strips are the palette's mantle step (#372 — token-backed, so pinned
/// headers follow the active Catppuccin flavor instead of a system
/// material). `listRowInsets` is zeroed so the backing spans the full row.
struct PinnedHeader<Content: View>: View {
    let fillsInteractiveWidth: Bool
    @ViewBuilder var content: () -> Content
    @EnvironmentObject private var theme: ThemeStore

    init(fillsInteractiveWidth: Bool = false,
         @ViewBuilder content: @escaping () -> Content) {
        self.fillsInteractiveWidth = fillsInteractiveWidth
        self.content = content
    }

    /// #372: pinned chrome strips use the palette's mantle step (the
    /// design's nav/chips chrome) instead of a system material, so the
    /// board chrome + section headers follow the active Catppuccin flavor.
    @ViewBuilder
    var body: some View {
        if fillsInteractiveWidth {
            content()
                .font(.subheadline.weight(.semibold))
                .foregroundStyle(theme.subtext1)
                .frame(maxWidth: .infinity, alignment: .leading)
                .background(theme.mantle, ignoresSafeAreaEdges: [])
                .listRowInsets(EdgeInsets())
        } else {
            content()
                .font(.subheadline.weight(.semibold))
                .foregroundStyle(theme.subtext1)
                .padding(.horizontal, 20)
                .padding(.vertical, 3)
                .frame(maxWidth: .infinity, alignment: .leading)
                .background(theme.mantle, ignoresSafeAreaEdges: [])
                .listRowInsets(EdgeInsets())
        }
    }
}

// MARK: - Fleet board (home)

struct FleetView: View {
    @ObservedObject var model: AppModel
    @EnvironmentObject private var theme: ThemeStore
    @State private var showSettings = false
    // #379: the How-to-connect sheet. Presented from the Settings '?' Help
    // button AND auto-presented over the board on an unpaired first launch
    // (fresh install); the DEBUG evidence driver opens the same binding.
    @State private var showConnectHelp = false
    /// #379: one-shot guard for the unpaired-launch auto-present. Fires at
    /// most ONCE per board lifetime (first launch while unpaired) — a
    /// deliberate device removal later must NOT re-pop the sheet.
    @State private var autoPresentedConnectHelp = false

    var body: some View {
        // #372: UI accent = the active flavor's mauve; the system color
        // scheme follows the flavor's light/dark axis so native chrome
        // rides the same axis as the palette. Applied INSIDE FleetView (not
        // on the app scene) so a live flavor flip re-evaluates this view
        // instead of tearing down the hosting tree — the recorded-evidence
        // driver's `.task` must survive a flavor change mid-sequence.
        let agents = Array(model.fleet.agents.values)
        // #364 B: chip set + effective filter are pure projections of the
        // CURRENT fleet — live counts, and a repo that vanished renders as
        // All without losing the user's last choice on the model.
        let chips = BoardModel.repoFilters(agents)
        let activeRepoFilter = BoardModel.reconcile(model.repoFilter,
                                                    against: chips)
        // #371: the deterministic repo-hue set is the filter-chip repo
        // list — chips, subgroup headers, and row chips all resolve the
        // same fnv1a32 % 8 assignment (RepoHue, consumed via ThemeStore).
        let repos = chips.map(\.repo)
        // #364 B2: the chips choose WHICH agents the #362 status sections
        // bucket; sections keep their locked order over the filtered set,
        // and #371 splits each section into repo subgroups.
        let sections = BoardModel.sections(
            BoardModel.agents(agents, in: activeRepoFilter))
        // #365: the top-bar chrome (.navigationTitle/.toolbar — the Settings
        // gear) renders only inside a navigation shell. The #354 cut deleted
        // the board's NavigationStack, orphaning those modifiers and leaving
        // the board with NO visible way into Settings; restore the shell.
        return NavigationStack {
            List {
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
                    repoChipsRow(chips: chips,
                                 total: agents.count,
                                 selection: activeRepoFilter)
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
                    boardSections(sections: sections, repos: repos)
#endif
                case .live:
                    boardSections(sections: sections, repos: repos)
                }
            }
            .listStyle(.plain)
            .listSectionSpacing(.compact)
            // #372: the board surface is the active flavor's base token —
            // rows ride transparent over it while the chrome strips use
            // mantle (PinnedHeader), matching the approved palette layering.
            // `.listRowBackground` keeps iOS 26 plain rows on the token
            // surface (system rows would paint white/black cards in the
            // forced light/dark scheme).
            .scrollContentBackground(.hidden)
            .background(theme.base)
            .listRowBackground(theme.base)
            // Issue #219: native pull-to-refresh on the one physical scroll
            // surface. `refreshFleet` is coalesced and never touches the SSE
            // stream task.
            .refreshable {
                await model.refreshFleet()
            }
            .navigationTitle("Fleet")
            // #365: Settings is an ALWAYS-VISIBLE top-bar control — a plain
            // gear Button (system gear shape, >=44 pt target, VoiceOver
            // label) opening the Settings sheet. The DEBUG demo toggle is
            // NOT on the main path: it lives in a secondary overflow menu
            // that exists only in Debug builds (Release shows the gear
            // alone).
            .toolbar {
                ToolbarItemGroup(placement: .topBarTrailing) {
#if DEBUG
                    Menu {
                        Button(model.mode == .demo ? "Exit demo" : "Demo mode",
                               systemImage: "sparkles") {
                            if model.mode == .demo {
                                model.exitDemo()
                            } else {
                                model.enterDemo()
                            }
                        }
                    } label: {
                        Image(systemName: "slider.horizontal.3")
                    }
                    .accessibilityLabel("Developer menu")
#endif
                    Button {
                        showSettings = true
                    } label: {
                        Image(systemName: "gearshape")
                    }
                    .accessibilityLabel("Settings")
                    .accessibilityHint("Opens connection and notification settings")
                    .frame(minWidth: 44, minHeight: 44)
                }
            }
            .sheet(isPresented: $showSettings) {
                SettingsView(model: model)
            }
            // #364 C: recents bottom sheet binds DIRECTLY to the model-owned
            // request — row taps, notification deep links, and the demo route
            // all funnel through `model.requestRecents`. Every request is a
            // fresh value, and the dismissal reconciler (`onDismiss`) clears
            // or re-arms it, so a first tap after ANY dismissal re-presents
            // (the old view-latched target + equality-guarded onChange
            // swallowed it).
            .sheet(item: $model.recentsRequest,
                   onDismiss: { model.recentsSheetDismissed() }) { request in
                RecentOutputSheet(agentId: request.agentId, model: model)
            }
            // #379: the How-to-connect sheet — the SAME shared content the
            // Settings '?' button presents from the Settings sheet. The
            // unpaired-launch auto-present drives this binding; the DEBUG
            // recorded-evidence driver opens it too (simctl cannot tap).
            .sheet(isPresented: $showConnectHelp) {
                HowToConnectSheet(host: model.hostURL?.absoluteString ?? "")
            }
            // #379: a fresh install (or any launch that finds the device
            // UNPAIRED) auto-presents the connect sheet once — first
            // launch guidance for the daemon-setup + pairing steps.
            .task(id: model.mode) {
                await autoPresentConnectHelpIfUnpaired()
            }
#if DEBUG
            .onChange(of: model.mode) { _, _ in
                applyDemoRouteIfNeeded()
            }
            .task {
                applyDemoRouteIfNeeded()
            }
            .task(id: model.mode) {
                await runDemoEvidenceIfNeeded()
            }
#endif
        }
        .tint(theme.accent)
        .preferredColorScheme(theme.flavor.isLight ? .light : .dark)
    }

    /// #364 B: the horizontal repo filter chip row ('All' + one chip per
    /// repo with the live agent count), rendered as the first board
    /// content row under the pinned chrome. Selecting a chip filters the
    /// agents the #362 status sections bucket; 'All' clears. Counts are
    /// always over the WHOLE fleet — filtering never re-zeroes the other
    /// chips.
    @ViewBuilder
    private func repoChipsRow(chips: [BoardModel.RepoFilterChip],
                              total: Int,
                              selection: String?) -> some View {
        let repos = chips.map(\.repo)
        return Section {
            ScrollView(.horizontal, showsIndicators: false) {
                HStack(spacing: 8) {
                    repoChipButton(label: "All", count: total,
                                   isSelected: selection == nil) {
                        model.repoFilter = nil
                    }
                    ForEach(chips) { chip in
                        repoChipButton(label: chip.repo, count: chip.count,
                                       isSelected: selection == chip.repo,
                                       repo: chip.repo, repos: repos) {
                            model.repoFilter = chip.repo
                        }
                    }
                }
                .padding(.horizontal, 20)
                .padding(.vertical, 4)
            }
            .listRowInsets(EdgeInsets())
            .listRowBackground(Color.clear)
            .listRowSeparator(.hidden)
        }
    }

    /// One filter chip: repo/All label + count badge, ≥44 pt hit target
    /// (#364 A3), visible selected state, VoiceOver label/value/selected
    /// trait. Press feedback comes from `BoardPressStyle`. #372 tokens: the
    /// selected chip fills with the palette accent (mauve) — never teal —
    /// and unselected chips carry the repo hue dot + surface-token chrome.
    private func repoChipButton(label: String, count: Int,
                                isSelected: Bool,
                                repo: String? = nil,
                                repos: [String] = [],
                                action: @escaping () -> Void) -> some View {
        Button(action: action) {
            HStack(spacing: 5) {
                if label != "All", let repo {
                    RoundedRectangle(cornerRadius: 2)
                        .fill(theme.repoHueColor(for: repo, among: repos))
                        .frame(width: 7, height: 7)
                        .accessibilityHidden(true)
                }
                Text(label)
                    .lineLimit(1)
                Text("\(count)")
                    .font(.caption.weight(.semibold))
                    .padding(.horizontal, 5)
                    .padding(.vertical, 1)
                    .background(isSelected ? theme.crust.opacity(0.22)
                                           : theme.surface2.opacity(0.30),
                                in: Capsule())
                    .accessibilityHidden(true)
            }
            .font(.subheadline.weight(.medium))
            .foregroundStyle(isSelected ? theme.crust : theme.subtext1)
            .padding(.horizontal, 12)
            .padding(.vertical, 6)
            .background(isSelected ? theme.accent : theme.base,
                        in: Capsule())
            .overlay(Capsule().stroke(
                isSelected ? theme.accent : theme.surface1, lineWidth: 1))
            .frame(minHeight: 44)
        }
        .buttonStyle(BoardPressStyle())
        .accessibilityElement(children: .ignore)
        .accessibilityLabel(label == "All" ? "All agents"
                                           : "Filter \(label)")
        .accessibilityValue(count == 1 ? "\(count) agent"
                                       : "\(count) agents")
        .accessibilityAddTraits(isSelected ? [.isSelected] : [])
    }

    /// The #371 board v2 renderer: one section per raw herdr status in the
    /// locked attention order (blocked → working → idle → unknown; a done
    /// section renders only when herdr reports it), each section split into
    /// always-open REPO SUBGROUPS (alphabetical, Other last) rendered as a
    /// tinted header band + its agent rows. Subgroups are NOT collapsible —
    /// no disclosure controls anywhere. `sections` arrives ALREADY filtered
    /// by the #364 B chip selection; `repos` is the fleet-wide repo set so
    /// subgroup + row hues match the filter chips' fnv1a32 % 8 assignment.
    @ViewBuilder
    private func boardSections(sections: BoardModel.Sections,
                               repos: [String]) -> some View {
        ForEach(sections.statuses) { status in
            Section {
                ForEach(status.subgroups) { subgroup in
                    repoSubgroupHeader(subgroup, repos: repos)
                    ForEach(subgroup.agents) { agent in
                        agentRow(agent, repos: repos)
                            // #372: iOS 26 plain lists paint their own row
                            // background unless each row opts into the token
                            // surface (a List-level `.listRowBackground` is not
                            // honored); rows ride the flavor's base.
                            .listRowBackground(theme.base)
                    }
                }
            } header: {
                PinnedHeader {
                    statusSectionHeader(status)
                }
            }
        }
    }

    /// The pinned status header: state-colored mark + raw status name +
    /// TOTAL count across the section's subgroups (#371: the total is the
    /// section's own count — it rescopes with the #364 B chip filter).
    @ViewBuilder
    private func statusSectionHeader(
        _ status: BoardModel.StatusSection) -> some View {
        HStack(spacing: 7) {
            RoundedRectangle(cornerRadius: 2)
                .fill(theme.stateColor(for: status.state))
                .frame(width: 8, height: 8)
                .accessibilityHidden(true)
            Text(status.header)
                .accessibilityLabel(status.header)
        }
    }

    /// One always-open repo subgroup band (#371): 2 pt hue rail + hue dot +
    /// repo name (label ink) + the subgroup's agent count. Renders as a
    /// plain non-collapsible row on the hue 9 %-over-mantle band; Other
    /// (gray surface2) sits last by construction in BoardModel.
    @ViewBuilder
    private func repoSubgroupHeader(_ subgroup: BoardModel.RepoSubgroup,
                                    repos: [String]) -> some View {
        // An empty lookup key is never in the fleet repo set, so the Other
        // subgroup resolves through the surface2 fallback (gray, design
        // lock) — never an accent-ring hue.
        let hue = theme.repoHue(for: subgroup.repo ?? "", among: repos)
        HStack(spacing: 7) {
            RoundedRectangle(cornerRadius: 2)
                .fill(theme.color(hue))
                .frame(width: 8, height: 8)
                .accessibilityHidden(true)
            Text(subgroup.displayName)
                .font(.caption.weight(.bold))
                .foregroundStyle(theme.repoInk(for: hue))
                .lineLimit(1)
            Spacer(minLength: 8)
            Text("\(subgroup.agents.count)")
                .font(.caption.weight(.semibold))
                .monospacedDigit()
                .foregroundStyle(theme.subtext1)
                .accessibilityHidden(true)
        }
        .padding(.horizontal, 20)
        .padding(.vertical, 4)
        .frame(maxWidth: .infinity, alignment: .leading)
        .overlay(alignment: .leading) {
            Rectangle()
                .fill(theme.color(hue))
                .frame(width: 2)
        }
        .accessibilityElement(children: .combine)
        .accessibilityLabel(subgroup.header)
        .listRowInsets(EdgeInsets())
        .listRowBackground(theme.repoBand(for: hue))
        .listRowSeparator(.hidden)
    }

    @ViewBuilder
    private func agentRow(_ agent: Agent, repos: [String]) -> some View {
        Button {
            // #364 A.2: a real row tap is a discrete action — one light
            // selection tick (drags that cancel never reach the action).
            model.requestRecents(for: agent.agentId, haptic: true)
        } label: {
            AgentRow(agent: agent,
                     stateEnteredAt: model.fleet.stateEnteredAt[agent.agentId],
                     repos: repos)
        }
        .buttonStyle(BoardPressStyle())
        .accessibilityElement(children: .combine)
        .accessibilityLabel(rowSummary(agent))
        .accessibilityHint("Double tap to open recent output")
    }

    /// Board chrome: connection status (live) + the pull-to-refresh hint.
    /// No search field; #364 B repo chips live in their own row below.
    @ViewBuilder
    private var boardChrome: some View {
        VStack(alignment: .leading, spacing: 0) {
            if model.mode == .live {
                connectionStatusLine
            }
            HStack(spacing: 4) {
                Image(systemName: "arrow.down")
                    .font(.caption2.weight(.semibold))
                    .foregroundStyle(theme.accent)
                    .accessibilityHidden(true)
                Text("pull to refresh · updates stream in automatically")
                    .font(.caption2)
                    .foregroundStyle(theme.subtext1)
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
                    .foregroundStyle(theme.subtext1)
            }
            .padding(.horizontal, 20)
            .padding(.top, 4)
        case .offline:
            Label("daemon offline — showing last-known board", systemImage: "wifi.slash")
                .font(.caption2)
                .foregroundStyle(theme.peach)
                .frame(maxWidth: .infinity, alignment: .leading)
                .padding(.horizontal, 20)
                .padding(.top, 4)
                .accessibilityLabel("daemon offline — showing last known board")
        case .error(let message):
            Text("⚠ \(message)")
                .font(.caption2)
                .foregroundStyle(theme.peach)
                .lineLimit(1)
                .frame(maxWidth: .infinity, alignment: .leading)
                .padding(.horizontal, 20)
                .padding(.top, 4)
        }
    }

    /// #379: auto-present the How-to-connect sheet when the board first
    /// appears on an UNPAIRED device (fresh install / first launch) — the
    /// one-shot guard keeps a later deliberate device removal from re-pop-
    /// ping it, and the mode gate keeps demo/paired launches quiet. The
    /// sleep lets the first frame settle so the sheet presents cleanly;
    /// mode is re-checked afterwards so a same-instant demo entry or
    /// registration cannot race the presentation.
    private func autoPresentConnectHelpIfUnpaired() async {
        guard !autoPresentedConnectHelp, model.mode == .needsSetup else { return }
        try? await Task.sleep(for: .milliseconds(700))
        guard !autoPresentedConnectHelp, model.mode == .needsSetup else { return }
        autoPresentedConnectHelp = true
        showConnectHelp = true
    }

#if DEBUG
    /// Deterministic evidence route: `-corralDemoDetail` opens the recents
    /// sheet for the featured demo agent right after seeding (simctl cannot
    /// inject the tap).
    private func applyDemoRouteIfNeeded() {
        guard model.mode == .demo,
              let agentId = model.demoDetailAgentId,
              model.fleet.agent(agentId) != nil,
              model.recentsRequest?.agentId != agentId else { return }
        Task { @MainActor in
            try? await Task.sleep(for: .milliseconds(500))
            guard model.mode == .demo,
                  model.fleet.agent(agentId) != nil else { return }
            model.requestRecents(for: agentId, haptic: false)
        }
    }

    /// #364 evidence drivers (launch-arg gated; never run otherwise). Each
    /// phase writes a marker file that the host-side screenshot script
    /// observes, so the recorded sequence is deterministic.
    private func runDemoEvidenceIfNeeded() async {
        if CorralDemoLaunch.wantsReopenEvidence(arguments: CommandLine.arguments) {
            await runReopenSequence()
        } else if CorralDemoLaunch.wantsFilterEvidence(arguments: CommandLine.arguments) {
            await runFilterSequence()
        } else if CorralDemoLaunch.wantsSettingsEvidence(arguments: CommandLine.arguments) {
            await runSettingsSequence()
        } else if CorralDemoLaunch.wantsConnectEvidence(arguments: CommandLine.arguments) {
            await runConnectSequence()
        } else if CorralDemoLaunch.wantsThemeEvidence(arguments: CommandLine.arguments) {
            await runThemeSequence()
        }
    }

    /// #364 C: open A → dismiss → open B → dismiss → open A in one
    /// recorded sequence. The driver calls the same model request path a
    /// row tap takes and clears the request the way a swipe/Done dismissal
    /// does — simctl cannot inject touches, so this is the synthetic
    /// stand-in for tap evidence (Guy: simulator evidence synthetic-only).
    /// Each phase settles (presentation/dismissal animation) before its
    /// marker, then holds so the host screenshot script has a stable frame.
    private func runReopenSequence() async {
        guard model.mode == .demo else { return }
        let agentA = DemoFleet.featuredAgentID
        let agentB = "herdr:demo-garden-blocked"
        guard model.fleet.agent(agentA) != nil,
              model.fleet.agent(agentB) != nil else { return }
        EvidenceMarkers.write("phase-1-board")
        try? await Task.sleep(for: .milliseconds(1500))
        model.requestRecents(for: agentA, haptic: false)
        try? await Task.sleep(for: .milliseconds(1500))
        EvidenceMarkers.write("phase-2-sheet-a")
        try? await Task.sleep(for: .milliseconds(1500))
        model.recentsRequest = nil
        try? await Task.sleep(for: .milliseconds(800))
        EvidenceMarkers.write("phase-3-board-after-a")
        try? await Task.sleep(for: .milliseconds(1500))
        model.requestRecents(for: agentB, haptic: false)
        try? await Task.sleep(for: .milliseconds(1500))
        EvidenceMarkers.write("phase-4-sheet-b")
        try? await Task.sleep(for: .milliseconds(1500))
        model.recentsRequest = nil
        try? await Task.sleep(for: .milliseconds(800))
        EvidenceMarkers.write("phase-5-board-after-b")
        try? await Task.sleep(for: .milliseconds(1500))
        // Same-agent reopen: the dismissal reconciler cleared the latch,
        // so this request is a fresh nil → request transition.
        model.requestRecents(for: agentA, haptic: false)
        try? await Task.sleep(for: .milliseconds(1500))
        EvidenceMarkers.write("phase-6-sheet-a-reopen")
        try? await Task.sleep(for: .milliseconds(1500))
        model.recentsRequest = nil
        try? await Task.sleep(for: .milliseconds(800))
        EvidenceMarkers.write("phase-7-done")
    }

    /// #364 B: chip evidence — All board, then the demo-atlas filter
    /// selected (chip highlighted, only demo-atlas agents across their
    /// status sections), then All again.
    private func runFilterSequence() async {
        guard model.mode == .demo else { return }
        EvidenceMarkers.write("phase-1-all")
        guard await themePause(1500) else { return }
        model.repoFilter = "demo-atlas"
        guard await themePause(700) else { return }
        EvidenceMarkers.write("phase-2-filtered-atlas")
        guard await themePause(1500) else { return }
        model.repoFilter = nil
        guard await themePause(700) else { return }
        EvidenceMarkers.write("phase-3-back-to-all")
        guard await themePause(1500) else { return }
        EvidenceMarkers.write("phase-4-done")
    }

    /// #365 evidence: the board with the always-visible Settings gear, then
    /// the Settings sheet open showing the connection host field. The
    /// driver flips the same `showSettings` state the gear's Button sets —
    /// simctl cannot inject the tap, so this is the synthetic stand-in.
    private func runSettingsSequence() async {
        guard model.mode == .demo else { return }
        EvidenceMarkers.write("phase-1-board")
        try? await Task.sleep(for: .milliseconds(1500))
        showSettings = true
        try? await Task.sleep(for: .milliseconds(1500))
        EvidenceMarkers.write("phase-2-settings-host-field")
        try? await Task.sleep(for: .milliseconds(1500))
        showSettings = false
        try? await Task.sleep(for: .milliseconds(800))
        EvidenceMarkers.write("phase-3-done")
        try? await Task.sleep(for: .milliseconds(1500))
    }

    /// #379 evidence: three frames from ONE deterministic launch on an
    /// UNPAIRED device (the app-level harness wiped any leftover identity
    /// first — see CorralDemoLaunch): (1) the auto-presented connect sheet
    /// over the fresh connect form — the REAL first-launch path, no driver
    /// action beyond waiting for the launch auto-present; (2) the Settings
    /// sheet whose Device section shows the identity read-out with NO
    /// grants list; (3) the shared HowToConnectSheet content (the same
    /// struct the Settings '?' button presents — simctl cannot tap that
    /// toolbar button, so the driver opens the board-level binding; the
    /// '?' wiring is source-pinned by the unit tests). The host row is
    /// seeded so step 2's Copy-host control is visible in its enabled
    /// state (simctl cannot type into the Host field); the copy action
    /// itself is pasteboard wiring, pinned by the unit tests.
    private func runConnectSequence() async {
        guard model.mode == .needsSetup else { return }
        guard await themePause(2000) else { return }
        EvidenceMarkers.write("phase-1-connect-autopresent")
        guard await themePause(4000) else { return }
        showConnectHelp = false
        guard await themePause(900) else { return }
        showSettings = true
        guard await themePause(1500) else { return }
        EvidenceMarkers.write("phase-2-settings-device-no-grants")
        guard await themePause(4000) else { return }
        showSettings = false
        guard await themePause(900) else { return }
        // SAFETY: fixed literal URL used only by the DEBUG evidence driver
        // to light up the Copy-host row for the phase-3 frame.
        model.hostURL = URL(string: "https://fleet.example.ts.net")!
        showConnectHelp = true
        guard await themePause(1500) else { return }
        EvidenceMarkers.write("phase-3-connect-sheet")
        guard await themePause(4000) else { return }
        showConnectHelp = false
        guard await themePause(900) else { return }
        EvidenceMarkers.write("phase-4-done")
        _ = await themePause(1500)
    }

    /// #372 evidence: Mocha default board → Settings (Appearance section)
    /// → flavor flips to Latte LIVE (sheet + board + rail all re-theme) →
    /// Latte board → Latte recents rail with the themed code/diff colors.
    /// The driver flips the same state the gear and the Appearance rows
    /// set — simctl cannot inject taps, so this is the synthetic stand-in.
    /// Holds are long so the host screenshot script lands INSIDE each
    /// phase (simulator frames can lag the marker stream on cold boots).
    private func runThemeSequence() async {
        guard model.mode == .demo else { return }
        // Cancellation-abort: if this task is ever cancelled mid-sequence
        // (scene teardown, demo exit), STOP instead of racing through the
        // remaining phases (a raced driver would flip the flavor and write
        // every marker instantly, corrupting the recorded evidence).
        guard await themePause(0) else { return }
        theme.setFlavor(.mocha)
        EvidenceMarkers.write("phase-1-board-mocha")
        guard await themePause(5000) else { return }
        showSettings = true
        guard await themePause(5000) else { return }
        EvidenceMarkers.write("phase-2-settings-mocha")
        guard await themePause(5000) else { return }
        theme.setFlavor(.latte)
        guard await themePause(5000) else { return }
        EvidenceMarkers.write("phase-3-settings-latte")
        guard await themePause(5000) else { return }
        showSettings = false
        guard await themePause(5000) else { return }
        EvidenceMarkers.write("phase-4-board-latte")
        guard await themePause(5000) else { return }
        model.requestRecents(for: DemoFleet.featuredAgentID, haptic: false)
        guard await themePause(5000) else { return }
        EvidenceMarkers.write("phase-5-recents-latte")
        guard await themePause(5000) else { return }
        model.recentsRequest = nil
        guard await themePause(3000) else { return }
        EvidenceMarkers.write("phase-6-done")
        _ = await themePause(5000)
    }

    /// Sleep that reports cancellation: `false` (and stops the caller) when
    /// the surrounding task was cancelled.
    private func themePause(_ milliseconds: Int64) async -> Bool {
        do {
            try await Task.sleep(for: .milliseconds(milliseconds))
            return true
        } catch {
            return false
        }
    }
#endif
}

#if DEBUG
/// #364 evidence: marker files the host screenshot script polls. Written
/// into the app's Documents/ux-evidence — never into the worktree.
enum EvidenceMarkers {
    static func write(_ name: String) {
        let fm = FileManager.default
        guard let docs = fm.urls(for: .documentDirectory,
                                 in: .userDomainMask).first else { return }
        let dir = docs.appendingPathComponent("ux-evidence",
                                              isDirectory: true)
        try? fm.createDirectory(at: dir, withIntermediateDirectories: true)
        try? (name + "\n").write(to: dir.appendingPathComponent(name + ".marker"),
                                 atomically: true, encoding: .utf8)
    }
}
#endif

// MARK: - Banner

struct BannerView: View {
    let banner: DriveBanner
    var dismiss: () -> Void
    @EnvironmentObject private var theme: ThemeStore

    var body: some View {
        HStack(spacing: 8) {
            Image(systemName: banner.isError ? "exclamationmark.triangle.fill" : "info.circle.fill")
                .foregroundStyle(banner.isError ? theme.red : theme.blue)
            Text("[\(banner.kind)] \(banner.message)")
                .font(.caption)
                .textSelection(.enabled)
            Spacer()
            Button { dismiss() } label: {
                Image(systemName: "xmark.circle.fill")
            }
            .buttonStyle(BoardPressStyle())
            .foregroundStyle(theme.subtext1)
            .accessibilityLabel("Dismiss banner")
        }
        .padding(8)
        .background(banner.isError ? theme.red.opacity(0.12) : theme.blue.opacity(0.12),
                    in: RoundedRectangle(cornerRadius: 8))
    }
}

// MARK: - Registration

struct RegistrationView: View {
    @ObservedObject var model: AppModel
    @EnvironmentObject private var theme: ThemeStore

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
                .foregroundStyle(theme.subtext1)
            if model.keyStorageWarning {
                Label("Keychain unavailable — the device key is stored in the plaintext in-app store. Use a device with Keychain support for production.",
                      systemImage: "exclamationmark.triangle.fill")
                    .font(.caption)
                    .foregroundStyle(theme.peach)
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
                .foregroundStyle(theme.subtext1)
        } header: {
            PinnedHeader { Text("Demo") }
        }
#endif
    }
}

// MARK: - Settings (Appearance, connection pairing, device identity, notifications, help)

/// #365: the surface behind the board's always-visible gear. The first
/// section is the CONNECTION pairing — host field + registration token —
/// which serves BOTH a fresh board (pair through Settings) and an
/// already-paired device re-pointing at a different host. Below it sit the
/// global notification pairing toggle and the DEVICE read-out.
///
/// #372: the FIRST section is Appearance — the ONLY theme control in the
/// whole app (placement lock: no picker on the board toolbar, the recents
/// header, or any tail top-right). Four Catppuccin flavor rows with the
/// locked swatch strips; selection persists through `ThemeStore`.
///
/// #379: the Device section is the post-cut identity read-out ONLY — Key
/// id, Keychain storage note, the read-only signed device label, the
/// paired/registration state, and the Remove action. The grants list and
/// every stale capability name are gone (the product grants nothing but
/// `read_tail`, out-of-band). The sheet's nav-bar '?' opens the shared
/// How-to-connect sheet (the same content the unpaired first launch
/// auto-presents over the board).
struct SettingsView: View {
    @ObservedObject var model: AppModel
    @Environment(\.dismiss) private var dismiss
    @EnvironmentObject private var theme: ThemeStore

    @State private var host: String
    @State private var token = ""
    @State private var registering = false
    /// #379: the Settings-header '?' Help entry — presents the SAME shared
    /// HowToConnectSheet the unpaired-launch auto-present shows over the
    /// board (each presentation site owns its binding; they are never both
    /// up at once).
    @State private var showConnectHelp = false

    /// #365: the host field opens pre-filled with the ACTIVE host so a
    /// paired device can re-point without retyping it; a fresh board falls
    /// back to the documented loopback default (the same default the
    /// RegistrationView connect section uses).
    init(model: AppModel) {
        self.model = model
        _host = State(initialValue: model.hostURL?.absoluteString
                                  ?? "127.0.0.1:8474")
    }

    var body: some View {
        NavigationStack {
            ScrollViewReader { proxy in
                Form {
                    appearanceSection
                    Section("Connection") {
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
                            .foregroundStyle(theme.subtext1)
                    }
                    Section {
                        // #379 evidence: the connect-evidence Settings
                        // frame scrolls this anchor into view (the Device
                        // read-out sits below Appearance + Connection and
                        // simctl cannot scroll the form).
                        LabeledContent("Key id", value: String((model.keyId ?? "—").prefix(16)))
                            .id("settings.device")
                        LabeledContent("Key storage",
                                       value: DeviceKeyStore.storageLocation == .keychain
                                           ? "Keychain" : "in-app store (⚠️ insecure)")
                        // #379: the grants list is gone — the product grants
                        // nothing but read_tail, provisioned out-of-band, so
                        // the Device section states the device's posture
                        // instead of enumerating capabilities.
                        Label("Read-only signed device", systemImage: "lock.shield")
                        LabeledContent("State", value: model.mode == .live ? "Paired" : "Not paired")
                        if model.keyStorageWarning {
                            Label("Keychain unavailable — the device key is stored in the plaintext in-app store. Use a device with Keychain support for production.",
                                  systemImage: "exclamationmark.triangle.fill")
                                .font(.caption)
                                .foregroundStyle(theme.peach)
                        }
                        // #379: Remove/revoke action — the pre-cut "Reset
                        // device identity" destructive action now lives in
                        // the Device section beside the identity it removes.
                        Button("Remove device", role: .destructive) {
                            model.resetDevice()
                            dismiss()
                        }
                    } header: {
                        Text("Device")
                    } footer: {
                        Text("Removing the device wipes its key and pairing from this phone — nothing is sent to the host.")
                            .foregroundStyle(theme.subtext1)
                    }
                    Section("Notifications") {
                        Toggle("State-change notifications",
                               isOn: Binding(
                                get: { model.notificationsEnabled },
                                set: { model.setNotificationsEnabled($0) }))
                        Text("Alerts when an agent starts, blocks, or finishes. No badges or catch-up.")
                            .font(.caption)
                            .foregroundStyle(theme.subtext1)
                    }
                }
                .navigationTitle("Settings")
                .toolbar {
                    // #379: Settings-header '?' Help entry — opens the same
                    // shared HowToConnectSheet the unpaired first launch
                    // auto-presents over the board.
                    ToolbarItem(placement: .topBarTrailing) {
                        Button {
                            showConnectHelp = true
                        } label: {
                            Image(systemName: "questionmark.circle")
                        }
                        .accessibilityLabel("How to connect")
                        .accessibilityHint("Opens daemon setup and device pairing steps")
                    }
                }
                // #372: the form surface follows the active flavor's base token
                // (native cells still provide the grouped chrome), and the
                // scheme is forced INSIDE the sheet so a live flavor flip
                // re-traits the presented form's system chrome too (an
                // app-level scheme change does not reach presented sheets).
                .scrollContentBackground(.hidden)
                .background(theme.base)
                .preferredColorScheme(theme.flavor.isLight ? .light : .dark)
                // #379: the '?' Help entry presents the shared connect sheet
                // from INSIDE this sheet's hierarchy (sheet-over-sheet needs
                // the inner presentation modifier in the presented tree).
                .sheet(isPresented: $showConnectHelp) {
                    HowToConnectSheet(host: host)
                }
#if DEBUG
                .task { await scrollDeviceIntoViewForConnectEvidence(proxy) }
#endif
            }
        }
    }

#if DEBUG
    /// #379 evidence: the connect-evidence Settings frame must show the
    /// Device identity read-out (grants list gone). Appearance + Connection
    /// push it below the fold on 390x844-class screens and simctl cannot
    /// scroll the form, so the DEBUG-only launch argument scrolls the
    /// Device anchor into view once the sheet settles.
    private func scrollDeviceIntoViewForConnectEvidence(_ proxy: ScrollViewProxy) async {
        guard CorralDemoLaunch.wantsConnectEvidence(arguments: CommandLine.arguments) else { return }
        try? await Task.sleep(for: .milliseconds(1000))
        guard CorralDemoLaunch.wantsConnectEvidence(arguments: CommandLine.arguments) else { return }
        withAnimation(.easeInOut(duration: 0.35)) {
            proxy.scrollTo("settings.device", anchor: .top)
        }
    }
#endif

    /// #372 Appearance: the ONLY theme picker (Settings-only placement
    /// lock). One row per Catppuccin flavor, locked order + swatch strips
    /// (base / surface1 / mauve / teal / red of THAT flavor), checkmark on
    /// the active row; selection persists.
    private var appearanceSection: some View {
        Section {
            ForEach(CatppuccinFlavor.allCases, id: \.self) { flavor in
                let selected = flavor == theme.flavor
                Button {
                    theme.setFlavor(flavor)
                } label: {
                    HStack(spacing: 12) {
                        FlavorSwatchStrip(flavor: flavor)
                        VStack(alignment: .leading, spacing: 2) {
                            Text(flavor.displayName)
                                .font(.subheadline.weight(.semibold))
                                .foregroundStyle(selected ? theme.accent
                                                          : theme.text)
                            Text(flavor.meta)
                                .font(.caption)
                                .foregroundStyle(theme.subtext1)
                        }
                        Spacer()
                        if selected {
                            Image(systemName: "checkmark")
                                .font(.subheadline.weight(.bold))
                                .foregroundStyle(theme.accent)
                                .accessibilityHidden(true)
                        }
                    }
                    .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .accessibilityElement(children: .combine)
                .accessibilityLabel("\(flavor.displayName), \(flavor.meta)")
                .accessibilityAddTraits(selected ? [.isSelected] : [])
            }
        } header: {
            Text("Appearance")
        } footer: {
            Text("Applies to the whole app — board, sheets, rail and settings.")
                .foregroundStyle(theme.subtext1)
        }
    }
}

/// The five-swatch flavor preview from the approved Appearance frame: the
/// flavor's base, surface1, accent (mauve), teal, and red — real tokens of
/// that palette, never fixed hexes. The strip previews a DIFFERENT flavor
/// than the active one, so it resolves that flavor's palette directly.
private struct FlavorSwatchStrip: View {
    let flavor: CatppuccinFlavor

    var body: some View {
        let palette = CatppuccinPalette.palette(for: flavor)
        HStack(spacing: 3) {
            ForEach([CatppuccinToken.base, .surface1, .mauve, .teal, .red],
                    id: \.self) { token in
                RoundedRectangle(cornerRadius: 3)
                    .fill(palette.color(token))
                    .frame(width: 14, height: 24)
            }
        }
        .accessibilityHidden(true)
    }
}

// MARK: - How to connect (#379)

/// #379: the in-app "How to connect" sheet — five numbered steps covering
/// daemon setup (launchd/one command + healthz, with the README Setup link),
/// reaching the daemon from the phone (Tailscale HTTPS serve URL / LAN, with
/// a copy-host control), pasting the host, registering with the pairing
/// token, and enabling state-change notifications. ONE shared content view
/// presented by BOTH entries: the Settings-header '?' Help button and the
/// unpaired-first-launch auto-present over the board.
///
/// `host` is the host the Copy control copies: the LIVE Connection field
/// text when the '?' button opens the sheet from Settings, and the active
/// registered host (empty on a fresh device) when the launch auto-present
/// shows it. An empty host disables the copy button and shows the setup
/// hint instead — nothing is copied that was never entered.
struct HowToConnectSheet: View {
    @Environment(\.dismiss) private var dismiss
    @EnvironmentObject private var theme: ThemeStore

    /// The host string offered for copy in step 2 (see type doc).
    let host: String

    var body: some View {
        NavigationStack {
            Form {
                Section {
                    Text("Run the daemon on your Mac first — then pair this phone with it. The full manual lives in the README (link below).")
                        .font(.subheadline)
                        .foregroundStyle(theme.subtext1)
                }
                Section {
                    Text("Install it under launchd (scripts/setup-corrald.sh, idempotent) or run it from a checkout with one command. Verify it answers:")
                        .font(.subheadline)
                        .foregroundStyle(theme.subtext1)
                    Text("curl -s http://127.0.0.1:8474/healthz   # → ok")
                        .font(.caption.monospaced())
                        .foregroundStyle(theme.subtext1)
                    // SAFETY: fixed literal URL to this repository's README
                    // Setup section anchor (the #376-added link target).
                    Link(destination: URL(string: "https://github.com/jirathip-dev/corral#setup")!) {
                        Label("Open the README Setup section", systemImage: "book")
                    }
                    .font(.subheadline)
                } header: {
                    stepHeader(number: 1, title: "Run the daemon on your Mac")
                }
                Section {
                    Text("The daemon binds loopback only. From the phone, reach it through the Mac's Tailscale HTTPS serve URL (https://<host>.<tailnet>.ts.net) or a LAN address.")
                        .font(.subheadline)
                        .foregroundStyle(theme.subtext1)
                    LabeledContent("Host",
                                   value: host.isEmpty
                                       ? "Not set — type it in Settings → Connection"
                                       : host)
                    Button {
                        UIPasteboard.general.string = host
                    } label: {
                        Label("Copy host", systemImage: "doc.on.doc")
                    }
                    .disabled(host.isEmpty)
                } header: {
                    stepHeader(number: 2, title: "Reach it from the phone")
                }
                Section {
                    Text("Open Settings → Connection and paste the host into the Host field.")
                        .font(.subheadline)
                        .foregroundStyle(theme.subtext1)
                } header: {
                    stepHeader(number: 3, title: "Open Settings and paste the Host")
                }
                Section {
                    Text("Paste the daemon's registration token into the Registration token field and tap Register device (read-only). The device pairs as a read-only signed device.")
                        .font(.subheadline)
                        .foregroundStyle(theme.subtext1)
                } header: {
                    stepHeader(number: 4, title: "Register with the pairing token")
                }
                Section {
                    Text("Turn on State-change notifications in Settings so you get start / blocked / finished alerts.")
                        .font(.subheadline)
                        .foregroundStyle(theme.subtext1)
                } header: {
                    stepHeader(number: 5, title: "Enable state-change notifications")
                }
            }
            .navigationTitle("How to connect")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .confirmationAction) {
                    Button("Done") {
                        dismiss()
                    }
                }
            }
            // #372/#379: same sheet treatment as the rest of the app — the
            // active flavor's base token under native grouped chrome, and
            // the scheme forced INSIDE the presented stack.
            .scrollContentBackground(.hidden)
            .background(theme.base)
            .preferredColorScheme(theme.flavor.isLight ? .light : .dark)
        }
        .presentationDragIndicator(.visible)
    }

    /// One numbered step header: the circle badge + title. Section headers
    /// cannot hold buttons, so the copy/link controls live in the rows.
    private func stepHeader(number: Int, title: String) -> some View {
        HStack(spacing: 10) {
            StepNumberBadge(number: number)
            Text(title)
                .font(.subheadline.weight(.semibold))
                .foregroundStyle(theme.text)
        }
        .accessibilityElement(children: .combine)
        .accessibilityLabel("\(number). \(title)")
    }
}

/// The circled step number from the approved sheet layout — accent-tinted
/// disc, bold numeral, token colors only.
private struct StepNumberBadge: View {
    let number: Int
    @EnvironmentObject private var theme: ThemeStore

    var body: some View {
        Text("\(number)")
            .font(.footnote.weight(.bold))
            .foregroundStyle(theme.accent)
            .frame(width: 24, height: 24)
            .background(theme.accent.opacity(0.14), in: Circle())
            .accessibilityHidden(true)
    }
}

// MARK: - Recents bottom sheet (#354 L2 recents v1 → #373 block-per-run)

/// Per-sheet-session collapse/reveal state (#373): a manual collapse is a
/// per-block toggle and lasts ONLY this sheet session — the sheet owns this
/// object, so dismissal destroys it (no persistence). Every block DEFAULTS
/// EXPANDED: a fresh session has an empty `collapsed` set. "Show all"
/// reveals a capped block's remaining lines inline once per session.
@MainActor
final class RecentsSheetSession: ObservableObject {
    @Published private(set) var collapsed: Set<String> = []
    @Published private(set) var revealed: Set<String> = []

    func toggleCollapsed(_ id: String) {
        if collapsed.contains(id) {
            collapsed.remove(id)
        } else {
            collapsed.insert(id)
        }
    }

    func reveal(_ id: String) {
        revealed.insert(id)
    }

    func reset() {
        collapsed = []
        revealed = []
    }
}

#if DEBUG
/// #373 evidence launch argument: forces the recents sheet to the LARGE
/// detent and drives the deterministic block-per-run phase sequence (marker
/// files under Documents/ux-evidence that the host screenshot script
/// observes). Present in Debug builds only; Release never contains it.
enum RecentsBlocksEvidence {
    static let argument = "-corralDemoRecentsBlocksEvidence"
}
#endif

/// Read-only recents: LIVE TAIL ONLY. The sheet auto-loads the agent's
/// bounded tail (≤200 lines, daemon cap), auto-refreshes while open, and
/// auto-scrolls to the newest content. Renders the tail as ROLE-RUN BLOCKS
/// (#373): a role change starts a block (You / Assistant / Tool run /
/// Status), blocks default EXPANDED, the whole header toggles a
/// session-scoped collapse, giant blocks cap at 20 lines with an inline
/// "Show all" reveal, and live appends land inside the current semantic
/// block. No load-earlier paging, no partition, no composer.
struct RecentOutputSheet: View {
    let agentId: String
    @ObservedObject var model: AppModel
    @Environment(\.dismiss) private var dismiss
    @EnvironmentObject private var theme: ThemeStore
    @StateObject private var session = RecentsSheetSession()
    // The live ScrollViewProxy — held so the DEBUG evidence driver can
    // re-anchor the view to the newest content after a phase change
    // (simctl cannot scroll; Release never touches it).
    @State private var recentsProxy: ScrollViewProxy?

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
                    Button("Done") {
                        // #364 A.2: explicit close control — one light
                        // tick; the swipe-down path deliberately stays
                        // silent (drag gestures never tick).
                        model.closeRecentsButtonTapped()
                        dismiss()
                    }
                }
            }
            // #372: the sheet's own chrome follows the active flavor (a
            // scene-level scheme change does not re-trait presented
            // sheets' system surfaces live).
            .task {
                refresh()
                while !Task.isCancelled {
                    try? await Task.sleep(nanoseconds: 5_000_000_000)
                    guard !Task.isCancelled else { return }
                    refresh()
                }
            }
#if DEBUG
            // #373 evidence: deterministic block-per-run phase sequence
            // (markers under Documents/ux-evidence). Launch-gated; never
            // runs without the evidence argument.
            .task {
                await runRecentsBlocksEvidenceIfNeeded()
            }
#endif
        }
        // #372: scheme forced at the SHEET level (covers the nav bar +
        // drag chrome of the presented stack).
        .preferredColorScheme(theme.flavor.isLight ? .light : .dark)
        .presentationDetents(detents)
        .presentationDragIndicator(.visible)
        // #373: the sheet's own session object rides the environment so
        // every block shares one per-session collapse/reveal state.
        .environmentObject(session)
    }

    /// The sheet's detents. #373 evidence: the deterministic capture runs
    /// at the LARGE detent (matching the approved 390x844 comp; simctl
    /// cannot drag the sheet), so the evidence argument forces it. Release
    /// and ordinary Debug launches keep the standard medium/large sheet.
    private var detents: Set<PresentationDetent> {
#if DEBUG
        if CommandLine.arguments.contains(RecentsBlocksEvidence.argument) {
            return [.large]
        }
#endif
        return [.medium, .large]
    }

    @ViewBuilder
    private var header: some View {
        VStack(alignment: .leading, spacing: 4) {
            if let agent {
                let style = StateStyle.style(for: agent.state)
                let stateColor = theme.stateColor(for: agent.state)
                HStack(spacing: 6) {
                    Circle()
                        .fill(style.isRing ? Color.clear : stateColor)
                        .overlay(Circle().stroke(stateColor, lineWidth: 1))
                        .frame(width: 10, height: 10)
                        .accessibilityHidden(true)
                    Text(style.label)
                        .font(.caption.weight(.semibold))
                        .foregroundStyle(stateColor)
                        .accessibilityLabel(style.accessibilityLabel)
                    if let repo = agent.workspace.repo {
                        Text(repo)
                            .font(.caption2.monospaced())
                            .foregroundStyle(theme.tailMuted)
                    }
                    if let branch = agent.workspace.branch {
                        Text(branch)
                            .font(.caption2.monospaced())
                            .foregroundStyle(theme.tailMuted)
                            .lineLimit(1)
                            .truncationMode(.middle)
                    }
                    Spacer()
                    if let reference = agent.attachment?.reference {
                        Text(reference)
                            .font(.caption2.monospaced())
                            .foregroundStyle(theme.tailQuiet)
                            .accessibilityLabel("Pane \(reference)")
                    }
                    if showLiveIndicator {
                        Circle()
                            .fill(theme.accent)
                            .frame(width: 6, height: 6)
                            .accessibilityHidden(true)
                        Text("live")
                            .font(.caption2.weight(.bold))
                            .foregroundStyle(theme.accent)
                    }
                }
            } else {
                Label("Agent no longer available", systemImage: "exclamationmark.triangle")
                    .font(.subheadline)
                    .foregroundStyle(theme.tailMuted)
            }
        }
        .padding(.horizontal, 16)
        .padding(.vertical, 10)
        .background(theme.base)
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
                        .tint(theme.accent)
                    Text("Loading recent output…")
                        .font(.caption)
                        .foregroundStyle(theme.tailMuted)
                }
                .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
                .padding(16)
            case .empty:
                Text("No output yet.")
                    .font(.caption)
                    .foregroundStyle(theme.tailMuted)
                    .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
                    .padding(16)
            case .error(let failure):
                VStack(alignment: .leading, spacing: 8) {
                    Label(TranscriptText.errorText(failure), systemImage: "exclamationmark.triangle")
                        .font(.caption)
                        .foregroundStyle(theme.codeDeletion)
                        .accessibilityLabel(TranscriptText.errorText(failure))
                    Button("Retry") {
                        refresh()
                    }
                    .buttonStyle(.bordered)
                    .tint(theme.accent)
                    .accessibilityLabel("Retry recent output")
                }
                .padding(16)
                .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
            case .loaded:
                blocksStream
            }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .background(theme.base)
    }

    /// The live tail as ROLE-RUN BLOCKS (#373): each block is one semantic
    /// role run (You / Assistant / Tool run / Status) that defaults
    /// EXPANDED and can be collapsed by tapping its whole header; a giant
    /// block caps at `lineCap` lines with an inline "Show all" reveal. The
    /// display model merges same-role material, so a growing live tail
    /// appends INTO the current semantic block; the stack auto-scrolls to
    /// the newest content whenever that block grows or a new one starts.
    /// Collapsing a block above never triggers a scroll (the change
    /// detector keys on the LAST block's text only), so the view never
    /// jumps.
    private var blocksStream: some View {
        let blocks = RecentOutputModel.displayBlocks(from: tail)
        return ScrollViewReader { proxy in
            ScrollView(.vertical) {
                // Plain VStack (not lazy): the bounded tail is ≤200 lines,
                // and a LazyVStack + programmatic anchors misbehaves when
                // collapse toggles shrink the content below the viewport
                // (stale offsets rendered a blank sheet).
                VStack(alignment: .leading, spacing: 8) {
                    ForEach(blocks) { block in
                        RecentRunBlockView(block: block)
                    }
                    Color.clear
                        .frame(height: 1)
                        .id("recents-bottom")
                }
                .padding(.horizontal, 12)
                .padding(.vertical, 10)
            }
            .onAppear {
                recentsProxy = proxy
                scrollToBottom(proxy)
            }
            .onChange(of: blocks.last?.text) { _, _ in
                scrollToBottom(proxy)
            }
            // #373: a collapse/expand toggle shrinks/grows content ABOVE
            // the anchor. SwiftUI keeps the stale contentOffset when the
            // content shrinks, which can leave the pinned view looking
            // blank; re-anchor to the newest content on any toggle so the
            // live pinned position never shows empty space (collapsing
            // while pinned at the bottom is therefore visually stable).
            .onChange(of: session.collapsed) { _, _ in
                DispatchQueue.main.async {
                    proxy.scrollTo("recents-bottom", anchor: .bottom)
                }
            }
        }
    }

    /// Auto-scroll to the newest row. #372 Reduce Motion: the scroll lands
    /// instantly (no animation) when the system Reduce Motion setting is
    /// on — the theme layer's plumbing, consumed by the #371 motion chip
    /// later.
    private func scrollToBottom(_ proxy: ScrollViewProxy) {
        DispatchQueue.main.async {
            if theme.reduceMotion {
                proxy.scrollTo("recents-bottom", anchor: .bottom)
            } else {
                withAnimation {
                    proxy.scrollTo("recents-bottom", anchor: .bottom)
                }
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

#if DEBUG
    /// #373 evidence: deterministic recents BLOCK-PER-RUN capture phases
    /// (single launch; the host screenshot script polls the markers under
    /// Documents/ux-evidence): Mocha default-expanded (cap + Show all +
    /// waiting line visible) → Mocha collapsed (every block's icon-only
    /// header + preview + rotated chevron) → Mocha Show-all reveal →
    /// Latte default-expanded (recessed panels + remapped ANSI colors) →
    /// Latte collapsed. The driver toggles the same session state the
    /// header taps set and the same theme state the Settings picker sets —
    /// simctl cannot inject taps, so this is the synthetic stand-in
    /// (established #364/#372 pattern). Cancellation-aborts like the board
    /// drivers, so a raced task can never corrupt the recorded sequence.
    private func runRecentsBlocksEvidenceIfNeeded() async {
        guard CommandLine.arguments.contains(RecentsBlocksEvidence.argument) else { return }
        guard await evidencePause(2000) else { return }
        guard await evidenceWaitForLoaded() else { return }
        theme.setFlavor(.mocha)
        // Phase 1: default-expanded at the newest content (bottom-anchored
        // by the sheet's auto-scroll): cap + Show all, ANSI-colored runs,
        // and the waiting line are all in this frame.
        EvidenceMarkers.write("phase-1-recents-mocha-expanded")
        guard await evidencePause(5000) else { return }

        // Phase 2: every block collapsed — the icon/role map, previews and
        // rotated chevrons (content shrinks to the headers; the view is at
        // the top).
        let ids = currentDisplayBlockIDs()
        for id in ids { session.toggleCollapsed(id) }
        guard await evidencePause(1500) else { return }
        EvidenceMarkers.write("phase-2-recents-mocha-collapsed")
        guard await evidencePause(5000) else { return }

        // Phase 3: expand everything and reveal the capped block (Show
        // all) — the cap's hidden rows now render and the button is gone.
        // Re-anchor to the newest content so the reveal is visible next to
        // the ANSI-colored runs.
        for id in ids { session.toggleCollapsed(id) }
        revealCappedBlock()
        await evidenceReanchorToBottom()
        guard await evidencePause(1500) else { return }
        EvidenceMarkers.write("phase-3-recents-mocha-showall")
        guard await evidencePause(5000) else { return }

        // Phase 4: fresh session (all expanded again) on Latte — the same
        // bottom view proves the remapped ANSI hues, the recessed panels,
        // and the waiting line on the light theme.
        session.reset()
        theme.setFlavor(.latte)
        await evidenceReanchorToBottom()
        guard await evidencePause(1500) else { return }
        EvidenceMarkers.write("phase-4-recents-latte-expanded")
        guard await evidencePause(5000) else { return }

        // Phase 5: Latte collapsed headers (parity + quiet status chrome).
        for id in currentDisplayBlockIDs() { session.toggleCollapsed(id) }
        guard await evidencePause(1500) else { return }
        EvidenceMarkers.write("phase-5-recents-latte-collapsed")
        guard await evidencePause(5000) else { return }
        EvidenceMarkers.write("phase-6-done")
        _ = await evidencePause(5000)
    }

    /// Re-anchor the scroll to the newest content (the same call the live
    /// tail uses) — simctl cannot scroll, so the evidence driver uses the
    /// sheet's live proxy.
    private func evidenceReanchorToBottom() async {
        guard await evidencePause(400) else { return }
        if let proxy = recentsProxy {
            scrollToBottom(proxy)
        }
    }

    /// Reveal (Show all) the one block that exceeds the 20-line cap.
    private func revealCappedBlock() {
        for block in RecentOutputModel.displayBlocks(from: tail)
        where block.cappedLineCount > 0 {
            session.reveal(block.id)
        }
    }

    /// The current display block ids (ordinal ids — stable while the tail
    /// only grows, so the driver collapses exactly what the sheet renders).
    private func currentDisplayBlockIDs() -> [String] {
        RecentOutputModel.displayBlocks(from: tail).map(\.id)
    }

    private func evidenceWaitForLoaded() async -> Bool {
        for _ in 0..<40 {
            if RecentOutputModel.phase(for: tail) == .loaded { return true }
            guard await evidencePause(250) else { return false }
        }
        return RecentOutputModel.phase(for: tail) == .loaded
    }

    private func evidencePause(_ milliseconds: Int64) async -> Bool {
        do {
            try await Task.sleep(for: .milliseconds(milliseconds))
            return true
        } catch {
            return false
        }
    }
#endif
}


// MARK: - Recents block renderer (#373 block-per-run)

/// Role presentation metadata for the block header. Headers are ICON-ONLY
/// (design lock: no role words visible — the tool name lives in the line
/// text; the role name stays in the accessible label). Role accents:
/// user = blue, assistant = mauve (the UI accent), tool = peach, status =
/// overlay0 (quiet). All tokens come from the #372 theme — never legacy
/// hexes.
enum RecentBlockStyle {
    static func iconName(for block: RecentOutputModel.DisplayBlock) -> String {
        switch block.kind {
        case .user:
            return "person.crop.circle"
        case .agent:
            return "sparkles"
        case .tool:
            switch block.tool ?? .generic {
            case .terminal: return "terminal"
            case .doc: return "doc.text"
            case .code: return "chevron.left.forwardslash.chevron.right"
            case .search: return "magnifyingglass"
            case .generic: return "ellipsis.rectangle"
            }
        case .system:
            return "info.circle"
        case .unknown:
            return "questionmark.circle"
        }
    }

    static func accentToken(for kind: TranscriptBlockKind) -> CatppuccinToken {
        switch kind {
        case .user: return .blue
        case .agent: return .mauve
        case .tool: return .peach
        case .system, .unknown: return .overlay0
        }
    }

    static func roleName(for kind: TranscriptBlockKind) -> String {
        switch kind {
        case .user: return "You"
        case .agent: return "Assistant"
        case .tool: return "Tool run"
        case .system: return "Status"
        case .unknown: return "Unknown activity"
        }
    }
}

/// One role-run block (#373): rounded block chrome on the theme's surface0
/// with a thin left accent per role, an icon-only header whose WHOLE width
/// toggles collapse (chevron rotates; a collapsed block shows a one-line
/// preview of its first content), and the run's content. Every block
/// defaults EXPANDED. Content rows never flex-squash: the stack sizes rows
/// to their natural height (`.fixedSize(vertical:)` on text rows), so a
/// 390x844 sheet cannot crush a block's content.
private struct RecentRunBlockView: View {
    let block: RecentOutputModel.DisplayBlock
    @EnvironmentObject private var theme: ThemeStore
    @EnvironmentObject private var session: RecentsSheetSession

    private var isCollapsed: Bool { session.collapsed.contains(block.id) }
    private var isRevealed: Bool { session.revealed.contains(block.id) }
    private var showsOutputPanel: Bool { block.kind == .tool }
    private var isHighlightedRun: Bool {
        RecentOutputRender.isCodeOrDiff(block.text)
    }
    private var accent: Color {
        theme.color(RecentBlockStyle.accentToken(for: block.kind))
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            header
            if !isCollapsed {
                bodyContent
            }
        }
        .background(blockBackground,
                    in: RoundedRectangle(cornerRadius: 10, style: .continuous))
        .overlay(alignment: .leading) {
            Rectangle()
                .fill(accent)
                .frame(width: 2)
                .accessibilityHidden(true)
        }
        .clipShape(RoundedRectangle(cornerRadius: 10, style: .continuous))
        .textSelection(.enabled)
        .accessibilityElement(children: .contain)
    }

    /// Block chrome: surface0 for content roles; Status (and unattributed
    /// raw) material is QUIET — its block recesses toward base (the
    /// design round's quiet status treatment, resolved from palette
    /// tokens only).
    private var blockBackground: Color {
        if block.kind == .system || block.kind == .unknown {
            return theme.mixed(.surface0, at: 0.55, over: .base)
        }
        return theme.surface0
    }

    // MARK: Header (icon-only; whole-header tap toggles collapse)

    private var header: some View {
        Button {
            session.toggleCollapsed(block.id)
        } label: {
            HStack(spacing: 8) {
                iconChip
                if isCollapsed {
                    previewLine
                }
                Spacer(minLength: 0)
                chevron
            }
            .padding(.leading, 10)
            .padding(.trailing, 8)
            .padding(.vertical, 5)
            .frame(maxWidth: .infinity, minHeight: 34, alignment: .leading)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .accessibilityLabel(headerLabel)
    }

    private var headerLabel: String {
        let role = RecentBlockStyle.roleName(for: block.kind)
        guard isCollapsed else {
            return role + ", expanded"
        }
        let preview = block.firstLine
        let clipped = preview.count <= 46
            ? preview
            : String(preview.prefix(45)) + "…"
        return clipped.isEmpty
            ? role + ", collapsed"
            : role + ", collapsed: " + clipped
    }

    /// The SF Symbol chip in the role's accent (tint = 16 % accent over
    /// surface0 — the design round's icon-chip mix).
    private var iconChip: some View {
        Image(systemName: RecentBlockStyle.iconName(for: block))
            .font(.system(size: 12, weight: .semibold))
            .foregroundStyle(accent)
            .frame(width: 22, height: 22)
            .background(
                RoundedRectangle(cornerRadius: 6, style: .continuous)
                    .fill(theme.mixed(RecentBlockStyle.accentToken(for: block.kind),
                                      at: 0.16, over: .surface0))
            )
            .accessibilityHidden(true)
    }

    /// Collapsed preview: the block's first content line, mono, one line
    /// (never a role word — for a tool run this is the invocation).
    private var previewLine: some View {
        Text(block.firstLine)
            .font(.caption.monospaced())
            .foregroundStyle(theme.overlay1)
            .lineLimit(1)
            .truncationMode(.tail)
            .accessibilityHidden(true)
    }

    private var chevron: some View {
        Image(systemName: "chevron.down")
            .font(.system(size: 12, weight: .semibold))
            .foregroundStyle(theme.subtext0)
            .rotationEffect(.degrees(isCollapsed ? -90 : 0))
            .animation(theme.reduceMotion ? nil : .easeOut(duration: 0.18),
                       value: isCollapsed)
            .accessibilityHidden(true)
    }

    // MARK: Body (default expanded; 20-line cap + Show all)

    @ViewBuilder
    private var bodyContent: some View {
        let chunks = contentChunks
        VStack(alignment: .leading, spacing: 4) {
            ForEach(chunks) { chunk in
                chunkView(chunk)
            }
            if !isRevealed && block.cappedLineCount > 0 {
                showAllButton
            }
        }
        .padding(.horizontal, 10)
        .padding(.top, 1)
        .padding(.bottom, 9)
    }

    /// Rows are chunked into output runs so each run renders inside ONE
    /// tinted panel; every other row renders alone.
    private var contentChunks: [BodyChunk] {
        let rows = isRevealed
            ? block.rows
            : Array(block.rows.prefix(RecentOutputModel.lineCap))
        var chunks: [BodyChunk] = []
        var index = 0
        var i = 0
        while i < rows.count {
            if rows[i].kind == .output {
                var end = i
                while end + 1 < rows.count && rows[end + 1].kind == .output {
                    end += 1
                }
                chunks.append(BodyChunk(id: index, rows: Array(rows[i...end])))
                i = end + 1
            } else {
                chunks.append(BodyChunk(id: index, rows: [rows[i]]))
                i += 1
            }
            index += 1
        }
        return chunks
    }

    @ViewBuilder
    private func chunkView(_ chunk: BodyChunk) -> some View {
        if chunk.rows.first?.kind == .output {
            if showsOutputPanel {
                outputPanel(chunk.rows)
            } else {
                VStack(alignment: .leading, spacing: 2) {
                    ForEach(Array(chunk.rows.enumerated()), id: \.offset) { _, row in
                        RecentOutputLineView(line: row.text,
                                             highlighted: isHighlightedRun)
                    }
                }
                .frame(maxWidth: .infinity, alignment: .leading)
            }
        } else {
            rowView(chunk.rows[0])
        }
    }

    @ViewBuilder
    private func rowView(_ row: RecentOutputModel.BlockRow) -> some View {
        switch row.kind {
        case .prose:
            Text(row.text)
                .font(.subheadline)
                .foregroundStyle(theme.text)
                .frame(maxWidth: .infinity, alignment: .leading)
                .fixedSize(horizontal: false, vertical: true)
        case .meta:
            Text(row.text)
                .font(.caption.monospaced())
                .foregroundStyle(theme.tailMuted)
                .frame(maxWidth: .infinity, alignment: .leading)
                .fixedSize(horizontal: false, vertical: true)
        case .call:
            callRow(row.text)
        case .waiting:
            // No-output run: muted inline placeholder — never its own block.
            Text(row.text)
                .font(.caption2.monospaced().italic())
                .foregroundStyle(theme.tailQuiet)
                .frame(maxWidth: .infinity, alignment: .leading)
                .fixedSize(horizontal: false, vertical: true)
                .padding(.leading, 12)
        case .output:
            RecentOutputLineView(line: row.text, highlighted: isHighlightedRun)
        }
    }

    /// One tool invocation: compact mono line. A shell echo keeps its `$`
    /// sigil in the block's role accent; a bare tool call has no sigil.
    private func callRow(_ text: String) -> some View {
        HStack(alignment: .firstTextBaseline, spacing: 6) {
            if text.hasPrefix("$") {
                Text("$")
                    .font(.caption.monospaced().weight(.bold))
                    .foregroundStyle(accent)
            }
            Text(strippedCallText(text))
                .font(.caption.monospaced())
                .foregroundStyle(theme.text)
                .frame(maxWidth: .infinity, alignment: .leading)
                .fixedSize(horizontal: false, vertical: true)
        }
    }

    private func strippedCallText(_ text: String) -> String {
        if text.hasPrefix("$ ") {
            return String(text.dropFirst(2))
        }
        return text == "$" ? "" : text
    }

    /// One output run rendered INLINE inside the tool block on a subtle
    /// tint: the theme's accepted output-panel token (#372 recess rule) —
    /// mantle on the dark flavors, BASE on Latte (a light theme recesses
    /// toward base, NOT mantle, so the ANSI hues keep contrast; the three
    /// Latte ANSI exception slots are documented in AppTheme).
    private func outputPanel(_ rows: [RecentOutputModel.BlockRow]) -> some View {
        VStack(alignment: .leading, spacing: 2) {
            ForEach(Array(rows.enumerated()), id: \.offset) { _, row in
                RecentOutputLineView(line: row.text,
                                     highlighted: isHighlightedRun)
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(.horizontal, 8)
        .padding(.vertical, 6)
        .background(theme.tailBackground,
                    in: RoundedRectangle(cornerRadius: 6, style: .continuous))
    }

    private var showAllButton: some View {
        Button {
            session.reveal(block.id)
        } label: {
            HStack(spacing: 6) {
                Text("Show all")
                Text("+\(block.cappedLineCount) lines")
                    .foregroundStyle(theme.tailQuiet)
            }
            .font(.caption.monospaced().weight(.semibold))
            .foregroundStyle(accent)
        }
        .buttonStyle(.plain)
        .padding(.top, 2)
        .accessibilityLabel("Show all \(block.cappedLineCount) more lines")
    }

    private struct BodyChunk: Identifiable {
        let id: Int
        let rows: [RecentOutputModel.BlockRow]
    }
}

/// One output line inside a block: mono, muted, ANSI-remapped syntax marks
/// when the run is code/diff (the theme's segment colors resolve through
/// the ACTIVE flavor's ANSI slots — #372 remap, no legacy hexes).
private struct RecentOutputLineView: View {
    let line: String
    let highlighted: Bool
    @EnvironmentObject private var theme: ThemeStore

    var body: some View {
        Group {
            if highlighted, !line.isEmpty {
                segmented
            } else {
                Text(line.isEmpty ? " " : line)
                    .font(.caption2.monospaced())
                    .foregroundStyle(theme.tailMuted)
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .fixedSize(horizontal: false, vertical: true)
    }

    private var segmented: Text {
        RecentOutputRender.highlightSegments(in: line)
            .reduce(Text("")) { partial, segment in
                partial + Text(segment.text)
                    .foregroundStyle(segmentColor(segment.kind))
            }
    }

    /// Plain output lines sit in the muted output tier; syntax marks
    /// (keywords, strings, +/-, comments) use the ANSI-slot segment colors.
    private func segmentColor(_ kind: RecentCodeSegmentKind) -> Color {
        switch kind {
        case .plain:
            return theme.tailMuted
        case .keyword, .string, .addition, .deletion, .comment:
            return theme.segmentColor(for: kind)
        }
    }
}
