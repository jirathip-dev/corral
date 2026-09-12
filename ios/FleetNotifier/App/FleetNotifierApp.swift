import SwiftUI

@main
struct FleetNotifierApp: App {
    @UIApplicationDelegateAdaptor(AppDelegate.self) private var appDelegate
    // #399: the production app always configures the host-profile store —
    // legacy single-host data migrates into it on the first upgraded
    // launch and every pairing (Add Host or legacy Connection) mirrors
    // into the ordered profile list.
    @StateObject private var model: AppModel
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

    init() {
#if DEBUG
        // #415 evidence (c): the successful-commit driver runs the REAL
        // Add Host prepare/register/commit flow against a fixture
        // transport (no daemon exists on the evidence simulator) — the
        // URLProtocol answers /host-key + /register + /events for the
        // fixture host URLs only.
        if CorralDemoLaunch.wantsAddHostCommitEvidence(arguments: CommandLine.arguments) {
            let config = URLSessionConfiguration.ephemeral
            config.protocolClasses = [AddHostCommitEvidenceURLProtocol.self]
            let session = URLSession(configuration: config)
            _model = StateObject(wrappedValue: AppModel(
                session: session,
                profileStore: HostProfileStore(directory: HostProfileStore.defaultDirectory())))
            return
        }
#endif
        _model = StateObject(wrappedValue: AppModel(
            profileStore: HostProfileStore(directory: HostProfileStore.defaultDirectory())))
    }

    var body: some Scene {
        WindowGroup {
            RootView(model: model)
                .environmentObject(theme)
                .task {
                    // Dev-only launch-arg harnesses (Debug only).
#if DEBUG
                    if CommandLine.arguments.contains("-corralHerdEvidence") {
                        HerdEvidence.seed(model)
                    } else if CorralDemoLaunch.wantsDetail(arguments: CommandLine.arguments) {
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
                                || Corral416Evidence.wantsDriver
                                || CorralDemoLaunch.wantsRepoLabelEvidence(arguments: CommandLine.arguments)
                                || CorralDemoLaunch.wantsCollapseEvidence(arguments: CommandLine.arguments)
                                || CorralDemoLaunch.wantsTitleEvidence(arguments: CommandLine.arguments)
                                || CorralDemoLaunch.wantsConnectionInputsEvidence(arguments: CommandLine.arguments)
                                || CorralDemoLaunch.wantsDeniedNotificationsEvidence(arguments: CommandLine.arguments)
                                || CorralDemoLaunch.wantsMultiHostBoardEvidence(arguments: CommandLine.arguments)
                                || CorralDemoLaunch.wantsMultiHostSettingsEvidence(arguments: CommandLine.arguments)
                                || CorralDemoLaunch.wantsMultiHostAddEvidence(arguments: CommandLine.arguments)
                                // #427: the filter-header evidence drivers
                                // seed the same three-profile demo.
                                || CorralDemoLaunch.wantsFilterHeaderEvidence(arguments: CommandLine.arguments)
                                || CorralDemoLaunch.wantsFilterHeaderConnectingEvidence(arguments: CommandLine.arguments)
                                || CorralDemoLaunch.wantsAddHostBgReturnEvidence(arguments: CommandLine.arguments)
                                || CorralDemoLaunch.wantsAddHostFailedEvidence(arguments: CommandLine.arguments)
                                || CorralDemoLaunch.wantsAddHostCommitEvidence(arguments: CommandLine.arguments)
                                || CommandLine.arguments.contains("-demoMode") {
                        // #401: the multi-host evidence drivers seed three
                        // synthetic profiles (live/offline/key-mismatch)
                        // instead of the single-host demo fleet.
                        if CorralDemoLaunch.wantsMultiHostBoardEvidence(arguments: CommandLine.arguments)
                            || CorralDemoLaunch.wantsMultiHostSettingsEvidence(arguments: CommandLine.arguments)
                            || CorralDemoLaunch.wantsMultiHostAddEvidence(arguments: CommandLine.arguments)
                            // #427: the filter-header drivers ride the same
                            // three-profile seed (Host B connecting for the
                            // connecting evidence launch).
                            || CorralDemoLaunch.wantsFilterHeaderEvidence(arguments: CommandLine.arguments)
                            || CorralDemoLaunch.wantsFilterHeaderConnectingEvidence(arguments: CommandLine.arguments) {
                            model.enterMultiHostDemo(
                                hostBConnecting: CorralDemoLaunch
                                    .wantsFilterHeaderConnectingEvidence(arguments: CommandLine.arguments))
                        } else if CorralDemoLaunch.wantsAddHostBgReturnEvidence(arguments: CommandLine.arguments)
                                    || CorralDemoLaunch.wantsAddHostFailedEvidence(arguments: CommandLine.arguments)
                                    || CorralDemoLaunch.wantsAddHostCommitEvidence(arguments: CommandLine.arguments) {
                            // #415: the Add Host lifecycle evidence seeds
                            // ONE original "Mac" profile (bg-return /
                            // failed-submit / successful-commit drivers).
                            model.enterAddHostEvidenceSeed()
                        } else {
                            model.enterDemo()
                        }
                        // #458 evidence: scenario A/C start in the seeded
                        // mode AFTER the demo seed (the Settings picker and
                        // the sequence driver then persist the REAL choice).
                        Corral458Presentation.seedIfNeeded(model)
                    }
#endif
                }
                .onChange(of: scenePhase) { _, phase in
                    // #453: ONE lifecycle routing point. Transient
                    // `.inactive` (Control Center, banners, app switcher)
                    // retains a healthy live session; only an actual
                    // `.background` ends it with cursor persistence. The
                    // split lives in the model seam so deterministic
                    // lifecycle tests drive the exact production path —
                    // never duplicate the decision here.
                    model.handleScenePhaseChange(phase)
                }
#if DEBUG
                // #427 evidence: Dynamic Type frames — run the WHOLE UI
                // (board + sheets) at the accessibility content size only
                // when the evidence launch argument is present. Without
                // the argument the modifier is a pass-through and the
                // user/system size category is untouched.
                .modifier(DebugAccessibilitySizeModifier())
#endif
        }
    }
}

#if DEBUG
/// #427 evidence: force the accessibility content size category when
/// `-corral427AccessibilitySizes` is passed (Debug builds only — the
/// release path never compiles this), so the Dynamic Type frames prove the
/// filter sheet's wrapping rows and vertical scroll reachability. Without
/// the argument the modifier leaves the environment untouched.
struct DebugAccessibilitySizeModifier: ViewModifier {
    func body(content: Content) -> some View {
        if CorralDemoLaunch.wantsFilterHeaderAccessibilitySizes(arguments: CommandLine.arguments)
            || Corral458Presentation.wantsAccessibilitySizes(arguments: CommandLine.arguments) {
            content.environment(\.sizeCategory, .accessibilityLarge)
        } else {
            content
        }
    }
}
#endif

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
/// `-corralDemoDeniedNotificationsEvidence` (#389) seeds the demo fleet in
/// MOCHA and forces the DENIED notification permission posture so the
/// Settings Notifications section's blocked guidance + 'Open iOS Settings'
/// action can be captured (a simulator cannot be denied notifications).
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
    /// #389: forces the DENIED notification permission posture in demo mode
    /// so the Settings Notifications section's blocked guidance + 'Open iOS
    /// Settings' action can be captured (a simulator cannot be denied
    /// notifications through simctl privacy — no notifications service).
    static let deniedNotificationsEvidenceArgument = "-corralDemoDeniedNotificationsEvidence"
    /// #401: seeds the deterministic MULTI-HOST demo state (Host A live,
    /// Host B offline with retained stale rows, Host C key mismatch) and
    /// records the All Hosts / one-host-filtered / partial-offline board
    /// frames (Mocha + Latte).
    static let multiHostBoardEvidenceArgument = "-corralDemoMultiHostBoardEvidence"
    /// #401: same multi-host demo state, recording the Settings Hosts list
    /// (per-host rows with error/last-seen/retry/rename/remove + the
    /// mismatch row) in Mocha + Latte.
    static let multiHostSettingsEvidenceArgument = "-corralDemoMultiHostSettingsEvidence"
    /// #401: same multi-host demo state, recording the Add Host sheet —
    /// name/URL entry with the B3 URL-derived name prefill, then the
    /// fingerprint confirmation phase (Mocha + Latte).
    static let multiHostAddEvidenceArgument = "-corralDemoMultiHostAddEvidence"
    /// #427 Direction A: records the filter/header surface matrix — the
    /// top-left Filters control + sheet (closed All/All, sheet default,
    /// host-only, repo-only, both scopes, offline host scope, zero-lane
    /// No-lanes board; Mocha + Latte).
    static let filterHeaderEvidenceArgument = "-corral427FilterEvidence"
    /// #427 Direction A: the same driver with Host B seeded CONNECTING so
    /// the board banner + sheet host rows show textual `connecting`.
    static let filterHeaderConnectingEvidenceArgument = "-corral427ConnectingHostEvidence"
    /// #427 Direction A: render the whole UI at the accessibility content
    /// size for the Dynamic Type evidence frames (Debug only).
    static let accessibilitySizesArgument = "-corral427AccessibilitySizes"
    /// #415: Add Host draft/error lifecycle evidence (a) — a partially
    /// entered Add Host draft survives an app-switch/return cycle (the
    /// host backgrounds this app via the Settings app and relaunches it).
    static let addHostBgReturnEvidenceArgument = "-corral415BgReturnEvidence"
    /// #415: Add Host draft/error lifecycle evidence (b) — a FAILED
    /// submit keeps the sheet open with a phase-identifying error and
    /// every draft value intact.
    static let addHostFailedEvidenceArgument = "-corral415FailedSubmitEvidence"
    /// #415: Add Host draft/error lifecycle evidence (c) — a SUCCESSFUL
    /// submit commits exactly one new host profile and the original Mac
    /// host stays present (fixture transport; see
    /// AddHostCommitEvidenceURLProtocol).
    static let addHostCommitEvidenceArgument = "-corral415CommitEvidence"

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

    /// #389: the denied-notifications Settings evidence driver.
    static func wantsDeniedNotificationsEvidence(arguments: [String]) -> Bool {
        arguments.contains(deniedNotificationsEvidenceArgument)
    }

    /// #401: the multi-host board evidence driver.
    static func wantsMultiHostBoardEvidence(arguments: [String]) -> Bool {
        arguments.contains(multiHostBoardEvidenceArgument)
    }

    /// #427: the Direction-A filter/header evidence driver.
    static func wantsFilterHeaderEvidence(arguments: [String]) -> Bool {
        arguments.contains(filterHeaderEvidenceArgument)
    }

    /// #427: the connecting-host filter/header evidence driver.
    static func wantsFilterHeaderConnectingEvidence(arguments: [String]) -> Bool {
        arguments.contains(filterHeaderConnectingEvidenceArgument)
    }

    /// #427: accessibility content size for the Dynamic Type evidence.
    static func wantsFilterHeaderAccessibilitySizes(arguments: [String]) -> Bool {
        arguments.contains(accessibilitySizesArgument)
    }

    /// #401: the multi-host Settings evidence driver.
    static func wantsMultiHostSettingsEvidence(arguments: [String]) -> Bool {
        arguments.contains(multiHostSettingsEvidenceArgument)
    }

    /// #401: the multi-host Add Host sheet evidence driver.
    static func wantsMultiHostAddEvidence(arguments: [String]) -> Bool {
        arguments.contains(multiHostAddEvidenceArgument)
    }

    /// #415: Add Host draft survives app-switch/return (evidence a).
    static func wantsAddHostBgReturnEvidence(arguments: [String]) -> Bool {
        arguments.contains(addHostBgReturnEvidenceArgument)
    }

    /// #415: failed Add Host submit keeps the sheet open (evidence b).
    static func wantsAddHostFailedEvidence(arguments: [String]) -> Bool {
        arguments.contains(addHostFailedEvidenceArgument)
    }

    /// #415: successful Add Host commit + original host present (evidence c).
    static func wantsAddHostCommitEvidence(arguments: [String]) -> Bool {
        arguments.contains(addHostCommitEvidenceArgument)
    }
}

/// #458 evidence: deterministic Settings → Board/Herd → relaunch scenarios.
/// The launch argument seeds the STARTING presentation in demo mode (the
/// picker's Binding and the driver then write the REAL persisted
/// preference), so every frame stays inside the actual Settings/root path
/// and a cold relaunch is the same `simctl terminate` + `launch` cycle a
/// user's relaunch takes.
enum Corral458Presentation {
    /// A: start Herd → Settings (Herd selected) → select Board → Board.
    static let herdScenarioArgument = "-corral458HerdScenario"
    /// B: relaunch that only proves the persisted value restored (Board
    /// after A; also used for the final Herd-restored relaunch after C/D).
    static let boardReloadScenarioArgument = "-corral458BoardReloadScenario"
    /// C: start Board → Settings (Board selected) → select Herd → Herd.
    static let boardScenarioArgument = "-corral458BoardScenario"
    /// D: restored Herd + Open Board temporary override (never persisted).
    static let openBoardScenarioArgument = "-corral458OpenBoardScenario"
    /// Dynamic Type evidence: render the whole UI at the accessibility
    /// content size (same convention as -corral427AccessibilitySizes).
    static let accessibilitySizesArgument = "-corral458AccessibilitySizes"

    static func wantsHerdScenario(arguments: [String]) -> Bool {
        arguments.contains(herdScenarioArgument)
    }

    static func wantsBoardScenario(arguments: [String]) -> Bool {
        arguments.contains(boardScenarioArgument)
    }

    static func wantsBoardReloadScenario(arguments: [String]) -> Bool {
        arguments.contains(boardReloadScenarioArgument)
    }

    static func wantsOpenBoardScenario(arguments: [String]) -> Bool {
        arguments.contains(openBoardScenarioArgument)
    }

    static func wantsAccessibilitySizes(arguments: [String]) -> Bool {
        arguments.contains(accessibilitySizesArgument)
    }

    /// Applies the scenario's starting presentation AFTER the demo seed so
    /// the marker frames start from the intended mode. Open Board (D) must
    /// start on the RESTORED saved value, so it never seeds here.
    /// The #458 evidence launches are demo-mode runs on a simulator whose
    /// notification authorization was never answered (simctl cannot tap the
    /// OS alert), so the one-shot OS prompt is suppressed exactly like the
    /// #415 commit driver suppresses it — demo mode has no live
    /// notification surface.
    @MainActor
    static func seedIfNeeded(_ model: AppModel) {
        let arguments = CommandLine.arguments
        guard wantsHerdScenario(arguments: arguments)
                || wantsBoardScenario(arguments: arguments)
                || wantsOpenBoardScenario(arguments: arguments)
                || wantsBoardReloadScenario(arguments: arguments) else { return }
        if wantsHerdScenario(arguments: arguments) {
            model.fleetPresentation = .herd
        } else if wantsBoardScenario(arguments: arguments) {
            model.fleetPresentation = .board
        }
        model.suppressOSNotificationPromptForDemoEvidence()
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
