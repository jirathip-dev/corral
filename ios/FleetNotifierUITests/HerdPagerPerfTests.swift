import XCTest

/// #574 pager-perf battery. Real input (swipes, taps), never scroll-offset
/// pokes. SKIPPED unless the on-demand marker `CORRAL574_PERF=1` is present in
/// the runner's environment, so the shared scheme's bare `xcodebuild test`
/// (CI) skips it. The harness sets that marker on the SIMULATOR's launchd
/// environment (`xcrun simctl launchctl setenv CORRAL574_PERF 1`) because
/// xcodebuild does not forward the shell environment to the test runner:
///
///   xcrun simctl launchctl setenv CORRAL574_PERF 1
///   TEST_RUNNER_CORRAL574_PERF=1 HERDR_XCODEBUILD_DIRECT=1 HERMES_SIM_TASK_ACTIVE=1 \
///   xcodebuild test -project ios/FleetNotifier.xcodeproj -scheme FleetNotifier \
///     -destination "platform=iOS Simulator,id=<udid>" -derivedDataPath /tmp/fn574-dd \
///     -only-testing:FleetNotifierUITests/HerdPagerPerfTests
///   xcrun simctl launchctl unsetenv CORRAL574_PERF
///
/// The app-under-test runs `-corralHerdEvidence -corral574Perf`: the sized
/// pager fixture (40 agents / 12 repositories, page 1 = atlas-vector, pages in
/// promo-then-alphabetical order), the parked scene clock, and the
/// `g574-perf-dump` control. Each phase ends with a dump tap; the app writes
/// `Documents/herd-perf/574-<seq>-on-demand.json` and mirrors the same JSON
/// into the `g574-perf-state` accessibility value, which this runner prints
/// between G574_DUMP_BEGIN/END markers. The host script copies the Documents
/// files out of the app container, so the raw records do not depend on log
/// parsing; the log carries the phase -> seq mapping.
final class HerdPagerPerfTests: XCTestCase {
    private struct Dump: Decodable {
        let seq: Int
        let reason: String
        let monotonic: Double
        let fixture: String
        let counters: [String: Int]
    }

    private let pageTurns = 6
    private let fieldID = "herd-field-repo:atlas-vector"
    /// Page 7 of the fixture's promo-then-alphabetical order (12 repos).
    private let pageSevenFieldID = "herd-field-repo:elder-docs"

    override func setUpWithError() throws {
        continueAfterFailure = false
        try XCTSkipUnless(ProcessInfo.processInfo.environment["CORRAL574_PERF"] == "1",
                          "perf battery is on-demand only (CORRAL574_PERF=1)")
    }

    func testPagerPerfBattery() throws {
        let app = XCUIApplication()
        app.launchArguments = ["-corralHerdEvidence", "-corral574Perf"]
        app.launch()

        let state = app.otherElements["g574-perf-state"]
        XCTAssertTrue(state.waitForExistence(timeout: 60), "perf fixture marker missing")
        let dumpButton = app.buttons["g574-perf-dump"]
        XCTAssertTrue(dumpButton.waitForExistence(timeout: 60), "perf dump control missing")
        // Page-1 landmark: the app disables Previous on the first paddock (the
        // nav pill's own label overrides its children, so the position text is
        // not individually addressable).
        XCTAssertTrue(waitOnFirstPage(app), "fixture did not park on page 1")
        let field = app.scrollViews[fieldID]
        XCTAssertTrue(field.waitForExistence(timeout: 60), "page-1 field missing")

        Thread.sleep(forTimeInterval: 2.0)
        _ = try dump(app, state: state, button: dumpButton, phase: "t0-settle")

        // t1: N horizontal page turns right (real swipes on the page-1 field;
        // the vertical field does not consume a horizontal drag).
        for _ in 0..<pageTurns { field.swipeLeft(velocity: .slow) }
        Thread.sleep(forTimeInterval: 1.5)
        XCTAssertTrue(app.scrollViews[pageSevenFieldID].waitForExistence(timeout: 30),
                      "t1 did not reach page 7")
        _ = try dump(app, state: state, button: dumpButton, phase: "t1-swipe-right")

        // t2: N page turns back left.
        let pageSeven = app.scrollViews[pageSevenFieldID]
        for _ in 0..<pageTurns { pageSeven.swipeRight(velocity: .slow) }
        Thread.sleep(forTimeInterval: 1.5)
        XCTAssertTrue(waitOnFirstPage(app), "t2 did not return to page 1")
        _ = try dump(app, state: state, button: dumpButton, phase: "t2-swipe-left")

        // t3: the same N page turns via the navigation buttons (separates
        // swipe cost from page-build cost).
        let next = app.buttons["herd-next"]
        let previous = app.buttons["herd-previous"]
        XCTAssertTrue(next.waitForExistence(timeout: 30))
        for _ in 0..<pageTurns { next.tap() }
        Thread.sleep(forTimeInterval: 1.5)
        XCTAssertTrue(app.scrollViews[pageSevenFieldID].waitForExistence(timeout: 30),
                      "t3 Next taps did not reach page 7")
        XCTAssertTrue(previous.isEnabled, "t3 Previous must be enabled on page 7")
        for _ in 0..<pageTurns { previous.tap() }
        Thread.sleep(forTimeInterval: 1.5)
        XCTAssertTrue(waitOnFirstPage(app), "t3 Previous taps did not return to page 1")
        _ = try dump(app, state: state, button: dumpButton, phase: "t3-buttons")

        // t4: the vertical control — M vertical swipes inside page 1's field.
        // Page 1 holds 12 agents (10 field rows), so the field really scrolls.
        let control = app.scrollViews[fieldID]
        for _ in 0..<3 { control.swipeUp(velocity: .slow) }
        for _ in 0..<3 { control.swipeDown(velocity: .slow) }
        Thread.sleep(forTimeInterval: 1.5)
        _ = try dump(app, state: state, button: dumpButton, phase: "t4-vertical-control")

        // t5: the sheet leg — open a horse's recents sheet, scroll it, close.
        let horseRow = app.buttons["herd-horse-::herdr:pager-fixture-0"]
        XCTAssertTrue(horseRow.waitForExistence(timeout: 30), "first horse row missing")
        horseRow.tap()
        let done = app.buttons["Done"]
        XCTAssertTrue(done.waitForExistence(timeout: 30), "recents sheet did not open")
        for _ in 0..<3 { app.swipeUp() }
        for _ in 0..<3 { app.swipeDown() }
        Thread.sleep(forTimeInterval: 1.0)
        done.tap()
        Thread.sleep(forTimeInterval: 1.0)
        _ = try dump(app, state: state, button: dumpButton, phase: "t5-sheet")

        print("G574_BATTERY_END epoch=\(Date().timeIntervalSince1970)")
    }

    /// Page-1 landmark: Previous is disabled on the first paddock.
    private func waitOnFirstPage(_ app: XCUIApplication) -> Bool {
        let previous = app.buttons["herd-previous"]
        guard previous.waitForExistence(timeout: 30) else { return false }
        let deadline = Date().addingTimeInterval(30)
        while Date() < deadline {
            if !previous.isEnabled { return true }
            Thread.sleep(forTimeInterval: 0.2)
        }
        return false
    }

    /// Tap the on-demand dump control and return the FIRST fresh (higher-seq)
    /// dump the app publishes. Falls back to a coordinate tap so a hit-test
    /// quirk cannot silently skip a phase; the seq assertion is the ground
    /// truth either way.
    private func dump(_ app: XCUIApplication, state: XCUIElement,
                      button: XCUIElement, phase: String) throws -> Dump {
        let before = decode(state.value as? String)?.seq ?? -1
        if button.isHittable { button.tap() }
        else { app.coordinate(withNormalizedOffset: CGVector(dx: 0.06, dy: 0.07)).tap() }
        var fresh: Dump?
        let deadline = Date().addingTimeInterval(30)
        while Date() < deadline {
            if let parsed = decode(state.value as? String), parsed.seq > before {
                fresh = parsed
                break
            }
            Thread.sleep(forTimeInterval: 0.2)
        }
        let parsed = try XCTUnwrap(fresh, "no fresh dump in phase \(phase) (before seq \(before))")
        let raw = try XCTUnwrap(state.value as? String)
        print("G574_DUMP_BEGIN phase=\(phase) epoch=\(Date().timeIntervalSince1970)")
        print(raw)
        print("G574_DUMP_END phase=\(phase) seq=\(parsed.seq)")
        let attachment = XCTAttachment(string: raw)
        attachment.name = "574-dump-\(phase)"
        attachment.lifetime = .keepAlways
        add(attachment)
        return parsed
    }

    private func decode(_ value: String?) -> Dump? {
        guard let value, let data = value.data(using: .utf8) else { return nil }
        return try? JSONDecoder().decode(Dump.self, from: data)
    }
}
