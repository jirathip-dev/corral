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
        XCTAssertTrue(route.contains("openBoard:{model.openBoard()}"),
                      "Herd's Open Board recovery must route through the temporary-override API")
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
        model.selectFleetPresentation(.herd)
        XCTAssertEqual(model.repoFilter, "demo-atlas")
        XCTAssertEqual(model.fleet.agents, snapshot)
        XCTAssertEqual(model.recentsRequest, boardRequest)
        model.requestRecents(for: horse.agent.agentId, hostProfileID: horse.hostProfileID, haptic: false)
        XCTAssertEqual(model.recentsRequest?.agentId, boardRequest.agentId)
        XCTAssertEqual(model.recentsRequest?.hostProfileID, boardRequest.hostProfileID)
        model.selectFleetPresentation(.board)
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

/// #458 stream-preservation probe: serves 200 text/event-stream and never
/// finishes — an idle fleet delivers zero frames. Lock-guarded request
/// counter (same pattern as the file-scope URLProtocol mocks in
/// FleetNotifierTests.swift).
private final class PresentationStreamURLProtocol: URLProtocol {
    private static let requestLock = NSLock()
    private static var requestCountStorage = 0

    static func resetRequestCount() {
        requestLock.lock()
        requestCountStorage = 0
        requestLock.unlock()
    }

    static var requestCount: Int {
        requestLock.lock()
        defer { requestLock.unlock() }
        return requestCountStorage
    }

    override class func canInit(with request: URLRequest) -> Bool { true }
    override class func canonicalRequest(for request: URLRequest) -> URLRequest { request }

    override func startLoading() {
        Self.requestLock.lock()
        Self.requestCountStorage += 1
        Self.requestLock.unlock()
        guard let url = request.url else {
            client?.urlProtocol(self, didFailWithError: URLError(.badURL))
            return
        }
        // SAFETY: HTTPURLResponse with fixed status/text is always valid.
        let response = HTTPURLResponse(
            url: url, statusCode: 200, httpVersion: "HTTP/1.1",
            headerFields: ["Content-Type": "text/event-stream"])!
        client?.urlProtocol(self, didReceive: response, cacheStoragePolicy: .notAllowed)
        client?.urlProtocol(self, didLoad: Data())
        // Deliberately never finishes: the daemon's stream stays open.
    }

    override func stopLoading() {}
}

@MainActor
final class PresentationPreferenceTests: XCTestCase {
    private func suite(_ name: String) throws -> UserDefaults {
        let suiteName = "presentation-\(name)-\(UUID().uuidString)"
        let defaults = try XCTUnwrap(UserDefaults(suiteName: suiteName))
        defaults.removePersistentDomain(forName: suiteName)
        return defaults
    }

    func testFreshLegacyInvalidAndStoredDefaultsRestorePresentation() throws {
        let fresh = try suite("fresh")
        let freshModel = AppModel(defaults: fresh)
        XCTAssertEqual(freshModel.fleetPresentation, .board)
        XCTAssertEqual(freshModel.mode, .needsSetup,
                       "presentation restoration must not bypass setup")

        let invalid = try suite("invalid")
        invalid.set("gallery", forKey: "fleetnotifier.fleetPresentation")
        let invalidModel = AppModel(defaults: invalid)
        XCTAssertEqual(invalidModel.fleetPresentation, .board)
        XCTAssertEqual(invalidModel.mode, .needsSetup)

        let stored = try suite("stored")
        stored.set(FleetPresentation.herd.rawValue,
                   forKey: AppModel.fleetPresentationKey)
        let restoredModel = AppModel(defaults: stored)
        XCTAssertEqual(restoredModel.fleetPresentation, .herd,
                       "a cold model must restore the saved presentation")
        XCTAssertEqual(restoredModel.mode, .needsSetup,
                       "restoring Herd must preserve the setup gate")
    }

    func testSettingsSelectionsApplyImmediatelyAndRoundTripBothValues() throws {
        let defaults = try suite("round-trip")
        let model = AppModel(defaults: defaults)

        model.selectFleetPresentation(.herd)
        XCTAssertEqual(model.fleetPresentation, .herd)
        XCTAssertEqual(model.savedFleetPresentation, .herd)
        XCTAssertEqual(defaults.string(forKey: AppModel.fleetPresentationKey),
                       FleetPresentation.herd.rawValue)
        XCTAssertEqual(AppModel(defaults: defaults).fleetPresentation, .herd,
                       "a cold model must restore Herd")

        model.selectFleetPresentation(.board)
        XCTAssertEqual(model.fleetPresentation, .board)
        XCTAssertEqual(model.savedFleetPresentation, .board)
        XCTAssertEqual(defaults.string(forKey: AppModel.fleetPresentationKey),
                       FleetPresentation.board.rawValue)
        XCTAssertEqual(AppModel(defaults: defaults).fleetPresentation, .board,
                       "a cold model must restore Board")
    }

    func testOpenBoardIsATemporaryOverrideThatNeverRewritesTheSavedPreference() throws {
        let defaults = try suite("open-board")
        let model = AppModel(defaults: defaults)
        model.selectFleetPresentation(.herd)
        XCTAssertEqual(model.fleetPresentation, .herd)
        XCTAssertEqual(model.savedFleetPresentation, .herd)

        model.openBoard()
        XCTAssertEqual(model.fleetPresentation, .board,
                       "Open Board applies immediately as a temporary override")
        XCTAssertEqual(model.savedFleetPresentation, .herd,
                       "the recovery override must not rewrite the saved preference")
        XCTAssertEqual(defaults.string(forKey: AppModel.fleetPresentationKey),
                       FleetPresentation.herd.rawValue,
                       "UserDefaults must keep the saved Herd choice untouched")

        model.selectFleetPresentation(.herd)
        XCTAssertEqual(model.fleetPresentation, .herd,
                       "a later explicit Settings selection wins immediately")
        XCTAssertEqual(model.savedFleetPresentation, .herd)
    }

    func testPresentationChangesPreserveTheLiveFleetStream() async throws {
        let defaults = try suite("stream")
        let model = AppModel(defaults: defaults)
        model.enterDemo()
        defer { model.stopLive() }
        let configuration = URLSessionConfiguration.ephemeral
        configuration.protocolClasses = [PresentationStreamURLProtocol.self]
        let session = URLSession(configuration: configuration)
        // SAFETY: fixed literal https URL used only by this test client.
        let client = CorraldClient(host: URL(string: "https://fleet.test")!,
                                   session: session)
        model.fleet.connect(client: client)
        try await Task.sleep(for: .milliseconds(400))
        XCTAssertTrue(model.fleet.isStreaming)
        let generation = model.fleet.connectionGeneration
        let snapshot = model.fleet.agents

        model.selectFleetPresentation(.herd)
        model.openBoard()
        model.selectFleetPresentation(.board)

        try await Task.sleep(for: .milliseconds(300))
        XCTAssertEqual(model.fleet.connectionGeneration, generation,
                       "presentation changes must never restart the live stream")
        XCTAssertTrue(model.fleet.isStreaming)
        XCTAssertEqual(model.fleet.agents, snapshot,
                       "presentation changes must not touch the live read model")
        let disconnect = model.fleet.disconnect()
        await disconnect?.value
    }

    func testDemoModeDoesNotLeakOrReadTheLivePreference() throws {
        let defaults = try suite("demo-isolation")
        defaults.set("gallery", forKey: AppModel.fleetPresentationKey)
        let model = AppModel(defaults: defaults)
        model.enterDemo()
        model.selectFleetPresentation(.herd)
        XCTAssertEqual(model.fleetPresentation, .herd)
        model.exitDemo()
        // .needsSetup: no legacy identity/profile store in this fixture —
        // the demo never fabricated a pairing, so the setup gate stays.
        XCTAssertEqual(model.mode, .needsSetup)
        XCTAssertEqual(defaults.string(forKey: AppModel.fleetPresentationKey),
                       FleetPresentation.herd.rawValue,
                       "the Settings choice survives the demo round-trip")
    }

    func testHerdsSetupPathKeepsBoardDefaultUntilAPreferenceExists() throws {
        let defaults = try suite("setup")
        let model = AppModel(defaults: defaults)
        XCTAssertEqual(model.mode, .needsSetup)
        XCTAssertEqual(model.fleetPresentation, .board,
                       "an unpaired model has no reason to prefer Herd")
        let reloaded = AppModel(defaults: defaults)
        XCTAssertEqual(reloaded.fleetPresentation, .board)
        XCTAssertEqual(reloaded.mode, .needsSetup)
    }

    func testToolbarKeepsOnlyTheSettingsGearAndNeverRegrowsTheTopSwitch() throws {
        let bundle = Bundle(for: HerdTests.self)
        let url = try XCTUnwrap(bundle.url(forResource: "FleetViews",
                                           withExtension: "swift.txt"))
        let source = try String(contentsOf: url, encoding: .utf8)
        let fromToolbar = try XCTUnwrap(source.range(of: ".toolbar {"))
        // The board toolbar is the FIRST toolbar in the file and ends at
        // the second .sheet modifier chain; Settings' own toolbar
        // (navigationTitle "Settings") comes later and is out of scope.
        let settingsTitle = try XCTUnwrap(source.range(of: "struct SettingsView: View {"))
        let boardToolbar = source[fromToolbar.lowerBound..<settingsTitle.lowerBound]
        XCTAssertFalse(boardToolbar.contains("ToolbarItem(placement: .principal)"),
                       "the principal top Board/Herd switch is removed")
        XCTAssertFalse(boardToolbar.contains("Text(mode.rawValue)"),
                       "no segment-styled mode switch may regrow in the toolbar")
        XCTAssertFalse(boardToolbar.contains("model.fleetPresentation = mode"),
                       "the toolbar must never write the presentation directly")
        XCTAssertTrue(boardToolbar.contains("Image(systemName: \"gearshape\")"),
                      "the Settings gear stays in the toolbar")
        XCTAssertTrue(boardToolbar.contains("SettingsView(model: model)"),
                      "Settings remains reachable from the top bar")
    }

    func testAppearanceSectionOwnsTheCanonicalPersistedPicker() throws {
        let bundle = Bundle(for: HerdTests.self)
        let url = try XCTUnwrap(bundle.url(forResource: "FleetViews",
                                           withExtension: "swift.txt"))
        let source = try String(contentsOf: url, encoding: .utf8)
        let fromAppearance = try XCTUnwrap(source.range(of: "private var appearanceSection: some View {"))
        let section = String(source[fromAppearance.lowerBound...])
        XCTAssertTrue(section.contains("Picker(\"Board or Herd\""),
                      "the canonical Board/Herd picker lives in the Appearance section")
        XCTAssertTrue(section.contains("model.selectFleetPresentation($0)"),
                      "a Settings selection routes through the persisting API")
        XCTAssertTrue(section.contains("accessibilityHint(\"Choose what the app opens to. Board is the default. This choice is saved.\")"),
                      "the saved-choice explanation is exposed to VoiceOver")
        XCTAssertTrue(section.contains(".pickerStyle(.segmented)"),
                      "the Board/Herd choice renders as a visible segmented control")
    }
}
