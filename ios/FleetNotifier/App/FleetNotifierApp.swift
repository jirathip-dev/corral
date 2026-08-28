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
                    // Dev-only launch-arg harnesses (see LiveVerifyRunner).
#if DEBUG
                    if CommandLine.arguments.contains("-liveVerify") {
                        LiveVerifyRunner(model: model).run()
                    } else if let presentation = CorralDemoLaunch.presentation(
                        arguments: CommandLine.arguments) {
                        model.enterDemo(presentation: presentation,
                                        detailAgentId: DemoFleet.featuredAgentID)
                    } else if CorralDemoLaunch.wantsIssues(arguments: CommandLine.arguments) {
                        model.enterDemo()
                        model.demoOpenIssues = true
                        model.demoOpenIssueNumber =
                            CorralDemoLaunch.issueDetailNumber(arguments: CommandLine.arguments)
                    } else if CommandLine.arguments.contains("-demoMode") {
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
/// Launch arguments for the permanent, local-only design-gate fixture. The
/// route is opt-in and has no effect on normal Debug launches or any Release
/// build. `-corralDemoBefore` implies the same detail route in the app task.
enum CorralDemoLaunch {
    static let detailArgument = "-corralDemoDetail"
    static let beforeArgument = "-corralDemoBefore"
    /// #267: open the read-only issue browser straight into demo (list
    /// evidence route; `detailIssueNumber` auto-expands one row's inline
    /// detail for the detail evidence route).
    static let issuesArgument = "-corralDemoIssues"
    static let detailIssueArgument = "-corralDemoIssuesDetail"

    static func presentation(arguments: [String]) -> AppModel.DemoPresentation? {
        guard arguments.contains(detailArgument) || arguments.contains(beforeArgument) else {
            return nil
        }
        return arguments.contains(beforeArgument) ? .before : .after
    }

    static func wantsIssues(arguments: [String]) -> Bool {
        arguments.contains(issuesArgument)
    }

    /// The issue number to auto-expand, when the detail evidence route is
    /// requested (nil = plain list).
    static func issueDetailNumber(arguments: [String]) -> UInt64? {
        guard arguments.contains(detailIssueArgument),
              let index = arguments.firstIndex(of: detailIssueArgument),
              arguments.indices.contains(index + 1),
              let number = UInt64(arguments[index + 1]) else { return nil }
        return number
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
