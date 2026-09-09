import XCTest
import SwiftUI
import UIKit
@testable import FleetNotifier

// MARK: - #464 App Icon picker: protocol-seam tests (mock-backed, no device)

/// The seam mock. It records every request, can hold completions open (rapid
/// taps), can simulate an unsupported device, and can deliberately leave the
/// live state unchanged so tests prove the re-read — not the completion
/// callback — drives the UI.
@MainActor
private final class MockAppIconSystem: AppIconSystem {
    var supportsAlternateIcons: Bool
    var alternateIconName: String?
    /// Real OS behaviour: a successful request moves the live state the picker
    /// re-reads. Tests flip this OFF to simulate a stale/lying system result.
    var appliesChanges = true
    private(set) var requestedNames: [String?] = []
    private var pending: [(name: String?, completion: @MainActor (Error?) -> Void)] = []

    init(supportsAlternateIcons: Bool = true, alternateIconName: String? = nil) {
        self.supportsAlternateIcons = supportsAlternateIcons
        self.alternateIconName = alternateIconName
    }

    func setAlternateIconName(_ name: String?,
                              completion: @escaping @MainActor (Error?) -> Void) {
        requestedNames.append(name)
        pending.append((name, completion))
    }

    var inFlightCount: Int { pending.count }

    /// Completes the OLDEST pending request the way the OS would.
    func completeNext(_ error: Error? = nil) {
        guard !pending.isEmpty else { return }
        let next = pending.removeFirst()
        if error == nil && appliesChanges { alternateIconName = next.name }
        next.completion(error)
    }
}

@MainActor
final class AppIconPickerTests: XCTestCase {

    private struct SimulatedFailure: LocalizedError {
        var errorDescription: String? { "simulated system failure" }
    }

    // MARK: Packaging contract (#463: exactly four names, Bay primary)

    func testShippingChoicesAreExactlyTheApprovedFourWithBayPrimary() {
        XCTAssertEqual(FleetAppIcon.shipping.map(\.displayName),
                       ["Bay", "Palomino", "Black", "Grey"])
        XCTAssertEqual(FleetAppIcon.shipping.map(\.assetName),
                       ["AppIcon", "Palomino", "Black", "Grey"])
        XCTAssertEqual(FleetAppIcon.shipping.map(\.alternateIconName),
                       [nil, "Palomino", "Black", "Grey"],
                       "nil alternateIconName restores Bay; there is no Bay alternate")
        XCTAssertNil(FleetAppIcon.bay.alternateIconName)
    }

    func testSystemStateMapsNilToBayAndUnknownNamesToNoTile() {
        XCTAssertEqual(FleetAppIcon(systemAlternateIconName: nil), .bay)
        XCTAssertEqual(FleetAppIcon(systemAlternateIconName: "Palomino"), .palomino)
        XCTAssertEqual(FleetAppIcon(systemAlternateIconName: "Black"), .black)
        XCTAssertEqual(FleetAppIcon(systemAlternateIconName: "Grey"), .grey)
        XCTAssertNil(FleetAppIcon(systemAlternateIconName: "Original"),
                     "a non-shipping system name must not be shown as a tile")
    }

    // MARK: Model behaviour

    func testInitialStateIsReadFromTheSystemNotAParallelPreference() {
        let system = MockAppIconSystem(alternateIconName: "Grey")
        let model = AppIconPickerModel(system: system)
        XCTAssertEqual(model.selection, .grey)
        XCTAssertTrue(model.supportsAlternateIcons)
        XCTAssertFalse(model.isApplying)
        XCTAssertNil(model.errorMessage)
    }

    func testTapCurrentSelectionIsANoOp() {
        let system = MockAppIconSystem(alternateIconName: nil)
        let model = AppIconPickerModel(system: system)
        model.select(.bay)
        XCTAssertTrue(system.requestedNames.isEmpty,
                      "tapping the live selection must not request a change")
        XCTAssertEqual(model.selection, .bay)
        XCTAssertFalse(model.isApplying)
    }

    func testSelectingAnAlternateRequestsTheStableNameAndConfirmsFromReread() {
        let system = MockAppIconSystem()
        let model = AppIconPickerModel(system: system)
        model.select(.palomino)
        XCTAssertEqual(system.requestedNames, ["Palomino"])
        XCTAssertTrue(model.isApplying)
        XCTAssertEqual(model.selection, .bay,
                       "the selection must not move before the system confirms")
        system.completeNext()
        XCTAssertFalse(model.isApplying)
        XCTAssertEqual(model.selection, .palomino)
        XCTAssertNil(model.errorMessage)
    }

    func testRestoreBayRequestsNilAlternateAndRereadConfirms() {
        let system = MockAppIconSystem(alternateIconName: "Grey")
        let model = AppIconPickerModel(system: system)
        model.select(.bay)
        XCTAssertEqual(system.requestedNames, [nil],
                       "restoring the primary means a nil alternateIconName")
        system.completeNext()
        XCTAssertEqual(model.selection, .bay)
        XCTAssertNil(model.errorMessage)
    }

    func testReportedSuccessWithStaleSystemStateIsNotShownAsSuccess() {
        let system = MockAppIconSystem(alternateIconName: nil)
        system.appliesChanges = false
        let model = AppIconPickerModel(system: system)
        model.select(.black)
        system.completeNext()
        XCTAssertEqual(model.selection, .bay,
                       "the re-read after completion is authoritative")
        XCTAssertNotNil(model.errorMessage,
                        "an unconfirmed change must surface a retryable error")
        XCTAssertFalse(model.isApplying)
    }

    func testFailureKeepsThePreviousSelectionAndStaysRetryable() {
        let system = MockAppIconSystem(alternateIconName: nil)
        let model = AppIconPickerModel(system: system)
        model.select(.grey)
        system.completeNext(SimulatedFailure())
        XCTAssertEqual(model.selection, .bay)
        let message = try? XCTUnwrap(model.errorMessage)
        XCTAssertNotNil(message)
        XCTAssertTrue(message?.contains("try again") == true,
                      "failure copy must be actionable")
        XCTAssertFalse(model.isApplying)
        model.select(.grey)
        XCTAssertEqual(system.requestedNames, ["Grey", "Grey"],
                       "a failed change must be retryable")
        system.completeNext()
        XCTAssertEqual(model.selection, .grey)
        XCTAssertNil(model.errorMessage)
    }

    func testUnsupportedDeviceNeverRequestsAndStaysHonest() {
        let system = MockAppIconSystem(supportsAlternateIcons: false,
                                       alternateIconName: nil)
        let model = AppIconPickerModel(system: system)
        XCTAssertFalse(model.supportsAlternateIcons)
        model.select(.palomino)
        XCTAssertTrue(system.requestedNames.isEmpty)
        XCTAssertNil(model.errorMessage,
                     "an unsupported device is a state, not a failure")
    }

    func testRapidDoubleTapSendsOneRequestAndLaterTapsStillWork() {
        let system = MockAppIconSystem()
        let model = AppIconPickerModel(system: system)
        model.select(.palomino)
        model.select(.black)
        XCTAssertEqual(system.requestedNames, ["Palomino"],
                       "in-flight requests must never overlap")
        XCTAssertEqual(system.inFlightCount, 1)
        system.completeNext()
        XCTAssertEqual(model.selection, .palomino)
        model.select(.black)
        XCTAssertEqual(system.requestedNames, ["Palomino", "Black"])
        system.completeNext()
        XCTAssertEqual(model.selection, .black)
    }

    func testRefreshRereadsLiveSystemStateForSceneActive() {
        let system = MockAppIconSystem(alternateIconName: nil)
        let model = AppIconPickerModel(system: system)
        system.alternateIconName = "Grey"
        model.refresh()
        XCTAssertEqual(model.selection, .grey,
                       "returning to the scene must re-read the system state")
        system.alternateIconName = nil
        model.refresh()
        XCTAssertEqual(model.selection, .bay)
    }

    // MARK: Shipped-packaging + live-runtime proof

    func testBuiltProductDeclaresExactlyTheShippingIconNames() throws {
        let info = try XCTUnwrap(Bundle.main.infoDictionary)
        let icons = try XCTUnwrap(info["CFBundleIcons"] as? [String: Any],
                                  "the built product must declare CFBundleIcons")
        let primary = try XCTUnwrap(icons["CFBundlePrimaryIcon"] as? [String: Any])
        XCTAssertEqual(primary["CFBundleIconName"] as? String, "AppIcon",
                       "the primary icon is the Bay master (AppIcon)")
        let alternates = try XCTUnwrap(icons["CFBundleAlternateIcons"] as? [String: Any])
        XCTAssertEqual(Set(alternates.keys), ["Palomino", "Black", "Grey"],
                       "the shipped alternates are exactly the three approved sets")
        XCTAssertNil(alternates["Bay"], "Bay must never be an alternate")
        XCTAssertEqual(Set(FleetAppIcon.shipping.compactMap(\.alternateIconName)),
                       Set(alternates.keys),
                       "the picker's stable names are exactly the shipped declarations")
    }

    func testLiveRuntimeReadsReportTheSystemTruthfully() {
        let supports = UIApplication.shared.supportsAlternateIcons
        let live = UIApplication.shared.alternateIconName
        let mapped = FleetAppIcon(systemAlternateIconName: live)
        if !supports {
            XCTAssertNil(live, "an unsupported system must not report an alternate")
        }
        if let live {
            XCTAssertNotNil(mapped,
                            "\(live) is outside the approved four-icon contract")
        } else {
            XCTAssertEqual(mapped, .bay, "nil alternateIconName means Bay")
        }
    }

    // MARK: Source wiring (bundled FleetViews.swift.txt + AppIconPicker.swift.txt)

    private func bundledSource(_ resource: String) throws -> String {
        let bundle = Bundle(for: AppIconPickerTests.self)
        let url = try XCTUnwrap(bundle.url(forResource: resource,
                                           withExtension: "swift.txt"))
        return try String(contentsOf: url, encoding: .utf8)
    }

    private func lineNumbers(of needle: String, in text: String) -> [Int] {
        text.split(separator: "\n", omittingEmptySubsequences: false)
            .enumerated()
            .filter { $0.element.contains(needle) }
            .map { $0.offset + 1 }
    }

    /// 1-based line numbers inside flat `#if DEBUG` regions (the same exact
    /// depth scan the SettingsAccessWiringTests use).
    private func debugActiveLines(_ source: String) -> Set<Int> {
        var active: Set<Int> = []
        var depth = 0
        for (index, line) in source.split(separator: "\n",
                                          omittingEmptySubsequences: false)
            .enumerated() {
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

    func testSettingsFormRendersTheAppIconSectionBelowAppearance() throws {
        let source = try bundledSource("FleetViews")
        let debug = debugActiveLines(source)

        let appearance = try XCTUnwrap(lineNumbers(of: "appearanceSection", in: source)
            .filter { !debug.contains($0) }.first,
            "the Appearance section must stay in the Settings form")
        let appIcon = try XCTUnwrap(lineNumbers(of: "appIconSection", in: source)
            .filter { !debug.contains($0) && $0 > appearance }.first,
            "the App Icon section must render below Appearance")
        XCTAssertEqual(lineNumbers(of: "Section {", in: source)
            .filter { !debug.contains($0) && $0 > appearance && $0 < appIcon }.count,
                       0,
                       "App Icon must be its own section directly below Appearance")
        XCTAssertEqual(lineNumbers(of: "Text(\"App Icon\")", in: source).count, 1)
        XCTAssertEqual(lineNumbers(of: "ForEach(FleetAppIcon.shipping)", in: source).count, 1,
                       "the grid must render exactly the four shipping choices")
        XCTAssertTrue(source.contains("appIcon.select(icon)"),
                      "a tile tap must route through the picker model")
        XCTAssertTrue(source.contains("appIcon.selection == icon"),
                      "the selected state must derive from live system state")
        XCTAssertTrue(source.contains("Alternate app icons aren't available on this device."),
                      "an unsupported device needs truthful copy")
        XCTAssertTrue(source.contains(".accessibilityAddTraits(selected ? [.isSelected] : [])"),
                      "the selected tile must expose the isSelected trait")
    }

    func testSettingsReReadsIconStateOnSceneActive() throws {
        let source = try bundledSource("FleetViews")
        let refresh = lineNumbers(of: "appIcon.refresh()", in: source)
        XCTAssertFalse(refresh.isEmpty, "the picker must re-read live state")
        XCTAssertTrue(source.contains(".onChange(of: scenePhase)"),
                      "scene-active returns must re-read the system state")
    }

    func testPickerSourceHasNoParallelIconStore() throws {
        let source = try bundledSource("AppIconPicker")
        // Comments may name the banned stores; only real code counts.
        let code = source.split(separator: "\n", omittingEmptySubsequences: false)
            .filter { !$0.trimmingCharacters(in: .whitespaces).hasPrefix("//") }
            .joined(separator: "\n")
        for forbidden in ["UserDefaults", "AppStorage", "NSUbiquitousKeyValueStore",
                          "FileManager", "URLSession", "Codable"] {
            XCTAssertFalse(code.contains(forbidden),
                           "the picker must not persist or infer icon state (\(forbidden))")
        }
        XCTAssertEqual(lineNumbers(of: "Image(\"AppIcon\")", in: source).count, 1)
        XCTAssertEqual(lineNumbers(of: "Image(\"Palomino\")", in: source).count, 1)
        XCTAssertEqual(lineNumbers(of: "Image(\"Black\")", in: source).count, 1)
        XCTAssertEqual(lineNumbers(of: "Image(\"Grey\")", in: source).count, 1)
        XCTAssertFalse(source.contains("Image(\"Original\")"),
                       "the legacy Original icon is not a shipping choice")
    }
}
