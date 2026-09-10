import XCTest
import SwiftUI
@testable import FleetNotifier

/// #459 wind/foliage proofs. Every sampled-phase assertion below goes
/// through the REAL production renderer: `RanchEnvironment` itself, or
/// `RanchPainter.draw` exactly as `RanchEnvironment` invokes it, rasterized
/// with `ImageRenderer`. There is no test-only drawing path. A regression
/// back to a static canopy makes these tests fail (proved separately with a
/// static-tree mutation probe).
@MainActor
final class HerdWindTests: XCTestCase {

    // MARK: Raster plumbing (the RGBA-in-sRGB pattern the icon tests use)

    private struct Raster {
        let width: Int
        let height: Int
        let bytes: [UInt8]
    }

    private enum RasterError: Error {
        case missingContext
    }

    private func raster(_ view: some View, width: Int, height: Int,
                        scale: CGFloat) throws -> Raster {
        let renderer = ImageRenderer(content: view.frame(width: CGFloat(width),
                                                          height: CGFloat(height)))
        renderer.scale = scale
        let rendered = try XCTUnwrap(renderer.uiImage, "renderer produced no output")
        let bitmap = try XCTUnwrap(rendered.cgImage, "renderer output has no bitmap")
        var bytes = [UInt8](repeating: 0, count: bitmap.width * bitmap.height * 4)
        let space = try XCTUnwrap(CGColorSpace(name: CGColorSpace.sRGB))
        try bytes.withUnsafeMutableBytes { buffer in
            guard let context = CGContext(data: buffer.baseAddress,
                                          width: bitmap.width, height: bitmap.height,
                                          bitsPerComponent: 8, bytesPerRow: bitmap.width * 4,
                                          space: space,
                                          bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue)
            else { throw RasterError.missingContext }
            context.draw(bitmap, in: CGRect(x: 0, y: 0,
                                            width: bitmap.width, height: bitmap.height))
        }
        return Raster(width: bitmap.width, height: bitmap.height, bytes: bytes)
    }

    /// The live production scene view, sampled at one phase.
    private func scene(elapsed: Double, night: Bool = false,
                       reduceMotion: Bool = false) throws -> Raster {
        let view = RanchEnvironment(night: night, scroll: 0, maxScroll: 0,
                                    elapsed: elapsed, reduceMotion: reduceMotion)
        return try raster(view, width: 390, height: 640, scale: 2)
    }

    /// One ranch plane drawn by the same painter call the scene view makes.
    private func plane(_ plane: RanchPlane, elapsed: Double, night: Bool = false,
                       scale: CGFloat = 2) throws -> Raster {
        let view = Canvas { context, size in
            context.scaleBy(x: size.width/390, y: size.height/640)
            let painter = RanchPainter(night: night, elapsed: elapsed)
            painter.draw(plane, &context, left: 0)
        }
        return try raster(view, width: 390, height: 640, scale: scale)
    }

    /// Pixels differing by more than `tolerance` on any channel between two
    /// rasters, inside an art-space rectangle (scaled by `scale`).
    private func changedPixels(_ one: Raster, _ two: Raster,
                               x: ClosedRange<Double>, y: ClosedRange<Double>,
                               tolerance: Int = 8, scale: Int = 2) -> Int {
        XCTAssertEqual(one.width, two.width)
        XCTAssertEqual(one.height, two.height)
        let x0 = Int((x.lowerBound * Double(scale)).rounded())
        let x1 = Int((x.upperBound * Double(scale)).rounded())
        let y0 = Int((y.lowerBound * Double(scale)).rounded())
        let y1 = Int((y.upperBound * Double(scale)).rounded())
        var changed = 0
        for row in y0..<y1 {
            for column in x0..<x1 {
                let index = (row * one.width + column) * 4
                if (0..<4).contains(where: { abs(Int(one.bytes[index+$0]) - Int(two.bytes[index+$0])) > tolerance }) {
                    changed += 1
                }
            }
        }
        return changed
    }

    /// Horizontal extent of drawn (non-transparent) geometry inside an
    /// art-space window, returned in art points.
    private func drawnExtent(_ raster: Raster, x: ClosedRange<Double>,
                             y: ClosedRange<Double>, scale: Int = 2) -> (minX: Double, maxX: Double) {
        let x0 = Int((x.lowerBound * Double(scale)).rounded())
        let x1 = Int((x.upperBound * Double(scale)).rounded())
        let y0 = Int((y.lowerBound * Double(scale)).rounded())
        let y1 = Int((y.upperBound * Double(scale)).rounded())
        var minX = Int.max
        var maxX = Int.min
        for row in y0..<y1 {
            for column in x0..<x1 {
                if raster.bytes[(row * raster.width + column) * 4 + 3] > 8 {
                    minX = min(minX, column)
                    maxX = max(maxX, column)
                }
            }
        }
        return (Double(minX) / Double(scale), Double(maxX) / Double(scale))
    }

    /// The single fully visible tree of the barnTrees plane authored at
    /// (238, 258, scale 0.5): canopy travel, trunk strip, window.
    private let canopyWindow = 212.0...264.0
    private let canopyBand = 238.0...280.0
    private let isolatedCanopyWindow = 200.0...280.0
    private let isolatedCanopyBand = 244.0...272.0
    private let groundedTrunkStrip = 236.0...240.0
    private let groundedTrunkRows = 274.0...277.0

    // MARK: Sampled phases through the live scene view

    func testLiveSceneCanopyGeometryChangesAcrossSampledPhases() throws {
        let phases: [Double] = [0, 1.3, 2.6, 3.9]
        let frames = try phases.map { try scene(elapsed: $0) }
        var measured: [String] = []
        // Opposite phases of the gust cycle: the canopy must be visibly
        // somewhere else. A static canopy yields zero changed pixels.
        for (first, second) in [(0, 2), (1, 3)] {
            let changed = changedPixels(frames[first], frames[second],
                                        x: canopyWindow, y: canopyBand)
            measured.append("p\(phases[first])->p\(phases[second]) changed=\(changed)")
            XCTAssertGreaterThanOrEqual(changed, 120,
                "canopy leaf clusters must visibly change across sampled phases (\(measured.last ?? ""))")
        }
        // The trunk bottom is below every leaf ellipse excursion: it must
        // stay perfectly still — the tree is swaying, not sliding.
        for frame in frames.dropFirst() {
            let trunkDrift = changedPixels(frames[0], frame,
                                           x: groundedTrunkStrip, y: groundedTrunkRows, tolerance: 0)
            XCTAssertEqual(trunkDrift, 0, "grounded trunk moved across sampled phases")
        }
        // Day and Night both animate (the wind is lighting-independent).
        let nightChanged = changedPixels(try scene(elapsed: 0, night: true),
                                         try scene(elapsed: 2.6, night: true),
                                         x: canopyWindow, y: canopyBand)
        XCTAssertGreaterThanOrEqual(nightChanged, 120, "night canopy must animate too")
        print("WIND scene-phases \(measured.joined(separator: " ")) night=\(nightChanged)")
    }

    func testCanopySilhouetteTravelsAndTrunkDoesNot() throws {
        // One full gust cycle, densely sampled: the canopy silhouette must
        // travel; a frozen implementation gives the same extents every time.
        var leftMin = Double.infinity
        var leftMax = -Double.infinity
        var rightMin = Double.infinity
        var rightMax = -Double.infinity
        for step in 0..<16 {
            let phase = RanchWind.period * Double(step) / 16
            let frame = try plane(.barnTrees, elapsed: phase)
            let extent = drawnExtent(frame, x: isolatedCanopyWindow, y: isolatedCanopyBand)
            leftMin = min(leftMin, extent.minX)
            leftMax = max(leftMax, extent.minX)
            rightMin = min(rightMin, extent.maxX)
            rightMax = max(rightMax, extent.maxX)
        }
        let swing = (leftMax - leftMin) + (rightMax - rightMin)
        XCTAssertGreaterThanOrEqual(swing, 2.0,
            "canopy silhouette must travel across the gust cycle (measured \(swing) points)")
        let first = try plane(.barnTrees, elapsed: 0)
        let second = try plane(.barnTrees, elapsed: 2.6)
        let trunkDrift = changedPixels(first, second, x: groundedTrunkStrip,
                                       y: groundedTrunkRows, tolerance: 0)
        XCTAssertEqual(trunkDrift, 0, "the trunk strip must be byte-stable across phases")
        print("WIND silhouette swing=\(swing)pt left=[\(leftMin),\(leftMax)] right=[\(rightMin),\(rightMax)]")
    }

    func testGrassRespondsToTheSharedGustThroughThePainter() throws {
        let first = try plane(.foreground, elapsed: 0)
        let second = try plane(.foreground, elapsed: 2.6)
        let changed = changedPixels(first, second, x: 0...390, y: 300...640, tolerance: 8)
        XCTAssertGreaterThanOrEqual(changed, 100,
            "grass bending must respond to the shared gust (measured \(changed) pixels)")
        print("WIND grass changed=\(changed)")
    }

    // MARK: Controls — Reduce Motion, determinism, grounded trunks

    func testReduceMotionKeepsTheProductionSceneIntentionalAndStatic() throws {
        for night in [false, true] {
            let frozenA = try scene(elapsed: 0, night: night, reduceMotion: true)
            let frozenB = try scene(elapsed: 2.6, night: night, reduceMotion: true)
            XCTAssertEqual(frozenA.bytes, frozenB.bytes,
                "Reduce Motion must render an identical static scene (night=\(night))")
            let animated = try scene(elapsed: 2.6, night: night)
            XCTAssertNotEqual(frozenA.bytes, animated.bytes,
                "the animated scene must differ from the static fallback (night=\(night))")
        }
    }

    func testSampledPhasesAreDeterministicAcrossIndependentInstances() throws {
        let one = try scene(elapsed: 4.1)
        let two = try scene(elapsed: 4.1)
        XCTAssertEqual(one.bytes, two.bytes,
            "the same phase must render byte-identically — no per-frame randomness")
        let planeOne = try plane(.barnTrees, elapsed: 4.1)
        let planeTwo = try plane(.barnTrees, elapsed: 4.1)
        XCTAssertEqual(planeOne.bytes, planeTwo.bytes,
            "painter sampling of one phase must be deterministic")
    }

    // MARK: Ambient policy ladder

    func testAmbientPolicyDisablesSceneMotionForPowerAndThermalFallbacks() {
        for thermal in [ProcessInfo.ThermalState.nominal, .fair] {
            XCTAssertTrue(HerdAmbientPolicy.ambientMotionAllowed(lowPower: false, thermal: thermal),
                "ambient motion stays on for \(thermal)")
        }
        for thermal in [ProcessInfo.ThermalState.serious, .critical] {
            XCTAssertFalse(HerdAmbientPolicy.ambientMotionAllowed(lowPower: false, thermal: thermal),
                "ambient motion must fall back to static for \(thermal)")
            XCTAssertFalse(HerdAmbientPolicy.ambientMotionAllowed(lowPower: true, thermal: thermal))
        }
        for thermal in [ProcessInfo.ThermalState.nominal, .fair] {
            XCTAssertFalse(HerdAmbientPolicy.ambientMotionAllowed(lowPower: true, thermal: thermal),
                "Low Power must prefer the static scene")
        }
    }

    // MARK: Wind contract — perceptible, bounded, coherent, deterministic

    func testWindContractIsPerceptibleSmoothAndCoherent() {
        var canopy: [Double] = []
        var time = 0.0
        while time <= RanchWind.period {
            canopy.append(RanchWind.sway(elapsed: time, anchor: 0,
                                         amplitude: RanchWind.canopyAmplitude))
            time += 0.01
        }
        let canopyTravel = (canopy.max() ?? 0) - (canopy.min() ?? 0)
        XCTAssertGreaterThanOrEqual(canopyTravel, 4.5,
            "canopy travel must stay clearly perceptible at phone scale (measured \(canopyTravel) points)")

        var grass: [Double] = []
        var biggestStep = 0.0
        time = 0.0
        while time <= RanchWind.period {
            grass.append(RanchWind.grassBend(elapsed: time, worldX: 0))
            time += 0.01
        }
        let grassTravel = (grass.max() ?? 0) - (grass.min() ?? 0)
        XCTAssertGreaterThanOrEqual(grassTravel, 3.4,
            "grass deflection must stay clearly perceptible (measured \(grassTravel) points)")

        // The painter only advances on the one scene clock, so a 100 ms tick
        // must never jump the foliage: bounded per-tick steps, smooth motion.
        time = 0.0
        while time < RanchWind.period {
            let here = RanchWind.sway(elapsed: time, anchor: 0.7, amplitude: RanchWind.canopyAmplitude)
            let next = RanchWind.sway(elapsed: time + 0.1, anchor: 0.7, amplitude: RanchWind.canopyAmplitude)
            biggestStep = max(biggestStep, abs(next - here))
            time += 0.1
        }
        XCTAssertLessThanOrEqual(biggestStep, 0.6,
            "one 100 ms scene tick must not teleport the canopy (measured \(biggestStep) points)")

        // Coherent grass: one travelling gust-front, neighbours move together.
        var widestNeighbourStep = 0.0
        time = 0.0
        while time < RanchWind.period {
            let here = RanchWind.grassBend(elapsed: time, worldX: 0)
            let next = RanchWind.grassBend(elapsed: time, worldX: 9)
            widestNeighbourStep = max(widestNeighbourStep, abs(next - here))
            time += 0.02
        }
        XCTAssertLessThanOrEqual(widestNeighbourStep, 0.55,
            "adjacent tufts must ride one coherent gust (measured \(widestNeighbourStep) points)")
        XCTAssertNotEqual(RanchWind.grassBend(elapsed: 0, worldX: 0),
                          RanchWind.grassBend(elapsed: 0, worldX: 200),
                          "the gust must travel across the field, not pulse uniformly")

        // Deterministic anchors: per-tree variation is a pure function of the
        // world position and repeats with the panorama tiles.
        XCTAssertEqual(RanchWind.anchor(97.5), RanchWind.anchor(97.5))
        XCTAssertEqual(RanchWind.anchor(97.5), RanchWind.anchor(97.5 + 390))
        XCTAssertNotEqual(RanchWind.anchor(97.5), RanchWind.anchor(200))
        XCTAssertNotEqual(RanchWind.anchor(0), RanchWind.anchor(195))
        print("WIND canopyTravel=\(canopyTravel) grassTravel=\(grassTravel) tickStep=\(biggestStep) neighbour=\(widestNeighbourStep)")
    }

    // MARK: Production wiring pins

    func testProductionSceneGateAndSingleTimingOwnerWiring() throws {
        let environment = try pinnedSource("RanchEnvironment")
        XCTAssertTrue(environment.contains("HerdAmbientPolicy.ambientMotionAllowed(lowPower:lowPower,thermal:thermalState)"),
            "the scene must consult the ambient policy for power/thermal fallbacks")
        XCTAssertTrue(environment.contains("ambientFrozen?0:elapsed"),
            "the painter must sample elapsed only when ambient motion is allowed")
        XCTAssertTrue(environment.contains(".NSProcessInfoPowerStateDidChange"))
        XCTAssertTrue(environment.contains("ProcessInfo.thermalStateDidChangeNotification"))
        XCTAssertFalse(environment.contains("Task{"),
            "the scene view must not start its own task")
        XCTAssertFalse(environment.contains("TimelineView"),
            "the scene view must not introduce a second timing owner")
        XCTAssertFalse(environment.contains("Timer"),
            "the scene view must not own a timer")
        XCTAssertFalse(environment.contains("DispatchQueue"))
        // The scene gate itself is unchanged: the existing HerdView lifecycle
        // (active/visible/connected => clock runs and motion is allowed)
        // still feeds both the clock and the wind.
        let herd = try pinnedSource("HerdView")
        XCTAssertTrue(herd.contains("RanchEnvironment(night:lighting.night"),
            "HerdView must keep constructing the production scene")
        XCTAssertTrue(herd.contains("elapsed:elapsed"))
        XCTAssertTrue(herd.contains("reduceMotion:!motionEnabled"),
            "the ambient freeze must stay tied to the scene gate")
    }

    private func pinnedSource(_ name: String) throws -> String {
        let url = try XCTUnwrap(Bundle(for: Self.self).url(forResource: name + ".swift",
                                                           withExtension: "txt"))
        return try String(contentsOf: url, encoding: .utf8).filter { !$0.isWhitespace }
    }
}
