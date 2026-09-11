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

// MARK: - #456 full-screen Herd shell (ranch behind safe areas, floating chrome)

/// Source-wiring pins over the bundled FleetViews/HerdView/RanchEnvironment
/// sources for the #456 layout contract: the procedural ranch is the
/// full-screen root behind the safe areas, the top scope + Settings float
/// over it (no board header strip, no duplicated navigation toolbar), the
/// bottom paddock navigation floats above the home indicator with >= 44 pt
/// targets, and the outage/counts/empty-scope surfaces survive.
final class FullScreenHerdShellWiringTests: XCTestCase {

    private func source(_ name: String) throws -> String {
        let url = try XCTUnwrap(Bundle(for: Self.self).url(forResource: name + ".swift",
                                                            withExtension: "txt"))
        return try String(contentsOf: url, encoding: .utf8)
    }

    /// Whitespace-stripped form — pins survive re-indentation.
    private func compact(_ text: String) -> String {
        text.filter { !$0.isWhitespace }
    }

    /// 1-based line numbers of every line whose `#if DEBUG` nesting makes it
    /// DEBUG-active (flat, non-nested pairs — same scan the #365 wiring
    /// tests use).
    private func debugActiveLines(_ source: String) -> Set<Int> {
        var active: Set<Int> = []
        var depth = 0
        for (index, line) in source.split(separator: "\n",
                                          omittingEmptySubsequences: false).enumerated() {
            let trimmed = line.trimmingCharacters(in: .whitespaces)
            if trimmed.hasPrefix("#if DEBUG") {
                depth += 1
            } else if trimmed.hasPrefix("#endif") {
                depth = max(0, depth - 1)
            }
            if depth > 0 { active.insert(index + 1) }
        }
        return active
    }

    func testFullScreenRanchSitsBehindTheHerdSurfaceAndSafeAreas() throws {
        let herd = try compact(source("HerdView"))
        // The ranch is the FIRST child of the body's root ZStack (behind the
        // content) and ignores the safe area, so Day/Night paint behind the
        // top scope/Settings row, the rail and the bottom navigation.
        let bodyStart = try XCTUnwrap(herd.range(of: "var body: some View {".filter { !$0.isWhitespace }))
        let ranch = try XCTUnwrap(herd.range(of: "RanchEnvironment(", range: bodyStart.upperBound..<herd.endIndex),
                                  "the full-screen ranch must render from HerdView's root")
        let zstack = try XCTUnwrap(herd.range(of: "ZStack{", range: bodyStart.upperBound..<ranch.lowerBound),
                                   "the ranch must sit inside the root ZStack")
        XCTAssertLessThan(zstack.lowerBound, ranch.lowerBound,
                          "the ranch is the background layer, not an overlay")
        let content = try XCTUnwrap(herd.range(of: "VStack(spacing:0){topChrome", range: ranch.upperBound..<herd.endIndex),
                                    "the floating chrome + content must render ABOVE the ranch")
        XCTAssertLessThan(ranch.lowerBound, content.lowerBound,
                          "content must be layered over the ranch")
        let safeArea = try XCTUnwrap(herd.range(of: ".ignoresSafeArea()", range: ranch.upperBound..<content.lowerBound),
                                     "the ranch must extend behind the top/bottom safe areas")
        XCTAssertLessThan(ranch.upperBound, safeArea.lowerBound,
                          "ignoresSafeArea must apply to the ranch, not the content")
        XCTAssertTrue(herd.contains("maxScroll:CGFloat(max(0,paddocks.count-1))*screen.size.width"),
                      "the full-screen ranch keeps the shared pager coverage input")
    }

    func testFloatingTopScopeAndSettingsReplaceTheBoardHeaderAndToolbar() throws {
        let board = try source("FleetViews")
        // #457: the herd branch gate moved into `showsHerdSurface` (shared
        // by the body branch and the filter sheet's presentation context).
        let herdStart = try XCTUnwrap(board.range(of: "if showsHerdSurface {"))
        let herdCall = try XCTUnwrap(board.range(of: "HerdView(horses:",
                                                 range: herdStart.upperBound..<board.endIndex))
        let branch = String(board[herdStart.lowerBound..<herdCall.lowerBound])
        XCTAssertFalse(branch.contains("filterHeaderControl"),
                       "the opaque board header strip must not render above the Herd surface")
        XCTAssertFalse(branch.contains("PinnedHeader"),
                       "no board pinned chrome belongs to the Herd branch")
        let route = compact(String(board[herdCall.lowerBound...]))
        for needle in ["scopeLabel:filterButtonLabel",
                       "scopeSummary:filterSummaryText",
                       "showFilters:$showFilters",
                       "showSettings:$showSettings"] {
            XCTAssertTrue(route.contains(needle),
                          "the floating scope/Settings controls must bind the board's reconciled scope (\(needle))")
        }
        XCTAssertTrue(compact(board).contains("?.hidden:.visible,for:.navigationBar"),
                      "Herd must hide the navigation toolbar (no duplicated gear/title chrome)")
        let herd = try compact(source("HerdView"))
        XCTAssertTrue(herd.contains("Button{showFilters=true}label:"),
                      "the floating scope control opens the real filter sheet")
        XCTAssertTrue(herd.contains("Button{showSettings=true}label:"),
                      "the floating Settings control opens the real Settings sheet")
        XCTAssertTrue(herd.contains("HerdGearGlyph(color:ranchTokens.inkColor)"),
                      "the floating Settings control draws its gear natively, in the ranch chrome ink (#457)")
        XCTAssertFalse(herd.contains("Image("),
                       "the Herd renderer stays procedural (fail-closed native-art gate)")
        XCTAssertTrue(herd.contains("privatevartopChrome:someView"),
                      "the floating top chrome is a single owned surface")
    }

    func testFloatingBottomPaddockNavigationKeeps44ptTargetsAndSafeAreaInset() throws {
        let herd = try compact(source("HerdView"))
        let navStart = try XCTUnwrap(herd.range(of: "privatevarnavigation:someView"))
        let navEnd = try XCTUnwrap(herd.range(of: "funcmovePage(", range: navStart.upperBound..<herd.endIndex))
        let nav = String(herd[navStart.lowerBound..<navEnd.lowerBound])
        XCTAssertTrue(nav.contains("Button(\"Previous\"){movePage(-1)}.disabled(index==0).frame(minWidth:44,minHeight:44)"),
                      "Previous keeps its >= 44 pt target")
        XCTAssertTrue(nav.contains("Button(\"Next\"){movePage(1)}.disabled(index+1>=paddocks.count).frame(minWidth:44,minHeight:44)"),
                      "Next keeps its >= 44 pt target")
        XCTAssertTrue(nav.contains("Text(\"\\(index+1)/\\(paddocks.count)\")"),
                      "the paddock position readout is preserved")
        XCTAssertTrue(nav.contains(".background(.regularMaterial,in:RoundedRectangle(cornerRadius:15))"),
                      "the bottom navigation floats as a rounded material pill, not an opaque full-width bar")
        XCTAssertTrue(nav.contains(".padding(.horizontal,12)") && nav.contains(".padding(.bottom,6)"),
                      "the floating navigation keeps a safe-area margin on both axes")
    }

    func testHerdPreservesOutageRecoveryCountsLongNamesAndEmptyScope() throws {
        let herd = try compact(source("HerdView"))
        XCTAssertTrue(herd.contains("ForEach([AgentState.blocked,.working,.idle,.done,.unknown],id:\\.self)"),
                      "all five truthful scoped counts stay rendered")
        XCTAssertTrue(herd.contains("Button(\"OpenBoard\",action:openBoard).frame(minWidth:44,minHeight:44)"),
                      "the outage keeps the Open Board recovery action")
        XCTAssertTrue(herd.contains("Button(\"Retry\"){Task{awaitretry()}}.frame(minWidth:44,minHeight:44)"),
                      "the outage keeps the Retry action")
        XCTAssertTrue(herd.contains("ContentUnavailableView(\"Noagentsinthisscope\""),
                      "an empty scope keeps its explicit empty state")
        XCTAssertTrue(herd.contains("Text(scopeSummary).font(.caption2).foregroundStyle(ranchTokens.mutedColor).lineLimit(1).truncationMode(.tail)"),
                      "long repository scope summaries stay single-line and truncate (ranch ink, #457)")
        XCTAssertTrue(herd.contains("Text(paddock.title).font(.headline).lineLimit(1)"),
                      "long paddock titles stay single-line")
        XCTAssertTrue(herd.contains("Text(lighting.explanation).font(.caption2)"),
                      "the truthful environment explanation stays visible")
    }

    func testRanchBackgroundCannotStealTapsAndIsCoveredWithoutStretch() throws {
        let ranch = try compact(source("RanchEnvironment"))
        // Scoped per Canvas host: the whole-file search alone would false-green
        // on the sibling rail-art occurrence (the #316 decoy lesson).
        let environmentStart = try XCTUnwrap(ranch.range(of: "structRanchEnvironment:View{"))
        let environmentEnd = try XCTUnwrap(ranch.range(of: "structRanchPainter{",
                                                       range: environmentStart.upperBound..<ranch.endIndex))
        let environment = String(ranch[environmentStart.lowerBound..<environmentEnd.lowerBound])
        XCTAssertTrue(environment.contains(".allowsHitTesting(false)"),
                      "the ranch background must never intercept taps")
        XCTAssertTrue(environment.contains(".clipped()"),
                      "the ranch background must clip to its cover frame")
        let railStart = try XCTUnwrap(ranch.range(of: "structRanchFrontRail:View{"))
        let rail = String(ranch[railStart.lowerBound...])
        XCTAssertTrue(rail.contains(".allowsHitTesting(false)"),
                      "the rail art must never intercept taps")
        let herd = try compact(source("HerdView"))
        XCTAssertTrue(herd.contains("HerdRanchCover{RanchEnvironment("),
                      "the ranch must ride the uniform cover container")
        XCTAssertTrue(herd.contains("HerdRanchViewport.scale(for:geometry.size)"),
                      "the cover derives ONE uniform scale for both axes")
        XCTAssertTrue(herd.contains(".frame(width:HerdRanchViewport.world.width*scale,height:HerdRanchViewport.world.height*scale)"),
                      "the ranch content receives the world-aspect frame (never a per-axis stretch)")
        XCTAssertFalse(herd.contains("scaleBy(x:"),
                       "the shell must never apply a per-axis canvas stretch")
    }

    func testFullScreenEvidenceHooksAreDebugOnly() throws {
        let herdSource = try source("HerdView")
        let debug = debugActiveLines(herdSource)
        for needle in ["-corral456FullScreenEvidence", "runFullScreenEvidence", "evidenceFullScreenRan"] {
            let lines = herdSource.split(separator: "\n", omittingEmptySubsequences: false)
                .enumerated()
                .filter { $0.element.contains(needle) }
                .map { $0.offset + 1 }
            XCTAssertFalse(lines.isEmpty, "\(needle) must exist for the #456 evidence driver")
            for line in lines {
                XCTAssertTrue(debug.contains(line),
                              "\(needle) must stay inside #if DEBUG (Release-inert)")
            }
        }
    }

    /// Runtime geometry: one uniform scale for both axes (never a per-axis
    /// stretch), cover semantics, centered crop.
    func testHerdRanchViewportScaleIsUniformCover() {
        XCTAssertEqual(HerdRanchViewport.world, CGSize(width: 390, height: 640),
                       "the cover preserves the approved native world size")
        for size in [CGSize(width: 393, height: 852),   // iPhone 16 (this lane's sim)
                     CGSize(width: 375, height: 667),   // iPhone SE 3rd gen
                     CGSize(width: 430, height: 932),   // iPhone 16 Pro Max
                     CGSize(width: 390, height: 844)] { // #455 reference phone
            let scale = HerdRanchViewport.scale(for: size)
            XCTAssertEqual(scale, max(size.width / 390, size.height / 640), accuracy: 0.0001,
                           "the cover scale is the single cover factor at \(size)")
            XCTAssertGreaterThanOrEqual(390 * scale, size.width - 0.001,
                                        "scaled world must cover the width at \(size)")
            XCTAssertGreaterThanOrEqual(640 * scale, size.height - 0.001,
                                        "scaled world must cover the height at \(size)")
        }
        XCTAssertEqual(HerdRanchViewport.scale(for: .zero), 1,
                       "a zero container must not produce a non-finite scale")
    }

    /// Runtime composition witness: the cover hands the ranch content a
    /// frame with the world's exact aspect ratio at ONE uniform scale — the
    /// mechanism that keeps the canvas from stretching. A per-axis frame
    /// (the base defect) makes the probe report the container's own aspect.
    @MainActor
    func testHerdRanchCoverGivesTheRanchOneUniformWorldAspect() async throws {
        final class ProbeBox { var size: CGSize? }
        for container in [CGSize(width: 393, height: 852),
                          CGSize(width: 375, height: 667),
                          CGSize(width: 430, height: 932),
                          CGSize(width: 390, height: 844)] {
            let box = ProbeBox()
            let probe = GeometryReader { proxy in
                Color.clear
                    .onAppear { box.size = proxy.size }
                    .onChange(of: proxy.size) { _, size in box.size = size }
            }
            let controller = UIHostingController(rootView: AnyView(
                HerdRanchCover { probe }
                    .frame(width: container.width, height: container.height)))
            let window = UIWindow(frame: CGRect(origin: .zero, size: container))
            window.rootViewController = controller
            window.makeKeyAndVisible()
            try await Task.sleep(for: .milliseconds(250))
            let reported = try XCTUnwrap(box.size,
                                         "the ranch content must receive a frame at \(container)")
            let scale = HerdRanchViewport.scale(for: container)
            XCTAssertEqual(reported.width, 390 * scale, accuracy: 0.5)
            XCTAssertEqual(reported.height, 640 * scale, accuracy: 0.5)
            XCTAssertEqual(reported.width / reported.height, 390.0 / 640.0, accuracy: 0.0005,
                           "the ranch frame must keep the world aspect (no per-axis stretch) at \(container)")
            XCTAssertGreaterThanOrEqual(reported.width, container.width - 0.5,
                                        "the scaled ranch must cover the container width")
            XCTAssertGreaterThanOrEqual(reported.height, container.height - 0.5,
                                        "the scaled ranch must cover the container height")
            window.isHidden = true
        }
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

// MARK: - #456-r1: runtime accessibility layout over the REAL HerdView

/// The #456 floating top chrome must survive Dynamic Type. At AX-XXXL the
/// pre-fix layout squeezed the scope pill: the label rendered OUTSIDE its own
/// material pill (into the status-bar/Dynamic Island band) and the counts
/// card covered the `All repositories` summary. These tests render the real
/// `HerdView` in a hosted window at the supported phone sizes and Dynamic
/// Type sizes and measure the pixels of the actual SwiftUI layout — a source
/// pin cannot show where a glyph landed. Re-introducing the squeeze (a
/// fixed-height chrome / dropped layout priority) turns the accessibility
/// cases RED while the default-size case stays GREEN as the positive control.
/// #457: the floating chrome renders the approved cream ranch Day glass with
/// dark ranch ink — the pixel detectors are anchored to that treatment (a
/// bright, low-saturation chrome band + dark ink glyphs).
@MainActor
final class FullScreenHerdShellAccessibilityLayoutTests: XCTestCase {

    /// 12 synthetic agents over 4 repositories — the #456 fixture shape
    /// (2 blocked at the rail, mixed states). Fictional data only.
    private func fleet() -> [HerdHorse] {
        let names = ["birch-clearing", "oak-before-dark", "spruce-hollow", "willow-bend",
                     "cedar-ridge", "aspen-grove", "juniper-south", "maple-stand",
                     "elder-flats", "hazel-hollow", "hawthorn-fork", "sumac-row"]
        let states: [AgentState] = [.blocked, .blocked, .done, .idle, .working, .unknown,
                                    .idle, .working, .done, .working, .idle, .working]
        let repos = ["atlas-vector", "cedar-tools", "maple-client", "willow-core"]
        return (0..<names.count).map { index in
            let agent = Agent(agentId: "herdr:r1-fixture-\(index)", state: states[index],
                              seq: UInt64(index + 1), ts: 1_800_000_000_000,
                              capabilities: ["read_tail"],
                              workspace: Workspace(repo: repos[(index / 3) % repos.count]),
                              attachment: Attachment(kind: "herdr", reference: "fixture:r1:\(index)"),
                              displayName: names[index])
            return HerdHorse(agent: agent, hostProfileID: nil, hostName: nil, disconnected: false)
        }
    }

    // MARK: pixel canvas (1 px == 1 pt)

    private struct Canvas {
        let width: Int
        let height: Int
        let pixels: [UInt8]

        init?(_ image: UIImage) {
            guard let cg = image.cgImage else { return nil }
            width = cg.width
            height = cg.height
            var data = [UInt8](repeating: 0, count: cg.width * cg.height * 4)
            let drawn = data.withUnsafeMutableBytes { buffer -> Bool in
                guard let context = CGContext(data: buffer.baseAddress, width: cg.width, height: cg.height,
                                              bitsPerComponent: 8, bytesPerRow: cg.width * 4,
                                              space: CGColorSpaceCreateDeviceRGB(),
                                              bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue)
                else { return false }
                context.draw(cg, in: CGRect(x: 0, y: 0, width: cg.width, height: cg.height))
                return true
            }
            guard drawn else { return nil }
            pixels = data
        }

        func rgb(_ x: Int, _ y: Int) -> (red: Double, green: Double, blue: Double) {
            let offset = (y * width + x) * 4
            return (Double(pixels[offset]), Double(pixels[offset + 1]), Double(pixels[offset + 2]))
        }

        func luminance(_ x: Int, _ y: Int) -> Double {
            let colour = rgb(x, y)
            return 0.299 * colour.red + 0.587 * colour.green + 0.114 * colour.blue
        }

        /// Chrome glyphs are the near-white label colour (the Day ranch's sky
        /// and hills are saturated and stay below the luminance gate).
        func isLabelGlyph(_ x: Int, _ y: Int) -> Bool {
            guard x >= 0, x < width, y >= 0, y < height else { return false }
            let colour = rgb(x, y)
            let saturation = max(colour.red, colour.green, colour.blue)
                - min(colour.red, colour.green, colour.blue)
            return luminance(x, y) > 200 && saturation < 45
        }

        func labelGlyphs(rows: Range<Int>, columns: Range<Int>) -> Int {
            var count = 0
            for y in rows where y >= 0 && y < height {
                for x in columns where x >= 0 && x < width && isLabelGlyph(x, y) { count += 1 }
            }
            return count
        }

        /// #457: the Day ranch chrome is the cream ranch glass. Pixel
        /// classes on this surface: the cream interior is near-neutral
        /// (green ≈ red); the anti-aliased edges blend toward the cool sky
        /// (slightly blue-dominant); the bright khaki hills and pale
        /// horizon sky are GREEN-dominant and must never read as chrome
        /// (the measured SE-AX frame shows the hills directly between the
        /// pill and the counts card). (The #456 detectors keyed on the old
        /// flavor material; the approved Day chrome inverts that luminance
        /// model.)
        func isChromeSurface(_ x: Int, _ y: Int) -> Bool {
            guard x >= 0, x < width, y >= 0, y < height else { return false }
            let colour = rgb(x, y)
            let saturation = max(colour.red, colour.green, colour.blue)
                - min(colour.red, colour.green, colour.blue)
            guard luminance(x, y) > 150, saturation < 80 else { return false }
            let redGreen = colour.green - colour.red
            if redGreen <= 12 { return true }
            let blueGreen = colour.blue - colour.green
            return blueGreen > 0 && blueGreen <= 8 && colour.red >= colour.blue - 45
        }

        /// First cream-chrome band (the floating pills/cards) at a column.
        func chromeBand(x: Int, from: Int, to: Int) -> (top: Int, bottom: Int)? {
            guard let top = (max(0, from)..<min(height, to)).first(where: { isChromeSurface(x, $0) }),
                  let bottom = (top + 4..<height).first(where: { !isChromeSurface(x, $0) })
            else { return nil }
            return (top, bottom)
        }

        /// Ranch-ink glyph pixels (#457 Day chrome labels — dark ink on the
        /// cream surface; the sky/field stay above the ink threshold).
        func isInkGlyph(_ x: Int, _ y: Int) -> Bool {
            guard x >= 0, x < width, y >= 0, y < height else { return false }
            let colour = rgb(x, y)
            let saturation = max(colour.red, colour.green, colour.blue)
                - min(colour.red, colour.green, colour.blue)
            return luminance(x, y) < 115 && saturation < 90
        }

        func inkGlyphs(rows: Range<Int>, columns: Range<Int>) -> Int {
            var count = 0
            for y in rows where y >= 0 && y < height {
                for x in columns where x >= 0 && x < width && isInkGlyph(x, y) { count += 1 }
            }
            return count
        }

        /// The bottom-most contiguous glyph band inside `rows` — the floating
        /// bottom navigation sits after the scrollable column, so it is the
        /// last text band above the safe-area edge.
        func bottomTextBand(rows: Range<Int>, columns: Range<Int>) -> (top: Int, bottom: Int)? {
            var bottom: Int?
            var y = min(rows.upperBound, height) - 1
            while y >= rows.lowerBound {
                if labelGlyphs(rows: y..<(y + 1), columns: columns) > 0 { bottom = y; break }
                y -= 1
            }
            guard let bottom else { return nil }
            var top = bottom
            var gap = 0
            y = bottom - 1
            while y >= rows.lowerBound {
                if labelGlyphs(rows: y..<(y + 1), columns: columns) > 0 {
                    top = y
                    gap = 0
                } else {
                    gap += 1
                    if gap > 3 { break }
                }
                y -= 1
            }
            return (top, bottom)
        }
    }

    private struct Snapshot {
        let canvas: Canvas
        let safeTop: CGFloat
        let safeBottom: CGFloat
    }

    /// Renders the REAL HerdView in a hosted window at `size` with the
    /// deterministic Day lighting and returns the measured screen pixels.
    /// The window must belong to the app's window scene: a scene-less window
    /// reports zero safe-area insets and never rasterizes its SwiftUI layers.
    private func render(_ size: CGSize, dynamicType: DynamicTypeSize) async throws -> Snapshot {
        UserDefaults.standard.set(HerdEnvironmentChoice.day.rawValue, forKey: "herdEnvironment")
        let scene = HerdView(horses: fleet(), obscured: false, select: { _ in },
                             openBoard: {}, retry: {})
            .environmentObject(ThemeStore())
            .environment(\.dynamicTypeSize, dynamicType)
        let controller = UIHostingController(rootView: AnyView(scene))
        let windowScene = UIApplication.shared.connectedScenes
            .compactMap { $0 as? UIWindowScene }.first
        let window = windowScene.map { UIWindow(windowScene: $0) } ?? UIWindow()
        window.frame = CGRect(origin: .zero, size: size)
        window.rootViewController = controller
        window.makeKeyAndVisible()
        defer { window.isHidden = true }
        try await Task.sleep(for: .milliseconds(700))
        let format = UIGraphicsImageRendererFormat.default()
        format.scale = 1
        let image = UIGraphicsImageRenderer(bounds: window.bounds, format: format).image { _ in
            window.drawHierarchy(in: window.bounds, afterScreenUpdates: true)
        }
        let canvas = try XCTUnwrap(Canvas(image), "the hosted HerdView must render a bitmap")
        return Snapshot(canvas: canvas, safeTop: window.safeAreaInsets.top,
                        safeBottom: window.safeAreaInsets.bottom)
    }

    /// The floating scope-chrome contract at any Dynamic Type size: the pill
    /// sits inside the top safe area, the scope label renders INSIDE the
    /// pill's own ranch surface (never above it in the status-bar band) and
    /// the counts card below it never covers the scope summary.
    private func assertScopeChromeHoldsItsLabel(_ snapshot: Snapshot, label: String) throws {
        let canvas = snapshot.canvas
        let pill = try XCTUnwrap(canvas.chromeBand(x: 44, from: 0, to: 400),
                                 "\(label): the floating scope pill must render")
        XCTAssertGreaterThanOrEqual(Double(pill.top), Double(snapshot.safeTop) - 2,
            "\(label): the floating chrome starts inside the top safe area "
            + "(pill top \(pill.top) pt, safe top \(snapshot.safeTop) pt)")
        XCTAssertGreaterThanOrEqual(pill.bottom - pill.top, 44,
            "\(label): the scope control keeps a >= 44 pt target "
            + "(measured chrome band \(pill.bottom - pill.top) pt)")
        let above = canvas.inkGlyphs(rows: 0..<pill.top, columns: 48..<200)
        XCTAssertEqual(above, 0,
            "\(label): \(above) px of scope text render ABOVE the pill "
            + "(status-bar / Dynamic Island band)")
        let inside = canvas.inkGlyphs(rows: pill.top..<pill.bottom, columns: 48..<200)
        XCTAssertGreaterThanOrEqual(inside, 40,
            "\(label): the scope label must render inside the pill's own material (found \(inside) px)")
        let card = try XCTUnwrap(canvas.chromeBand(x: 44, from: pill.bottom + 2, to: canvas.height),
                                 "\(label): the counts card must render below the floating scope pill")
        let gap = canvas.inkGlyphs(rows: (pill.bottom + 1)..<card.top, columns: 48..<200)
        XCTAssertEqual(gap, 0,
            "\(label): \(gap) px of scope text render between the pill and the counts card — "
            + "the counts card must never cover the scope summary")
    }

    /// #456 AC2 on the tall phones: the scope label stays inside its pill and
    /// the counts card clear of it at accessibility sizes.
    func testFloatingScopeChromeHoldsItsLabelAtAccessibilitySizes() async throws {
        for (name, dynamicType) in [("AX1", DynamicTypeSize.accessibility1),
                                    ("AX3", DynamicTypeSize.accessibility3),
                                    ("AX5", DynamicTypeSize.accessibility5)] {
            let snapshot = try await render(CGSize(width: 393, height: 852), dynamicType: dynamicType)
            try assertScopeChromeHoldsItsLabel(snapshot, label: "iPhone 16 393x852 \(name)")
        }
        let large = try await render(CGSize(width: 430, height: 932), dynamicType: .accessibility5)
        try assertScopeChromeHoldsItsLabel(large, label: "iPhone Pro Max 430x932 AX5")
    }

    /// Small phone: the default size is the positive control (the approved
    /// composition) and AX-XXXL must still hold the chrome inside its pill.
    func testSmallPhoneKeepsTheFloatingChromeContained() async throws {
        let control = try await render(CGSize(width: 375, height: 667), dynamicType: .large)
        try assertScopeChromeHoldsItsLabel(control, label: "iPhone SE 375x667 default (control)")
        let accessible = try await render(CGSize(width: 375, height: 667), dynamicType: .accessibility5)
        try assertScopeChromeHoldsItsLabel(accessible, label: "iPhone SE 375x667 AX-XXXL")
    }

    /// Runtime reachability: the floating controls keep >= 44 pt usable
    /// targets at AX-XXXL and the floating bottom navigation's target band
    /// stays fully inside the safe area.
    func testFloatingControlsKeepReachableTargetsAtAccessibilitySizes() async throws {
        for size in [CGSize(width: 393, height: 852), CGSize(width: 375, height: 667)] {
            let snapshot = try await render(size, dynamicType: .accessibility5)
            let canvas = snapshot.canvas
            let label = "AX-XXXL \(Int(size.width))x\(Int(size.height))"
            let pill = try XCTUnwrap(canvas.chromeBand(x: 44, from: 0, to: 400),
                                     "\(label): the floating scope pill must render")
            XCTAssertGreaterThanOrEqual(pill.bottom - pill.top, 44,
                "\(label): the floating scope control keeps a >= 44 pt target "
                + "(measured chrome band \(pill.bottom - pill.top) pt)")
            let nav = try XCTUnwrap(canvas.bottomTextBand(rows: max(0, canvas.height - 240)..<canvas.height,
                                                          columns: 24..<(canvas.width - 24)),
                                    "\(label): the floating bottom navigation labels must render")
            XCTAssertGreaterThanOrEqual(nav.bottom - nav.top, 8,
                "\(label): the bottom navigation keeps its label glyph band")
            XCTAssertGreaterThanOrEqual(Double(nav.top) - 22, 0,
                "\(label): the 44 pt Previous/position/Next target band stays on screen")
            XCTAssertLessThanOrEqual(Double(nav.bottom) + 22,
                                     Double(size.height) - Double(snapshot.safeBottom) + 2,
                "\(label): the floating bottom navigation stays inside the safe area")
        }
    }
}

// MARK: - #457 ranch-context controls + shared filter sheet

/// #457: the shared filter sheet's explicit Board/Herd presentation
/// context, the sealed ranch Day/Night token palette, the dual-fallback
/// chrome recipe and the environment/theme independence.
///
/// The runtime cases render the REAL `RanchChromeSurface` recipe in a
/// hosted window: the sealed tokens must composite into the ranch
/// Day/Night surfaces (bright warm cream / near-black blue-green —
/// independent of ambient flavor), and the Reduce Transparency /
/// Increase Contrast branch must paint the OPAQUE ranch solid (a source
/// pin alone cannot show a composite, and the Day/Night frames prove the
/// environment axis actually reaches the rendered pixels). The wiring
/// cases slice the bundled sources: exactly ONE shared sheet with the
/// explicit context, herd tokens ONLY in the herd chrome, and the board
/// path byte-unchanged. The interaction case proves the lighting the
/// chrome resolves is REPORTED to the sheet-context owner (FleetView).
@MainActor
final class ContextualFilterSheetTests: XCTestCase {

    // MARK: bundled-source helpers

    private func source(_ name: String) throws -> String {
        let url = try XCTUnwrap(Bundle(for: Self.self).url(forResource: name + ".swift",
                                                            withExtension: "txt"))
        return try String(contentsOf: url, encoding: .utf8)
    }

    /// Whitespace-stripped form — pins survive re-indentation.
    private func compact(_ text: String) -> String {
        text.filter { !$0.isWhitespace }
    }

    private func slice(_ source: String, from: String, to: String) throws -> String {
        let start = try XCTUnwrap(source.range(of: from),
                                  "start marker missing: \(from)")
        let end = try XCTUnwrap(source.range(of: to, range: start.upperBound..<source.endIndex),
                                "end marker missing after \(from): \(to)")
        return String(source[start.lowerBound..<end.lowerBound])
    }

    /// 1-based line numbers of every line whose `#if DEBUG` nesting makes
    /// it DEBUG-active (same flat depth scan the #456 wiring tests use).
    private func debugActiveLines(_ source: String) -> Set<Int> {
        var active: Set<Int> = []
        var depth = 0
        for (index, line) in source.split(separator: "\n",
                                          omittingEmptySubsequences: false).enumerated() {
            let trimmed = line.trimmingCharacters(in: .whitespaces)
            if trimmed.hasPrefix("#if DEBUG") {
                depth += 1
            } else if trimmed.hasPrefix("#endif") {
                depth = max(0, depth - 1)
            }
            if depth > 0 { active.insert(index + 1) }
        }
        return active
    }

    // MARK: pixel canvas (1 px == 1 pt)

    private struct Canvas {
        let width: Int
        let height: Int
        let pixels: [UInt8]

        init?(_ image: UIImage) {
            guard let cg = image.cgImage else { return nil }
            width = cg.width
            height = cg.height
            var data = [UInt8](repeating: 0, count: cg.width * cg.height * 4)
            let drawn = data.withUnsafeMutableBytes { buffer -> Bool in
                guard let context = CGContext(data: buffer.baseAddress, width: cg.width,
                                              height: cg.height, bitsPerComponent: 8,
                                              bytesPerRow: cg.width * 4,
                                              space: CGColorSpaceCreateDeviceRGB(),
                                              bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue)
                else { return false }
                context.draw(cg, in: CGRect(x: 0, y: 0, width: cg.width, height: cg.height))
                return true
            }
            guard drawn else { return nil }
            pixels = data
        }

        func rgb(_ x: Int, _ y: Int) -> (red: Double, green: Double, blue: Double) {
            let offset = (y * width + x) * 4
            return (Double(pixels[offset]), Double(pixels[offset + 1]), Double(pixels[offset + 2]))
        }

        func luminance(_ x: Int, _ y: Int) -> Double {
            let colour = rgb(x, y)
            return 0.299 * colour.red + 0.587 * colour.green + 0.114 * colour.blue
        }
    }

    /// Renders `RanchChromeSurface` over `background` in a hosted window at
    /// 1 px == 1 pt and returns the rasterized canvas. The window must
    /// belong to the app's window scene (a scene-less window never
    /// rasterizes its SwiftUI layers). `fallback` drives the explicit
    /// opaque override — the system Reduce Transparency / Increase
    /// Contrast environment keys are read-only in hosted windows.
    private func renderChrome(tokens: RanchControlTokens, over background: Color,
                              fallback: Bool = false) async throws -> Canvas {
        let scene = ZStack {
            background
            RanchChromeSurface(tokens: tokens, cornerRadius: 15,
                               forcesOpaque: fallback ? true : nil)
                .frame(width: 220, height: 80)
        }
        let controller = UIHostingController(rootView: AnyView(scene))
        let windowScene = UIApplication.shared.connectedScenes
            .compactMap { $0 as? UIWindowScene }.first
        let window = windowScene.map { UIWindow(windowScene: $0) } ?? UIWindow()
        window.frame = CGRect(origin: .zero, size: CGSize(width: 320, height: 120))
        window.rootViewController = controller
        window.makeKeyAndVisible()
        defer { window.isHidden = true }
        try await Task.sleep(for: .milliseconds(700))
        let format = UIGraphicsImageRendererFormat.default()
        format.scale = 1
        let image = UIGraphicsImageRenderer(bounds: window.bounds, format: format).image { _ in
            window.drawHierarchy(in: window.bounds, afterScreenUpdates: true)
        }
        return try XCTUnwrap(Canvas(image), "the hosted chrome surface must render a bitmap")
    }

    // MARK: sealed palette + WCAG floor

    func testSealedRanchDayNightPaletteMatchesTheApprovedPrototype() {
        // The #455 V1/A prototype's sealed custom properties (variant-a.html
        // SHA-256 ca0a1a09…bf61). Drift in any value fails here.
        XCTAssertEqual(RanchControlTokens.day,
                       RanchControlTokens(ink: "#23362f", muted: "#3e5147",
                                          accent: "#304e41", line: "#8c9a89",
                                          solid: "#f6f5e1"))
        XCTAssertEqual(RanchControlTokens.night,
                       RanchControlTokens(ink: "#edf1e8", muted: "#c2cec9",
                                          accent: "#d4e4db", line: "#647b80",
                                          solid: "#1d2b33"))
        XCTAssertEqual(RanchControlTokens.resolve(night: false), .day)
        XCTAssertEqual(RanchControlTokens.resolve(night: true), .night)
        // The sheet context is explicit and equatable — .board can never
        // masquerade as a herd context.
        XCTAssertNotEqual(FilterSheetContext.herd(night: false), .board)
        XCTAssertNotEqual(FilterSheetContext.herd(night: true), FilterSheetContext.herd(night: false))
        XCTAssertEqual(FilterSheetContext.herd(night: true), FilterSheetContext.herd(night: true))
    }

    func testRanchChromeTextClearsAAOverBrightSkyAndDarkField() {
        // The RanchPainter field colors: day sky/field and night sky/ground.
        let painters = ["#388fc4", "#8e9c58", "#121a32", "#273f3e"]
        for (label, tokens) in [("day", RanchControlTokens.day),
                                ("night", RanchControlTokens.night)] {
            for (tier, hex) in [("ink", tokens.ink), ("muted", tokens.muted)] {
                let worst = SheetBackdrop.worstContrast(ink: hex, tint: tokens.solid,
                                                        over: painters)
                XCTAssertGreaterThanOrEqual(worst, SheetBackdrop.minimumContrast,
                    "\(label) \(tier) must clear the 4.5:1 floor over every ranch "
                    + "field candidate (worst \(worst))")
            }
        }
    }

    // MARK: runtime composite (the real recipe)

    func testChromeRecipeRendersRanchDayAndNightSurfacesOverAnyField() async throws {
        // Day over a saturated blue sky: the locked ranch tint dominates —
        // a warm, bright surface. Night over a bright field: near-black
        // with the blue-green cast. A theme/flavor material (the base
        // behavior) can never produce BOTH at once.
        let day = try await renderChrome(tokens: .day, over: Color(red: 0, green: 0, blue: 1))
        let dayCentre = day.rgb(160, 60)
        XCTAssertGreaterThan(dayCentre.red, 185, "day chrome must render bright")
        XCTAssertGreaterThan(dayCentre.green, 185, "day chrome must render bright")
        XCTAssertGreaterThan(dayCentre.blue, 150, "day chrome must render bright")
        XCTAssertGreaterThan(dayCentre.red, dayCentre.blue - 25,
                             "day chrome keeps the warm cream cast")

        let night = try await renderChrome(tokens: .night, over: Color(red: 1, green: 0.6, blue: 0))
        let nightCentre = night.rgb(160, 60)
        XCTAssertLessThan(nightCentre.red, 95, "night chrome must render dark over a bright field")
        XCTAssertLessThan(nightCentre.green, 105, "night chrome must render dark over a bright field")
        XCTAssertLessThan(night.luminance(160, 60), 120,
                          "night chrome luminance must stay in the dark field range")

        XCTAssertGreaterThan(day.luminance(160, 60) - night.luminance(160, 60), 60,
                             "Day and Night chrome must be visibly different surfaces")
    }

    func testReduceTransparencyAndHighContrastPaintTheOpaqueRanchSolid() async throws {
        // Exact-solid assertions: the opaque branch adds NOTHING of the
        // background — a blur branch leaks the underlying color and fails.
        let dayFallback = try await renderChrome(tokens: .day, over: Color(red: 0, green: 0, blue: 1),
                                                 fallback: true)
        let dayCentre = dayFallback.rgb(160, 60)
        XCTAssertEqual(dayCentre.red, 246, accuracy: 3, "day Reduce Transparency = opaque #f6f5e1")
        XCTAssertEqual(dayCentre.green, 245, accuracy: 3, "day Reduce Transparency = opaque #f6f5e1")
        XCTAssertEqual(dayCentre.blue, 225, accuracy: 3, "day Reduce Transparency = opaque #f6f5e1")

        let nightFallback = try await renderChrome(tokens: .night, over: Color(red: 1, green: 0.6, blue: 0),
                                                   fallback: true)
        let nightCentre = nightFallback.rgb(160, 60)
        XCTAssertEqual(nightCentre.red, 29, accuracy: 3, "night high contrast = opaque #1d2b33")
        XCTAssertEqual(nightCentre.green, 43, accuracy: 3, "night high contrast = opaque #1d2b33")
        XCTAssertEqual(nightCentre.blue, 51, accuracy: 3, "night high contrast = opaque #1d2b33")
    }

    // MARK: explicit context wiring (ONE sheet)

    func testTheOneSharedSheetTakesTheExplicitPresentationContext() throws {
        let board = try source("FleetViews")
        XCTAssertEqual(board.components(separatedBy: ".sheet(isPresented: $showFilters)").count - 1, 1,
                       "exactly ONE sheet presentation may serve the filter surface")
        XCTAssertTrue(board.contains("FilterScopeSheet(model: model, context: filterSheetContext)"),
                      "the shared sheet must receive the explicit presentation context")
        let context = try slice(board,
                                from: "private var filterSheetContext: FilterSheetContext {",
                                to: "/// #456/#458: the Herd surface renders")
        XCTAssertTrue(compact(context).contains("showsHerdSurface?.herd(night:herdChromeNight):.board"),
                      "the context must resolve from the surface the control lives on and the "
                      + "reported ranch lighting — never from ambient styles")
        let gate = try slice(board,
                             from: "private var showsHerdSurface: Bool {",
                             to: "private var herdDisconnected: Bool {")
        XCTAssertTrue(compact(gate).contains("model.fleetPresentation==.herd&&model.mode!=.needsSetup"),
                      "the herd gate stays the saved-presentation + setup gate")

        let sheet = try slice(board,
                              from: "struct FilterScopeSheet: View {",
                              to: "/// #457: the sheet surface follows the explicit presentation context")
        XCTAssertTrue(sheet.contains("let context: FilterSheetContext"),
                      "the sheet must OWN the context (one shared implementation)")
        XCTAssertTrue(sheet.contains("if case .herd(let night) = context { return .resolve(night: night) }"),
                      "only .herd resolves ranch tokens")
        for needle in ["tokens?.inkColor ?? theme.text",
                       "tokens?.mutedColor ?? theme.subtext1",
                       "tokens?.accentColor ?? theme.accent",
                       "tokens?.lineColor.opacity(0.35) ?? theme.surface1.opacity(0.35)",
                       "tokens?.lineColor.opacity(0.4) ?? theme.surface1.opacity(0.4)",
                       "tokens?.solidColor.opacity(0.92) ?? theme.base.opacity(0.92)",
                       "tokens?.solidColor.opacity(0.55) ?? theme.base.opacity(0.55)"] {
            XCTAssertTrue(sheet.contains(needle),
                          "every sheet color must fall back to its existing Catppuccin token: \(needle)")
        }
        XCTAssertTrue(sheet.contains(".modifier(FilterSheetBackdrop(tokens: tokens, boardTint: theme.base))"),
                      "the sheet surface must follow the SAME explicit context")
        XCTAssertTrue(board.contains("TranslucentSheetBackdrop(tint: boardTint)"),
                      "the board-launched sheet keeps the #385/#416 translucent backdrop")
        XCTAssertTrue(board.contains("RanchChromeSurface(tokens: tokens, cornerRadius: 0)"),
                      "the herd-launched sheet takes the ranch chrome surface")
    }

    func testBoardLaunchedFiltersAndOtherSheetsKeepTheirCatppuccinTreatment() throws {
        let board = try source("FleetViews")
        let header = try slice(board,
                               from: "private func filterHeaderControl(",
                               to: "/// Connection indicator line")
        XCTAssertFalse(header.contains("RanchControl"),
                       "the board Filters control must not consume ranch tokens")
        XCTAssertFalse(header.contains("ranchChromeSurface"),
                       "the board Filters control keeps its board chrome")
        for (label, from, to) in [
            ("SettingsView", "\nstruct SettingsView: View {", "\n// MARK: - How to connect"),
            ("RecentOutputSheet", "struct RecentOutputSheet: View {", "\n// MARK: - Recents block renderer")] {
            let slice = try slice(board, from: from, to: to)
            XCTAssertFalse(slice.contains("RanchControl"),
                           "\(label) must not inherit herd styling (#457: no whole-app recolor)")
            XCTAssertFalse(slice.contains("ranchChromeSurface"),
                           "\(label) keeps its own sheet treatment")
        }
        XCTAssertEqual(board.components(separatedBy: "ranchChromeSurface(").count - 1, 1,
                       "the ranch chrome surface extension is the ONLY definition (no herd "
                       + "styling call sites inside FleetViews)")
    }

    func testHerdTriggerCountsAndGearTakeTheRanchChromeNotTheAppFlavorText() throws {
        let herd = try compact(source("HerdView"))
        XCTAssertTrue(herd.contains("ranchTokens:RanchControlTokens{.resolve(night:lighting.night)}"),
                      "the chrome palette resolves from the SAME lighting the ranch renders")
        // The trigger/count cluster only (the outage banner below it is a
        // separate #456 surface and keeps its treatment).
        let chrome = try slice(herd, from: "privatevartopChrome", to: "privatevaroutage")
        for needle in ["HerdFilterGlyph(color:ranchTokens.accentColor)",
                       "foregroundStyle(ranchTokens.inkColor).lineLimit(1)",
                       "foregroundStyle(ranchTokens.mutedColor)",
                       "HerdGearGlyph(color:ranchTokens.inkColor)"] {
            XCTAssertTrue(chrome.contains(needle),
                          "the trigger/count chrome must take the ranch tokens: \(needle)")
        }
        XCTAssertEqual(chrome.components(separatedBy: ".ranchChromeSurface(ranchTokens)").count - 1, 3,
                       "scope pill, Settings control and counts card ride the ranch chrome surface")
        XCTAssertFalse(chrome.contains(".regularMaterial"),
                       "no trigger/count surface may keep the flavor material (#457 AC1)")
        XCTAssertFalse(chrome.contains("theme.text") || chrome.contains("theme.subtext1"),
                       "no trigger/count text may inherit the app flavor's light/dark text")
        // Out-of-scope herd surfaces stay exactly as #456 shipped them.
        let nav = try slice(herd, from: "privatevarnavigation", to: "funcmovePage(")
        XCTAssertTrue(nav.contains(".background(.regularMaterial,in:RoundedRectangle(cornerRadius:15))"),
                      "the bottom paddock navigation keeps its #456 material pill")
        // The environment axis never writes the app theme.
        XCTAssertFalse(herd.contains("setFlavor"),
                       "the herd surface must never rewrite the global theme preference")
        XCTAssertFalse(herd.contains("flavorKey"),
                       "the herd surface must never touch the theme preference key")
    }

    func testEnvironmentSelectionStaysIndependentOfTheSavedThemePreference() throws {
        XCTAssertNotEqual("herdEnvironment", ThemeStore.flavorKey,
                          "the ranch environment lives in its own preference, not the theme's")
        let suiteName = "corral457-theme-\(UUID().uuidString)"
        let defaults = try XCTUnwrap(UserDefaults(suiteName: suiteName))
        defaults.removePersistentDomain(forName: suiteName)
        let theme = ThemeStore(defaults: defaults)
        theme.setFlavor(.macchiato)
        let savedFlavor = defaults.string(forKey: ThemeStore.flavorKey)
        XCTAssertEqual(savedFlavor, CatppuccinFlavor.macchiato.rawValue)

        // Drive the herd environment exactly like the Settings picker does
        // and evaluate every explicit sheet context for both lights.
        let previous = UserDefaults.standard.string(forKey: "herdEnvironment")
        defer {
            if let previous {
                UserDefaults.standard.set(previous, forKey: "herdEnvironment")
            } else {
                UserDefaults.standard.removeObject(forKey: "herdEnvironment")
            }
        }
        for choice in [HerdEnvironmentChoice.night, .day, .auto] {
            UserDefaults.standard.set(choice.rawValue, forKey: "herdEnvironment")
            for context in [FilterSheetContext.board,
                            .herd(night: HerdSun.resolve(choice, now: Date()).night)] {
                _ = context
            }
        }
        XCTAssertEqual(theme.flavor, .macchiato,
                       "an environment change must never flip the live flavor")
        XCTAssertEqual(defaults.string(forKey: ThemeStore.flavorKey), savedFlavor,
                       "an environment change must never rewrite the saved preference")
        XCTAssertEqual(UserDefaults.standard.string(forKey: "herdEnvironment"),
                       HerdEnvironmentChoice.auto.rawValue,
                       "the environment preference itself round-trips")
    }

    // MARK: interaction (lighting report)

    func testHerdChromeReportsItsResolvedLightingForTheSheetContext() async throws {
        let previous = UserDefaults.standard.string(forKey: "herdEnvironment")
        defer {
            if let previous {
                UserDefaults.standard.set(previous, forKey: "herdEnvironment")
            } else {
                UserDefaults.standard.removeObject(forKey: "herdEnvironment")
            }
        }
        UserDefaults.standard.set(HerdEnvironmentChoice.day.rawValue, forKey: "herdEnvironment")
        final class Box { var values: [Bool] = [] }
        let box = Box()
        let scene = HerdView(horses: [], obscured: false,
                             showFilters: .constant(false), showSettings: .constant(false),
                             onLightingNight: { box.values.append($0) },
                             select: { _ in }, openBoard: {}, retry: {})
            .environmentObject(ThemeStore())
        let controller = UIHostingController(rootView: AnyView(scene))
        let windowScene = UIApplication.shared.connectedScenes
            .compactMap { $0 as? UIWindowScene }.first
        let window = windowScene.map { UIWindow(windowScene: $0) } ?? UIWindow()
        window.frame = CGRect(origin: .zero, size: CGSize(width: 393, height: 852))
        window.rootViewController = controller
        window.makeKeyAndVisible()
        defer { window.isHidden = true }

        var attempts = 0
        while box.values.last != false && attempts < 30 {
            try await Task.sleep(for: .milliseconds(100))
            attempts += 1
        }
        XCTAssertEqual(box.values.first, false,
                       "the Day environment must be reported up (the sheet's Herd context "
                       + "is styled from this exact value)")

        UserDefaults.standard.set(HerdEnvironmentChoice.night.rawValue, forKey: "herdEnvironment")
        attempts = 0
        while box.values.last != true && attempts < 30 {
            try await Task.sleep(for: .milliseconds(100))
            attempts += 1
        }
        XCTAssertEqual(box.values.last, true,
                       "flipping the herd environment must report Night up — without touching "
                       + "the theme preference")
    }

    // MARK: evidence driver hygiene

    func testContextEvidenceDriverStaysDebugOnly() throws {
        let board = try source("FleetViews")
        let debug = debugActiveLines(board)
        for needle in ["runContextFilterSheetSequence", "contextFilterEvidenceRan",
                       "runContextBoardShot", "contextBoardShotRan"] {
            let lines = board.split(separator: "\n", omittingEmptySubsequences: false)
                .enumerated()
                .filter { $0.element.contains(needle) }
                .map { $0.offset + 1 }
            XCTAssertFalse(lines.isEmpty, "\(needle) must exist for the #457 evidence driver")
            for line in lines {
                XCTAssertTrue(debug.contains(line),
                              "\(needle) must stay inside #if DEBUG (Release-inert); line \(line)")
            }
        }
        XCTAssertTrue(board.contains("static let contextArgument = \"-corral457ContextEvidence\""))
        XCTAssertTrue(board.contains("static let opaqueChromeArgument = \"-corral457ForceOpaqueChrome\""))
        XCTAssertTrue(board.contains("Corral457Evidence.forcesOpaqueChrome"),
                      "the DEBUG evidence force must take the SAME opaque branch")
        XCTAssertTrue(board.contains("reduceTransparency || contrast == .increased"),
                      "the chrome fallback must take the system accessibility signals")
        XCTAssertTrue(board.contains("if let forcesOpaque { return forcesOpaque }"),
                      "the explicit override must resolve first (the deterministic seam)")
        // The scroll hook rides the DEBUG block in the sheet.
        let scrollLines = board.split(separator: "\n", omittingEmptySubsequences: false)
            .enumerated()
            .filter { $0.element.contains("Corral457Evidence.scrollNotification") }
            .map { $0.offset + 1 }
        XCTAssertFalse(scrollLines.isEmpty)
        for line in scrollLines {
            XCTAssertTrue(debug.contains(line),
                          "the sheet scroll hook must stay inside #if DEBUG; line \(line)")
        }
    }
}
