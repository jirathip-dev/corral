import SwiftUI

@main
struct FleetNotifierApp: App {
    @UIApplicationDelegateAdaptor(AppDelegate.self) private var appDelegate
    @StateObject private var model = AppModel()
    @Environment(\.scenePhase) private var scenePhase

    var body: some Scene {
        WindowGroup {
            RootView(model: model)
                .task {
                    // Dev-only launch-arg harnesses (Debug only).
#if DEBUG
                    if CorralDemoLaunch.wantsDetail(arguments: CommandLine.arguments) {
                        model.enterDemo(detailAgentId: CorralDemoLaunch.detailAgentID)
                    } else if CorralDemoLaunch.wantsReopenEvidence(arguments: CommandLine.arguments)
                                || CorralDemoLaunch.wantsFilterEvidence(arguments: CommandLine.arguments)
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
enum CorralDemoLaunch {
    static let detailArgument = "-corralDemoDetail"
    static let reopenEvidenceArgument = "-corralDemoUXEvidence"
    static let filterEvidenceArgument = "-corralDemoFilterEvidence"

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
