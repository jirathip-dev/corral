import XCTest
import SwiftUI
import CryptoKit
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
        // #551 r2: a retained row keeps its LAST-KNOWN pose and withholds
        // motion — it is never recast to `unknown`.
        XCTAssertEqual(stale.state, .working)
        XCTAssertEqual(stale.pose(elapsed: 2.0, reduceMotion: false), .working)
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

    /// Reduce Motion and a disconnected source render a static pose through
    /// the full model-plus-renderer pipeline — the disconnected source in its
    /// LAST-KNOWN pose (#551 r2), never recast to the unknown art.
    func testReduceMotionAndDisconnectedSourcesRenderStatic() throws {
        let working = horse(.working)
        let reducedPose = working.pose(elapsed: 3.0, reduceMotion: true)
        let reducedDrawing = drawn(reducedPose, working.gait(elapsed: 3.0, reduceMotion: true))
        assertSameDrawing(reducedDrawing, drawn(.stand), "reduce motion")
        let stale = horse(.working, disconnected: true)
        let staleDrawing = drawn(stale.pose(elapsed: 3.0, reduceMotion: false),
                                 stale.gait(elapsed: 3.0, reduceMotion: false))
        assertSameDrawing(staleDrawing, drawn(.working, .standstill), "disconnected")
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

    /// #551 r3: the row's `y:` offset must go through the FENCED `HerdHorse.bob`
    /// (the twin of `roam`), never the inline clock-driven expression that let a
    /// retained idle row animate. The bundled HerdView source is the file the app
    /// target compiles, so reverting the call site turns this RED.
    func testHorseButtonOffsetRoutesTheBobThroughTheFencedModel() throws {
        let url = try XCTUnwrap(Bundle(for: Self.self).url(forResource: "HerdView.swift", withExtension: "txt"),
                                "HerdView.swift.txt must ride in the test bundle")
        let source = try String(contentsOf: url, encoding: .utf8).filter { !$0.isWhitespace }
        let call = "privatefunchorseButton("
        let start = try XCTUnwrap(source.range(of: call))
        let button = String(source[start.lowerBound...])
        let fenced = "y:horse.bob(elapsed:elapsed,reduceMotion:reduced,enabled:motionEnabled)"
        XCTAssertEqual(button.components(separatedBy: fenced).count - 1, 1,
                       "the row's y: offset must call horse.bob exactly once")
        XCTAssertEqual(button.components(separatedBy: "y:horse.state==.idle").count - 1, 0,
                       "the inline bob expression must not survive anywhere in horseButton")
    }

    // MARK: #530 hoof-to-leg correspondence

    /// Every coat/breed combination is reachable only through the name-derived
    /// identity axes, so the matrix below walks each one by a deterministic
    /// search over that constructor instead of a fixed name table.
    private func identity(coat: Int, breed: Int) throws -> HorseIdentity {
        let found = (0..<4096).lazy
            .map { HorseIdentity(name: "i530-coat\(coat)-breed\(breed)-\($0)") }
            .first { $0.coat == coat && $0.breed == breed }
        return try XCTUnwrap(found, "no name-derived identity reaches coat \(coat) breed \(breed)")
    }

    /// The centre of an ink's axis-aligned box. The leg, hoof and tuft inks
    /// are each one centrally symmetric rounded rect, so for every rotation
    /// the box centre IS the rect's centre.
    private func centre(_ ink: HorseInk) -> CGPoint {
        CGPoint(x: ink.path.boundingRect.midX, y: ink.path.boundingRect.midY)
    }

    /// #530: the correspondence check is anchored on the DRAWN LEG INK, not
    /// on the gait model. `HerdArt.leg` rotates one rigid rounded rect (with
    /// its hoof band) about the hip, so for any pose rest angle, diagonal
    /// swing, -4-degree working lean and outer transform the offset vector
    /// from the leg ink's own centre to a member ink's centre is
    /// rotation-invariant: a member authored on the leg's axis sits at
    /// exactly the canonical offset length — any angular displacement grows
    /// it (law of cosines) — and the fetlock tuft must additionally share the
    /// hoof's axis and distal side. A tuft rotated by the swing alone (the
    /// pre-#530 form, which drops the pose rest angle) or by a mirrored angle
    /// lands off-axis by up to 12 pt and fails the cross/dot pair.
    ///
    /// The matrix walks every breed, every coat index, every pose and eight
    /// gait phases at both authored amplitudes.
    func testEveryHoofAndFetlockTuftStaysOnItsLegAnchorAcrossAllVariantsAndPhases() throws {
        let poses: [HorsePose] = [.stand, .working, .blocked, .done, .unknown, .graze, .alertStatic]
        var gaits: [HorseGait] = [.standstill]
        for sample in 0..<8 {
            gaits.append(HorseGait(phase: Double(sample) / 8, swing: 22))
            gaits.append(HorseGait(phase: Double(sample) / 8, swing: 8))
        }
        for breed in 0..<3 {
            let height = [34.0, 31, 28][breed]
            for coat in 0..<8 {
                let witness = try identity(coat: coat, breed: breed)
                for pose in poses {
                    for gait in gaits {
                        let inks = art.drawing(witness, pose: pose, gait: gait)
                        let label = "coat \(coat) breed \(breed) \(pose.rawValue) "
                            + "phase \(gait.phase) swing \(gait.swing)"
                        for index in 0..<4 {
                            let leg = try ink("leg-\(index)", in: inks)
                            let hoof = try ink("leg-\(index)-hoof", in: inks)
                            let anchor = centre(leg)
                            let hoofCentre = centre(hoof)
                            let v = CGPoint(x: hoofCentre.x - anchor.x, y: hoofCentre.y - anchor.y)
                            XCTAssertEqual(Double(hypot(v.x, v.y)), height / 2 - 2.5, accuracy: 0.01,
                                           "\(label) leg-\(index) hoof must sit on its leg's axis")
                            guard breed == 2, index == 0 || index == 2 else { continue }
                            let tuft = try ink("leg-\(index)-tuft", in: inks)
                            let tuftCentre = centre(tuft)
                            let t = CGPoint(x: tuftCentre.x - anchor.x, y: tuftCentre.y - anchor.y)
                            XCTAssertEqual(Double(hypot(t.x, t.y)), height / 2 - 5, accuracy: 0.01,
                                           "\(label) leg-\(index) fetlock tuft must sit on its leg's axis")
                            XCTAssertEqual(Double(v.x * t.y - v.y * t.x), 0, accuracy: 0.05,
                                           "\(label) leg-\(index) fetlock tuft must share the hoof's axis")
                            XCTAssertGreaterThan(Double(v.x * t.x + v.y * t.y), 0,
                                                 "\(label) leg-\(index) fetlock tuft must sit distal, on the hoof's side")
                        }
                    }
                }
            }
        }
    }

    /// The painted output of one drawing — colour, opacity, stroke and the
    /// path geometry — serialized at a fixed precision and hashed. This is
    /// exactly the value set `HerdArt.paint` consumes, so an equal digest
    /// means an unchanged frame.
    static func inkDigest(_ inks: [HorseInk]) -> String {
        var text = ""
        for ink in inks {
            text += "\(ink.color)|\(ink.opacity)|\(ink.stroke)|"
            ink.path.forEach { element in
                switch element {
                case let .move(to: p): text += "M\(Self.number(p.x)),\(Self.number(p.y));"
                case let .line(to: p): text += "L\(Self.number(p.x)),\(Self.number(p.y));"
                case let .quadCurve(to: p, control: c):
                    text += "Q\(Self.number(c.x)),\(Self.number(c.y)),"
                        + "\(Self.number(p.x)),\(Self.number(p.y));"
                case let .curve(to: p, control1: a, control2: b):
                    text += "C\(Self.number(a.x)),\(Self.number(a.y)),"
                        + "\(Self.number(b.x)),\(Self.number(b.y)),"
                        + "\(Self.number(p.x)),\(Self.number(p.y));"
                case .closeSubpath: text += "Z;"
                }
            }
            text += "\n"
        }
        return SHA256.hash(data: Data(text.utf8)).map { String(format: "%02x", $0) }.joined()
    }

    private static func number(_ value: CGFloat) -> String { String(format: "%.6f", Double(value)) }

    /// The ink digests every pose which has a zero rest angle (and every
    /// non-draft breed in the running poses) must keep FOREVER — captured
    /// before the #530 fix by running this same test with an empty table
    /// against `HerdArt.swift` at the pre-fix base `origin/integration`
    /// 1d774d1f5d5270193b028f29f36670eac3fbabfb`. The printed
    /// `i530-base-ink|…` lines are the same table, so a reviewer can
    /// re-derive each value from `probe-gait.py`'s logs.
    static let baseInk: [String: String] = [
        "b0|stand|still": "41831ebbe121c18f627f9fcab3d0e31e71bf0570748e8e7a83b8a6588619c1c7",
        "b0|stand|step": "63f1db99e2117b62f14902416b3348881eb9d73450ecf783b1718aeb381763ab",
        "b0|done|still": "6826c13a44b774ea5e5c9f5c4f6c6da328ce1672466d4bc1859510780aa00102",
        "b0|done|step": "6826c13a44b774ea5e5c9f5c4f6c6da328ce1672466d4bc1859510780aa00102",
        "b0|alertStatic|still": "e6e091b2877bb7e4d7f5c4eca258997dc91c283c9dc5692086033fa4d2919638",
        "b0|alertStatic|step": "e6e091b2877bb7e4d7f5c4eca258997dc91c283c9dc5692086033fa4d2919638",
        "b0|graze|still": "14f2065db49d36e448d909e61de4ee1ea0b7c00aa46a97fc3fb0f556c772d712",
        "b0|graze|step": "14f2065db49d36e448d909e61de4ee1ea0b7c00aa46a97fc3fb0f556c772d712",
        "b0|working|trot": "3257dfc723300c02fc96101ddcfbb2c11bd55d1d3973a8c0dd6f8233f53d6c88",
        "b0|blocked|trot": "fdad6ff56e5b0cf3c7c9655b63c61071ad5a90d546948b2c19a0d1f0d1e66709",
        "b0|unknown|trot": "7d4516786e5fbc457abd28b8e0fffff543f73d012404afa4ea1445ffff3d957e",
        "b1|stand|still": "3080763e4059ba6ec475bdddb25e130be947105965e6ff85e759ef296c6bcaf2",
        "b1|stand|step": "c22852e2b3dcb039520578bcfeac13c919b5bfcaa00f3d7171308363e2f49396",
        "b1|done|still": "59fe62d2b163730b7b329ad41c12a76149fa3b04b06f5e5c26a479757530e1b6",
        "b1|done|step": "59fe62d2b163730b7b329ad41c12a76149fa3b04b06f5e5c26a479757530e1b6",
        "b1|alertStatic|still": "77a87f13ad058062c8dc7550ccba60ecb311a1af7bf5da94ac9471c42f56b98a",
        "b1|alertStatic|step": "77a87f13ad058062c8dc7550ccba60ecb311a1af7bf5da94ac9471c42f56b98a",
        "b1|graze|still": "4c6c1ca6596020dc9d8011e17612a8b7aaaaea661cfe78cbf1b1e989c5dc2b36",
        "b1|graze|step": "4c6c1ca6596020dc9d8011e17612a8b7aaaaea661cfe78cbf1b1e989c5dc2b36",
        "b1|working|trot": "e6ee7d9305e4a3d10130942181f6d62b7dce5e6b77d0d86da19fbf47bf9d8e77",
        "b1|blocked|trot": "84fd98b3bbd652493092af99cc1b2d0c7eaf38d5440c2333b2946b941a7d02a7",
        "b1|unknown|trot": "0e049ea0bfd564142b32f7375397132aa5e74b0352951efa5214004ef09a0e72",
        "b2|stand|still": "6c9131b86dd2c37308a30f2c5093ce49962de4366c0a4b4844ff500fc8ad1211",
        "b2|stand|step": "b5efd1d23b9dd3bc7714e9dd01837fe7c29b71db2734d6a2e9bd8776c576fbce",
        "b2|done|still": "c5d50f2408772b437cfc361d0ec0a8f7953ef5162f1d394a5348da5bddcfc868",
        "b2|done|step": "c5d50f2408772b437cfc361d0ec0a8f7953ef5162f1d394a5348da5bddcfc868",
        "b2|alertStatic|still": "f68239239e4dc9ca9817345a48898d5455b857b1d0bab31feda52ec061582e14",
        "b2|alertStatic|step": "f68239239e4dc9ca9817345a48898d5455b857b1d0bab31feda52ec061582e14",
        "b2|graze|still": "49200b8f6889cb6e666daba264bdb5c997e1dbc78f3e800daa9a407e96ec07fb",
        "b2|graze|step": "49200b8f6889cb6e666daba264bdb5c997e1dbc78f3e800daa9a407e96ec07fb",
    ]

    /// #530 requirement 4, byte-identical form: the correspondence fix moves
    /// the draft fetlock tufts in working / blocked / unknown (rest angle
    /// non-zero) and nowhere else. Every sample below renders the exact
    /// pre-fix ink — the frames a caller sees for standing, idle-stepping,
    /// grazing, settled and alert-static horses, at both authored amplitudes.
    func testZeroRestAnglePosesKeepTheBaseInkAcrossTheCorrespondenceFix() throws {
        let idleStep = HorseGait(phase: 0.25, swing: 8)
        let trot = HorseGait(phase: 0.25, swing: 22)
        var samples: [(key: String, breed: Int, pose: HorsePose, gait: HorseGait)] = []
        for breed in 0..<3 {
            for pose in [HorsePose.stand, .done, .alertStatic, .graze] {
                samples.append(("b\(breed)|\(pose.rawValue)|still", breed, pose, .standstill))
                samples.append(("b\(breed)|\(pose.rawValue)|step", breed, pose, idleStep))
            }
        }
        for breed in 0..<2 {
            for pose in [HorsePose.working, .blocked, .unknown] {
                samples.append(("b\(breed)|\(pose.rawValue)|trot", breed, pose, trot))
            }
        }
        XCTAssertEqual(samples.count, 30)
        var table: [String: String] = [:]
        for sample in samples {
            let witness = try identity(coat: 0, breed: sample.breed)
            let digest = Self.inkDigest(art.drawing(witness, pose: sample.pose, gait: sample.gait))
            table[sample.key] = digest
            print("i530-base-ink|\(sample.key)|\(digest)")
        }
        XCTAssertEqual(table, Self.baseInk,
                       "every pose with a zero rest angle must keep the pre-fix ink exactly")
    }
}
