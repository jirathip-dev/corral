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
        if HerdPerf574.enabled {
            // #574: the perf launch seeds ONLY the sized pager fixture, so the
            // pager's initial reconciliation lands on the fixture's page 1
            // (atlas-vector) instead of the small demo seed's promo-first repo.
            HerdPerf574.fixture = pagerFixtureDescription
            model.fleet.seedDemo(agents: pagerFixture(), rev: 1)
        } else {
            model.fleet.seedDemo(agents: seedAgents(dense:false), rev: 1)
        }
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
    static func reorderedAgents() -> [String:Agent] {
        var agents = seedAgents(dense:false)
        agents["herdr:herd-fixture-2"]?.state = .working
        agents["herdr:herd-fixture-4"]?.state = .idle
        agents["herdr:herd-fixture-7"]?.state = .idle
        return agents
    }
    struct Marker: Encodable {
        let phase: String
        let sceneID: String
        let night: Bool
        let environment: String
        let scroll: Double
        let paddock: String?
        let paddockOrder: [String]
        let workingPaddocks: [String]
        let paddockPosition: Int?
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
        let paddocks = HerdProjection.paddocks(scene.horses)
        let marker = Marker(phase:phase,sceneID:scene.sceneID.uuidString,night:scene.lighting.night,
                            environment:scene.effectiveEnvironment.rawValue,scroll:Double(scene.scroll),paddock:scene.paddockID,
                            paddockOrder:paddocks.map(\.title),
                            workingPaddocks:paddocks.filter { $0.horses.contains { $0.state == .working } }.map(\.title),
                            paddockPosition:paddocks.firstIndex { $0.id == scene.paddockID }.map { $0+1 },
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

/// #574 pager-perf instrumentation. DEBUG-only, opt-in via `-corral574Perf`
/// (read ONCE into `enabled`); when off every counted site costs exactly one
/// boolean check on that flag — no allocation, no string formatting, no
/// preference write, no timer, no logging. Counters are plain `Int`s, dumped
/// on demand as JSON to `Documents/herd-perf/` and mirrored into the
/// `g574-perf-state` accessibility value (the #568 `g568-geometry` pattern),
/// so a host script can read a dump without log parsing.
enum HerdPerf574 {
    static let enabled = CommandLine.arguments.contains("-corral574Perf")
    enum Site: Int, CaseIterable {
        case herdViewBody, paddockProjections, ranchBody, ranchPlanePaints, scrollChanges,
             pagerPageGeometry, rowBuilds, artBounds, captionResolutions,
             edgeOpacity, edgeGroupBodies, rowSnapTargets
        var name: String {
            switch self {
            case .herdViewBody: return "herdViewBody"
            case .paddockProjections: return "paddockProjections"
            case .ranchBody: return "ranchBody"
            case .ranchPlanePaints: return "ranchPlanePaints"
            case .scrollChanges: return "scrollChanges"
            case .pagerPageGeometry: return "pagerPageGeometry"
            case .rowBuilds: return "rowBuilds"
            case .artBounds: return "artBounds"
            case .captionResolutions: return "captionResolutions"
            case .edgeOpacity: return "edgeOpacity"
            case .edgeGroupBodies: return "edgeGroupBodies"
            case .rowSnapTargets: return "rowSnapTargets"
            }
        }
    }
    static var counts = [Int](repeating: 0, count: Site.allCases.count)
    /// Repos in the MOST RECENT `HerdProjection.paddocks` value (the fixture
    /// makes this constant; it proves the projection's size in the dump).
    static var projectionRepos = 0
    static var seq = 0
    static var fixture = "unseeded"
    static var lastDumpJSON = ""

    @inline(__always) static func tick(_ site: Site) {
        guard enabled else { return }
        counts[site.rawValue] += 1
    }
    static func noteProjection(repos: Int) {
        guard enabled else { return }
        counts[Site.paddockProjections.rawValue] += 1
        projectionRepos = repos
    }
    struct Dump: Encodable {
        let seq: Int
        let reason: String
        let monotonic: Double
        let wall: Double
        let fixture: String
        let counters: [String: Int]
    }
    /// On-demand dump: a monotonic timestamp, the counter set and the
    /// fixture/size description, written to `Documents/herd-perf/` and
    /// returned as JSON for the `g574-perf-state` accessibility value.
    @discardableResult
    static func dump(reason: String) -> String {
        guard enabled else { return lastDumpJSON }
        seq += 1
        var counters: [String: Int] = [:]
        for site in Site.allCases { counters[site.name] = counts[site.rawValue] }
        counters["paddockProjectionRepos"] = projectionRepos
        let payload = Dump(seq: seq, reason: reason,
                           monotonic: ProcessInfo.processInfo.systemUptime,
                           wall: Date().timeIntervalSince1970,
                           fixture: fixture, counters: counters)
        let encoder = JSONEncoder()
        encoder.outputFormatting = [.sortedKeys, .prettyPrinted]
        guard let data = try? encoder.encode(payload),
              let json = String(data: data, encoding: .utf8) else { return lastDumpJSON }
        lastDumpJSON = json
        do {
            let directory = FileManager.default.urls(for: .documentDirectory, in: .userDomainMask)[0]
                .appendingPathComponent("herd-perf")
            try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
            let name = "574-" + String(format: "%03d", seq) + "-" + reason + ".json"
            try data.write(to: directory.appendingPathComponent(name), options: .atomic)
        } catch { print("HerdPerf dump write failed: \(error)") }
        return json
    }
}

extension HerdEvidence {
    /// #574: the deterministic pager fixture at build-32 fleet size — 40
    /// agents across 12 repositories (skewed like the Mac's fleet: one
    /// 12-agent repo, a long tail), each row carrying the canonical
    /// `worktree_path` a facts-bearing frame resolves (the daemon's 155-row
    /// `git_worktree_facts` map is frame-level decode work; the
    /// client-visible rows are these 40). Three rows are blocked so the rail
    /// renders; five repos carry a working row (the #551 promo order) and the
    /// rest do not, so the page order is deterministic:
    /// atlas-vector, birch-daemon, cedar-tools, maple-client, willow-core,
    /// aspen-fixtures, elder-docs, hawthorn-ops, hazel-wire, juniper-ui,
    /// rowan-crates, sumac-ios. Same shape is seeded in BOTH A/B arms.
    static let pagerRepoSizes: [(String, Int)] = [
        ("atlas-vector", 12), ("birch-daemon", 5), ("cedar-tools", 5), ("maple-client", 4),
        ("willow-core", 3), ("aspen-fixtures", 2), ("elder-docs", 2), ("hawthorn-ops", 3),
        ("hazel-wire", 1), ("juniper-ui", 1), ("rowan-crates", 1), ("sumac-ios", 1)]
    static let pagerStates: [AgentState] = [
        .working, .idle, .blocked, .working, .done, .idle, .unknown, .working, .done, .blocked, .idle, .done,
        .working, .idle, .done, .unknown, .idle,
        .done, .idle, .working, .blocked, .idle,
        .unknown, .working, .idle, .done,
        .idle, .working, .done,
        .idle, .done,
        .unknown, .idle,
        .done, .idle, .unknown,
        .idle,
        .done,
        .unknown,
        .idle]
    static var pagerFixtureDescription: String {
        let agents = pagerRepoSizes.reduce(0) { $0 + $1.1 }
        return "build-32 fleet size: \(agents) agents, \(pagerRepoSizes.count) repositories, "
            + "155-worktree-fact frame shape, 3 blocked at rail (-corral574Perf)"
    }
    static func pagerFixture() -> [String: Agent] {
        var agents: [String: Agent] = [:]
        var index = 0
        for (repo, count) in pagerRepoSizes {
            for n in 0..<count {
                let id = "herdr:pager-fixture-\(index)"
                agents[id] = Agent(agentId: id, state: pagerStates[index], seq: UInt64(index + 1),
                                   ts: 1_800_000_000_000, capabilities: ["read_tail"],
                                   workspace: Workspace(repo: repo, branch: "g\(index)-lane",
                                                        worktreePath: "worktrees/\(repo)/agent-\(n)",
                                                        dirty: index % 3 == 0, ahead: 0,
                                                        behind: UInt64(index % 4)),
                                   attachment: Attachment(kind: "herdr", reference: "fixture:574:\(index)"),
                                   displayName: "\(repo)-\(n)")
                index += 1
            }
        }
        return agents
    }
    /// #574: seeds the sized pager fixture and records the fixture
    /// description every dump carries. Needs `HerdEvidence.model` (set by
    /// `HerdEvidence.seed` on the `-corralHerdEvidence` launch).
    @discardableResult
    static func seedPagerPerf() -> String {
        guard HerdPerf574.enabled else { return HerdPerf574.fixture }
        HerdPerf574.fixture = pagerFixtureDescription
        if let model {
            model.enterDemo()
            model.fleet.seedDemo(agents: pagerFixture(), rev: 1)
            model.fleetPresentation = .herd
        }
        return HerdPerf574.fixture
    }
}

extension HerdView {
    func runHerdEvidence() async {
        guard CommandLine.arguments.contains("-corralHerdEvidence"), !evidenceRan else { return }
        evidenceRan = true
        evidenceEnvironment = .day
        evidenceElapsed = 25-HorseIdentity(name:"willow-bend").phase
        guard await evidencePause() else { return }
        evidencePhase = "449-01-active-first"
        guard await evidencePause(10) else { return }
        HerdEvidence.model?.fleet.seedDemo(agents:HerdEvidence.reorderedAgents(),rev:2)
        guard await evidencePause() else { return }
        evidencePhase = "449-02-selection-retained"
        guard await evidencePause(10) else { return }
        HerdEvidence.model?.fleet.seedDemo(agents:HerdEvidence.seedAgents(dense:false),rev:3)
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
        HerdEvidence.model?.fleet.seedDemo(agents:HerdEvidence.seedAgents(dense:true),rev:4)
        guard await evidencePause() else { return }
        evidencePhase = "06-dense-blocked"
        guard await evidencePause() else { return }
        evidenceReduceMotion = false
        evidenceElapsed = nil
        guard await evidencePause() else { return }
        evidencePhase = "07-clock-running"
        guard await evidencePause() else { return }
        // #528: the Open Board recovery override is REMOVED with the Herd
        // disconnect panel; this recorded-evidence harness still needs its
        // final Board frame, so it flips the presentation directly (the
        // same surface the Settings picker writes).
        HerdEvidence.model?.fleetPresentation = .board
    }
    private func evidencePause(_ seconds:Int = 2) async -> Bool {
        do { try await Task.sleep(for:.seconds(seconds)); return true }
        catch { return false }
    }
}
#endif
