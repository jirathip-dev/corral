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

    // MARK: Preview art: production-renderer proof + approved-art identity

    /// One rasterized image's RGBA bytes in a known sRGB context.
    private struct Raster {
        let width: Int
        let height: Int
        let bytes: [UInt8]

        var alphaSum: Int {
            var total = 0
            var index = 3
            while index < bytes.count { total += Int(bytes[index]); index += 4 }
            return total
        }

        var opaquePixelCount: Int {
            var count = 0
            var index = 3
            while index < bytes.count {
                if bytes[index] > 0 { count += 1 }
                index += 4
            }
            return count
        }
    }

    private enum RasterError: Error {
        case missingContext
    }

    /// Renders the REAL production view through ImageRenderer — the same
    /// path the Settings tile draws — and returns its pixels.
    @MainActor
    private func rasterize(_ view: some View, width: Int, height: Int) throws -> Raster {
        let renderer = ImageRenderer(content: view.frame(width: CGFloat(width),
                                                         height: CGFloat(height)))
        renderer.scale = 1
        let image = try XCTUnwrap(renderer.uiImage, "ImageRenderer produced no image")
        return try rasterize(image)
    }

    private func rasterize(_ image: UIImage) throws -> Raster {
        let cgImage = try XCTUnwrap(image.cgImage, "image has no CGImage")
        let width = cgImage.width
        let height = cgImage.height
        var bytes = [UInt8](repeating: 0, count: width * height * 4)
        let space = try XCTUnwrap(CGColorSpace(name: CGColorSpace.sRGB))
        try bytes.withUnsafeMutableBytes { buffer in
            guard let context = CGContext(data: buffer.baseAddress,
                                          width: width, height: height,
                                          bitsPerComponent: 8, bytesPerRow: width * 4,
                                          space: space,
                                          bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue)
            else { throw RasterError.missingContext }
            context.draw(cgImage, in: CGRect(x: 0, y: 0, width: width, height: height))
        }
        return Raster(width: width, height: height, bytes: bytes)
    }

    /// The approved Treatment-A master for an icon. The master BYTES ride in
    /// the TEST bundle only, base64-encoded by the test pre-build phase (the
    /// app-product art scan must never see extra loose artwork).
    private func masterImage(_ icon: FleetAppIcon) throws -> UIImage {
        let bundle = Bundle(for: AppIconPickerTests.self)
        let url = try XCTUnwrap(bundle.url(forResource: "master-\(masterCoat(icon))",
                                           withExtension: "base64"),
                                "approved master fixture master-\(masterCoat(icon)).base64 missing")
        let encoded = try String(contentsOf: url, encoding: .utf8)
        let data = try XCTUnwrap(
            Data(base64Encoded: encoded.trimmingCharacters(in: .whitespacesAndNewlines)),
            "approved master fixture is not valid base64")
        return try XCTUnwrap(UIImage(data: data), "master fixture does not decode")
    }

    private func masterCoat(_ icon: FleetAppIcon) -> String {
        switch icon {
        case .bay: return "bay"
        case .palomino: return "palomino"
        case .black: return "black"
        case .grey: return "grey"
        }
    }

    private func meanAbsoluteDelta(_ a: Raster, _ b: Raster) -> Double {
        guard a.bytes.count == b.bytes.count, !a.bytes.isEmpty else { return .infinity }
        var total = 0
        for index in a.bytes.indices { total += abs(Int(a.bytes[index]) - Int(b.bytes[index])) }
        return Double(total) / Double(a.bytes.count)
    }

    private func maxAbsoluteDelta(_ a: Raster, _ b: Raster) -> Int {
        guard a.bytes.count == b.bytes.count else { return .max }
        var maximum = 0
        for index in a.bytes.indices {
            maximum = max(maximum, abs(Int(a.bytes[index]) - Int(b.bytes[index])))
        }
        return maximum
    }

    /// Positive and negative controls: without both, a zero-alpha result
    /// proves nothing about the picker (the r1 review's probe-integrity rule).
    @MainActor
    func testPickerPreviewHarnessHasValidControls() throws {
        let symbol = try rasterize(Image(systemName: "gearshape").resizable().scaledToFit(),
                                   width: 52, height: 52)
        XCTAssertGreaterThan(symbol.alphaSum, 0,
                             "positive control: a system symbol must render pixels")
        let master = try rasterize(try masterImage(.bay))
        XCTAssertGreaterThan(master.alphaSum, 0,
                             "positive control: the bundled approved master must decode")
        let blank = try rasterize(Image("NoSuchPreviewAsset464").resizable().scaledToFit(),
                                  width: 52, height: 52)
        XCTAssertEqual(blank.alphaSum, 0,
                       "negative control: a missing asset must render zero pixels")
    }

    /// Every Settings tile renders its approved master art through the REAL
    /// production call site, and nothing else can satisfy it.
    @MainActor
    func testEveryPickerTileRendersTheApprovedMasterArt() throws {
        for icon in FleetAppIcon.shipping {
            let tile = try rasterize(icon.preview.resizable().scaledToFit(),
                                     width: 52, height: 52)
            XCTAssertGreaterThan(tile.alphaSum, 0,
                                 "\(icon.displayName) preview must render the shipped art")
            XCTAssertGreaterThan(tile.opaquePixelCount, 0,
                                 "\(icon.displayName) preview must contain opaque pixels")

            // Art identity: the same production Image rendered at the
            // master's own 1024x1024 size must reproduce the approved master.
            let rendered = try rasterize(icon.preview.resizable(), width: 1024, height: 1024)
            let master = try rasterize(try masterImage(icon))
            XCTAssertEqual(rendered.width, master.width)
            XCTAssertEqual(rendered.height, master.height)
            let maximum = maxAbsoluteDelta(rendered, master)
            let mean = meanAbsoluteDelta(rendered, master)
            // Measured on iOS 26.5 (the log carries the numbers): the same
            // art through the production renderer is exact or within one
            // channel level (mean <= 0.13); different approved art differs by
            // a mean of >= 17. These bounds sit far from both.
            print("PREVIEW_IDENTITY \(icon.displayName) max=\(maximum) mean=\(mean)")
            XCTAssertLessThanOrEqual(maximum, 2,
                                     "\(icon.displayName) preview is not the approved master art "
                                     + "(max channel delta \(maximum))")
            XCTAssertLessThan(mean, 1.0,
                              "\(icon.displayName) preview drifts from the approved master art "
                              + "(mean channel delta \(mean))")
        }
    }

    /// The four previews are pairwise distinct approved art, so a swapped
    /// mapping cannot satisfy the identity assertion above.
    @MainActor
    func testPickerPreviewsArePairwiseDistinctArt() throws {
        var rendered: [(FleetAppIcon, Raster)] = []
        for icon in FleetAppIcon.shipping {
            rendered.append((icon, try rasterize(icon.preview.resizable().scaledToFit(),
                                                 width: 52, height: 52)))
        }
        for first in rendered.indices {
            for second in rendered.indices where second > first {
                let delta = meanAbsoluteDelta(rendered[first].1, rendered[second].1)
                print("PREVIEW_PAIR \(rendered[first].0.displayName)-\(rendered[second].0.displayName) delta=\(delta)")
                // Measured minimum between two approved coats is 17.2; the
                // bound keeps a wrong-art swap far outside the identity range.
                XCTAssertGreaterThan(delta, 5.0,
                                     "\(rendered[first].0.displayName) and "
                                     + "\(rendered[second].0.displayName) previews must be distinct art "
                                     + "(mean channel delta \(delta))")
            }
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
        // The preview mapping itself is proved at RUNTIME (the ImageRenderer
        // tests above); this source pin keeps only the invariants that are
        // genuinely structural — no parallel store and no non-preview loader.
        let imageNames = source.split(separator: "\n").compactMap { line -> String? in
            guard let start = line.range(of: "Image(\"") else { return nil }
            let rest = line[start.upperBound...]
            guard let end = rest.firstIndex(of: "\"") else { return nil }
            return String(rest[..<end])
        }
        XCTAssertEqual(imageNames.sorted(),
                       ["BayPreview", "BlackPreview", "GreyPreview", "PalominoPreview"],
                       "the only image loaders may be the four loadable preview imagesets")
        XCTAssertFalse(code.contains("Image(\"Original\")"),
                       "the legacy Original icon is not a shipping choice")
    }
}
