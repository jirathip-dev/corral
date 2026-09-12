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
    /// (238, 258, scale 0.5): canopy travel, trunk strip, window. The window
    /// starts at x=218 so the barn (right edge x=216) can never pin the
    /// measured left edge.
    private let canopyWindow = 212.0...264.0
    private let canopyBand = 238.0...280.0
    private let isolatedCanopyWindow = 218.0...280.0
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
        XCTAssertGreaterThanOrEqual(swing, 3.0,
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

    // MARK: #460 sky drift — positional motion through the live scene view

    /// Per-pixel luminance of a raster. The sky plane fills an opaque
    /// gradient across the whole 390x640 world, so every sampled pixel in
    /// the sky band has a defined color and no alpha weighting is needed.
    private func luminance(_ raster: Raster) -> (pixels: [UInt8], width: Int, height: Int) {
        var out = [UInt8](repeating: 0, count: raster.width * raster.height)
        for index in 0..<out.count {
            let base = index * 4
            let r = Int(raster.bytes[base])
            let g = Int(raster.bytes[base + 1])
            let b = Int(raster.bytes[base + 2])
            out[index] = UInt8((2126 * r + 7152 * g + 722 * b) / 10000)
        }
        return (out, raster.width, raster.height)
    }

    private func region(_ x: ClosedRange<Double>, _ y: ClosedRange<Double>,
                        scale: Int) -> (x0: Int, x1: Int, y0: Int, y1: Int) {
        (Int((x.lowerBound * Double(scale)).rounded()),
         Int((x.upperBound * Double(scale)).rounded()),
         Int((y.lowerBound * Double(scale)).rounded()),
         Int((y.upperBound * Double(scale)).rounded()))
    }

    /// Sum of |base(x) − moved(x+dx)| over an art-space region: the later
    /// frame equals the earlier one translated RIGHT by `dx` where this
    /// reaches its minimum. Brightness-only changes leave the minimum at 0.
    private func shiftDelta(_ base: [UInt8], _ moved: [UInt8], width: Int, dx: Int,
                            x: ClosedRange<Double>, y: ClosedRange<Double>, scale: Int) -> Int {
        let r = region(x, y, scale: scale)
        var sum = 0
        for row in r.y0..<r.y1 {
            for column in r.x0..<r.x1 {
                let other = column + dx
                guard other >= 0, other < width else { continue }
                sum += abs(Int(base[row * width + column]) - Int(moved[row * width + other]))
            }
        }
        return sum
    }

    private func bestShift(_ base: [UInt8], _ moved: [UInt8], width: Int,
                           search: ClosedRange<Int>, x: ClosedRange<Double>,
                           y: ClosedRange<Double>, scale: Int) -> (shift: Int, delta: Int) {
        var best = (shift: search.lowerBound, delta: Int.max)
        for dx in search {
            let delta = shiftDelta(base, moved, width: width, dx: dx, x: x, y: y, scale: scale)
            if delta < best.delta { best = (dx, delta) }
        }
        return best
    }

    /// Day clouds are the only moving content in the sky band, so the
    /// whole-band luminance is the cloud field; its best alignment across a
    /// real-time gap is the measured drift, and the static overlay (dx 0)
    /// must be worse — a brightness-only change would minimise at 0.
    func testDayCloudsDriftPositionallyThroughTheProductionScene() throws {
        let scale = 2
        let gap = 12.0
        let window = 0.0...390.0
        let skyBand = 0.0...140.0
        let first = luminance(try scene(elapsed: 0))
        let later = luminance(try scene(elapsed: gap))
        let expected = Int((RanchSky.cloudSpeed * gap * Double(scale)).rounded())
        // The cloud field's real-time travel is the shared drift PLUS the
        // bounded per-tile bob (≤ 1.5 pt amplitude, ≤ 3 pt peak-to-peak), so
        // the sampled alignment lands within 4 pt of the nominal drift.
        let best = bestShift(first.pixels, later.pixels, width: first.width,
                             search: (expected - 30)...(expected + 30),
                             x: window, y: skyBand, scale: scale)
        let atZero = shiftDelta(first.pixels, later.pixels, width: first.width, dx: 0,
                                x: window, y: skyBand, scale: scale)
        let measuredRate = Double(best.shift) / (Double(scale) * gap)
        XCTAssertLessThanOrEqual(abs(best.shift - expected), 8,
            "day clouds must drift at ~\(RanchSky.cloudSpeed) pt/s: expected a \(expected) px shift after \(gap)s, best match \(best.shift) px")
        XCTAssertGreaterThanOrEqual(measuredRate, 2.0,
            "the measured cloud travel must stay clearly perceptible (measured \(measuredRate) pt/s)")
        XCTAssertLessThanOrEqual(measuredRate, 3.0,
            "the measured cloud travel must stay gentle (measured \(measuredRate) pt/s)")
        XCTAssertLessThan(best.delta, atZero,
            "the cloud change must be positional: the drifted match (\(best.delta)) must beat the static overlay (\(atZero))")
        print("SKY clouds gap=\(gap)s bestShift=\(best.shift)px expected=\(expected) rate=\(measuredRate)pt/s delta=\(best.delta) identity=\(atZero)")
    }

    /// The night star field is measured as a luminance mask (the gradient,
    /// clouds and Milky Way stay far below the threshold in the night
    /// palette), so the assertion isolates star POSITIONS: the later mask
    /// aligns with the earlier one shifted by the shared celestial drift,
    /// while the brightness-only twinkle cannot produce that alignment.
    func testNightStarFieldMovesPositionallyNotJustTwinkle() throws {
        let scale = 2
        let gap = 20.0
        let window = 20.0...280.0
        let skyBand = 0.0...145.0
        let first = luminance(try scene(elapsed: 0, night: true))
        let later = luminance(try scene(elapsed: gap, night: true))
        let starA = first.pixels.map { $0 > 85 ? UInt8(1) : UInt8(0) }
        let starB = later.pixels.map { $0 > 85 ? UInt8(1) : UInt8(0) }
        let lit = region(window, skyBand, scale: scale)
        var litCount = 0
        for row in lit.y0..<lit.y1 {
            for column in lit.x0..<lit.x1 where starA[row * first.width + column] == 1 { litCount += 1 }
        }
        XCTAssertGreaterThanOrEqual(litCount, 80,
            "the night sky band must contain sampled stars (measured \(litCount) lit pixels)")
        let expected = Int((RanchSky.celestialSpeed * gap * Double(scale)).rounded())
        let best = bestShift(starA, starB, width: first.width,
                             search: (expected - 25)...(expected + 25),
                             x: window, y: skyBand, scale: scale)
        let atZero = shiftDelta(starA, starB, width: first.width, dx: 0,
                                x: window, y: skyBand, scale: scale)
        XCTAssertLessThanOrEqual(abs(best.shift - expected), 3,
            "the seeded star field must MOVE by the celestial drift: expected \(expected) px after \(gap)s, best match \(best.shift) px (stars=\(litCount))")
        XCTAssertLessThan(best.delta * 2, atZero,
            "star motion must be positional, not a brightness re-roll: drifted mismatch \(best.delta) vs static \(atZero)")
        print("SKY stars gap=\(gap)s bestShift=\(best.shift)px expected=\(expected) lit=\(litCount) mismatch=\(best.delta) identity=\(atZero)")
    }

    /// The Milky Way band shares the celestial rate with the stars and
    /// dominates this window, so the band window's best alignment is the
    /// band's own measured positional drift (the night clouds move at a
    /// different rate and cannot explain it).
    func testNightMilkyWayBandMovesCoherentlyWithTheCelestialDrift() throws {
        let scale = 2
        let gap = 20.0
        let window = 60.0...280.0
        let skyBand = 0.0...145.0
        let first = luminance(try scene(elapsed: 0, night: true))
        let later = luminance(try scene(elapsed: gap, night: true))
        let expected = Int((RanchSky.celestialSpeed * gap * Double(scale)).rounded())
        let best = bestShift(first.pixels, later.pixels, width: first.width,
                             search: max(1, expected - 20)...(expected + 20),
                             x: window, y: skyBand, scale: scale)
        let atZero = shiftDelta(first.pixels, later.pixels, width: first.width, dx: 0,
                                x: window, y: skyBand, scale: scale)
        XCTAssertLessThanOrEqual(abs(best.shift - expected), 5,
            "the Milky Way must ride the shared celestial drift: expected \(expected) px after \(gap)s, best match \(best.shift) px")
        XCTAssertLessThan(best.delta, atZero,
            "the band change must be positional: drifted match \(best.delta) must beat the static overlay \(atZero)")
        print("SKY band gap=\(gap)s bestShift=\(best.shift)px expected=\(expected) delta=\(best.delta) identity=\(atZero)")
    }

    /// One 100 ms scene tick must not teleport the sky, while 30 s of real
    /// time must visibly move it; the night field stays byte-deterministic
    /// across independent instances (same elapsed, same seeded identities).
    func testSkyMotionIsBoundedPerTickAndDeterministic() throws {
        let first = try scene(elapsed: 0)
        let tick = try scene(elapsed: 0.1)
        let jumped = changedPixels(first, tick, x: 0...390, y: 0...200, tolerance: 8)
        XCTAssertLessThanOrEqual(jumped, 2000,
            "one 100 ms tick must not jump the sky (measured \(jumped) changed pixels)")
        let travelled = changedPixels(first, try scene(elapsed: 30),
                                      x: 0...390, y: 0...140, tolerance: 8)
        XCTAssertGreaterThanOrEqual(travelled, 20000,
            "30 s of real time must move the sky visibly at phone scale (measured \(travelled) changed pixels)")
        XCTAssertEqual(try scene(elapsed: 6.3, night: true).bytes,
                       try scene(elapsed: 6.3, night: true).bytes,
                       "the night sky must render byte-identically for one elapsed — no per-frame randomness")
        print("SKY tick100ms=\(jumped) travel30s=\(travelled)")
    }

    /// Reduce Motion (and the Low Power / thermal fallback that feeds the
    /// same gate) must render the scene's own intentional static
    /// composition — the elapsed-0 frame — not an arbitrary frozen phase.
    func testSkyFrozenCompositionIsTheIntentionalStaticScene() throws {
        XCTAssertEqual(try scene(elapsed: 30, night: true, reduceMotion: true).bytes,
                       try scene(elapsed: 0, night: true).bytes,
                       "frozen night sky must equal the intentional elapsed-0 composition")
        XCTAssertEqual(try scene(elapsed: 25, reduceMotion: true).bytes,
                       try scene(elapsed: 0).bytes,
                       "frozen day sky must equal the intentional elapsed-0 composition")
    }

    /// Production wiring pins for the sky drift: both fields sample the one
    /// shared clock through the bounded sky math (no second timing owner),
    /// the moon stays a fixed landmark, and the drift rates stay in the
    /// perceptible-but-gentle band the issue requires.
    func testSkyDriftWiringAndTimingOwnerPins() throws {
        let environment = try pinnedSource("RanchEnvironment")
        XCTAssertTrue(environment.contains("RanchSky.cloudDrift(elapsed)"),
            "the clouds must sample the shared clock through RanchSky")
        XCTAssertTrue(environment.contains("RanchSky.celestialDrift(elapsed)"),
            "stars and the Milky Way must sample the shared celestial drift")
        XCTAssertTrue(environment.contains("RanchSky.bandCopyRange(elapsed:elapsed,left:left)"),
            "the Milky Way wrap must follow the drift and the visible window")
        XCTAssertTrue(environment.contains("ellipse(306,56,16,16)"),
            "the moon must stay a fixed landmark")
        XCTAssertFalse(environment.contains("TimelineView"))
        XCTAssertFalse(environment.contains("Timer"))
        XCTAssertFalse(environment.contains("DispatchQueue"))
        XCTAssertGreaterThanOrEqual(RanchSky.cloudSpeed, 2.0,
            "cloud drift must stay clearly perceptible")
        XCTAssertLessThanOrEqual(RanchSky.cloudSpeed, 4.0,
            "cloud drift must stay gentle")
        XCTAssertGreaterThanOrEqual(RanchSky.celestialSpeed, 0.5,
            "night-sky drift must stay perceptible")
        XCTAssertLessThanOrEqual(RanchSky.celestialSpeed, 1.2,
            "night-sky drift must stay slow")
        XCTAssertNotEqual(RanchSky.cloudSpeed, RanchSky.celestialSpeed,
            "cloud and celestial drift must remain distinguishable")
        XCTAssertEqual(RanchSky.cloudDrift(4), 4 * RanchSky.cloudSpeed)
        XCTAssertEqual(RanchSky.celestialDrift(3), 3 * RanchSky.celestialSpeed)
    }

    private func pinnedSource(_ name: String) throws -> String {
        let url = try XCTUnwrap(Bundle(for: Self.self).url(forResource: name + ".swift",
                                                           withExtension: "txt"))
        return try String(contentsOf: url, encoding: .utf8).filter { !$0.isWhitespace }
    }
}
