import SwiftUI

@main
struct FleetNotifierApp: App {
    @StateObject private var model = AppModel()
    @Environment(\.scenePhase) private var scenePhase

    var body: some Scene {
        WindowGroup {
            RootView(model: model)
                .task {
                    // Dev-only launch-arg harnesses (see LiveVerifyRunner).
                    if CommandLine.arguments.contains("-liveVerify") {
                        LiveVerifyRunner(model: model).run()
                    } else if CommandLine.arguments.contains("-demoMode") {
                        model.enterDemo()
                    }
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
                        }
                    @unknown default:
                        break
                    }
                }
        }
    }
}

struct RootView: View {
    @ObservedObject var model: AppModel

    var body: some View {
        FleetView(model: model)
    }
}
