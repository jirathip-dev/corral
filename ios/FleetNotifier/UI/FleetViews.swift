import SwiftUI
import Combine
import UIKit

// MARK: - #354/#371 L2 read-only FleetNotifier
//
// Surfaces after the client cut:
// - Board (home): raw-herdr-status sections in the locked attention order
//   (blocked → working → idle → unknown; done only when herdr reports it),
//   each section grouping its rows into always-open REPO SUBGROUPS (#371:
//   alphabetical, Other last). Row = agent name · state chip · time-in-state
//   · repo chip · branch + small pane ref — the per-row repo chip hides
//   while a #384 repo pill is active (the board then shows only that repo).
//   NO search, NO actions.
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
/// (the repo renders as a colored label chip echoing its subgroup hue —
/// #384: hidden while a repo pill is active, see WorkspaceLine).
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
    /// #384: while a repo pill is active the board shows only that repo, so
    /// the per-row repo name label is redundant and hidden by the
    /// workspace line (WorkspaceLine renders the color-only echo instead).
    var hideRepoLabel: Bool = false
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
            WorkspaceLine(agent: agent, repos: repos,
                          hideRepoLabel: hideRepoLabel)
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

/// #401 D6: the compact textual host badge on multi-host board rows (All
/// Hosts with 2+ profiles). Text-only identity — color never carries the
/// meaning (D8); it is a plain caption-tier capsule on the token surface.
private struct HostBadgeChip: View {
    let name: String
    @EnvironmentObject private var theme: ThemeStore

    var body: some View {
        Text(name)
            .font(.caption2.weight(.semibold))
            .foregroundStyle(theme.subtext1)
            .lineLimit(1)
            .padding(.horizontal, 6)
            .padding(.vertical, 1)
            .background(theme.surface2.opacity(0.35),
                        in: RoundedRectangle(cornerRadius: 4))
            .accessibilityElement(children: .ignore)
            .accessibilityLabel("Host \(name)")
    }
}

/// #401 C6: the retained STALE row marker — "stale · last seen 6m ago"
/// (offline-only when no stamp exists). Self-ticks at 30 s so the age stays
/// honest without re-rendering the board; the state chip above it keeps the
/// LAST REPORTED state — never recast (C7).
private struct StaleRowLabel: View {
    let lastSeenMs: UInt64
    @State private var now = UInt64(Date().timeIntervalSince1970 * 1000)
    @EnvironmentObject private var theme: ThemeStore

    var body: some View {
        HStack(spacing: 4) {
            Image(systemName: "clock.arrow.circlepath")
                .font(.caption2.weight(.semibold))
                .foregroundStyle(theme.subtext1)
                .accessibilityHidden(true)
            Text(text)
                .font(.caption2)
                .foregroundStyle(theme.subtext1)
                .lineLimit(1)
        }
        .accessibilityElement(children: .ignore)
        .accessibilityLabel(text)
        .task(id: "stale-row-ticker") {
            while !Task.isCancelled {
                now = UInt64(Date().timeIntervalSince1970 * 1000)
                do {
                    try await Task.sleep(nanoseconds: 30_000_000_000)
                } catch {
                    break
                }
            }
        }
    }

    private var text: String {
        if lastSeenMs > 0 {
            return "stale · \(RelativeTime.lastSeenLabel(lastSeenMs: lastSeenMs, nowMs: now))"
        }
        return "stale · offline"
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
/// #384: while a repo pill is active (not 'All') the per-row repo name is
/// redundant — the board already shows only that repo — so the label chip
/// is replaced by a COLOR-ONLY hue echo sized to the chip's footprint
/// (rows keep their height; no layout jump when the pill toggles). The
/// repo identity is never lost: the active pill + the subgroup caption
/// name it, and tapping 'All' restores the full label chip instantly.
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
    /// #384: hide the per-row repo name label while a repo pill is active
    /// (color-only echo keeps the row height — see the body).
    var hideRepoLabel: Bool = false
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
            if hideRepoLabel {
                // #384: under an active repo pill the filter + the subgroup
                // caption already name the repo, so the per-row NAME label
                // is removed — only the deterministic hue remains as a
                // color-only echo (no text, no chip chrome) framed to the
                // label chip's height so rows never jump on pill toggle.
                repoColorEcho(for: w.repo)
            } else {
                // #371: the repo is a colored label chip (hue dot + name,
                // echoing the subgroup header); orphans carry the Other
                // chip — the repo identity is never color-only.
                RepoLabelChip(repo: w.repo, repos: repos)
                    .layoutPriority(SegmentPolicy.priority(for: .repo))
            }
            if let branch = w.branch {
                // The separator sits BETWEEN segments: with the repo label
                // hidden the branch leads the line (no stray leading dot).
                if !hideRepoLabel {
                    segmentSeparator
                }
                Text(branch)
                    .font(.caption2.monospaced())
                    .foregroundStyle(theme.subtext1)
                    .lineLimit(1)
                    .truncationMode(SegmentPolicy.truncationMode(for: .branch))
                    .layoutPriority(SegmentPolicy.priority(for: .branch))
            }
            if let basename = Self.worktreeBasename(w) {
                // #384: the basename only takes a leading separator when a
                // segment precedes it (chip under 'All', or a branch).
                if !hideRepoLabel || w.branch != nil {
                    segmentSeparator
                }
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

    /// #384: the inter-segment separator dot between repo/branch/basename.
    private var segmentSeparator: some View {
        Text("·").font(.caption2).foregroundStyle(theme.subtext1)
    }

    /// #384: the color-only echo of the row's repo hue while a repo pill is
    /// active — the hue dot WITHOUT any repo name text (the pill + subgroup
    /// caption carry the identity). An invisible caption2 spacer keeps the
    /// label chip's EXACT text line box, and the dot carries the chip's
    /// vertical padding, so the row keeps its exact height under the filter
    /// (only the name disappears — no layout jump on pill toggle). Voice-
    /// Over-hidden: the row summary + pill still name the repo.
    private func repoColorEcho(for repo: String?) -> some View {
        HStack(spacing: 5) {
            RoundedRectangle(cornerRadius: 2)
                .fill(theme.repoHueColor(for: repo ?? "", among: repos))
                .frame(width: 6, height: 6)
            // Transparent caption2 spacer (same font as the hidden label):
            // keeps the chip's line box so the row height never changes.
            // opacity(0) is purely visual — the spacer deterministically
            // stays in the layout.
            Text(" ").font(.caption2.weight(.bold)).opacity(0)
        }
        .padding(.leading, 5)
        .padding(.trailing, 7)
        .padding(.vertical, 2)
        .accessibilityHidden(true)
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
    /// #386: which status sections are collapsed. View-owned so the state
    /// lives for the board session ONLY — never persisted, never restored
    /// (consistent with #373's per-sheet session state) — and every fresh
    /// session defaults to ALL EXPANDED.
    @State private var sectionCollapse = BoardModel.StatusSectionCollapse()
#if DEBUG
    /// #387: scroll requests from the recorded-evidence driver. simctl
    /// cannot drag the board list, so the DEBUG driver asks the board's
    /// ScrollViewReader (see body) to scroll; a repeated request for the
    /// same anchor is an idempotent no-op, so a re-fired evidence task
    /// (the .task(id:) hook fires twice on demo entry) cannot corrupt a
    /// captured phase.
    @State private var evidenceScrollTarget: String?
#endif

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
        // #384: a repo pill is ACTIVE when the reconciled filter names a
        // repo — every row then belongs to that repo, so the per-row repo
        // name labels are redundant and hidden. 'All' (nil filter)
        // restores them instantly: the flag re-derives from the same pure
        // reconcile on every body evaluation — no extra state, no timer.
        let rowRepoLabelsHidden = activeRepoFilter != nil
        // #364 B2: the chips choose WHICH agents the #362 status sections
        // bucket; sections keep their locked order over the filtered set,
        // and #371 splits each section into repo subgroups.
        let sections = BoardModel.sections(
            BoardModel.agents(agents, in: activeRepoFilter))
        // #401 multi-host board (D1-D7): with 2+ profiles the board renders
        // the #400 COMPOSITE rows (aggregateBoardRows — never re-derived
        // ranking) with a host-chip row above the repo chips; every host
        // chip shows its TOTAL lane count + health independent of the repo
        // filter (D3), repo chips/choices recalc for the selected host
        // (D4), rows merge by repo across hosts (D5), and compact textual
        // host badges show on rows only in All Hosts (D6). With one profile
        // every host* value below is inert and the legacy single-host path
        // below renders byte-identically (F1 parity).
        let multiHost = model.multiHostConfigured
        let aggregateRows = model.aggregateBoardRows ?? []
        let hostFilter = model.hostFilter
        // D2/D4: rows of the selected host (nil = every host).
        let hostRows = BoardModel.rows(aggregateRows, forHost: hostFilter)
        // D4: repo chip set + counts over the SELECTED host's rows.
        let hostRepoChips = BoardModel.repoFilters(hostRows)
        let activeHostRepoFilter = BoardModel.reconcile(model.repoFilter,
                                                        against: hostRepoChips)
        let hostRepos = hostRepoChips.map(\.repo)
        // D5/C6/C7: bucket the ALREADY-ranked rows (canonical + live-first
        // from #400) into status sections → merged repo subgroups.
        let hostSections = BoardModel.hostSections(
            BoardModel.rows(hostRows, in: activeHostRepoFilter))
        // D3/D7: host chips in user-controlled order, counts from the
        // UNFILTERED aggregate (repo-independent).
        let hostChipInputs = model.profiles.map { profile in
            BoardModel.HostFilterChip(
                profileID: profile.id,
                displayName: profile.displayName,
                laneCount: BoardModel.laneCounts(aggregateRows)[profile.id] ?? 0,
                health: BoardModel.hostChipHealth(
                    for: model.hostRuntimeFacts(for: profile)))
        }
        let hostChipRow = BoardModel.hostChips(hosts: hostChipInputs)
        let hostOutageSummary = BoardModel.hostOutageSummary(hosts: hostChipInputs)
        // D6: badges only in All Hosts with 2+ profiles.
        let showRowHostBadges = multiHost && hostFilter == nil
        // #365: the top-bar chrome (.navigationTitle/.toolbar — the Settings
        // gear) renders only inside a navigation shell. The #354 cut deleted
        // the board's NavigationStack, orphaning those modifiers and leaving
        // the board with NO visible way into Settings; restore the shell.
        return NavigationStack {
            // #387: the board list rides a ScrollViewReader so the DEBUG
            // recorded-evidence driver can reach the SCROLLED nav-bar state
            // (simctl cannot drag the list). The reader itself is passive;
            // no scroll machinery compiles into Release.
            ScrollViewReader { proxy in
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
                        // #401 D2: with 2+ profiles the HOST-chip row sits
                        // ABOVE the repo-chip row (All, then hosts in
                        // user-controlled order); the compact D7 outage
                        // summary rides under it. Single-host layout is
                        // unchanged (the row is hidden with one profile).
                        if model.multiHostConfigured {
                            hostChipsRow(chips: hostChipRow,
                                         selection: model.hostFilter)
                            if let hostOutageSummary {
                                hostOutageSummaryRow(hostOutageSummary)
                            }
                        }
                        if model.multiHostConfigured {
                            repoChipsRow(chips: hostRepoChips,
                                         total: hostRows.count,
                                         selection: activeHostRepoFilter)
                        } else {
                            repoChipsRow(chips: chips,
                                         total: agents.count,
                                         selection: activeRepoFilter)
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
                        if model.multiHostConfigured {
                            hostBoardSections(sections: hostSections,
                                              repos: hostRepos,
                                              hideRepoLabels: activeHostRepoFilter != nil,
                                              showHostBadges: showRowHostBadges)
                        } else {
                            boardSections(sections: sections, repos: repos,
                                          hideRepoLabels: rowRepoLabelsHidden)
                        }
#endif
                    case .live:
                        if model.multiHostConfigured {
                            hostBoardSections(sections: hostSections,
                                              repos: hostRepos,
                                              hideRepoLabels: activeHostRepoFilter != nil,
                                              showHostBadges: showRowHostBadges)
                        } else {
                            boardSections(sections: sections, repos: repos,
                                          hideRepoLabels: rowRepoLabelsHidden)
                        }
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
                // #387: the board header is chrome-only — NO 'Fleet' title
                // text in the large-title state or the scrolled inline state
                // (app identity is the gear, top-right). An EMPTY title with
                // INLINE display mode reserves no large-title band at rest and
                // the collapsed bar shows no text when scrolled either — the
                // freed space belongs to the board (content starts naturally
                // higher; no extra insets forced).
                .navigationTitle("")
                .navigationBarTitleDisplayMode(.inline)
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
                    // #400 E1: the sheet receives the row's COMPOSITE
                    // identity and resolves exactly the owning host.
                    RecentOutputSheet(agentId: request.agentId,
                                      hostProfileID: request.hostProfileID,
                                      model: model)
                }
                // #379: the How-to-connect sheet — the SAME shared content the
                // Settings '?' button presents from the Settings sheet. The
                // unpaired-launch auto-present drives this binding; the DEBUG
                // recorded-evidence driver opens it too (simctl cannot tap).
                .sheet(isPresented: $showConnectHelp) {
                    HowToConnectSheet(host: model.hostURL?.absoluteString ?? "")
                }
                // #399 B6: the legacy-migration fingerprint confirmation —
                // the profile is paused (no stream) until the user confirms
                // the pinned host key on this sheet.
                .sheet(item: $model.fingerprintConfirmation) { request in
                    FingerprintConfirmationSheet(model: model, request: request)
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
#if DEBUG
                // #387: the recorded-evidence driver's scroll requests —
                // simctl cannot drag the list, so the DEBUG driver asks
                // the list to scroll to a row id. Fired from a .task(id:)
                // (like the proven #379 settings scroll) with a short
                // settle so the request lands after the current update;
                // instant + idempotent: a repeated request for the same
                // anchor is a no-op, so a re-fired evidence task cannot
                // corrupt a phase.
                .task(id: evidenceScrollTarget) {
                    guard let target = evidenceScrollTarget else { return }
                    try? await Task.sleep(for: .milliseconds(400))
                    proxy.scrollTo(target, anchor: .top)
                }
#endif
            }
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
            // #387: the chips row is the demo board's FIRST content row —
            // its id is the ScrollViewReader 'top' anchor the recorded-
            // evidence driver returns to (see BoardEvidenceAnchor.top).
            .id("board.filter-chips")
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

    /// #401 D2/D3: the horizontal HOST-filter chip row rendered ABOVE the
    /// repo-chip row when 2+ profiles exist — All first, then every host in
    /// the user-controlled order (Settings drag-to-reorder drives the same
    /// store order). Each host chip always shows that host's TOTAL lane
    /// count + health, independent of the repo filter; zero-lane and
    /// offline hosts stay visible (D3).
    @ViewBuilder
    private func hostChipsRow(chips: [BoardModel.HostFilterChip],
                              selection: UUID?) -> some View {
        Section {
            ScrollView(.horizontal, showsIndicators: false) {
                HStack(spacing: 8) {
                    ForEach(chips) { chip in
                        hostChipButton(chip,
                                       isSelected: selection == chip.profileID) {
                            model.selectHostFilter(chip.profileID)
                        }
                    }
                }
                .padding(.horizontal, 20)
                .padding(.vertical, 4)
            }
            .id("board.host-chips")
            .listRowInsets(EdgeInsets())
            .listRowBackground(Color.clear)
            .listRowSeparator(.hidden)
        }
    }

    /// One host-filter chip: All / host name + total-lane count + a health
    /// dot whose COLOR never carries the meaning alone — the health text
    /// label rides on the chip whenever the host is not live and in the
    /// VoiceOver label/value always (D3/D8). ≥44 pt hit target, visible
    /// selected state, selected trait.
    private func hostChipButton(_ chip: BoardModel.HostFilterChip,
                                isSelected: Bool,
                                action: @escaping () -> Void) -> some View {
        Button(action: action) {
            HStack(spacing: 5) {
                Circle()
                    .fill(hostHealthColor(chip.health))
                    .frame(width: 7, height: 7)
                    .accessibilityHidden(true)
                Text(chip.isAll ? "All" : chip.displayName)
                    .lineLimit(1)
                Text("\(chip.laneCount)")
                    .font(.caption.weight(.semibold))
                    .padding(.horizontal, 5)
                    .padding(.vertical, 1)
                    .background(isSelected ? theme.crust.opacity(0.22)
                                           : theme.surface2.opacity(0.30),
                                in: Capsule())
                    .accessibilityHidden(true)
                if !chip.isAll, chip.health != .live {
                    Text(chip.health.label)
                        .font(.caption2.weight(.medium))
                        .lineLimit(1)
                        .foregroundStyle(hostHealthColor(chip.health))
                }
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
        .accessibilityLabel(chip.isAll ? "All hosts"
                                       : "Filter host \(chip.displayName)")
        .accessibilityValue("\(chip.laneCount) lanes, \(chip.health.label)")
        .accessibilityAddTraits(isSelected ? [.isSelected] : [])
    }

    /// The health dot color (D8 — color never alone; the textual health
    /// label and VoiceOver value always accompany it). Resolves through the
    /// shared token mapping so all four themes stay in sync.
    private func hostHealthColor(_ health: BoardModel.HostChipHealth) -> Color {
        theme.color(BoardModel.hostHealthToken(health))
    }

    /// #401 D7: the ONE compact board-level outage summary row ("1 host
    /// offline · …"), rendered under the host chips when any host is not
    /// live. Never a full-width reconnect banner per retry.
    @ViewBuilder
    private func hostOutageSummaryRow(_ text: String) -> some View {
        Section {
            HStack(spacing: 6) {
                Image(systemName: "wifi.slash")
                    .font(.caption2.weight(.semibold))
                    .foregroundStyle(theme.peach)
                    .accessibilityHidden(true)
                Text(text)
                    .font(.caption2.weight(.medium))
                    .foregroundStyle(theme.peach)
                    .lineLimit(1)
                Spacer(minLength: 0)
            }
            .padding(.horizontal, 20)
            .padding(.vertical, 3)
            .frame(maxWidth: .infinity, alignment: .leading)
            .accessibilityElement(children: .combine)
            .listRowInsets(EdgeInsets())
            .listRowBackground(Color.clear)
            .listRowSeparator(.hidden)
        }
    }

    /// #371/#386 board renderer: one section per raw herdr status in
    /// the locked attention order (blocked → working → idle → unknown; a
    /// done section renders only when herdr reports it), each section led
    /// by the #386 THICK collapsible status bar. While EXPANDED the
    /// section shows its always-open REPO SUBGROUP captions (alphabetical,
    /// Other last) + agent rows; a COLLAPSED section renders its bar alone
    /// (counts stay on the bar). Subgroup captions are NOT collapsible —
    /// the status bar is the only disclosure control. `sections` arrives
    /// ALREADY filtered by the #364 B chip selection; `repos` is the
    /// fleet-wide repo set so subgroup + row hues match the filter chips'
    /// fnv1a32 % 8 assignment. #384: `hideRepoLabels` (true while a repo
    /// pill is active) is threaded into every row so the per-row repo name
    /// labels disappear under the filter and reappear under 'All'.
    @ViewBuilder
    private func boardSections(sections: BoardModel.Sections,
                               repos: [String],
                               hideRepoLabels: Bool) -> some View {
        ForEach(sections.statuses) { status in
            Section {
                // #386: a collapsed status section hides its subgroups and
                // rows — the bar above is all that remains (instant; no
                // animation, so Reduce Motion is unaffected).
                if !sectionCollapse.isCollapsed(status.state) {
                    ForEach(status.subgroups) { subgroup in
                        repoSubgroupHeader(subgroup, repos: repos)
                        ForEach(subgroup.agents) { agent in
                            agentRow(agent, repos: repos,
                                     hideRepoLabel: hideRepoLabels)
                                // #387: every board row carries its agent id as
                                // the ScrollViewReader anchor the recorded-
                                // evidence driver scrolls to (the scrolled
                                // nav-bar state; simctl cannot drag).
                                .id(agent.agentId)
                                // #372: iOS 26 plain lists paint their own
                                // row background unless each row opts into
                                // the token surface (a List-level
                                // `.listRowBackground` is not honored);
                                // rows ride the flavor's base.
                                .listRowBackground(theme.base)
                        }
                    }
                }
            } header: {
                PinnedHeader(fillsInteractiveWidth: true) {
                    statusSectionBar(status)
                }
            }
        }
    }

    /// The #386 status bar: the section's THICK full-width header —
    /// taller and bolder than the old caption row, on the theme's
    /// surface1 tier (the chrome strips around it stay mantle, so the bar
    /// contrasts per palette) — carrying the state-colored mark, the raw
    /// status name + TOTAL count, and a chevron that rotates to point
    /// right when the section is collapsed. The WHOLE bar is the tap
    /// target (≥44 pt); collapse is INSTANT (no animation beyond the
    /// static chevron rotation, so Reduce Motion is unaffected) and the
    /// state is the board-session-only `sectionCollapse`.
    @ViewBuilder
    private func statusSectionBar(
        _ status: BoardModel.StatusSection) -> some View {
        let isCollapsed = sectionCollapse.isCollapsed(status.state)
        Button {
            sectionCollapse.toggle(status.state)
        } label: {
            HStack(spacing: 8) {
                RoundedRectangle(cornerRadius: 2)
                    .fill(theme.stateColor(for: status.state))
                    .frame(width: 10, height: 10)
                    .accessibilityHidden(true)
                Text(status.header)
                    .font(.headline.weight(.bold))
                    .foregroundStyle(theme.text)
                    .lineLimit(1)
                Spacer(minLength: 8)
                Image(systemName: "chevron.down")
                    .font(.subheadline.weight(.semibold))
                    .foregroundStyle(theme.subtext1)
                    .rotationEffect(.degrees(isCollapsed ? -90 : 0))
                    .accessibilityHidden(true)
            }
            .padding(.horizontal, 20)
            .padding(.vertical, 12)
            .frame(maxWidth: .infinity, minHeight: 44, alignment: .leading)
            .background(theme.surface1)
        }
        .buttonStyle(BoardPressStyle())
        .accessibilityElement(children: .ignore)
        .accessibilityLabel(status.header)
        .accessibilityValue(isCollapsed ? "Collapsed" : "Expanded")
        .accessibilityHint("Toggles the \(status.state.displayName) section")
    }

    /// One always-open repo subgroup caption (#371 → #386 DEMOTED): a
    /// small/secondary caption row — 2 pt hue rail + small hue chip +
    /// repo name + count in caption2 subtext1 type (never the section
    /// bar's headline tier) — on the hue 9 %-over-mantle band. NOT
    /// collapsible (no disclosure control anywhere on the row); Other
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
                .font(.caption2.weight(.semibold))
                .foregroundStyle(theme.subtext1)
                .lineLimit(1)
            Spacer(minLength: 8)
            Text("\(subgroup.agents.count)")
                .font(.caption2.weight(.medium))
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
    private func agentRow(_ agent: Agent, repos: [String],
                          hideRepoLabel: Bool) -> some View {
        Button {
            // #364 A.2: a real row tap is a discrete action — one light
            // selection tick (drags that cancel never reach the action).
            model.requestRecents(for: agent.agentId, haptic: true)
        } label: {
            AgentRow(agent: agent,
                     stateEnteredAt: model.fleet.stateEnteredAt[agent.agentId],
                     repos: repos,
                     hideRepoLabel: hideRepoLabel)
        }
        .buttonStyle(BoardPressStyle())
        .accessibilityElement(children: .combine)
        .accessibilityLabel(rowSummary(agent))
        .accessibilityHint("Double tap to open recent output")
    }

    /// #401 D5/D6/C6/C7 multi-host board renderer: status sections first,
    /// then merged repo subgroups (same repo name from several hosts shares
    /// one subgroup — no host sections/tabs). Rows arrive ALREADY ranked by
    /// #400 (canonical + live-first inside each bucket); this renderer adds
    /// the D6 compact host badge (All Hosts only) and the C6 stale/last-seen
    /// line on retained stale rows — the raw state token is never recast.
    @ViewBuilder
    private func hostBoardSections(sections: BoardModel.HostSections,
                                   repos: [String],
                                   hideRepoLabels: Bool,
                                   showHostBadges: Bool) -> some View {
        ForEach(sections.statuses) { status in
            Section {
                if !sectionCollapse.isCollapsed(status.state) {
                    ForEach(status.subgroups) { subgroup in
                        hostRepoSubgroupHeader(subgroup, repos: repos)
                        ForEach(subgroup.rows) { row in
                            hostAgentRow(row, repos: repos,
                                         hideRepoLabels: hideRepoLabels,
                                         showHostBadge: showHostBadges)
                                // Composite row id — equal raw agent ids on
                                // two hosts never collide as anchors.
                                .id(row.id)
                                .listRowBackground(theme.base)
                        }
                    }
                }
            } header: {
                PinnedHeader(fillsInteractiveWidth: true) {
                    hostStatusSectionBar(status)
                }
            }
        }
    }

    /// The #401 host-board status bar — the same thick full-width #386 bar
    /// visual (state mark, raw status + TOTAL count, collapse chevron) for
    /// the multi-host sections; the collapse state is shared with the
    /// single-host board (`sectionCollapse`, session-only).
    @ViewBuilder
    private func hostStatusSectionBar(
        _ status: BoardModel.HostStatusSection) -> some View {
        let isCollapsed = sectionCollapse.isCollapsed(status.state)
        Button {
            sectionCollapse.toggle(status.state)
        } label: {
            HStack(spacing: 8) {
                RoundedRectangle(cornerRadius: 2)
                    .fill(theme.stateColor(for: status.state))
                    .frame(width: 10, height: 10)
                    .accessibilityHidden(true)
                Text(status.header)
                    .font(.headline.weight(.bold))
                    .foregroundStyle(theme.text)
                    .lineLimit(1)
                Spacer(minLength: 8)
                Image(systemName: "chevron.down")
                    .font(.subheadline.weight(.semibold))
                    .foregroundStyle(theme.subtext1)
                    .rotationEffect(.degrees(isCollapsed ? -90 : 0))
                    .accessibilityHidden(true)
            }
            .padding(.horizontal, 20)
            .padding(.vertical, 12)
            .frame(maxWidth: .infinity, minHeight: 44, alignment: .leading)
            .background(theme.surface1)
        }
        .buttonStyle(BoardPressStyle())
        .accessibilityElement(children: .ignore)
        .accessibilityLabel(status.header)
        .accessibilityValue(isCollapsed ? "Collapsed" : "Expanded")
        .accessibilityHint("Toggles the \(status.state.displayName) section")
    }

    /// The #401 repo subgroup caption — the same demoted caption row visual
    /// as the single-host board, fed by a multi-host subgroup (rows instead
    /// of agents; the count is the subgroup's row count).
    @ViewBuilder
    private func hostRepoSubgroupHeader(_ subgroup: BoardModel.HostRepoSubgroup,
                                        repos: [String]) -> some View {
        let hue = theme.repoHue(for: subgroup.repo ?? "", among: repos)
        HStack(spacing: 7) {
            RoundedRectangle(cornerRadius: 2)
                .fill(theme.color(hue))
                .frame(width: 8, height: 8)
                .accessibilityHidden(true)
            Text(subgroup.displayName)
                .font(.caption2.weight(.semibold))
                .foregroundStyle(theme.subtext1)
                .lineLimit(1)
            Spacer(minLength: 8)
            Text("\(subgroup.rows.count)")
                .font(.caption2.weight(.medium))
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

    /// One #401 composite board row: same tap surface as the single-host
    /// row but the recents request carries the row's HOST profile (E1 —
    /// an equal raw id on another host can never be opened), the state
    /// duration reads the OWNING store's stateEnteredAt, and the row
    /// carries the compact textual host badge (D6) + the stale/last-seen
    /// marker (C6).
    @ViewBuilder
    private func hostAgentRow(_ row: HostBoardRow, repos: [String],
                              hideRepoLabels: Bool,
                              showHostBadge: Bool) -> some View {
        let hostName = showHostBadge ? hostDisplayName(for: row) : nil
        Button {
            model.requestRecents(for: row.agent.agentId,
                                 hostProfileID: row.identity.hostProfileID,
                                 haptic: true)
        } label: {
            VStack(alignment: .leading, spacing: 2) {
                AgentRow(agent: row.agent,
                         stateEnteredAt: model.stateEnteredAt(
                            hostProfileID: row.identity.hostProfileID,
                            agentID: row.agent.agentId),
                         repos: repos,
                         hideRepoLabel: hideRepoLabels)
                HStack(spacing: 6) {
                    if let hostName {
                        HostBadgeChip(name: hostName)
                    }
                    if row.isStale {
                        StaleRowLabel(lastSeenMs: row.lastSeen)
                    }
                }
                .padding(.top, 1)
            }
        }
        .buttonStyle(BoardPressStyle())
        .accessibilityElement(children: .combine)
        .accessibilityLabel(hostRowSummary(row, hostName: hostName))
        .accessibilityHint("Double tap to open recent output")
    }

    /// The display name of a row's owning host (D6 badge text). Rows always
    /// come from configured profiles, but a mid-render removal yields nil —
    /// the badge simply disappears with the host.
    private func hostDisplayName(for row: HostBoardRow) -> String? {
        model.profiles.first { $0.id == row.identity.hostProfileID }?.displayName
    }

    /// One VoiceOver summary for a composite board row: the agent identity,
    /// state, repo/branch/pane, plus the host badge and staleness facts —
    /// color never carries the meaning alone (D8).
    private func hostRowSummary(_ row: HostBoardRow, hostName: String?) -> String {
        var parts: [String] = [
            row.agent.title ?? row.agent.displayName ?? row.agent.agentId,
            "State: \(StateStyle.style(for: row.agent.state).label)",
        ]
        if let repo = row.agent.workspace.repo { parts.append(repo) }
        if let branch = row.agent.workspace.branch { parts.append(branch) }
        if let reference = row.agent.attachment?.reference {
            parts.append("Pane \(reference)")
        }
        if let hostName {
            parts.append("Host \(hostName)")
        }
        if row.isStale {
            parts.append("stale")
            if row.lastSeen > 0 {
                let now = UInt64(Date().timeIntervalSince1970 * 1000)
                parts.append(RelativeTime.lastSeenLabel(lastSeenMs: row.lastSeen,
                                                        nowMs: now))
            }
        }
        return parts.joined(separator: ", ")
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
        } else if CorralDemoLaunch.wantsGlassEvidence(arguments: CommandLine.arguments) {
            await runGlassSequence()
        } else if CorralDemoLaunch.wantsRepoLabelEvidence(arguments: CommandLine.arguments) {
            await runRepoLabelSequence()
        } else if CorralDemoLaunch.wantsCollapseEvidence(arguments: CommandLine.arguments) {
            await runCollapseSequence()
        } else if CorralDemoLaunch.wantsTitleEvidence(arguments: CommandLine.arguments) {
            await runTitleSequence()
        } else if CorralDemoLaunch.wantsConnectionInputsEvidence(arguments: CommandLine.arguments) {
            await runConnectionInputsSequence()
        } else if CorralDemoLaunch.wantsDeniedNotificationsEvidence(arguments: CommandLine.arguments) {
            await runDeniedNotificationsSequence()
        } else if CorralDemoLaunch.wantsMultiHostBoardEvidence(arguments: CommandLine.arguments) {
            await runMultiHostBoardSequence()
        } else if CorralDemoLaunch.wantsMultiHostSettingsEvidence(arguments: CommandLine.arguments) {
            await runMultiHostSettingsSequence()
        } else if CorralDemoLaunch.wantsMultiHostAddEvidence(arguments: CommandLine.arguments) {
            await runMultiHostAddSequence()
        } else if CorralDemoLaunch.wantsAddHostBgReturnEvidence(arguments: CommandLine.arguments)
                    || CorralDemoLaunch.wantsAddHostFailedEvidence(arguments: CommandLine.arguments)
                    || CorralDemoLaunch.wantsAddHostCommitEvidence(arguments: CommandLine.arguments) {
            await runAddHostLifecycleSequence()
        }
    }

    /// #415 evidence: opens the Settings sheet so its DEBUG task presents
    /// the AddHostSheet, whose own DEBUG task records the bg-return /
    /// failed-submit / successful-commit phases (simctl cannot tap the
    /// gear or the Add host row). The board itself stays behind the
    /// sheets — the frames only ever show the pairing surfaces.
    private func runAddHostLifecycleSequence() async {
        guard model.mode == .demo else { return }
        guard await themePause(0) else { return }
        theme.setFlavor(.mocha)
        showSettings = true
        _ = await themePause(1000)
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

    /// #385 evidence: one deterministic launch records the translucent
    /// sheets over the busy board — Mocha board → Mocha recents sheet →
    /// Latte recents sheet (live flavor flip while presented) → Mocha
    /// Settings sheet → Latte Settings sheet. The driver flips the same
    /// state the gear button and row taps set (simctl cannot inject
    /// touches) and the same flavor the Appearance rows set; each phase
    /// writes a marker file the host screenshot script observes.
    /// Cancellation-aborts like the other drivers, so a raced task can
    /// never corrupt the recorded sequence.
    private func runGlassSequence() async {
        guard model.mode == .demo else { return }
        guard await themePause(0) else { return }
        theme.setFlavor(.mocha)
        EvidenceMarkers.write("phase-1-board-mocha")
        guard await themePause(4000) else { return }
        model.requestRecents(for: DemoFleet.featuredAgentID, haptic: false)
        guard await themePause(4000) else { return }
        EvidenceMarkers.write("phase-2-recents-mocha")
        guard await themePause(5000) else { return }
        theme.setFlavor(.latte)
        guard await themePause(4000) else { return }
        EvidenceMarkers.write("phase-3-recents-latte")
        guard await themePause(5000) else { return }
        model.recentsRequest = nil
        guard await themePause(2000) else { return }
        // #385 A/B evidence: the Latte BOARD alone at the same scroll
        // position — the recents sheet frame (phase-3) can then be
        // pixel-compared against the exact underlying content it covers.
        EvidenceMarkers.write("phase-4-board-latte")
        guard await themePause(4000) else { return }
        theme.setFlavor(.mocha)
        guard await themePause(1000) else { return }
        showSettings = true
        guard await themePause(4000) else { return }
        EvidenceMarkers.write("phase-5-settings-mocha")
        guard await themePause(5000) else { return }
        theme.setFlavor(.latte)
        guard await themePause(4000) else { return }
        EvidenceMarkers.write("phase-6-settings-latte")
        guard await themePause(5000) else { return }
        showSettings = false
        guard await themePause(1500) else { return }
        EvidenceMarkers.write("phase-7-done")
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

    /// #386 evidence: one deterministic launch records the board-hierarchy
    /// change — Mocha board with the BLOCKED status section COLLAPSED
    /// (thick bar alone, chevron rotated right) above the EXPANDED working
    /// section (thick bar + small repo captions + rows), the SAME collapse
    /// in Latte after a live flavor flip, then every remaining section
    /// collapsed so the frame proves an empty section still renders its
    /// bar (counts on the bar). The driver flips the same state a bar tap
    /// sets (`sectionCollapse.collapse` — IDEMPOTENT: the `.task(id:)`
    /// evidence hook can fire twice on demo entry and a second pass must
    /// never undo the first; the interactive bar keeps `toggle`) and the
    /// same flavor the Appearance rows set — simctl cannot inject touches.
    /// Cancellation-aborts like the other drivers, so a raced task can
    /// never corrupt the sequence.
    private func runCollapseSequence() async {
        guard model.mode == .demo else { return }
        guard await themePause(0) else { return }
        theme.setFlavor(.mocha)
        // Collapse the blocked section first and let the update settle
        // before the marker so the captured frame is the settled state.
        sectionCollapse.collapse(.blocked)
        guard await themePause(800) else { return }
        EvidenceMarkers.write("phase-1-board-mocha")
        guard await themePause(5000) else { return }
        theme.setFlavor(.latte)
        guard await themePause(4000) else { return }
        EvidenceMarkers.write("phase-2-board-latte")
        guard await themePause(5000) else { return }
        // Collapse every remaining section: the board must show five bars
        // and nothing else (an empty status section still renders its bar).
        for state in AgentState.allCases {
            sectionCollapse.collapse(state)
        }
        EvidenceMarkers.write("phase-3-all-collapsed-latte")
        guard await themePause(5000) else { return }
        EvidenceMarkers.write("phase-4-done")
        _ = await themePause(1500)
    }

    /// #384 evidence: one deterministic launch records the row-label
    /// visibility rule — Mocha board with EVERY row showing its per-row
    /// repo label chip ('All'), then the demo-atlas repo pill active (the
    /// board shows only demo-atlas rows WITHOUT repo name labels — only
    /// the color-only hue echo stays and rows keep their height), then
    /// 'All' restored (labels back instantly), and the SAME All →
    /// filtered → All trio in Latte after a live flavor flip. The driver
    /// flips the same `model.repoFilter` state the chips row sets and the
    /// same flavor the Appearance rows set — simctl cannot inject touches.
    /// Cancellation-aborts like the other drivers, so a raced task can
    /// never corrupt the captured sequence.
    private func runRepoLabelSequence() async {
        guard model.mode == .demo else { return }
        guard await themePause(0) else { return }
        theme.setFlavor(.mocha)
        model.repoFilter = nil
        EvidenceMarkers.write("phase-1-board-mocha-all")
        guard await themePause(5000) else { return }
        model.repoFilter = "demo-atlas"
        guard await themePause(2000) else { return }
        EvidenceMarkers.write("phase-2-board-mocha-filtered")
        guard await themePause(5000) else { return }
        model.repoFilter = nil
        guard await themePause(2000) else { return }
        EvidenceMarkers.write("phase-3-board-mocha-restored-all")
        guard await themePause(5000) else { return }
        theme.setFlavor(.latte)
        guard await themePause(2000) else { return }
        EvidenceMarkers.write("phase-4-board-latte-all")
        guard await themePause(5000) else { return }
        model.repoFilter = "demo-atlas"
        guard await themePause(2000) else { return }
        EvidenceMarkers.write("phase-5-board-latte-filtered")
        guard await themePause(5000) else { return }
        model.repoFilter = nil
        guard await themePause(2000) else { return }
        EvidenceMarkers.write("phase-6-board-latte-restored-all")
        guard await themePause(5000) else { return }
        EvidenceMarkers.write("phase-7-done")
        _ = await themePause(1500)
    }

    /// #387 evidence: one deterministic launch records the chrome-only
    /// board header — Mocha at the TOP of the board and MOCHA SCROLLED
    /// (the nav bar collapsed, still title-free), then the same pair in
    /// Latte after a live flavor flip. simctl cannot drag the list, so
    /// the driver scrolls through ScrollViewReader requests (instant +
    /// idempotent — a re-fired evidence task cannot corrupt a phase);
    /// each phase writes a marker the host screenshot script observes.
    private func runTitleSequence() async {
        guard model.mode == .demo else { return }
        guard await themePause(0) else { return }
        theme.setFlavor(.mocha)
        // Deterministic top for EVERY pass (the .task(id:) evidence hook
        // can fire twice on demo entry): the chips row is the demo
        // board's first content row, so landing it at the top edge IS the
        // top-of-board state.
        evidenceScrollTarget = BoardEvidenceAnchor.top
        guard await themePause(2000) else { return }
        EvidenceMarkers.write("phase-1-board-mocha-top")
        guard await themePause(4000) else { return }
        // Scroll below the fold: the nav bar rides its collapsed state.
        evidenceScrollTarget = BoardEvidenceAnchor.belowFold
        guard await themePause(2000) else { return }
        EvidenceMarkers.write("phase-2-board-mocha-scrolled")
        guard await themePause(4000) else { return }
        theme.setFlavor(.latte)
        guard await themePause(1500) else { return }
        // Latte, still scrolled: the SAME collapsed chrome on the light
        // palette (a live flavor flip must not restore any title).
        EvidenceMarkers.write("phase-3-board-latte-scrolled")
        guard await themePause(4000) else { return }
        evidenceScrollTarget = BoardEvidenceAnchor.top
        guard await themePause(2000) else { return }
        EvidenceMarkers.write("phase-4-board-latte-top")
        guard await themePause(4000) else { return }
        EvidenceMarkers.write("phase-5-done")
        _ = await themePause(1500)
    }

    /// #388 evidence: the Settings Connection section — UNPAIRED (host +
    /// token + Register on the themed surface1 fields) and PAIRED (host +
    /// registration status row, NO token field) — across Macchiato, Mocha,
    /// and Latte. The driver seeds the demo registration key id for the
    /// paired phases (the sim has no daemon to register against; the
    /// published key id is exactly what the section reads) and flips the
    /// same flavor state the Appearance rows set — simctl cannot inject
    /// taps or type, so this is the synthetic stand-in. Cancellation-aborts
    /// like the other drivers, so a raced task can never corrupt the
    /// recorded sequence.
    ///
    /// Each phase writes its marker AFTER the state settles (2.5 s) and
    /// then HOLDS 9 s so the host capture script — whose simctl screenshot
    /// can lag several seconds on a cold sim — always lands inside the
    /// phase it names (a 4 s window raced the capture and the frame
    /// drifted into the NEXT phase's flavor).
    private func runConnectionInputsSequence() async {
        guard model.mode == .demo else { return }
        guard await themePause(0) else { return }
        // Unpaired phases — a fresh demo holds no key id, so the section
        // renders host + token + Register on every palette.
        theme.setFlavor(.macchiato)
        guard await themePause(2500) else { return }
        showSettings = true
        guard await themePause(2500) else { return }
        EvidenceMarkers.write("phase-1-settings-macchiato-unpaired")
        guard await themePause(9000) else { return }
        theme.setFlavor(.mocha)
        guard await themePause(2500) else { return }
        EvidenceMarkers.write("phase-2-settings-mocha-unpaired")
        guard await themePause(9000) else { return }
        theme.setFlavor(.latte)
        guard await themePause(2500) else { return }
        EvidenceMarkers.write("phase-3-settings-latte-unpaired")
        guard await themePause(9000) else { return }
        // Paired phases: seed the registered identity the way a successful
        // register() stores it (DEBUG driver only — no daemon is involved,
        // so nothing else reacts to the key id; mode stays .demo, so no
        // live networking starts). The section now shows the status row
        // and hides the token field.
        // SAFETY: fixed literal demo device key id — DEBUG evidence driver
        // only; the real daemon derives ids as dev_<hex(sha256(pubkey)[..16])>.
        model.keyId = "dev_3f88a1b2c3d4e5f6"
        guard await themePause(2500) else { return }
        EvidenceMarkers.write("phase-4-settings-latte-paired")
        guard await themePause(9000) else { return }
        theme.setFlavor(.mocha)
        guard await themePause(2500) else { return }
        EvidenceMarkers.write("phase-5-settings-mocha-paired")
        guard await themePause(9000) else { return }
        theme.setFlavor(.macchiato)
        guard await themePause(2500) else { return }
        EvidenceMarkers.write("phase-6-settings-macchiato-paired")
        guard await themePause(9000) else { return }
        showSettings = false
        guard await themePause(2000) else { return }
        EvidenceMarkers.write("phase-7-done")
        _ = await themePause(2000)
    }

    /// #389 evidence: the Notifications section's DENIED guidance — Mocha
    /// Settings sheet scrolled (by the SettingsView DEBUG scroll task — the
    /// section sits below Appearance + Connection + Device on 390x844 and
    /// simctl cannot drag) to the Notifications anchor showing the 'Corral
    /// can't alert you…' blocked row + the 'Open iOS Settings' action. The
    /// denied posture is FORCED by the launch argument in demo mode (a
    /// simulator cannot be denied notifications: `simctl privacy` has no
    /// notifications service and the OS alert cannot be answered without
    /// touch injection) — the frame is the synthetic stand-in the unit
    /// suite pins. Marker AFTER the state settles, then a 9 s hold so the
    /// host capture always lands inside the named phase. Marker names are
    /// unique to this driver so the wiring tests can pin single writes.
    private func runDeniedNotificationsSequence() async {
        guard model.mode == .demo else { return }
        guard await themePause(0) else { return }
        theme.setFlavor(.mocha)
        EvidenceMarkers.write("phase-1-denied-mocha-board")
        guard await themePause(2000) else { return }
        showSettings = true
        guard await themePause(2500) else { return }
        EvidenceMarkers.write("phase-2-denied-settings-notifications")
        guard await themePause(9000) else { return }
        showSettings = false
        guard await themePause(1500) else { return }
        EvidenceMarkers.write("phase-3-denied-done")
        _ = await themePause(1500)
    }

    /// #401 evidence: the multi-host board sequence — All Hosts (Host A
    /// live rows + Host B RETAINED STALE rows with last-seen age + badges,
    /// "1 host offline" summary), then the Host A filter (badges gone, repo
    /// chips rescoped), then Latte All Hosts, then Host B alone (stale rows
    /// only), then a host+repo combination — Mocha + Latte per the locked
    /// evidence gate. The driver flips the same `model.hostFilter` /
    /// `repoFilter` state the chips set and the same flavor the Appearance
    /// rows set — simctl cannot inject taps. Cancellation-aborts like the
    /// other drivers, so a raced task can never corrupt the sequence.
    private func runMultiHostBoardSequence() async {
        guard model.mode == .demo, model.multiHostConfigured else { return }
        guard await themePause(0) else { return }
        theme.setFlavor(.mocha)
        model.selectHostFilter(nil)
        model.repoFilter = nil
        guard await themePause(2500) else { return }
        EvidenceMarkers.write("phase-1-mh-board-all-mocha")
        guard await themePause(9000) else { return }
        model.selectHostFilter(model.profiles.first { $0.displayName == "Host A" }?.id)
        guard await themePause(2000) else { return }
        EvidenceMarkers.write("phase-2-mh-board-host-a-mocha")
        guard await themePause(9000) else { return }
        theme.setFlavor(.latte)
        model.selectHostFilter(nil)
        guard await themePause(2500) else { return }
        EvidenceMarkers.write("phase-3-mh-board-all-latte")
        guard await themePause(9000) else { return }
        model.selectHostFilter(model.profiles.first { $0.displayName == "Host B" }?.id)
        guard await themePause(2000) else { return }
        EvidenceMarkers.write("phase-4-mh-board-host-b-latte")
        guard await themePause(9000) else { return }
        // Host A + repo demo-atlas: both filters apply together (D4).
        model.selectHostFilter(model.profiles.first { $0.displayName == "Host A" }?.id)
        model.repoFilter = "demo-atlas"
        guard await themePause(2000) else { return }
        EvidenceMarkers.write("phase-5-mh-board-host-repo-latte")
        guard await themePause(9000) else { return }
        model.selectHostFilter(nil)
        model.repoFilter = nil
        theme.setFlavor(.mocha)
        guard await themePause(1500) else { return }
        EvidenceMarkers.write("phase-6-mh-board-done")
        _ = await themePause(1500)
    }

    /// #401 evidence: the Settings Hosts list — every configured host's row
    /// (health, URL, fingerprint/key-id/grants-expiry, error/last seen,
    /// Retry/Rename/Remove, Host C's key-mismatch notice) in Mocha, then
    /// Latte after a live flavor flip. The SettingsView DEBUG task scrolls
    /// the Hosts anchor into view (the section sits below Connection on
    /// 390x844-class screens and simctl cannot drag).
    private func runMultiHostSettingsSequence() async {
        guard model.mode == .demo, model.multiHostConfigured else { return }
        guard await themePause(0) else { return }
        theme.setFlavor(.mocha)
        showSettings = true
        // SettingsView's own DEBUG task scrolls settings.hosts into view.
        guard await themePause(6000) else { return }
        EvidenceMarkers.write("phase-1-mh-settings-mocha")
        guard await themePause(9000) else { return }
        theme.setFlavor(.latte)
        guard await themePause(4000) else { return }
        EvidenceMarkers.write("phase-2-mh-settings-latte")
        guard await themePause(9000) else { return }
        showSettings = false
        guard await themePause(1500) else { return }
        EvidenceMarkers.write("phase-3-mh-settings-done")
        _ = await themePause(1500)
    }

    /// #401 evidence: the Add Host sheet driver — opens Settings so its
    /// DEBUG task presents the AddHostSheet, whose own DEBUG task records
    /// the entry (B3 name prefill) + fingerprint-confirmation phases and
    /// dismisses itself after the final marker.
    private func runMultiHostAddSequence() async {
        guard model.mode == .demo, model.multiHostConfigured else { return }
        guard await themePause(0) else { return }
        theme.setFlavor(.mocha)
        showSettings = true
        _ = await themePause(1000)
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

/// #387 evidence scroll anchors: `.top` is the demo board's first content
/// row (the filter-chips row — see repoChipsRow's `.id("board.filter-chips")`);
/// `.belowFold` is a demo row below the first-viewport fold (the idle
/// agent — every agent row carries its agent id), so scrolling it to the
/// viewport top edge puts the nav bar in its fully collapsed state.
/// simctl cannot drag, so the title-evidence driver scrolls through
/// ScrollViewReader requests.
enum BoardEvidenceAnchor {
    static let top = "board.filter-chips"
    static let belowFold = "herdr:demo-ledger-idle"
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

// MARK: - #385 Liquid Glass / translucent sheet backdrop

/// The shared #385 sheet backdrop: RecentOutputSheet and the Settings sheet
/// float over this so the board content behind shows through softly (the
/// approved terminal-transparency look). The sheets' TEXT layers keep their
/// opaque token backing (cards, native cells, the recents header strip's
/// caption row) — the translucency lives in the sheet background between
/// and around them, so every text tier keeps its current AA contrast while
/// the board reads through the glass.
///
/// - iOS 26+: the NATIVE Liquid Glass surface — SwiftUI `glassEffect`,
///   availability-gated at compile time, tinted with the active flavor's
///   base token through the API's theme hook (`Glass.tint`).
/// - iOS 17–25: the translucent fallback — the flavor's base at the locked
///   `SheetBackdrop.fallbackTintAlpha` (0.85–0.90 spec band) over an
///   ultra-thin material blur. Deployment target is 17.0, so this is what
///   older runtimes actually render; `SheetBackdropTests` locks the
///   constants and the 4.5:1 worst-case contrast math.
private struct TranslucentSheetBackdrop: View {
    /// The active flavor's base token (resolved by the caller so a live
    /// flavor flip re-creates the backdrop with the new tint).
    let tint: Color

    var body: some View {
        ZStack {
            if #available(iOS 26.0, *) {
                // #385 iOS 26+: Native Liquid Glass. The tint stays at
                // `SheetBackdrop.glassTintOpacity` — a full-opacity tint
                // paints the glass into a flat solid (measured on the 26.5
                // sim) and hides the board behind the sheet entirely. The
                // CLEAR style is used rather than `.regular`: over the
                // system dimming scrim the regular glass reads as an
                // opaque dark slab; clear glass keeps the terminal
                // transparency the approved spec calls for (board content
                // visibly through the sheet — pixel-verified).
                Rectangle()
                    .fill(Color.clear)
                    .glassEffect(.clear
                        .tint(tint.opacity(SheetBackdrop.glassTintOpacity)),
                        in: Rectangle())
            } else {
                Rectangle().fill(.ultraThinMaterial)
                tint.opacity(SheetBackdrop.fallbackTintAlpha)
            }
        }
        .accessibilityHidden(true)
    }
}

extension View {
    /// #385: give a sheet presentation the shared translucent backdrop
    /// (native Liquid Glass on iOS 26+, tinted-material fallback below).
    func translucentSheetBackdrop(_ tint: Color) -> some View {
        presentationBackground { TranslucentSheetBackdrop(tint: tint) }
    }
}

// MARK: - Settings (Appearance, connection pairing, device identity, notifications, help)

/// #388: ONE themed input surface shared by the Settings Connection
/// fields (Host + Registration token). The old `.roundedBorder` default
/// rendered SQUARE, near-black boxes under the forced dark scheme; every
/// color here is a Catppuccin token — surface1 fill (never near-black),
/// text ink, subtext0 placeholder, a 10 pt continuous corner radius (no
/// squares), and a hairline surface2 border that turns accent while the
/// field is focused — so the fields follow the active flavor on all four
/// palettes, Latte light included.
///
/// The placeholder is drawn by the view itself (hidden while text is
/// present): SwiftUI exposes no public placeholder-color API, so a system
/// placeholder would ignore the palette tokens.
private struct ConnectionField: View {
    @EnvironmentObject private var theme: ThemeStore
    let title: String
    let secure: Bool
    @Binding var text: String

    @FocusState private var focused: Bool

    var body: some View {
        ZStack(alignment: .leading) {
            if text.isEmpty {
                Text(title)
                    .foregroundStyle(theme.subtext0)
                    .allowsHitTesting(false)
                    .accessibilityHidden(true)
            }
            if secure {
                SecureField("", text: $text)
                    .focused($focused)
                    .foregroundStyle(theme.text)
                    .tint(theme.accent)
                    .accessibilityLabel(title)
            } else {
                TextField("", text: $text)
                    .focused($focused)
                    .foregroundStyle(theme.text)
                    .tint(theme.accent)
                    .textInputAutocapitalization(.never)
                    .autocorrectionDisabled()
                    .accessibilityLabel(title)
            }
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 8)
        .background(theme.surface1,
                    in: RoundedRectangle(cornerRadius: 10, style: .continuous))
        .overlay {
            RoundedRectangle(cornerRadius: 10, style: .continuous)
                .strokeBorder(focused ? theme.accent : theme.surface2,
                              lineWidth: 1)
        }
    }
}

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
///
/// #388: once the device is REGISTERED the Connection section hides the
/// Registration-token field — the host stays editable, a status row + a
/// small Re-register action replace the pairing rows, and Re-register
/// reveals the token field again. Remove device returns the unpaired
/// form naturally.
struct SettingsView: View {
    @ObservedObject var model: AppModel
    @Environment(\.dismiss) private var dismiss
    @EnvironmentObject private var theme: ThemeStore
    /// #389: the blocked-permission escape hatch — opens THIS app's
    /// notification permission in the system Settings app.
    @Environment(\.openURL) private var openURL

    @State private var host: String
    @State private var token = ""
    @State private var registering = false
    /// #399: Add Host sheet (fingerprint-verified pairing, B3).
    @State private var showAddHost = false
    /// #401 D7: the host row currently awaiting Remove-Host confirmation.
    @State private var hostBeingRemoved: HostProfile?
    /// #401 D7: the host row currently being renamed (display name only).
    @State private var hostBeingRenamed: HostProfile?
    @State private var renameDraft = ""
    /// #401 D7: per-host inline errors (e.g. a rejected rename) — keyed by
    /// profile id so a failed rename lands on the row that failed.
    @State private var hostRowErrors: [UUID: String] = [:]
    /// #388: the paired section's small 'Re-register' action sets this —
    /// revealing the Registration-token field + Register button again so a
    /// registered device can re-point at a new host. Unpaired devices see
    /// the token field unconditionally; Remove device (Device section)
    /// clears the identity and the sheet reopens in the unpaired state.
    @State private var revealTokenField = false
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

    /// #388: the compact key-id spelling shared by the Device read-out row
    /// and the paired Connection status row (first 16 characters).
    private var deviceKeyID: String {
        String((model.keyId ?? "—").prefix(16))
    }

    /// #389: the blocked-permission escape hatch. `UIApplication
    /// .openSettingsURLString` is the canonical way to open THIS app's
    /// notification permission in the system Settings app.
    private func openAppSettings() {
        guard let url = URL(string: UIApplication.openSettingsURLString) else { return }
        openURL(url)
    }

    var body: some View {
        NavigationStack {
            ScrollViewReader { proxy in
                Form {
                    appearanceSection
                    Section("Connection") {
                        ConnectionField(title: "Host (Tailscale host or loopback)",
                                        secure: false,
                                        text: $host)
                        // #388: once the device is REGISTERED the
                        // Registration-token field is pointless — the
                        // section shows the host (still editable so a
                        // paired device can re-point) + the registration
                        // status row + a small Re-register action that
                        // reveals the token field again. Remove device in
                        // the Device section clears the identity and the
                        // unpaired form below returns naturally.
                        if model.isRegistered && !revealTokenField {
                            Text("Device registered · Key ID \(deviceKeyID) · read-only signed")
                                .font(.caption)
                                .foregroundStyle(theme.subtext1)
                            Button("Re-register") {
                                revealTokenField = true
                            }
                            .font(.subheadline)
                        } else {
                            ConnectionField(title: "Registration token",
                                            secure: true,
                                            text: $token)
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
                    }
                    if model.hostProfilesConfigured {
                        hostsSection
                    }
                    Section {
                        // #379 evidence: the connect-evidence Settings
                        // frame scrolls this anchor into view (the Device
                        // read-out sits below Appearance + Connection and
                        // simctl cannot scroll the form).
                        LabeledContent("Key ID", value: deviceKeyID)
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
                        // #397: with 2+ hosts every host enrolls and
                        // notifies INDEPENDENTLY — per-host state lives on
                        // each Hosts row below (enroll / clear that host's
                        // token); this is the retained GLOBAL control.
                        // Pending empty-token cleanups (per-host disable or
                        // host removal while the host was unreachable)
                        // surface here and clear when that host reconnects.
                        Toggle("State-change notifications",
                               isOn: Binding(
                                // #389: while the permission is BLOCKED the
                                // switch shows OFF regardless of the persisted
                                // intent — the blocked row below explains why
                                // and the 'Open iOS Settings' action is the
                                // path back. When the user allows
                                // notifications in the system Settings and
                                // returns, the persisted intent (if it was
                                // ON) resurfaces automatically.
                                get: { model.notificationsEnabled
                                    && !model.notificationPermission.showsBlockedGuidance },
                                set: { model.setNotificationsEnabled($0) }))
                        // #389 evidence + DEBUG scroll: the row-level anchor
                        // (same convention as #379's settings.device) lets
                        // the denied-state driver bring the Notifications
                        // section into view — Appearance + Connection +
                        // Device sit above it on 390x844 and simctl cannot
                        // scroll the form.
                        .id("settings.notifications")
                        // #389: a blocked permission (.denied/.restricted)
                        // shows WHY + an 'Open iOS Settings' action instead
                        // of the enable toggle silently failing — iOS
                        // delivers nothing (no APNs token, no local alert)
                        // until the user allows notifications there. The
                        // status is refreshed on Settings appear and on
                        // every foreground (refreshNotificationPermission).
                        if model.notificationPermission.showsBlockedGuidance {
                            Label("Corral can't alert you — notifications are off for this app in iOS Settings.",
                                  systemImage: "bell.slash")
                                .font(.caption)
                                .foregroundStyle(theme.peach)
                            Button("Open iOS Settings") {
                                openAppSettings()
                            }
                            .font(.subheadline)
                        } else {
                            Text("Alerts when an agent starts, blocks, or finishes on any paired host. Each host's own state is on its row in the Hosts section. No badges or catch-up.")
                                .font(.caption)
                                .foregroundStyle(theme.subtext1)
                        }
                        if !model.pendingPushTokenClears.isEmpty {
                            let names = model.pendingPushClearNames()
                            Label("Notification enrollment cleanup pending for: \(names.joined(separator: ", ")). It clears when that host reconnects; a removed host that never returns must be dropped host-side.",
                                  systemImage: "arrow.triangle.2.circlepath")
                                .font(.caption)
                                .foregroundStyle(theme.peach)
                                .id("settings.notifications.pending-clear")
                        }
                    }
                    .task { await model.refreshNotificationPermission() }
                }
                .navigationTitle("Settings")
                .toolbar {
                    // #401 D2: drag-to-reorder the Hosts rows (2+ hosts) —
                    // the same store order the board's host chips follow.
                    ToolbarItem(placement: .topBarLeading) {
                        if model.profiles.count > 1 {
                            EditButton()
                        }
                    }
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
                // #385: the old opaque `.background(theme.base)` here is gone —
                // the sheet now floats over the shared translucent backdrop
                // (presentationBackground below), so the form's inter-cell
                // surface shows the board content softly instead of painting
                // an opaque base over the whole sheet.
                .scrollContentBackground(.hidden)
                .preferredColorScheme(theme.flavor.isLight ? .light : .dark)
                // #379: the '?' Help entry presents the shared connect sheet
                // from INSIDE this sheet's hierarchy (sheet-over-sheet needs
                // the inner presentation modifier in the presented tree).
                .sheet(isPresented: $showConnectHelp) {
                    HowToConnectSheet(host: host)
                }
                // #399: the Add Host sheet (fingerprint-verified pairing)
                // presents from inside Settings, over the same backdrop.
                .sheet(isPresented: $showAddHost) {
                    AddHostSheet(model: model)
                }
                // #401 D7: the per-host Rename alert — display name only
                // (B5: URL/identity changes are remove-and-re-pair).
                .alert("Rename host",
                       isPresented: Binding(
                        get: { hostBeingRenamed != nil },
                        set: { if !$0 { hostBeingRenamed = nil } }),
                       presenting: hostBeingRenamed) { profile in
                    TextField("Host name", text: $renameDraft)
                    Button("Save") { renameHost(profile) }
                    Button("Cancel", role: .cancel) {
                        hostBeingRenamed = nil
                    }
                } message: { _ in
                    Text("Changes the display name only. The URL and host identity cannot be edited — changing either is remove-and-re-pair.")
                }
#if DEBUG
                .task { await scrollDeviceIntoViewForConnectEvidence(proxy) }
                // #389: the denied-state evidence driver scrolls the
                // Notifications section into view (its own .task — see
                // scrollNotificationsIntoViewForDeniedEvidence).
                .task { await scrollNotificationsIntoViewForDeniedEvidence(proxy) }
                // #401: the multi-host Settings evidence driver scrolls the
                // Hosts rows into view (see
                // scrollHostsIntoViewForMultiHostEvidence); the Add Host
                // evidence driver presents the AddHostSheet (see
                // presentAddHostForMultiHostEvidence).
                .task { await scrollHostsIntoViewForMultiHostEvidence(proxy) }
                .task { await presentAddHostForMultiHostEvidence() }
                // #415 evidence: the Add Host lifecycle drivers (bg-return
                // / failed-submit / successful-commit) present the same
                // sheet from the Settings state.
                .task { await presentAddHostForLifecycleEvidence() }
                // #415 evidence (c): when the AddHostSheet dismisses after
                // the successful commit, scroll the Hosts rows into view
                // for the "original Mac host still present" frame.
                .onChange(of: showAddHost) { _, presented in
                    guard !presented else { return }
                    Task { await scrollHostsForAddHostCommitEvidence(proxy) }
                }
#endif
            }
        }
        // #385: the Settings sheet floats over the shared translucent
        // backdrop (Liquid Glass on iOS 26+, tinted-material fallback
        // below) instead of painting an opaque base fill over the
        // presentation.
        .translucentSheetBackdrop(theme.base)
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

    /// #389 evidence: the denied-state Settings frame must show the
    /// Notifications section's blocked guidance. Appearance + Connection +
    /// Device push it below the fold on 390x844-class screens and simctl
    /// cannot scroll the form, so the DEBUG-only launch argument scrolls
    /// the `settings.notifications` anchor (on the toggle row) into view
    /// once the sheet settles — the scroll lands at the form's bottom,
    /// which is exactly the Notifications section.
    private func scrollNotificationsIntoViewForDeniedEvidence(_ proxy: ScrollViewProxy) async {
        guard CorralDemoLaunch.wantsDeniedNotificationsEvidence(arguments: CommandLine.arguments) else { return }
        try? await Task.sleep(for: .milliseconds(1200))
        guard CorralDemoLaunch.wantsDeniedNotificationsEvidence(arguments: CommandLine.arguments) else { return }
        withAnimation(.easeInOut(duration: 0.35)) {
            proxy.scrollTo("settings.notifications", anchor: .top)
        }
    }

    /// #401 evidence: the multi-host Settings frame must show the Hosts
    /// rows. Appearance + Connection push them below the fold on
    /// 390x844-class screens and simctl cannot drag, so the DEBUG-only
    /// launch argument scrolls the first host row's `settings.hosts`
    /// anchor into view once the sheet settles.
    private func scrollHostsIntoViewForMultiHostEvidence(_ proxy: ScrollViewProxy) async {
        guard CorralDemoLaunch.wantsMultiHostSettingsEvidence(arguments: CommandLine.arguments) else { return }
        try? await Task.sleep(for: .milliseconds(3500))
        guard CorralDemoLaunch.wantsMultiHostSettingsEvidence(arguments: CommandLine.arguments) else { return }
        withAnimation(.easeInOut(duration: 0.35)) {
            proxy.scrollTo("settings.hosts", anchor: .top)
        }
    }

    /// #415 evidence (c): after the successful commit's AddHostSheet
    /// dismissal, scroll the Hosts rows into view so the frame shows the
    /// original Mac host PLUS the exactly-one new host (simctl cannot
    /// drag; the hosts section sits below Connection).
    private func scrollHostsForAddHostCommitEvidence(_ proxy: ScrollViewProxy) async {
        guard CorralDemoLaunch.wantsAddHostCommitEvidence(arguments: CommandLine.arguments) else { return }
        try? await Task.sleep(for: .milliseconds(1200))
        guard CorralDemoLaunch.wantsAddHostCommitEvidence(arguments: CommandLine.arguments) else { return }
        withAnimation(.easeInOut(duration: 0.35)) {
            proxy.scrollTo("settings.hosts", anchor: .top)
        }
    }

    /// #401 evidence: the Add Host sheet driver presents the AddHostSheet
    /// from the multi-host Settings state (the sheet's own DEBUG task then
    /// records the entry + confirmation phases — simctl cannot tap the Add
    /// host row).
    private func presentAddHostForMultiHostEvidence() async {
        guard CorralDemoLaunch.wantsMultiHostAddEvidence(arguments: CommandLine.arguments) else { return }
        try? await Task.sleep(for: .milliseconds(3500))
        guard CorralDemoLaunch.wantsMultiHostAddEvidence(arguments: CommandLine.arguments) else { return }
        showAddHost = true
    }

    /// #415 evidence: presents the AddHostSheet from the Settings state
    /// for the bg-return / failed / commit drivers (simctl cannot tap the
    /// Add host row; the FleetView-level driver opened Settings first).
    private func presentAddHostForLifecycleEvidence() async {
        guard CorralDemoLaunch.wantsAddHostBgReturnEvidence(arguments: CommandLine.arguments)
                || CorralDemoLaunch.wantsAddHostFailedEvidence(arguments: CommandLine.arguments)
                || CorralDemoLaunch.wantsAddHostCommitEvidence(arguments: CommandLine.arguments) else { return }
        try? await Task.sleep(for: .milliseconds(3500))
        guard CorralDemoLaunch.wantsAddHostBgReturnEvidence(arguments: CommandLine.arguments)
                || CorralDemoLaunch.wantsAddHostFailedEvidence(arguments: CommandLine.arguments)
                || CorralDemoLaunch.wantsAddHostCommitEvidence(arguments: CommandLine.arguments) else { return }
        showAddHost = true
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

    /// #401 D2/D7: the Hosts section — one row per configured host in the
    /// USER-CONTROLLED order (drag to reorder with 2+ hosts; the board's
    /// host chips follow the same store order — D2), each row carrying the
    /// full per-host surface: connection posture + error, last seen,
    /// Retry, fingerprint (copyable), key id, grants/expiry, rename in
    /// place (B5) and Remove Host (B7 local unlink). The Add Host entry
    /// (fingerprint-verified pairing, B3) closes the section.
    private var hostsSection: some View {
        Section {
            ForEach(model.profiles) { profile in
                hostRow(profile)
                    // The first host row carries the Settings scroll anchor
                    // (evidence driver + a11y navigation target).
                    .id(profile == model.profiles.first
                        ? "settings.hosts"
                        : profile.id.uuidString)
            }
            .onMove(perform: moveHosts)
            Button {
                showAddHost = true
            } label: {
                Label("Add host", systemImage: "plus.circle")
            }
            .id("settings.add-host")
            .accessibilityHint("Pairs another corrald host with fingerprint verification")
        } header: {
            Text("Hosts")
        } footer: {
            Text(model.profiles.count > 1
                 ? "Each host pairs independently with this device's shared key. Drag the rows to set the order the board's host chips follow; URL/key changes are remove-and-re-pair."
                 : "Each host pairs independently with this device's shared key; adding a host verifies its fingerprint before any registration token is used.")
                .foregroundStyle(theme.subtext1)
        }
    }

    /// One host's full Settings row (D7): health + display name header,
    /// URL, fingerprint/key-id/grants-expiry read-out, per-posture guidance
    /// (awaiting fingerprint / key mismatch), Retry when a connection can
    /// be re-attempted, Rename (display name only — B5), and Remove Host.
    @ViewBuilder
    private func hostRow(_ profile: HostProfile) -> some View {
        let health = BoardModel.hostChipHealth(
            for: model.hostRuntimeFacts(for: profile))
        VStack(alignment: .leading, spacing: 6) {
            HStack(spacing: 8) {
                Circle()
                    .fill(theme.color(BoardModel.hostHealthToken(health)))
                    .frame(width: 8, height: 8)
                    .accessibilityHidden(true)
                Text(profile.displayName)
                    .font(.subheadline.weight(.semibold))
                    .foregroundStyle(theme.text)
                    .lineLimit(1)
                Text(health.label)
                    .font(.caption2.weight(.semibold))
                    .foregroundStyle(theme.color(BoardModel.hostHealthToken(health)))
                Spacer(minLength: 0)
                if profile.id == model.activeProfile?.id {
                    Text("Active")
                        .font(.caption2.weight(.medium))
                        .foregroundStyle(theme.subtext1)
                        .accessibilityLabel("Active host")
                }
            }
            if let rowError = hostRowErrors[profile.id] {
                Label(rowError, systemImage: "exclamationmark.triangle.fill")
                    .font(.caption)
                    .foregroundStyle(theme.red)
            }
            LabeledContent("URL", value: profile.urlString)
                .textSelection(.enabled)
            if let fingerprint = profile.fingerprint {
                LabeledContent("Fingerprint",
                               value: HostKeyTrust.shortFingerprint(fingerprint))
                    .textSelection(.enabled)
            }
            if let keyID = profile.keyId {
                LabeledContent("Key ID", value: String(keyID.prefix(16)))
            }
            if let expiryText = BoardModel.expiryText(epochSeconds: profile.expiryTs) {
                LabeledContent("Grants expiry",
                               value: profile.grants.isEmpty
                                   ? "\(expiryText) (no grants)"
                                   : expiryText)
            }
            // #397: per-host notification state (enroll + DEBUG-bridge for
            // THIS host) under the retained global Notifications control.
            // Disabling clears the host's enrolled APNs token best-effort;
            // a paused/mismatched host cannot enroll — its toggle renders
            // OFF (the persisted intent stays true and resurfaces once the
            // host can connect) and is disabled.
            if profile.keyId != nil {
                Toggle("Notify about this host",
                       isOn: Binding(
                        get: { profile.mayConnect
                                && profile.notificationsEnabled },
                        set: { model.setHostNotificationsEnabled(profileID: profile.id,
                                                                  enabled: $0) }))
                    .disabled(!profile.mayConnect)
                    .accessibilityLabel("Notify about \(profile.displayName)")
                    .accessibilityHint("Controls whether this host may send state-change alerts")
                    .id("settings.hosts.notify.\(profile.id.uuidString)")
            }
            if profile.connectionState == .awaitingFingerprintConfirmation
                || (profile.hostKeyB64 == nil && model.keyContinuityState == .pending) {
                Label("Host key not confirmed — the board stays paused until you verify this host's fingerprint.",
                      systemImage: "lock.shield")
                    .font(.caption)
                    .foregroundStyle(theme.peach)
                    .id("settings.hosts.awaiting")
                Button("Review host fingerprint") {
                    model.requestFingerprintReview(profileID: profile.id)
                }
                .font(.subheadline)
            }
            if profile.connectionState == .keyMismatch
                || model.keyContinuityState == .mismatch
                || model.coordinator?.posture(profileID: profile.id) == .mismatch {
                Label("This host presented a different host key than the one you paired. Corral paused it — the last safe board state is kept. Remove the host and pair it again with a fresh token.",
                      systemImage: "exclamationmark.triangle.fill")
                    .font(.caption)
                    .foregroundStyle(theme.red)
                    .id("settings.hosts.mismatch")
            } else if let failure = hostConnectionFailure(profile) {
                Label(failure, systemImage: "wifi.exclamationmark")
                    .font(.caption)
                    .foregroundStyle(theme.peach)
            }
            if let lastSeen = profile.lastSuccessfulConnectionTs, lastSeen > 0 {
                LabeledContent("Last seen", value: lastSeenText(lastSeenMs: lastSeen))
                    .foregroundStyle(theme.subtext1)
            }
            HStack(spacing: 16) {
                Button("Retry") {
                    model.retryHostConnection(profile)
                }
                .font(.subheadline)
                .disabled(!profile.mayConnect)
                .accessibilityLabel("Retry connection for \(profile.displayName)")
                Button("Rename") {
                    hostBeingRenamed = profile
                    renameDraft = profile.displayName
                }
                .font(.subheadline)
                .accessibilityLabel("Rename \(profile.displayName)")
                Spacer(minLength: 0)
                Button("Remove host", role: .destructive) {
                    hostBeingRemoved = profile
                }
                .font(.subheadline)
                .accessibilityLabel("Remove \(profile.displayName)")
            }
            .frame(minHeight: 44)
        }
        .padding(.vertical, 4)
        .confirmationDialog("Remove \(profile.displayName)?",
                            isPresented: Binding(
                                get: { hostBeingRemoved?.id == profile.id },
                                set: { if !$0 { hostBeingRemoved = nil } }),
                            titleVisibility: .visible) {
            Button("Remove host", role: .destructive) {
                model.removeHost(profileID: profile.id)
                hostBeingRemoved = nil
                if model.profiles.isEmpty {
                    dismiss()
                }
            }
            Button("Cancel", role: .cancel) {}
        } message: {
            Text("Removes this host's profile, cursor and saved board metadata from this phone. The host's registry entry stays until it is removed host-side; the device key is shared and stays.")
        }
        .accessibilityElement(children: .contain)
        .accessibilityLabel("\(profile.displayName), \(health.label)")
    }

    /// The per-host connection failure copy shown under the row (D7): the
    /// owning store's `.error` reason (active fleet store or the host's
    /// coordinator session store) — never a board banner.
    private func hostConnectionFailure(_ profile: HostProfile) -> String? {
        let store: FleetStore?
        if profile.id == model.activeProfileID {
            store = model.fleet
        } else {
            store = model.coordinator?.store(profileID: profile.id)
        }
        guard let store,
              case .error(let message) = store.connectionState,
              !message.isEmpty else { return nil }
        return message
    }

    /// Last-seen copy for a host row: "4m ago" relative text (pure).
    private func lastSeenText(lastSeenMs: UInt64) -> String {
        let now = UInt64(Date().timeIntervalSince1970 * 1000)
        return RelativeTime.lastSeenLabel(lastSeenMs: lastSeenMs, nowMs: now)
    }

    /// #401 D2: Settings drag-to-reorder (2+ hosts) — routes through the
    /// model so the store order (and therefore the board's host chips)
    /// updates atomically.
    private func moveHosts(from source: IndexSet, to destination: Int) {
        model.moveHosts(from: source, to: destination)
    }

    /// Per-host rename save: only the display name (B5); a duplicate or
    /// empty name surfaces inline on that host's row.
    private func renameHost(_ profile: HostProfile) {
        let name = renameDraft.trimmingCharacters(in: .whitespacesAndNewlines)
        hostBeingRenamed = nil
        if let error = model.renameHost(id: profile.id, to: name) {
            hostRowErrors[profile.id] = error
        } else {
            hostRowErrors.removeValue(forKey: profile.id)
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

// MARK: - #399 Add Host (fingerprint-verified pairing, B3)

/// The Add Host sheet: phase 1 collects the display name + URL and
/// fetches `/host-key` (validating the X25519 form); phase 2 shows the
/// derived fingerprint for EXPLICIT confirmation and only then accepts
/// the registration token and calls `/register` with the shared phone
/// Ed25519 key. Full pinned key + returned grants/expiry persist in the
/// active host profile.
///
/// #415: every field/phase binds to the MODEL-owned scene-scoped draft
/// (`model.addHostDraft`) — never sheet `@State` — so app-switch/return
/// and sheet view-identity churn from normal scene lifecycle updates
/// preserve the entered host name, URL, token, and the current host-key
/// verification phase. A failed submit keeps the sheet open with a
/// phase-identifying error; only a successful commit dismisses.
struct AddHostSheet: View {
    @ObservedObject var model: AppModel
    @Environment(\.dismiss) private var dismiss
    @EnvironmentObject private var theme: ThemeStore

    var body: some View {
        NavigationStack {
            Form {
                if let prepared = model.addHostDraft.prepared {
                    confirmationSection(prepared)
                } else {
                    entrySection
                }
            }
            .navigationTitle("Add host")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("Cancel") {
                        // #415: Cancel abandons the pairing — the
                        // scene-scoped draft (incl. the transient token)
                        // is cleared. Failure never dismisses here.
                        model.clearAddHostDraft()
                        dismiss()
                    }
                }
                ToolbarItem(placement: .topBarLeading) {
                    if model.addHostDraft.prepared != nil {
                        Button {
                            // #415: back to the entry phase for
                            // CORRECTION — name/URL/token all stay in the
                            // draft; the confirmed pairing is dropped.
                            model.addHostDraft.prepared = nil
                            model.addHostDraft.errorMessage = nil
                        } label: {
                            Label("Edit host details", systemImage: "chevron.backward")
                        }
                        .accessibilityLabel("Edit host details")
                    }
                }
            }
            .scrollContentBackground(.hidden)
            .background(theme.base)
            .preferredColorScheme(theme.flavor.isLight ? .light : .dark)
            // #401 rev B3: prefill the host NAME from the URL as it is
            // typed (Tailscale first label) until the user has entered a
            // name — the reviewed gap where the sheet bound `name` but
            // never used the existing HostURLForm.displayNameCandidate.
            // #415: the prefill writes into the model-owned draft.
            .onChange(of: model.addHostDraft.urlString) { _, newValue in
                guard model.addHostDraft.name.isEmpty else { return }
                model.addHostDraft.name = HostURLForm.displayNameCandidate(for: newValue)
            }
        }
        .presentationDragIndicator(.visible)
        .translucentSheetBackdrop(theme.base)
#if DEBUG
        // #401 evidence: the Add Host sheet records its two phases — (1)
        // name/URL entry with the B3 URL-derived NAME PREFILL (the driver
        // types the URL through the real binding so the real onChange fills
        // the name), (2) the fingerprint confirmation phase fed by the
        // synthetic fixture (no network on the evidence sim). Mocha entry,
        // Latte confirmation — representative light/dark per locked H.
        // #415: the driver writes through the model-owned draft (the
        // same bindings the user's typing uses).
        .task {
            if CorralDemoLaunch.wantsMultiHostAddEvidence(arguments: CommandLine.arguments) {
                await runMultiHostAddSheetEvidence()
            } else if CorralDemoLaunch.wantsAddHostBgReturnEvidence(arguments: CommandLine.arguments) {
                await runBgReturnEvidence()
            } else if CorralDemoLaunch.wantsAddHostFailedEvidence(arguments: CommandLine.arguments) {
                await runFailedSubmitEvidence()
            } else if CorralDemoLaunch.wantsAddHostCommitEvidence(arguments: CommandLine.arguments) {
                await runCommitEvidence()
            }
        }
#endif
    }

#if DEBUG
    /// #401 evidence: entry (name prefill) + fingerprint-confirmation
    /// phases via the synthetic fixture — see the .task comment above.
    private func runMultiHostAddSheetEvidence() async {
        guard await settingsSettle() else { return }
        theme.setFlavor(.mocha)
        model.addHostDraft.urlString = DemoFleet.DemoHosts.addHostURL
        guard await settingsSettle() else { return }
        EvidenceMarkers.write("phase-1-mh-add-entry-mocha")
        guard await hold() else { return }
        theme.setFlavor(.latte)
        guard await settingsSettle() else { return }
        model.addHostDraft.prepared = AppModel.PreparedHostPairing(
            displayName: model.addHostDraft.name.isEmpty ? "demo-host-d" : model.addHostDraft.name,
            urlString: model.addHostDraft.urlString,
            hostKey: HostKeyResponse(algorithm: "X25519",
                                     publicKey: DemoFleet.DemoHosts.addHostKey,
                                     note: nil),
            fingerprint: HostKeyTrust.fingerprint(
                forBase64: DemoFleet.DemoHosts.addHostKey) ?? "FINGER-DEMO")
        guard await settingsSettle() else { return }
        EvidenceMarkers.write("phase-2-mh-add-confirm-latte")
        _ = await hold()
        try? await Task.sleep(for: .milliseconds(1500))
        EvidenceMarkers.write("phase-3-mh-add-done")
        model.clearAddHostDraft()
        dismiss()
    }

    /// #415 evidence (a): a partially entered draft survives an
    /// app-switch/return cycle. The driver types the real name/URL into
    /// the draft, marks the state, then HOLDS across the host's
    /// background (Settings app launch) + return (app relaunch); the
    /// frame captured after the return must show every field populated.
    private func runBgReturnEvidence() async {
        guard await settingsSettle() else { return }
        model.addHostDraft.name = "Bazzite"
        model.addHostDraft.urlString = DemoFleet.DemoHosts.addHostURL
        guard await settingsSettle() else { return }
        EvidenceMarkers.write("phase-a-415-bg-filled")
        // Hold ~24 s: the host backgrounds this app and relaunches it
        // inside this window, then captures the returned frame.
        guard await hold() else { return }
        guard await hold() else { return }
        guard await hold() else { return }
        EvidenceMarkers.write("phase-a-415-bg-returned")
        _ = await hold()
        EvidenceMarkers.write("phase-a-415-done")
        model.clearAddHostDraft()
        dismiss()
    }

    /// #415 evidence (b): a FAILED submit keeps the sheet open with a
    /// phase-identifying error and every draft value intact. The driver
    /// calls the SAME verify path the button invokes against a
    /// connection-refused loopback URL (real transport failure; no
    /// daemon on the evidence sim).
    private func runFailedSubmitEvidence() async {
        guard await settingsSettle() else { return }
        model.addHostDraft.name = "Bazzite"
        model.addHostDraft.urlString = "http://127.0.0.1:1"
        guard await settingsSettle() else { return }
        await model.verifyAddHostDraft()
        // Wait for the failure to land (fast: connection refused).
        var attempts = 0
        while model.addHostDraft.errorMessage == nil, attempts < 40 {
            try? await Task.sleep(for: .milliseconds(250))
            attempts += 1
        }
        guard await settingsSettle() else { return }
        EvidenceMarkers.write("phase-b-415-failed-sheet-open")
        guard await hold() else { return }
        EvidenceMarkers.write("phase-b-415-done")
        model.clearAddHostDraft()
        dismiss()
    }

    /// #415 evidence (c): a SUCCESSFUL submit commits exactly one new
    /// host profile and clears the draft; the follow-up Settings frame
    /// shows the original Mac host still present. The app was launched
    /// with a DEBUG fixture URLSession (see
    /// AddHostCommitEvidenceURLProtocol) so /host-key + /register +
    /// /events resolve deterministically — the flow code below is the
    /// REAL prepare/complete path, transport only is fixture.
    private func runCommitEvidence() async {
        guard await settingsSettle() else { return }
        model.addHostDraft.name = "Bazzite"
        model.addHostDraft.urlString = AppModel.addHostEvidenceNewHostURL
        guard await settingsSettle() else { return }
        await model.verifyAddHostDraft()
        var attempts = 0
        while model.addHostDraft.prepared == nil, attempts < 40 {
            try? await Task.sleep(for: .milliseconds(250))
            attempts += 1
        }
        guard await settingsSettle() else { return }
        // The confirmation phase WITH the token filled (SecureField dots;
        // the token itself is never rendered in full or logged).
        model.addHostDraft.token = AppModel.addHostEvidenceToken
        EvidenceMarkers.write("phase-c-415-confirm-before-submit")
        guard await hold() else { return }
        // Real submit through the model outcome — dismisses only on
        // success, exactly once.
        guard let pairing = model.addHostDraft.prepared else {
            EvidenceMarkers.write("phase-c-415-no-pairing")
            return
        }
        let outcome = await model.completeAddHost(pairing, token: model.addHostDraft.token)
        guard case .success = outcome else {
            EvidenceMarkers.write("phase-c-415-commit-failed")
            model.clearAddHostDraft()
            dismiss()
            return
        }
        // Success: dismiss exactly once (the same action the register
        // button takes) — the Settings Hosts list behind the sheet then
        // shows the ORIGINAL Mac host plus exactly one new host (the
        // SettingsView driver scrolls settings.hosts into view).
        dismiss()
        guard await settingsSettle() else { return }
        EvidenceMarkers.write("phase-c-415-committed")
        guard await hold() else { return }
        EvidenceMarkers.write("phase-c-415-done")
    }

    private func settingsSettle() async -> Bool {
        do {
            try await Task.sleep(for: .milliseconds(2500))
            return true
        } catch {
            return false
        }
    }

    private func hold() async -> Bool {
        do {
            try await Task.sleep(for: .milliseconds(9000))
            return true
        } catch {
            return false
        }
    }
#endif

    /// Phase 1: name + URL entry. #415: fields bind straight to the
    /// model-owned scene-scoped draft, so churn can never clear them.
    private var entrySection: some View {
        Section {
            ConnectionField(title: "Host name", secure: false,
                            text: $model.addHostDraft.name)
            ConnectionField(title: "https://host (Tailscale serve URL or loopback)",
                            secure: false,
                            text: $model.addHostDraft.urlString)
            if let errorMessage = model.addHostDraft.errorMessage {
                Label(errorMessage, systemImage: "exclamationmark.triangle.fill")
                    .font(.caption)
                    .foregroundStyle(theme.red)
            }
            Button {
                Task { await model.verifyAddHostDraft() }
            } label: {
                if model.addHostDraft.isWorking {
                    ProgressView().controlSize(.small)
                } else {
                    Text("Verify host key")
                }
            }
            .disabled(model.addHostDraft.isWorking
                      || model.addHostDraft.name.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
                      || model.addHostDraft.urlString.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
            Text("Corral fetches the host's X25519 identity key and shows its fingerprint BEFORE any registration token is accepted. Nothing is saved yet.")
                .font(.caption)
                .foregroundStyle(theme.subtext1)
        } header: {
            Text("Host")
        } footer: {
            Text("Remote hosts must use https:// (the daemon's Tailscale HTTPS serve URL); http:// is accepted for loopback development hosts only.")
                .foregroundStyle(theme.subtext1)
        }
    }

    /// Phase 2: fingerprint confirmation + registration token.
    @ViewBuilder
    private func confirmationSection(_ pairing: AppModel.PreparedHostPairing) -> some View {
        Section {
            LabeledContent("Host", value: pairing.displayName)
            LabeledContent("URL", value: pairing.urlString)
            VStack(alignment: .leading, spacing: 6) {
                Text("Fingerprint")
                    .font(.caption)
                    .foregroundStyle(theme.subtext1)
                Text(pairing.fingerprint)
                    .font(.caption2.monospaced())
                    .textSelection(.enabled)
            }
            .accessibilityElement(children: .combine)
            .accessibilityLabel("Fingerprint \(pairing.fingerprint)")
            Button {
                UIPasteboard.general.string = pairing.fingerprint
            } label: {
                Label("Copy fingerprint", systemImage: "doc.on.doc")
            }
            .font(.subheadline)
            Text("Compare this fingerprint with the one shown by the daemon host. If it matches, enter the registration token to pair.")
                .font(.caption)
                .foregroundStyle(theme.subtext1)
        } header: {
            Text("Confirm the host identity")
        }
        Section {
            ConnectionField(title: "Registration token", secure: true,
                            text: $model.addHostDraft.token)
            Button {
                complete(pairing)
            } label: {
                if model.addHostDraft.isWorking {
                    ProgressView().controlSize(.small)
                } else {
                    Text("Confirm fingerprint & register")
                }
            }
            .disabled(model.addHostDraft.isWorking || model.addHostDraft.token.isEmpty)
            if let errorMessage = model.addHostDraft.errorMessage {
                Label(errorMessage, systemImage: "exclamationmark.triangle.fill")
                    .font(.caption)
                    .foregroundStyle(theme.red)
            }
        } header: {
            Text("Pair")
        }
    }

    /// #415: submit the confirmed pairing through the model. The model
    /// clears the draft ONLY after the commit succeeds and returns the
    /// outcome; this sheet dismisses exactly once — on success. A failure
    /// keeps the sheet open with the draft's phase-identifying error and
    /// every value available for correction/retry.
    private func complete(_ pairing: AppModel.PreparedHostPairing) {
        guard !model.addHostDraft.isWorking else { return }
        Task {
            let outcome = await model.completeAddHost(pairing,
                                                      token: model.addHostDraft.token)
            if case .success = outcome {
                dismiss()
            }
        }
    }
}

/// #399 B6: the launch-time fingerprint confirmation for a MIGRATED
/// legacy host. The app pauses once, fetches the host key, and only opens
/// the live stream after the user confirms the pinned identity. Decline
/// keeps the profile paused; Remove Host unlinks locally (B7).
struct FingerprintConfirmationSheet: View {
    @ObservedObject var model: AppModel
    let request: AppModel.FingerprintConfirmationRequest
    @Environment(\.dismiss) private var dismiss
    @EnvironmentObject private var theme: ThemeStore

    private enum Phase: Equatable {
        case loading
        case failed(String)
        case ready(HostKeyResponse)
    }

    @State private var phase: Phase = .loading
    @State private var confirmRemove = false

    var body: some View {
        NavigationStack {
            Form {
                Section {
                    Text("“\(request.profileName)” was paired by an older version of Corral. Verify its identity fingerprint before the board connects — Corral never auto-accepts a host key.")
                        .font(.subheadline)
                        .foregroundStyle(theme.subtext1)
                } header: {
                    Text("Verify this host")
                }
                switch phase {
                case .loading:
                    Section {
                        HStack {
                            Spacer()
                            ProgressView()
                            Spacer()
                        }
                    }
                case .failed(let message):
                    Section {
                        Label("Could not fetch the host key — \(message)",
                              systemImage: "wifi.exclamationmark")
                            .font(.caption)
                            .foregroundStyle(theme.peach)
                        Button("Retry") { load() }
                    }
                case .ready(let response):
                    if HostKeyTrust.isWellFormed(response),
                       let fingerprint = HostKeyTrust.fingerprint(forBase64: response.publicKey) {
                        Section {
                            Text(fingerprint)
                                .font(.caption2.monospaced())
                                .textSelection(.enabled)
                            Button {
                                UIPasteboard.general.string = fingerprint
                            } label: {
                                Label("Copy fingerprint", systemImage: "doc.on.doc")
                            }
                            .font(.subheadline)
                        } header: {
                            Text("Host fingerprint")
                        } footer: {
                            Text("Compare it with the identity the host itself shows. Confirm only if it matches.")
                                .foregroundStyle(theme.subtext1)
                        }
                        Section {
                            Button("Confirm — it's my host") {
                                model.confirmFingerprint(profileID: request.profileID,
                                                         hostKeyB64: response.publicKey,
                                                         fingerprint: fingerprint)
                                dismiss()
                            }
                            Button("Not now") {
                                model.deferFingerprintConfirmation()
                                dismiss()
                            }
                            Button("Remove host", role: .destructive) {
                                confirmRemove = true
                            }
                        } footer: {
                            Text("Removing the host unlinks it on this phone only — the daemon registry entry stays until the host removes it.")
                                .foregroundStyle(theme.subtext1)
                        }
                        .confirmationDialog("Remove \(request.profileName)?",
                                            isPresented: $confirmRemove,
                                            titleVisibility: .visible) {
                            Button("Remove host", role: .destructive) {
                                model.removeHost(profileID: request.profileID)
                                dismiss()
                            }
                            Button("Cancel", role: .cancel) {}
                        } message: {
                            Text("Removes the host profile, cursor and saved board metadata from this phone. The shared device key stays.")
                        }
                    } else {
                        Section {
                            Label("The host did not return a well-formed X25519 key — pairing is stopped.",
                                  systemImage: "exclamationmark.triangle.fill")
                                .font(.caption)
                                .foregroundStyle(theme.red)
                            Button("Not now") {
                                model.deferFingerprintConfirmation()
                                dismiss()
                            }
                            Button("Remove host", role: .destructive) {
                                confirmRemove = true
                            }
                        }
                        .confirmationDialog("Remove \(request.profileName)?",
                                            isPresented: $confirmRemove,
                                            titleVisibility: .visible) {
                            Button("Remove host", role: .destructive) {
                                model.removeHost(profileID: request.profileID)
                                dismiss()
                            }
                            Button("Cancel", role: .cancel) {}
                        }
                    }
                }
            }
            .navigationTitle("Confirm host key")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("Later") {
                        model.deferFingerprintConfirmation()
                        dismiss()
                    }
                }
            }
            .scrollContentBackground(.hidden)
            .background(theme.base)
            .preferredColorScheme(theme.flavor.isLight ? .light : .dark)
        }
        .presentationDragIndicator(.visible)
        .translucentSheetBackdrop(theme.base)
        .task(id: request.id) { load() }
    }

    private func load() {
        phase = .loading
        Task {
            do {
                let response = try await model.fetchHostKey(profileID: request.profileID)
                phase = .ready(response)
            } catch {
                phase = .failed(error.localizedDescription)
            }
        }
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
    /// #400 E1: the row's host profile (nil = legacy single-host runtime /
    /// demo). The sheet resolves every read through the composite identity
    /// so an equal raw id on another host is never touched.
    let hostProfileID: UUID?
    @ObservedObject var model: AppModel
    @Environment(\.dismiss) private var dismiss
    @EnvironmentObject private var theme: ThemeStore
    @StateObject private var session = RecentsSheetSession()
    // The live ScrollViewProxy — held so the DEBUG evidence driver can
    // re-anchor the view to the newest content after a phase change
    // (simctl cannot scroll; Release never touches it).
    @State private var recentsProxy: ScrollViewProxy?

    private var agent: Agent? { model.fleetAgent(hostProfileID: hostProfileID, agentID: agentId) }
    private var tail: TailPane? { model.fleetTailPane(hostProfileID: hostProfileID, agentID: agentId) }
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
        // #385: the recents sheet floats over the shared translucent
        // backdrop (Liquid Glass on iOS 26+, tinted-material fallback
        // below) so the busy board behind shows through the sheet surface
        // between the blocks.
        .translucentSheetBackdrop(theme.base)
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
        // #385: the header strip's caption row KEEPS its opaque base
        // backing — muted/dim caption tiers (tailMuted/tailQuiet) must not
        // float over the translucent backdrop in the darkest underlying
        // case (SheetBackdropTests locks the tiers that can).
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
                // #385: the non-loaded states keep an OPAQUE base backing
                // (they paint directly on the sheet surface, which is now
                // translucent) so their muted ink keeps today's AA.
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
                .background(theme.base)
            case .empty:
                Text("No output yet.")
                    .font(.caption)
                    .foregroundStyle(theme.tailMuted)
                    .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
                    .padding(16)
                    .background(theme.base)
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
                .background(theme.base)
            case .loaded:
                // #385: the loaded block stream floats over the translucent
                // sheet backdrop — the blocks' own opaque card chrome keeps
                // their ink at today's contrast while the board behind shows
                // through between the cards.
                blocksStream
            }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
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
        // #400 E1: the reload drive carries the sheet's composite identity
        // (host profile + raw agent id) — it resolves exactly the owning
        // host and never falls back to another host.
        model.driveReadTail(agent: agent, hostProfileID: hostProfileID,
                            driveClient: driveClient, silent: true)
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
