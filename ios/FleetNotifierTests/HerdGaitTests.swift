import XCTest
import SwiftUI
@testable import FleetNotifier

/// #448 acceptance through the ACTUAL procedural renderer
/// (`HerdArt.drawing`): the four leg inks and their hoof inks are sampled
/// across gait phases and must travel coordinated diagonal arcs around the
/// approved rest angles, while every static / reduced-motion / disconnected
/// pose stays byte-identical to the approved fixed-angle rendering. Clock
/// ticks and source strings are deliberately not evidence here.
final class HerdGaitTests: XCTestCase {
    private let art = HerdArt()
    private let witness = HorseIdentity(name: "herd-gait-witness")
    /// The approved fixed working pose, indexed by leg ink part.
    private let workingRest = [28.0, -26.0, -26.0, 24.0]
    private let legX = [84.0, 78.0, 43.0, 51.0]

    private var belly: Double { [66.0, 69, 72][witness.breed] }
    private var legHeight: Double { [34.0, 31, 28][witness.breed] }

    private func drawn(_ pose: HorsePose, _ gait: HorseGait = .standstill) -> [HorseInk] {
        art.drawing(witness, pose: pose, gait: gait)
    }

    private func ink(_ part: String, in inks: [HorseInk]) throws -> HorseInk {
        try XCTUnwrap(inks.first { $0.part == part }, "renderer did not emit ink part \(part)")
    }

    private func hoofCenter(leg index: Int, pose: HorsePose, gait: HorseGait) throws -> CGPoint {
        let hoof = try ink("leg-\(index)-hoof", in: drawn(pose, gait))
        return CGPoint(x: hoof.path.boundingRect.midX, y: hoof.path.boundingRect.midY)
    }

    /// Where the hoof ink must sit for a given leg angle: the hoof centre
    /// offset `height - 2.5` below the hip pivot (x, belly-6), rotated by
    /// `rest + diagonal swing`. The working pose's approved -4-degree body
    /// lean is a rigid rotation, so it preserves every distance between two
    /// sampled leg positions; absolute checks below use the lean-free
    /// standing pose and working samples are compared by delta magnitude.
    private func expectedHoof(leg index: Int, pose: HorsePose, gait: HorseGait) -> CGPoint {
        let pivot = CGPoint(x: legX[index], y: belly - 6)
        let rest = pose == .working ? workingRest[index] : 0
        let direction = index == 0 || index == 3 ? 1.0 : -1.0
        let swing = gait.isStepping && (pose == .working || pose == .stand)
            ? gait.swing * sin(2 * .pi * gait.phase) : 0
        let angle = CGFloat(rest + direction * swing) * .pi / 180
        return CGPoint(x: pivot.x - (legHeight - 2.5) * sin(angle),
                       y: pivot.y + (legHeight - 2.5) * cos(angle))
    }

    private func distance(_ a: CGPoint, _ b: CGPoint) -> Double {
        hypot(Double(a.x - b.x), Double(a.y - b.y))
    }

    private func assertSameDrawing(_ left: [HorseInk], _ right: [HorseInk],
                                   _ label: String = "",
                                   file: StaticString = #filePath, line: UInt = #line) {
        XCTAssertEqual(left.count, right.count, "\(label) ink count", file: file, line: line)
        for (index, pair) in zip(left, right).enumerated() {
            XCTAssertEqual(pair.0.path, pair.1.path, "\(label) ink \(index) path", file: file, line: line)
            XCTAssertEqual(pair.0.color, pair.1.color, "\(label) ink \(index) color", file: file, line: line)
            XCTAssertEqual(pair.0.opacity, pair.1.opacity, "\(label) ink \(index) opacity", file: file, line: line)
            XCTAssertEqual(pair.0.stroke, pair.1.stroke, "\(label) ink \(index) stroke", file: file, line: line)
            XCTAssertEqual(pair.0.part, pair.1.part, "\(label) ink \(index) part", file: file, line: line)
        }
    }

    private func assertDifferentDrawing(_ left: [HorseInk], _ right: [HorseInk],
                                        _ label: String = "",
                                        file: StaticString = #filePath, line: UInt = #line) {
        let differs = left.count != right.count || zip(left, right).contains { $0.0.path != $0.1.path }
        XCTAssertTrue(differs, "\(label) expected stepping gait to change rendered geometry", file: file, line: line)
    }

    private func horse(_ state: AgentState, disconnected: Bool = false) -> HerdHorse {
        HerdHorse(agent: Agent(agentId: "gait-witness-\(state.rawValue)", state: state),
                  hostProfileID: nil, hostName: nil, disconnected: disconnected)
    }

    // MARK: renderer geometry

    /// Approved poses render byte-identical under a standstill gait; an
    /// explicit stepping gait changes exactly the two moving poses.
    func testApprovedStaticPosesStayByteIdenticalWithStandstillGait() throws {
        for pose in [HorsePose.stand, .working, .blocked, .done, .unknown, .graze, .alertStatic] {
            assertSameDrawing(drawn(pose), drawn(pose, .standstill), "\(pose)")
        }
        assertDifferentDrawing(drawn(.working), drawn(.working, HorseGait(phase: 0.25, swing: 22)), "working")
        assertDifferentDrawing(drawn(.stand), drawn(.stand, HorseGait(phase: 0.25, swing: 8)), "stand")
        // The approved frozen running pose is the phase-0 sample: the cycle
        // passes through it and leaves it untouched when the swing is zero.
        assertSameDrawing(drawn(.working), drawn(.working, HorseGait(phase: 0, swing: 22)), "working phase 0")
    }

    /// Every sampled phase must land each leg-0..3 hoof ink exactly on its
    /// diagonal hip arc (standing pose, no body lean), and distinct phases
    /// must render distinct leg positions through the real renderer.
    func testStandGaitRidesTheHipArcAtEverySampledPhase() throws {
        var centers: [CGPoint] = []
        for sample in 0..<8 {
            let gait = HorseGait(phase: Double(sample) / 8, swing: 8)
            for index in 0..<4 {
                let measured = try hoofCenter(leg: index, pose: .stand, gait: gait)
                let expected = expectedHoof(leg: index, pose: .stand, gait: gait)
                XCTAssertEqual(Double(measured.x), Double(expected.x), accuracy: 0.05,
                               "leg \(index) at phase \(sample)/8 must ride its hip arc (\(measured) vs \(expected))")
                XCTAssertEqual(Double(measured.y), Double(expected.y), accuracy: 0.05,
                               "leg \(index) at phase \(sample)/8 must ride its hip arc (\(measured) vs \(expected))")
                centers.append(measured)
            }
        }
        let xs = centers.map(\.x)
        let top = try XCTUnwrap(xs.max())
        let bottom = try XCTUnwrap(xs.min())
        let spread = top - bottom
        XCTAssertGreaterThan(spread, 1.5, "the four legs must sweep a visible stride across the cycle")
        let distinct = Set(centers.map { Int($0.x * 100) })
        XCTAssertGreaterThan(distinct.count, 8, "sampled phases must render distinct leg positions")
    }

    /// The working trot moves every hoof on the same approved arc: the
    /// antiphase travel magnitude matches the modelled hip arc (the fixed
    /// -4-degree body lean is rigid, so it cannot change it), and the
    /// rendered phases are distinct.
    func testWorkingGaitMovesHoovesAlongTheApprovedArc() throws {
        let stepOut = HorseGait(phase: 0.25, swing: 22)
        let stepBack = HorseGait(phase: 0.75, swing: 22)
        var centers: [CGPoint] = []
        for index in 0..<4 {
            let out = try hoofCenter(leg: index, pose: .working, gait: stepOut)
            let back = try hoofCenter(leg: index, pose: .working, gait: stepBack)
            let modelled = distance(expectedHoof(leg: index, pose: .working, gait: stepOut),
                                    expectedHoof(leg: index, pose: .working, gait: stepBack))
            let measuredTravel = distance(out, back)
            XCTAssertEqual(measuredTravel, modelled, accuracy: 0.05,
                           "leg \(index) travel must match the approved gait arc (\(measuredTravel) vs \(modelled))")
            XCTAssertGreaterThan(measuredTravel, 1.5, "leg \(index) must visibly travel between antiphase samples")
            centers += [out, back]
        }
        XCTAssertGreaterThan(Set(centers.map { Int($0.x * 100) }).count, 4,
                             "antiphase samples must render distinct working-pose leg positions")
        for sample in [1, 2, 3, 5, 6, 7] {
            assertDifferentDrawing(drawn(.working),
                                   drawn(.working, HorseGait(phase: Double(sample) / 8, swing: 22)),
                                   "phase \(sample)/8 vs frozen pose")
        }
    }

    /// Opposing diagonals step in opposite directions (legs 0+3 together,
    /// legs 1+2 against them) and every hoof stays anchored to its leg arc.
    func testOpposingDiagonalPairsStepOppositeAndHoovesStayAnchored() throws {
        let stepOut = HorseGait(phase: 0.25, swing: 22)
        let stepBack = HorseGait(phase: 0.75, swing: 22)
        var travel: [Double] = []
        for index in 0..<4 {
            let out = try hoofCenter(leg: index, pose: .stand, gait: stepOut)
            let back = try hoofCenter(leg: index, pose: .stand, gait: stepBack)
            travel.append(Double(out.x - back.x))
            let pivot = CGPoint(x: legX[index], y: belly - 6)
            for (label, point) in [("forward", out), ("back", back)] {
                XCTAssertEqual(distance(point, pivot), legHeight - 2.5, accuracy: 0.05,
                               "leg \(index) hoof must stay anchored on its arc (\(label))")
            }
        }
        XCTAssertGreaterThan(travel[0] * travel[3], 0, "legs 0 and 3 are one diagonal pair")
        XCTAssertLessThan(travel[0] * travel[1], 0, "legs 0 and 1 are opposing diagonals")
        XCTAssertLessThan(travel[0] * travel[2], 0, "legs 0 and 2 are opposing diagonals")
        for (index, value) in travel.enumerated() {
            XCTAssertGreaterThan(abs(value), 1.5, "leg \(index) must visibly travel between antiphase samples")
        }
    }

    // MARK: phase contract

    /// The cycle is normalized and purely a function of its inputs: a full
    /// cycle later and repeatedly drawn samples render byte-identically.
    func testGaitCycleNormalizationAndDeterminismKeepEverySampleStable() throws {
        XCTAssertEqual(HorseGait(phase: 0.25, swing: 9), HorseGait(phase: 1.25, swing: 9))
        XCTAssertEqual(HorseGait(phase: 2.25, swing: 9), HorseGait(phase: 0.25, swing: 9))
        XCTAssertEqual(HorseGait(phase: -0.25, swing: 9), HorseGait(phase: 0.75, swing: 9))
        let gait = HorseGait(phase: 0.5, swing: 11)
        assertSameDrawing(drawn(.stand, gait), drawn(.stand, gait), "redraw")
        assertSameDrawing(drawn(.stand, gait), drawn(.stand, HorseGait(phase: 1.5, swing: 11)), "next cycle")
    }

    /// The model derives gait from the shared clock's `elapsed` only, with
    /// state-appropriate amplitudes and standstill everywhere static.
    func testHerdHorseGaitDerivationIsStateAppropriateAndMotionSafe() throws {
        let working = horse(.working)
        let workingGait = working.gait(elapsed: 3.0, reduceMotion: false)
        XCTAssertTrue(workingGait.isStepping)
        XCTAssertEqual(workingGait.swing, 22, accuracy: 0.0001)
        XCTAssertEqual(working.gait(elapsed: 3.0, reduceMotion: false), workingGait, "deterministic")
        let nextCycle = working.gait(elapsed: 3.0 + 2.4, reduceMotion: false)
        XCTAssertEqual(nextCycle.phase, workingGait.phase, accuracy: 1e-9, "one cycle later")
        XCTAssertEqual(nextCycle.swing, workingGait.swing, accuracy: 0.0001)
        XCTAssertNotEqual(working.gait(elapsed: 4.2, reduceMotion: false), workingGait, "phase advances")
        XCTAssertEqual(working.gait(elapsed: 3.0, reduceMotion: true), .standstill)

        let idle = horse(.idle)
        XCTAssertEqual(idle.pose(elapsed: 55 - idle.identity.phase, reduceMotion: false), .graze)
        XCTAssertEqual(idle.gait(elapsed: 55 - idle.identity.phase, reduceMotion: false), .standstill,
                       "grazing stays deliberate, never stepping")
        XCTAssertEqual(idle.pose(elapsed: 40 - idle.identity.phase, reduceMotion: false), .stand)
        let idleGait = idle.gait(elapsed: 40 - idle.identity.phase, reduceMotion: false)
        XCTAssertTrue(idleGait.isStepping)
        XCTAssertEqual(idleGait.swing, 8, accuracy: 0.0001)
        XCTAssertEqual(idle.gait(elapsed: 40 - idle.identity.phase, reduceMotion: true), .standstill)

        for state in [AgentState.blocked, .done, .unknown] {
            XCTAssertEqual(horse(state).gait(elapsed: 2.0, reduceMotion: false), .standstill, "\(state)")
        }
        let stale = horse(.working, disconnected: true)
        XCTAssertEqual(stale.pose(elapsed: 2.0, reduceMotion: false), .unknown)
        XCTAssertEqual(stale.gait(elapsed: 2.0, reduceMotion: false), .standstill)
        XCTAssertEqual(working.gait(elapsed: .nan, reduceMotion: false), .standstill, "non-finite clock")
    }

    /// Idle stepping is deliberately gentler than the working trot, but it
    /// is still visible motion through the renderer.
    func testIdleSteppingIsGentlerThanTheWorkingTrotThroughTheRenderer() throws {
        let workingGait = horse(.working).gait(elapsed: 3.0, reduceMotion: false)
        let idle = horse(.idle)
        let idleGait = idle.gait(elapsed: 40 - idle.identity.phase, reduceMotion: false)
        XCTAssertGreaterThan(workingGait.swing, idleGait.swing)
        func travel(_ swing: Double) throws -> Double {
            let out = try hoofCenter(leg: 0, pose: .stand, gait: HorseGait(phase: 0.25, swing: swing))
            let back = try hoofCenter(leg: 0, pose: .stand, gait: HorseGait(phase: 0.75, swing: swing))
            return distance(out, back)
        }
        let workingTravel = try travel(workingGait.swing)
        let idleTravel = try travel(idleGait.swing)
        XCTAssertGreaterThan(workingTravel, idleTravel * 1.5, "idle must not trot")
        XCTAssertGreaterThan(idleTravel, 0.5, "idle stepping must still move the legs")
    }

    /// A stepping gait can never reach a deliberately static pose.
    func testSteppingGaitNeverReachesStaticPoses() throws {
        let brisk = HorseGait(phase: 0.25, swing: 22)
        for pose in [HorsePose.graze, .blocked, .alertStatic, .done, .unknown] {
            assertSameDrawing(drawn(pose), drawn(pose, brisk), "\(pose)")
        }
    }

    /// Reduce Motion and a disconnected source render the approved static
    /// pose through the full model-plus-renderer pipeline.
    func testReduceMotionAndDisconnectedSourcesRenderStatic() throws {
        let working = horse(.working)
        let reducedPose = working.pose(elapsed: 3.0, reduceMotion: true)
        let reducedDrawing = drawn(reducedPose, working.gait(elapsed: 3.0, reduceMotion: true))
        assertSameDrawing(reducedDrawing, drawn(.stand), "reduce motion")
        let stale = horse(.working, disconnected: true)
        let staleDrawing = drawn(stale.pose(elapsed: 3.0, reduceMotion: false),
                                 stale.gait(elapsed: 3.0, reduceMotion: false))
        assertSameDrawing(staleDrawing, drawn(.unknown), "disconnected")
    }

    /// The runtime call site must pass the gait into the REAL horseButton
    /// renderer call with the same lifecycle gate as the pose — a renderer
    /// API test alone cannot catch a view that never advances the phase.
    /// The bundled HerdView source is the file the app target compiles
    /// (test pre-build copy), so removing the argument turns this RED.
    func testHorseButtonCallSitePassesGaitIntoTheRenderer() throws {
        let url = try XCTUnwrap(Bundle(for: Self.self).url(forResource: "HerdView.swift", withExtension: "txt"),
                                "HerdView.swift.txt must ride in the test bundle")
        let source = try String(contentsOf: url, encoding: .utf8).filter { !$0.isWhitespace }
        let call = "privatefunchorseButton("
        let start = try XCTUnwrap(source.range(of: call))
        let button = String(source[start.lowerBound...])
        let painted = "HerdArt().paint(&context,identity:horse.identity,"
            + "pose:horse.pose(elapsed:elapsed,reduceMotion:reduced||!motionEnabled),"
            + "gait:horse.gait(elapsed:elapsed,reduceMotion:reduced||!motionEnabled))"
        XCTAssertEqual(button.components(separatedBy: painted).count - 1, 1,
                       "horseButton must paint with the derived gait exactly once")
    }
}
