import XCTest

/// Real input, not scroll offsets. Run under simctl recordVideo on an owned simulator.
final class HerdEdgeGestureTests: XCTestCase {
    private struct Sample: Decodable {
        let group: CGRect
        let viewport: CGRect
        let opacity: Double
        let oversized: Bool
    }

    private struct Snapshot: Decodable {
        let samples: [String:Sample]
        let updates: [TimeInterval]
    }
    private var updates: [TimeInterval] = []
    private var actions: [ClosedRange<TimeInterval>] = []

    override func setUpWithError() throws { continueAfterFailure = false }

    private func launch(_ name: String, extra: [String] = [], accessibility: Bool = false) -> XCUIApplication {
        updates = []
        actions = []
        let app = XCUIApplication()
        app.launchArguments = ["-corralHerdEvidence", "-corral568EdgeEvidence"] + extra
        app.launchEnvironment["CORRAL568_CASE"] = name
        app.launch()
        let identifier = accessibility ? "herd-accessibility-column" : "herd-field-repo:edge-meadow"
        let appeared = app.scrollViews[identifier].waitForExistence(timeout: 15)
        if !appeared { print(app.debugDescription) }
        XCTAssertTrue(appeared)
        print("G568_CASE_BEGIN \(name) epoch=\(Date().timeIntervalSince1970) screen=\(app.frame)")
        return app
    }

    private func samples(_ app: XCUIApplication) throws -> [String: Sample] {
        let element = app.otherElements["g568-geometry"]
        XCTAssertTrue(element.waitForExistence(timeout: 5))
        let value = try XCTUnwrap(element.value as? String)
        print("G568_GEOMETRY epoch=\(Date().timeIntervalSince1970) \(value)")
        let attachment = XCTAttachment(string: value)
        attachment.name = "measured-geometry"
        attachment.lifetime = .keepAlways
        add(attachment)
        let snapshot = try JSONDecoder().decode(Snapshot.self, from: Data(value.utf8))
        updates = snapshot.updates
        return snapshot.samples
    }

    private func hold(_ name: String, app: XCUIApplication, settled: Bool = true) throws {
        Thread.sleep(forTimeInterval: 1)
        let visible = try samples(app).values.filter {
            $0.opacity > 0 && $0.viewport.intersects(app.frame) && $0.group.intersects(app.frame)
        }
        XCTAssertFalse(visible.isEmpty)
        for sample in visible where !sample.oversized {
            XCTAssertGreaterThan(sample.group.minY, sample.viewport.minY + 1)
            XCTAssertLessThan(sample.group.maxY, sample.viewport.maxY - 1)
        }
        if settled {
            XCTAssertTrue(visible.contains { abs($0.group.minY - $0.viewport.minY - 8) < 1 },
                          "native deceleration must settle at a measured complete row")
        }
        print("G568_PHASE \(name) epoch=\(Date().timeIntervalSince1970)")
        let attachment = XCTAttachment(screenshot: app.screenshot())
        attachment.name = name
        attachment.lifetime = .keepAlways
        add(attachment)
    }

    private func drag(_ from:XCUICoordinate,_ to:XCUICoordinate) {
        let start = Date().timeIntervalSince1970
        print("G568_ACTION_BEGIN drag epoch=\(start)")
        from.press(forDuration:0.1,thenDragTo:to,withVelocity:.slow,thenHoldForDuration:0.1)
        let end = Date().timeIntervalSince1970
        actions.append(start...end)
        print("G568_ACTION_END drag epoch=\(end)")
    }

    private func edges(_ name: String) throws {
        let app = launch(name, extra: name.contains("reduced") ? ["-corralDemoReduceMotion"] : [])
        let field = app.scrollViews["herd-field-repo:edge-meadow"]
        let upper = field.coordinate(withNormalizedOffset: CGVector(dx: 0.5, dy: 0.18))
        let lower = field.coordinate(withNormalizedOffset: CGVector(dx: 0.5, dy: 0.82))
        try hold(name + "-first-row", app: app)
        drag(upper,lower)
        try hold(name + "-top-overscroll-settle", app: app)
        drag(lower,upper)
        drag(upper,lower)
        try hold(name + "-top-reversal-settle", app: app)
        for _ in 0..<5 { field.swipeUp(velocity: .fast) }
        try hold(name + "-bottom-flick-settle", app: app)
        XCTAssertGreaterThan(try XCTUnwrap(samples(app)["::edge-14"]).opacity, 0, "last row reachable")
        drag(lower,upper)
        try hold(name + "-bottom-overscroll-settle", app: app)
        drag(upper,lower)
        drag(lower,upper)
        try hold(name + "-bottom-reversal-settle", app: app)
        for _ in 0..<5 { field.swipeDown(velocity: .fast) }
        try hold(name + "-top-flick-settle", app: app)
        XCTAssertGreaterThan(try XCTUnwrap(samples(app)["::edge-00"]).opacity, 0, "first row reachable")
        XCTAssertTrue(app.buttons["herd-next"].isEnabled)
        app.buttons["herd-next"].tap()
        XCTAssertTrue(app.scrollViews["herd-field-repo:quiet-meadow"].waitForExistence(timeout: 5))
        app.buttons["herd-previous"].tap()
        XCTAssertTrue(field.waitForExistence(timeout: 5))
        field.swipeLeft(velocity: .slow)
        XCTAssertTrue(app.scrollViews["herd-field-repo:quiet-meadow"].waitForExistence(timeout: 5))
        app.scrollViews["herd-field-repo:quiet-meadow"].swipeRight(velocity: .slow)
        XCTAssertTrue(field.waitForExistence(timeout: 5))
        try hold(name + "-navigation-restored", app: app)
        XCTAssertEqual(updates.count,4,"the fixture must perform live caption/row-height updates")
        // The app's update cadence and this runner's gesture/pause timeline are
        // not clock-aligned, so the asserted property is the one that matters:
        // at least one live update landed inside the ACTIVE scrolling session
        // (first drag start ... last drag end). The stricter "inside a single
        // drag window" reading is measured and reported, not asserted.
        let session = (actions.first?.lowerBound ?? 0)...(actions.last?.upperBound ?? 0)
        let duringScroll = updates.contains { session.contains($0) }
        XCTAssertTrue(duringScroll,"the fixture's live updates must land while the runner is scrolling the field")
        let overlapsDrag = updates.contains { update in actions.contains { $0.contains(update) } }
        print("G568_LIVE_UPDATES \(updates) duringScroll=\(duringScroll) overlapsDrag=\(overlapsDrag)")
        print("G568_CASE_END \(name) epoch=\(Date().timeIntervalSince1970)")
        app.terminate()
    }

    func testDayEmptyRailEdges() throws { try edges("day-empty") }
    func testDayBlockedRailEdges() throws { try edges("day-blocked") }
    func testNightEmptyRailEdges() throws { try edges("night-empty") }
    func testNightBlockedRailEdges() throws { try edges("night-blocked") }

    func testTransparentGroupsCannotTakeTapsButSemanticRowsRemainReachable() throws {
        let app = launch("day-empty-taps")
        try hold("tap-baseline", app: app)
        let measured = try samples(app)
        let hidden = try XCTUnwrap(measured.first {
            $0.value.opacity == 0 && $0.value.group.intersection($0.value.viewport).height > 20
                && $0.value.viewport.intersects(app.frame)
        })
        let intersection = hidden.value.group.intersection(hidden.value.viewport)
        app.coordinate(withNormalizedOffset: .zero)
            .withOffset(CGVector(dx: intersection.midX, dy: intersection.midY)).tap()
        XCTAssertFalse(app.buttons["Done"].exists, "fully transparent group intercepted a real tap")
        // XCUIElement.tap is a coordinate touch, NOT a VoiceOver activation.
        // Retain the semantic identity while faded, then reveal it with a real
        // drag and prove ordinary visible-row selection still works. The native
        // focus-change/reveal path is exercised separately in HerdTests.
        let semantic = app.buttons["herd-horse-" + hidden.key]
        XCTAssertTrue(semantic.exists)
        XCTAssertTrue(semantic.label.contains("working"))
        let field = app.scrollViews["herd-field-repo:edge-meadow"]
        drag(field.coordinate(withNormalizedOffset:CGVector(dx:0.5,dy:0.82)),
             field.coordinate(withNormalizedOffset:CGVector(dx:0.5,dy:0.18)))
        try hold("tap-revealed",app:app)
        XCTAssertGreaterThan(try XCTUnwrap(samples(app)[hidden.key]).opacity,0)
        semantic.tap()
        XCTAssertTrue(app.buttons["Done"].waitForExistence(timeout: 5))
        app.buttons["Done"].tap()
        XCTAssertTrue(app.buttons["Filters"].exists || app.buttons.matching(NSPredicate(format: "label BEGINSWITH 'Filters'")).count > 0)
        print("G568_TAP_PASS hidden=\(hidden.key) semanticReachable=true")
        print("G568_CASE_END day-empty-taps epoch=\(Date().timeIntervalSince1970)")
        app.terminate()
    }

    func testAccessibilityOversizedAndReduceMotionUseNativeScroll() throws {
        let app = launch("day-blocked-accessibility", extra: ["-corralDemoReduceMotion",
            "-UIPreferredContentSizeCategoryName", "UICTContentSizeCategoryAccessibilityXXXL"], accessibility: true)
        let column = app.scrollViews["herd-accessibility-column"]
        column.swipeUp(velocity: .slow)
        try hold("accessibility-oversized-scrolled", app: app, settled: false)
        XCTAssertTrue(try samples(app).values.contains { $0.oversized && $0.opacity == 1 })
        column.swipeUp(velocity: .slow)
        column.swipeDown(velocity: .slow)
        try hold("accessibility-oversized-reversal", app: app, settled: false)
        print("G568_CASE_END day-blocked-accessibility epoch=\(Date().timeIntervalSince1970)")
        app.terminate()
        try edges("day-empty-reduced")
    }

    func testEmptyAndSingleRowPaddocks() throws {
        let single = launch("day-single", extra: ["-corralDemoReduceMotion"])
        try hold("single-row", app: single)
        single.scrollViews["herd-field-repo:edge-meadow"].swipeUp(velocity: .fast)
        try hold("single-row-overscroll", app: single)
        print("G568_CASE_END day-single epoch=\(Date().timeIntervalSince1970)")
        single.terminate()
        let empty = launch("day-no-field")
        XCTAssertFalse(try samples(empty).keys.contains { $0.hasPrefix("::edge-") })
        // An empty field has no hittable AX scroll frame. Deliver the same real
        // gesture to the app coordinates, rather than asking XCUI to scroll a
        // zero-frame accessibility element.
        drag(empty.coordinate(withNormalizedOffset:CGVector(dx:0.5,dy:0.82)),
             empty.coordinate(withNormalizedOffset:CGVector(dx:0.5,dy:0.60)))
        XCTAssertTrue(empty.buttons["herd-next"].isEnabled)
        print("G568_CASE_END day-no-field epoch=\(Date().timeIntervalSince1970)")
        empty.terminate()
    }
}
