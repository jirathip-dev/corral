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
        let herdStart = try XCTUnwrap(board.range(of: "if model.fleetPresentation == .herd"))
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
        XCTAssertTrue(herd.contains("HerdGearGlyph(color:theme.text)"),
                      "the floating Settings control draws its gear natively")
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
        XCTAssertTrue(herd.contains("Text(scopeSummary).font(.caption2).foregroundStyle(theme.subtext1).lineLimit(1).truncationMode(.tail)"),
                      "long repository scope summaries stay single-line and truncate")
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
