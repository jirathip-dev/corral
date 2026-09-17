import XCTest

/// Real input, not scroll offsets. Run under simctl recordVideo on an owned simulator.
final class HerdEdgeGestureTests: XCTestCase {
    private struct Sample: Decodable {
        let group: CGRect
        let viewport: CGRect
        let opacity: Double
        let oversized: Bool
    }

    override func setUpWithError() throws { continueAfterFailure = false }

    private func launch(_ name: String, extra: [String] = [], accessibility: Bool = false) -> XCUIApplication {
        let app = XCUIApplication()
        app.launchArguments = ["-corralHerdEvidence", "-corral568EdgeEvidence"] + extra
        app.launchEnvironment["CORRAL568_CASE"] = name
        app.launch()
        let identifier = accessibility ? "herd-accessibility-column" : "herd-field-repo:edge-meadow"
        XCTAssertTrue(app.scrollViews[identifier].waitForExistence(timeout: 15))
        print("G568_CASE_BEGIN \(name) epoch=\(Date().timeIntervalSince1970) screen=\(app.frame)")
        return app
    }

    private func samples(_ app: XCUIApplication) throws -> [String: Sample] {
        let element = app.otherElements["g568-geometry"]
        XCTAssertTrue(element.waitForExistence(timeout: 5))
        let value = try XCTUnwrap(element.value as? String)
        return try JSONDecoder().decode([String: Sample].self, from: Data(value.utf8))
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

    private func edges(_ name: String) throws {
        let app = launch(name, extra: name.contains("reduced") ? ["-corralDemoReduceMotion"] : [])
        let field = app.scrollViews["herd-field-repo:edge-meadow"]
        let upper = field.coordinate(withNormalizedOffset: CGVector(dx: 0.5, dy: 0.18))
        let lower = field.coordinate(withNormalizedOffset: CGVector(dx: 0.5, dy: 0.82))
        try hold(name + "-first-row", app: app)
        upper.press(forDuration: 0.1, thenDragTo: lower, withVelocity: .slow, thenHoldForDuration: 0.1)
        try hold(name + "-top-overscroll-settle", app: app)
        lower.press(forDuration: 0.1, thenDragTo: upper, withVelocity: .slow, thenHoldForDuration: 0.1)
        upper.press(forDuration: 0.1, thenDragTo: lower, withVelocity: .slow, thenHoldForDuration: 0.1)
        try hold(name + "-top-reversal-settle", app: app)
        for _ in 0..<5 { field.swipeUp(velocity: .fast) }
        try hold(name + "-bottom-flick-settle", app: app)
        XCTAssertGreaterThan(try XCTUnwrap(samples(app)["::edge-14"]).opacity, 0, "last row reachable")
        lower.press(forDuration: 0.1, thenDragTo: upper, withVelocity: .slow, thenHoldForDuration: 0.1)
        try hold(name + "-bottom-overscroll-settle", app: app)
        upper.press(forDuration: 0.1, thenDragTo: lower, withVelocity: .slow, thenHoldForDuration: 0.1)
        lower.press(forDuration: 0.1, thenDragTo: upper, withVelocity: .slow, thenHoldForDuration: 0.1)
        try hold(name + "-bottom-reversal-settle", app: app)
        for _ in 0..<5 { field.swipeDown(velocity: .fast) }
        try hold(name + "-top-flick-settle", app: app)
        XCTAssertGreaterThan(try XCTUnwrap(samples(app)["::edge-00"]).opacity, 0, "first row reachable")
        XCTAssertTrue(app.buttons["Next"].isEnabled)
        app.buttons["Next"].tap()
        XCTAssertTrue(app.scrollViews["herd-field-repo:quiet-meadow"].waitForExistence(timeout: 5))
        app.buttons["Previous"].tap()
        XCTAssertTrue(field.waitForExistence(timeout: 5))
        field.swipeLeft(velocity: .slow)
        XCTAssertTrue(app.scrollViews["herd-field-repo:quiet-meadow"].waitForExistence(timeout: 5))
        app.scrollViews["herd-field-repo:quiet-meadow"].swipeRight(velocity: .slow)
        XCTAssertTrue(field.waitForExistence(timeout: 5))
        try hold(name + "-navigation-restored", app: app)
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
        // The same invisible row still has semantic content. An accessibility
        // automation request must reveal/reach it instead of dropping its identity.
        let semantic = app.buttons["herd-horse-" + hidden.key]
        XCTAssertTrue(semantic.exists)
        semantic.tap()
        XCTAssertTrue(app.buttons["Done"].waitForExistence(timeout: 5))
        app.buttons["Done"].tap()
        XCTAssertTrue(app.buttons["Filters"].exists || app.buttons.matching(NSPredicate(format: "label BEGINSWITH 'Filters'")).count > 0)
        print("G568_TAP_PASS hidden=\(hidden.key) semanticReachable=true")
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
        app.terminate()
        try edges("day-empty-reduced")
    }

    func testEmptyAndSingleRowPaddocks() throws {
        let single = launch("day-single", extra: ["-corralDemoReduceMotion"])
        try hold("single-row", app: single)
        single.scrollViews["herd-field-repo:edge-meadow"].swipeUp(velocity: .fast)
        try hold("single-row-overscroll", app: single)
        single.terminate()
        let empty = launch("day-no-field")
        XCTAssertFalse(try samples(empty).keys.contains { $0.hasPrefix("::edge-") })
        empty.scrollViews["herd-field-repo:edge-meadow"].swipeUp(velocity: .fast)
        XCTAssertTrue(empty.buttons["Next"].isEnabled)
        empty.terminate()
    }
}
