import SwiftUI

@main
struct FleetNotifierApp: App {
    @UIApplicationDelegateAdaptor(AppDelegate.self) private var appDelegate
    @StateObject private var model = AppModel()
    // #372: the app-wide theme (flavor + Reduce Motion). Owned here so the
    // flavor's color scheme/tint reach every sheet and the picker's choice
    // persists across launches. #371: the DEBUG-only launch argument
    // `-corralDemoReduceMotion` forces the Reduce-Motion provider so the
    // simulator evidence can capture the static working dot — simctl cannot
    // toggle the system Reduce Motion setting.
    @StateObject private var theme = ThemeStore(reduceMotionProvider: {
#if DEBUG
        if CommandLine.arguments.contains("-corralDemoReduceMotion") {
            return true
        }
#endif
        return UIAccessibility.isReduceMotionEnabled
    })
    @Environment(\.scenePhase) private var scenePhase

    var body: some Scene {
        WindowGroup {
            RootView(model: model)
                .environmentObject(theme)
                .task {
                    // Dev-only launch-arg harnesses (Debug only).
#if DEBUG
                    if CorralDemoLaunch.wantsDetail(arguments: CommandLine.arguments) {
                        model.enterDemo(detailAgentId: CorralDemoLaunch.detailAgentID)
                    } else if CorralDemoLaunch.wantsConnectEvidence(arguments: CommandLine.arguments) {
                        // #379 evidence: guarantee the UNPAIRED first-launch
                        // state no matter what the app container holds from
                        // earlier runs — the board's real auto-present then
                        // shows the How-to-connect sheet, and the driver
                        // steps through Settings and the sheet behind marker
                        // files (no demo fleet: the connect frames must show
                        // the real fresh-install surfaces).
                        model.resetDevice()
                    } else if CorralDemoLaunch.wantsReopenEvidence(arguments: CommandLine.arguments)
                                || CorralDemoLaunch.wantsFilterEvidence(arguments: CommandLine.arguments)
                                || CorralDemoLaunch.wantsSettingsEvidence(arguments: CommandLine.arguments)
                                || CorralDemoLaunch.wantsThemeEvidence(arguments: CommandLine.arguments)
                                || CorralDemoLaunch.wantsGlassEvidence(arguments: CommandLine.arguments)
                                || CorralDemoLaunch.wantsRepoLabelEvidence(arguments: CommandLine.arguments)
                                || CorralDemoLaunch.wantsCollapseEvidence(arguments: CommandLine.arguments)
                                || CorralDemoLaunch.wantsTitleEvidence(arguments: CommandLine.arguments)
                                || CorralDemoLaunch.wantsConnectionInputsEvidence(arguments: CommandLine.arguments)
                                || CommandLine.arguments.contains("-demoMode") {
                        model.enterDemo()
                    }
#endif
                }
                .onChange(of: scenePhase) { _, phase in
                    switch phase {
                    case .background, .inactive:
                        // Backgrounded = no connection (D5): drop the SSE
                        // stream; the cursor is persisted for resume.
                        model.stopLive()
                    case .active:
                        if model.mode == .live {
                            model.startLive()
                            // #101: re-sync grants on foreground so a
                            // host-side promotion appears without a device
                            // reset (idempotent; never blocks the stream).
                            Task { await model.refreshGrants() }
                        }
                    @unknown default:
                        break
                    }
                }
        }
    }
}

#if DEBUG
/// Launch arguments for the deterministic local design-gate fixture. The
/// route is opt-in and has no effect on normal Debug launches or any Release
/// build. `-corralDemoDetail` seeds the demo fleet and opens the featured
/// agent's recents sheet (simulator capture cannot inject the tap).
/// `-corralDemoUXEvidence` / `-corralDemoFilterEvidence` seed the plain
/// demo fleet and drive the #364 recorded evidence sequences (markers in
/// Documents/ux-evidence that the host screenshot script observes).
/// `-corralDemoSettingsEvidence` (#365) drives the board + Settings-sheet
/// sequence behind the always-visible gear. `-corralDemoThemeEvidence`
/// (#372) drives the Mocha → Appearance → Latte board/recents sequence.
/// `-corralDemoConnectEvidence` (#379) wipes any leftover identity so the
/// launch is UNPAIRED, then records the auto-presented How-to-connect
/// sheet, the Settings sheet (Device section without the grants list), and
/// the shared connect sheet content.
/// `-corralDemoGlassEvidence` (#385) seeds the demo fleet and records the
/// translucent recents + Settings sheets (Mocha + Latte) over the busy
/// board.
/// `-corralDemoRepoLabelEvidence` (#384) seeds the demo fleet and records
/// the per-row repo label visibility rule — All rows WITH their repo label
/// chips vs the demo-atlas repo pill active (rows WITHOUT repo name labels,
/// color-only hue echo, same row heights), then All restored, in Mocha and
/// Latte.
/// `-corralDemoCollapseEvidence` (#386) seeds the demo fleet and records
/// the board hierarchy change — thick collapsible status bars vs demoted
/// repo subgroup captions — with the blocked section collapsed and the
/// working section expanded (Mocha + Latte), then all sections collapsed.
/// `-corralDemoTitleEvidence` (#387) seeds the demo fleet and records the
/// chrome-only board header — no 'Fleet' title text in the top OR scrolled
/// nav-bar states (Mocha + Latte), via the board ScrollViewReader driver.
/// `-corralDemoConnectionInputsEvidence` (#388) seeds the demo fleet and
/// records the Settings Connection section — themed surface1 inputs on
/// Macchiato/Mocha/Latte in the unpaired state, then the paired status row
/// (token field hidden) after the driver seeds a demo registration key id.
enum CorralDemoLaunch {
    static let detailArgument = "-corralDemoDetail"
    static let reopenEvidenceArgument = "-corralDemoUXEvidence"
    static let filterEvidenceArgument = "-corralDemoFilterEvidence"
    static let settingsEvidenceArgument = "-corralDemoSettingsEvidence"
    static let themeEvidenceArgument = "-corralDemoThemeEvidence"
    static let connectEvidenceArgument = "-corralDemoConnectEvidence"
    static let glassEvidenceArgument = "-corralDemoGlassEvidence"
    static let repoLabelEvidenceArgument = "-corralDemoRepoLabelEvidence"
    static let collapseEvidenceArgument = "-corralDemoCollapseEvidence"
    static let titleEvidenceArgument = "-corralDemoTitleEvidence"
    static let connectionInputsEvidenceArgument = "-corralDemoConnectionInputsEvidence"

    static var detailAgentID: String {
        DemoFleet.featuredAgentID
    }

    static func wantsDetail(arguments: [String]) -> Bool {
        arguments.contains(detailArgument)
    }

    static func wantsReopenEvidence(arguments: [String]) -> Bool {
        arguments.contains(reopenEvidenceArgument)
    }

    static func wantsFilterEvidence(arguments: [String]) -> Bool {
        arguments.contains(filterEvidenceArgument)
    }

    static func wantsSettingsEvidence(arguments: [String]) -> Bool {
        arguments.contains(settingsEvidenceArgument)
    }

    static func wantsThemeEvidence(arguments: [String]) -> Bool {
        arguments.contains(themeEvidenceArgument)
    }

    static func wantsConnectEvidence(arguments: [String]) -> Bool {
        arguments.contains(connectEvidenceArgument)
    }

    /// #385: records the translucent recents + Settings sheets over the
    /// busy demo board (Mocha + Latte) behind marker files — see
    /// `FleetView.runGlassSequence()`.
    static func wantsGlassEvidence(arguments: [String]) -> Bool {
        arguments.contains(glassEvidenceArgument)
    }

    /// #384: records the per-row repo label visibility rule — All rows
    /// with their repo label chips vs the demo-atlas pill active (rows
    /// without repo name labels, same row heights), then All restored, in
    /// Mocha and Latte — see `FleetView.runRepoLabelSequence()`.
    static func wantsRepoLabelEvidence(arguments: [String]) -> Bool {
        arguments.contains(repoLabelEvidenceArgument)
    }

    /// #386: records the board-hierarchy change — thick collapsible
    /// status bars vs demoted repo captions, one section collapsed and
    /// one expanded (Mocha + Latte) — see
    /// `FleetView.runCollapseSequence()`.
    static func wantsCollapseEvidence(arguments: [String]) -> Bool {
        arguments.contains(collapseEvidenceArgument)
    }

    /// #387: records the chrome-only board header — Mocha + Latte at the
    /// top AND scrolled (no 'Fleet' title text in either nav-bar state) —
    /// see `FleetView.runTitleSequence()`.
    static func wantsTitleEvidence(arguments: [String]) -> Bool {
        arguments.contains(titleEvidenceArgument)
    }

    /// #388: records the Settings Connection inputs — unpaired (themed
    /// host + token + Register) then paired (status row, no token field)
    /// across Macchiato/Mocha/Latte — see
    /// `FleetView.runConnectionInputsSequence()`.
    static func wantsConnectionInputsEvidence(arguments: [String]) -> Bool {
        arguments.contains(connectionInputsEvidenceArgument)
    }
}
#endif

struct RootView: View {
    @ObservedObject var model: AppModel

    var body: some View {
        FleetView(model: model)
            // #79 defect 1: a cold launch with a restored registration
            // must connect without waiting for a scenePhase transition
            // (registering while already foregrounded never fired
            // .active, leaving the board offline). Idempotent: mode
            // gate + startLive's hostURL guard + connect's streamTask
            // guard — a later .active transition is then a no-op.
            .task {
                if model.mode == .live {
                    model.startLive()
                    // #101: cold-launch grants refresh (restored meta may
                    // be stale — the host promoted grants since last run).
                    await model.refreshGrants()
                }
            }
    }
}
