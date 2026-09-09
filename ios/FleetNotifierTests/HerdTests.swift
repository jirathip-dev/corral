import XCTest
import SwiftUI
@testable import FleetNotifier

final class HerdTests: XCTestCase {
    func testNativeDynamicTypeTapZonesAndSharedDestinationWiring() throws {
        func source(_ name: String) throws -> String {
            let url = try XCTUnwrap(Bundle(for:Self.self).url(forResource:name+".swift",withExtension:"txt"))
            return try String(contentsOf:url,encoding:.utf8).filter { !$0.isWhitespace }
        }
        let herd = try source("HerdView")
        let buttonStart = try XCTUnwrap(herd.range(of:"privatefunchorseButton("))
        let button = String(herd[buttonStart.lowerBound...])
        XCTAssertTrue(button.contains("Button{select(horse)}label:"))
        XCTAssertTrue(button.contains(".frame(minWidth:156,minHeight:44).contentShape(Rectangle())"))
        XCTAssertTrue(button.contains(".accessibilityLabel("))
        XCTAssertTrue(herd.contains("minimum:dynamicType.isAccessibilitySize?width-32:156"))
        XCTAssertTrue(herd.contains("paddockID=HerdProjection.reconciledPaddockID(paddockID,in:ids)"))
        let board = try source("FleetViews")
        let start = try XCTUnwrap(board.range(of:"HerdView(horses:"))
        let end = try XCTUnwrap(board.range(of:"retry:",range:start.upperBound..<board.endIndex))
        let route = String(board[start.lowerBound..<end.lowerBound])
        XCTAssertTrue(route.contains("HerdProjection.multiple(hostSections"))
        XCTAssertTrue(route.contains("HerdProjection.single(sections"))
        XCTAssertTrue(route.contains("model.requestRecents(for:horse.agent.agentId,hostProfileID:horse.hostProfileID,haptic:false)"))
    }

    func testIdentityMatchesOriginalV1AndSurvivesEveryPresentationInput() {
        let identity = HorseIdentity(name: "willow-bend")
        XCTAssertEqual(identity.coat, 6)
        XCTAssertEqual(identity.breed, 2)
        XCTAssertEqual(identity.mane, 2)
        XCTAssertEqual(identity.tack, 1)
        XCTAssertEqual(identity.accessory, 2)
        for state in AgentState.allCases {
            var agent = Agent(agentId: "opaque", state: state, displayName: "willow-bend")
            for refresh in 1...10 {
                agent.seq = UInt64(refresh)
                agent.title = "Changed task title"
                let horse = HerdHorse(agent: agent, hostProfileID: UUID(), hostName: "Another host", disconnected: false)
                XCTAssertEqual(horse.identity, identity)
                _ = horse.pose(elapsed: Double(refresh), reduceMotion: false)
                XCTAssertEqual(HorseIdentity(name: horse.name), identity)
            }
        }
        XCTAssertNotEqual(HorseIdentity(name: "Willow-bend"), identity)
    }

    func testStatesRailStaticSubstitutionAndRoamingBounds() {
        for state in AgentState.allCases {
            let horse = HerdHorse(agent: Agent(agentId: state.rawValue, state: state),
                                  hostProfileID: nil, hostName: nil, disconnected: false)
            XCTAssertEqual(horse.atRail, state == .blocked)
            for tick in 0...120 {
                let x = horse.roam(elapsed: Double(tick), enabled: true)
                XCTAssertLessThanOrEqual(abs(x), 4)
                XCTAssertGreaterThanOrEqual(12-abs(x), 8, "art remains inside stationary 156-point hit zone")
                XCTAssertEqual(horse.roam(elapsed: Double(tick), enabled: false), 0)
                if state == .done || state == .unknown || state == .blocked { XCTAssertEqual(x, 0) }
            }
            if state == .working || state == .idle {
                XCTAssertEqual(horse.pose(elapsed: 25-horse.identity.phase, reduceMotion: true), .stand)
            }
            if state == .done { XCTAssertEqual(horse.pose(elapsed: 25, reduceMotion: false), .done) }
            if state == .idle { XCTAssertEqual(horse.pose(elapsed: 25-horse.identity.phase, reduceMotion: false), .graze) }
            let stale = HerdHorse(agent: horse.agent, hostProfileID: nil, hostName: nil, disconnected: true)
            XCTAssertEqual(stale.state, .unknown)
            XCTAssertEqual(stale.pose(elapsed: 25, reduceMotion: false), .unknown)
            XCTAssertEqual(stale.roam(elapsed: 25, enabled: true), 0)
            XCTAssertTrue(stale.statusText.contains("last known"))
        }
    }

    @MainActor func testBoardHerdShareFiltersSnapshotAndRecentOutputDestination() throws {
        let model = AppModel()
        model.enterDemo()
        defer { model.stopLive() }
        model.repoFilter = "demo-atlas"
        let agents = Array(model.fleet.agents.values)
        let scope = BoardModel.reconcile(model.repoFilter, against: BoardModel.repoFilters(agents))
        let sections = BoardModel.sections(BoardModel.agents(agents, in: scope))
        let horses = HerdProjection.single(sections, host: nil, disconnected: false)
        let horse = try XCTUnwrap(horses.first)
        model.requestRecents(for: horse.agent.agentId, haptic: false)
        let boardRequest = try XCTUnwrap(model.recentsRequest)
        let snapshot = model.fleet.agents
        model.fleetPresentation = .herd
        XCTAssertEqual(model.repoFilter, "demo-atlas")
        XCTAssertEqual(model.fleet.agents, snapshot)
        XCTAssertEqual(model.recentsRequest, boardRequest)
        model.requestRecents(for: horse.agent.agentId, hostProfileID: horse.hostProfileID, haptic: false)
        XCTAssertEqual(model.recentsRequest?.agentId, boardRequest.agentId)
        XCTAssertEqual(model.recentsRequest?.hostProfileID, boardRequest.hostProfileID)
        model.fleetPresentation = .board
        XCTAssertEqual(model.repoFilter, "demo-atlas")
        XCTAssertEqual(model.fleet.agents, snapshot)
        XCTAssertEqual(Set(horses.map(\.agent.agentId)), Set(BoardModel.agents(agents, in: scope).map(\.agentId)))
    }

    func testMultiHostFiltersAndPaddockCountsDoNotDoubleCountRail() {
        let a = UUID(), b = UUID()
        let rows = [HostBoardRow(identity: CompositeAgentID(hostProfileID: a, agentID: "same"),
            agent: Agent(agentId: "same", state: .blocked, workspace: Workspace(repo: "r")), isStale: false, lastSeen: 1),
            HostBoardRow(identity: CompositeAgentID(hostProfileID: b, agentID: "same"),
            agent: Agent(agentId: "same", state: .working, workspace: Workspace(repo: "r")), isStale: true, lastSeen: 1)]
        let selected = BoardModel.rows(BoardModel.rows(rows, forHost: a), in: "r")
        let horses = HerdProjection.multiple(BoardModel.hostSections(selected), names: [a:"A", b:"B"])
        XCTAssertEqual(horses.map(\.hostProfileID), [a])
        let all = HerdProjection.multiple(BoardModel.hostSections(rows), names: [a:"A", b:"B"])
        XCTAssertEqual(Set(all.map(\.id)).count, 2)
        let paddocks = HerdProjection.paddocks(all)
        XCTAssertEqual(paddocks.count, 1)
        XCTAssertEqual(paddocks[0].field.count + paddocks[0].blockedCount, all.count)
        XCTAssertEqual(all.filter(\.disconnected).count, 1)
    }

    func testPaddocksPrioritizeWorkingRepositoriesThenAlphabetizeEachTier() {
        let horses = [
            herdHorse("inactive-alpha", repo: "alpha", state: .idle),
            herdHorse("active-zulu", repo: "zulu", state: .working),
            herdHorse("active-beta", repo: "beta", state: .working),
            herdHorse("active-other", repo: nil, state: .working),
            herdHorse("inactive-gamma", repo: "gamma", state: .done),
        ]
        let expected: [String?] = ["beta", "zulu", nil, "alpha", "gamma"]
        XCTAssertEqual(HerdProjection.paddocks(horses).map(\.repo), expected)
    }

    func testPaddocksDoNotPromoteDisconnectedWorkingOrBlockedAgents() {
        let horses = [
            herdHorse("stale-alpha", repo: "alpha", state: .working, disconnected: true),
            herdHorse("blocked-beta", repo: "beta", state: .blocked),
            herdHorse("active-zulu", repo: "zulu", state: .working),
        ]
        let expected: [String?] = ["zulu", "alpha", "beta"]
        XCTAssertEqual(HerdProjection.paddocks(horses).map(\.repo), expected)
    }

    func testPaddockOrderingTracksWorkingIdleDoneAndReconnectTransitions() {
        let alpha = herdHorse("inactive-alpha", repo: "alpha", state: .idle)
        func repos(_ state: AgentState, disconnected: Bool = false) -> [String?] {
            HerdProjection.paddocks([
                alpha,
                herdHorse("changing-zulu", repo: "zulu", state: state, disconnected: disconnected),
            ]).map(\.repo)
        }
        XCTAssertEqual(repos(.working), ["zulu", "alpha"])
        XCTAssertEqual(repos(.idle), ["alpha", "zulu"])
        XCTAssertEqual(repos(.done), ["alpha", "zulu"])
        XCTAssertEqual(repos(.working, disconnected: true), ["alpha", "zulu"])
        XCTAssertEqual(repos(.working, disconnected: false), ["zulu", "alpha"])
    }

    func testPaddockSelectionPreservesIdentityAcrossReorderAndFallsBackAfterRemoval() {
        let selected = "repo:zulu"
        let activeFirst = HerdProjection.paddocks([
            herdHorse("inactive-alpha", repo: "alpha", state: .idle),
            herdHorse("active-zulu", repo: "zulu", state: .working),
        ]).map(\.id)
        XCTAssertEqual(activeFirst, [selected, "repo:alpha"])
        XCTAssertEqual(HerdProjection.reconciledPaddockID(nil, in: activeFirst), selected)

        let reordered = HerdProjection.paddocks([
            herdHorse("active-alpha", repo: "alpha", state: .working),
            herdHorse("inactive-zulu", repo: "zulu", state: .idle),
        ]).map(\.id)
        XCTAssertEqual(reordered, ["repo:alpha", selected])
        XCTAssertEqual(HerdProjection.reconciledPaddockID(selected, in: reordered), selected)
        XCTAssertEqual(HerdProjection.reconciledPaddockID(selected, in: ["repo:alpha"]), "repo:alpha")
        XCTAssertNil(HerdProjection.reconciledPaddockID(selected, in: []))
    }

    func testNativePlaneRatiosDirectionCoverageAndOneSceneLighting() {
        XCTAssertEqual(RanchEnvironment.planes, [.sky,.hills,.ground,.barnTrees,.rearFences,.foreground])
        XCTAssertEqual(RanchPlane.allCases.map(\.ratio), [0.05,0.12,0.22,0.4,0.4,0.65,1,0])
        for plane in RanchPlane.allCases {
            XCTAssertEqual(plane.offset(scroll: 100, coverage: 500, reduceMotion: false), -100*plane.ratio)
            XCTAssertEqual(plane.offset(scroll: -100, coverage: 500, reduceMotion: false), 100*plane.ratio)
            XCTAssertLessThanOrEqual(abs(plane.offset(scroll: 100_000, coverage: 390, reduceMotion: false)), 390)
            XCTAssertEqual(plane.offset(scroll: 100, coverage: 500, reduceMotion: true), 0)
            XCTAssertEqual(plane.offset(scroll: .nan, coverage: 500, reduceMotion: false), 0)
        }
        let day = RanchPainter(night: false, elapsed: 0)
        let night = RanchPainter(night: true, elapsed: 0)
        for index in 0...500 { XCTAssertEqual(day.noise(index), night.noise(index)) }
    }

    @MainActor func testClockCancellationOnDismissalStopsRealTicks() async throws {
        let clock = HerdClock()
        clock.start()
        try await Task.sleep(for: .milliseconds(350))
        XCTAssertGreaterThan(clock.ticks, 0)
        clock.stop()
        let stopped = clock.ticks
        try await Task.sleep(for: .milliseconds(350))
        XCTAssertEqual(clock.ticks, stopped, "a lifecycle-owned animation source survived dismissal")
        XCTAssertFalse(clock.running)
        XCTAssertEqual(clock.elapsed, 0)
        clock.start()
        try await Task.sleep(for: .milliseconds(150))
        XCTAssertGreaterThan(clock.ticks, stopped)
        clock.stop()
    }

    @MainActor func testRemovingRealHerdViewCancelsItsAnimationSource() async throws {
        let clock = HerdClock()
        let horse = HerdHorse(agent: Agent(agentId: "lifecycle", state: .idle),
                              hostProfileID: nil, hostName: nil, disconnected: false)
        let scene = HerdView(horses: [horse], obscured: false, select: { _ in },
                             openBoard: {}, retry: {}, clock: clock)
        let controller = UIHostingController(rootView: AnyView(scene.environmentObject(ThemeStore())))
        let window = UIWindow(frame: CGRect(x: 0, y: 0, width: 393, height: 852))
        window.rootViewController = controller
        window.makeKeyAndVisible()
        try await Task.sleep(for: .milliseconds(300))
        clock.start()
        try await Task.sleep(for: .milliseconds(250))
        XCTAssertGreaterThan(clock.ticks, 0)
        controller.rootView = AnyView(Color.clear)
        try await Task.sleep(for: .milliseconds(300))
        let ticks = clock.ticks
        XCTAssertFalse(clock.running, "real .onDisappear must cancel its clock")
        try await Task.sleep(for: .milliseconds(250))
        XCTAssertEqual(clock.ticks, ticks, "hidden Herd continues rendering")
        clock.stop()
        window.isHidden = true
    }

    func testOriginalGrazingHasHorseNeckJawAndGroundedMuzzle() throws {
        let art = try HerdArt.load()
        let identity = HorseIdentity(name: "willow-bend")
        let neck = try XCTUnwrap(art.drawing(identity, pose: .graze).first { $0.part == "grazing-neck" })
        // Sample the actual filled path consumed by Canvas, at the 132×100
        // in-scene point size. Old V1 has an 8-point tube and ends at y=84.
        let width = (70...125).filter { neck.path.contains(CGPoint(x: $0, y: 55)) }.count
        XCTAssertGreaterThanOrEqual(width, 14, "withers-to-throat must be a load-bearing neck, not a tube")
        let head = try XCTUnwrap(art.drawing(identity, pose: .graze).first { $0.part == "grazing-head" })
        XCTAssertGreaterThanOrEqual(head.path.boundingRect.maxY, 94, "muzzle reaches the grass beside grounded hooves")
        XCTAssertTrue(head.path.contains(CGPoint(x: 113, y: 78)), "distinct cheek/jaw behind the muzzle")
        let ears = art.drawing(identity, pose: .graze).filter { $0.part == "grazing-ear" }
        XCTAssertEqual(ears.count, 2, "two separated ears at the poll")
        XCTAssertTrue(ears.allSatisfy { $0.path.boundingRect.height >= 6 })
    }

    private func herdHorse(_ id: String, repo: String?, state: AgentState,
                           disconnected: Bool = false) -> HerdHorse {
        HerdHorse(agent: Agent(agentId: id, state: state, workspace: Workspace(repo: repo)),
                  hostProfileID: nil, hostName: nil, disconnected: disconnected)
    }
}
