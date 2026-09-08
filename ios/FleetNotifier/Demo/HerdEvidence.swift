#if DEBUG
import SwiftUI

// Opt-in fictional data, never a live fallback. Uses the real FleetView,
// BoardModel projections, HerdView, environment and native vector renderer.
@MainActor
enum HerdEvidence {
    static weak var model: AppModel?
    static func seed(_ model: AppModel) {
        self.model = model
        model.enterDemo()
        model.fleet.seedDemo(agents: seedAgents(dense:false),rev:1)
        model.fleetPresentation = .herd
    }
    static func seedAgents(dense:Bool) -> [String:Agent] {
        let names = ["birch-clearing","oak-before-dark","spruce-hollow","willow-bend", "cedar-ridge", "aspen-grove",
                     "juniper-south","maple-stand","elder-flats","hazel-hollow","hawthorn-fork","sumac-row"]
        let states: [AgentState] = [.blocked,.blocked,.done,.idle,.working,.unknown,.idle,.working,.done,.working,.idle,.working]
        var result: [String:Agent] = [:]
        for i in 0..<(dense ? 60 : names.count) {
            let name = names[i%names.count] + (i < names.count ? "" : "-\(i)")
            let id = "herdr:herd-fixture-\(i)"
            let state: AgentState = dense && i < 12 ? .blocked : states[i%states.count]
            let repo = ["atlas-vector","cedar-tools","maple-client","willow-core"][(i/4)%4]
            result[id] = Agent(agentId:id,state:state,seq:UInt64(i+1),ts:1_800_000_000_000,
                               capabilities:["read_tail"],workspace:Workspace(repo:repo),
                               attachment:Attachment(kind:"herdr",reference:"fixture:\(i)"),displayName:name)
        }
        return result
    }
    struct Marker: Encodable {
        let phase: String
        let sceneID: String
        let night: Bool
        let environment: String
        let scroll: Double
        let paddock: String?
        let reduceMotion: Bool
        let clockRunning: Bool
        let ticks: Int
        let horseIDs: [String]
        let planeOffsets: [String:Double]
        let fixture: String
        let repositoryFilter: String?
        let hostFilter: String?
        let selectedAgent: String?
    }
    static func observeDismissal(scene: HerdView) {
        guard CommandLine.arguments.contains("-corralHerdEvidence") else { return }
        Task {
            do { try await Task.sleep(for:.milliseconds(700)) }
            catch { return }
            record("dismissed-stable",scene:scene)
        }
    }
    static func record(_ phase:String,scene:HerdView) {
        guard CommandLine.arguments.contains("-corralHerdEvidence") else { return }
        let marker = Marker(phase:phase,sceneID:scene.sceneID.uuidString,night:scene.lighting.night,
                            environment:scene.effectiveEnvironment.rawValue,scroll:Double(scene.scroll),paddock:scene.paddockID,
                            reduceMotion:scene.reduced,clockRunning:scene.clock.running,ticks:scene.clock.ticks,
                            horseIDs:scene.horses.map(\.id),planeOffsets:Dictionary(uniqueKeysWithValues:
                                RanchPlane.allCases.map { ($0.rawValue,Double($0.offset(scroll:scene.scroll,coverage:10_000,
                                    reduceMotion:$0 == .horses ? false : !scene.motionEnabled))) }),
                            fixture:"DEBUG fictional fleet; not live operational evidence",
                            repositoryFilter:model?.repoFilter,
                            hostFilter:model?.hostFilterProfile?.id.uuidString,
                            selectedAgent:model?.recentsRequest?.agentId)
        do {
            let directory = FileManager.default.urls(for:.documentDirectory,in:.userDomainMask)[0].appendingPathComponent("herd-evidence")
            try FileManager.default.createDirectory(at:directory,withIntermediateDirectories:true)
            let encoder = JSONEncoder(); encoder.outputFormatting = [.sortedKeys,.prettyPrinted]
            try encoder.encode(marker).write(to:directory.appendingPathComponent(phase+".json"),options:.atomic)
        } catch { print("Herd evidence write failed: \(error)") }
    }
}

extension HerdView {
    func runHerdEvidence() async {
        guard CommandLine.arguments.contains("-corralHerdEvidence"), !evidenceRan else { return }
        evidenceRan = true
        evidenceEnvironment = .day
        evidenceElapsed = 25-HorseIdentity(name:"willow-bend").phase
        guard await evidencePause() else { return }
        evidencePhase = "01-day-grazing"
        guard await evidencePause() else { return }
        // ONE view/scene identity, same agents, filters, paddock and pose.
        evidenceEnvironment = .night
        guard await evidencePause() else { return }
        evidencePhase = "02-night-same-scene"
        guard await evidencePause() else { return }
        movePage(1)
        guard await evidencePause() else { return }
        evidencePhase = "03-parallax-next"
        guard await evidencePause() else { return }
        evidenceReduceMotion = true
        guard await evidencePause() else { return }
        evidencePhase = "04-reduce-motion"
        guard await evidencePause() else { return }
        evidenceEnvironment = .auto
        guard await evidencePause() else { return }
        evidencePhase = "05-auto-fallback"
        guard await evidencePause() else { return }
        HerdEvidence.model?.fleet.seedDemo(agents:HerdEvidence.seedAgents(dense:true),rev:2)
        guard await evidencePause() else { return }
        evidencePhase = "06-dense-blocked"
        guard await evidencePause() else { return }
        evidenceReduceMotion = false
        evidenceElapsed = nil
        guard await evidencePause() else { return }
        evidencePhase = "07-clock-running"
        guard await evidencePause() else { return }
        openBoard()
    }
    private func evidencePause() async -> Bool {
        do { try await Task.sleep(for:.seconds(2)); return true }
        catch { return false }
    }
}
#endif
