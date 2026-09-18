import SwiftUI
import XCTest
import Vision
@testable import FleetNotifier

// MARK: - #564 consumer proof: wire preservation + the Board card

/// Two base-compilable, value-driven proofs:
///
/// 1. WIRE PRESERVATION — a decoded daemon frame must keep
///    `git_worktree_facts` instead of throwing it away. At the pre-fix head
///    the key is dropped by decode, so the re-encoded frame loses it.
///
/// 2. THE BOARD CARD (`WorkspaceLine`, the `repo · branch · dirty ·
///    ↑ahead↓behind` line, FleetViews.swift:511-529) must not present a
///    stale git fact as current, and suppressing the fact must not change
///    the row's geometry.
///
/// Every input is a decoded daemon FRAME (JSON built to the daemon's own
/// serialization: a snapshot body with `git_worktree_facts` keyed by the
/// canonical worktree path). The card FIXTURES ARE VALUE-IDENTICAL apart
/// from the fact row's `stale` verdict, so a rendered difference can only
/// come from the freshness gate:
///
/// - stale frame: `{fact_age_ms: 120000, stale: true}`
/// - fresh frame: `{fact_age_ms: 7, stale: false}`
/// - no-fact frames: no map at all / a map without this path
///
/// The reference image is the same row with NO positive git fact
/// (`dirty: false`, `ahead: 0`, `behind: 0`).
@MainActor
final class WorktreeFreshnessConsumerTests: XCTestCase {
    private let lane = "/hosts/a/worktrees/corral/lane-a"

    // MARK: Fixtures (one frame per freshness state, otherwise identical)

    private func snapshotJSON(worktreeFacts: String?) -> String {
        let facts = worktreeFacts.map { #","git_worktree_facts":{\#($0)}"# } ?? ""
        return """
        {"agents":{"a":{"agent_id":"a","source":"herdr","tool":"herdr","state":"working",
        "capabilities":[],"workspace":{"repo":"corral","branch":"g564-lane",
        "worktree_path":"\(lane)","dirty":true,"ahead":2,"behind":3}}},
        "epoch":"564-fixture","generated_at":1789578054645\(facts),"rev":7,"schema_version":5}
        """
    }

    private func decodedRow(worktreeFacts: String?) throws -> Agent {
        let json = snapshotJSON(worktreeFacts: worktreeFacts)
        return try XCTUnwrap(try JSONDecoder().decode(Snapshot.self, from: Data(json.utf8)).agents["a"])
    }

    private func rowWithStaleFact() throws -> Agent {
        try decodedRow(worktreeFacts: #""\#(lane)":{"fact_age_ms":120000,"stale":true}"#)
    }

    private func rowWithFreshFact() throws -> Agent {
        try decodedRow(worktreeFacts: #""\#(lane)":{"fact_age_ms":7,"stale":false}"#)
    }

    /// A frame that carries no facts map at all (pre-#492 / legacy shape).
    private func rowWithoutFactsMap() throws -> Agent {
        try decodedRow(worktreeFacts: nil)
    }

    /// A frame whose map simply has no row for this canonical path.
    private func rowWithoutARow() throws -> Agent {
        try decodedRow(worktreeFacts: #""/hosts/z/worktrees/corral/lane-z":{"fact_age_ms":7,"stale":false}"#)
    }

    /// The same identity band with no positive git fact to show.
    private var absentReference: Agent {
        Agent(agentId: "a", workspace: Workspace(repo: "corral", branch: "g564-lane",
                                                 worktreePath: lane))
    }

    private func theme() throws -> ThemeStore {
        let suite = "WorktreeFreshnessConsumerTests.\(UUID().uuidString)"
        let defaults = try XCTUnwrap(UserDefaults(suiteName: suite))
        addTeardownBlock { defaults.removePersistentDomain(forName: suite) }
        return ThemeStore(defaults: defaults, reduceMotionProvider: { true })
    }

    // MARK: 1. Wire preservation (the pre-fix RED, provable at both heads)

    func testDecodedFactsSurviveSnapshotReEncode() throws {
        let json = snapshotJSON(worktreeFacts: #""\#(lane)":{"fact_age_ms":120000,"stale":true}"#)
        let snapshot = try JSONDecoder().decode(Snapshot.self, from: Data(json.utf8))
        let encoded = try JSONEncoder().encode(snapshot)
        let object = try XCTUnwrap(JSONSerialization.jsonObject(with: encoded) as? [String: Any])
        let facts = try XCTUnwrap(
            object["git_worktree_facts"] as? [String: [String: Any]],
            "a decoded snapshot must preserve git_worktree_facts (the pre-fix decoder drops it)"
        )
        XCTAssertEqual(facts.count, 1)
        XCTAssertEqual((facts[lane]?["fact_age_ms"] as? NSNumber)?.uint64Value, 120_000)
        XCTAssertEqual((facts[lane]?["stale"] as? NSNumber)?.boolValue, true)
    }

    func testDecodedFactsSurviveDeltaReEncode() throws {
        let json = """
        {"del":[],"epoch":"564-fixture",
        "git_worktree_facts":{"\(lane)":{"fact_age_ms":121000,"stale":true}},
        "rev":8,
        "upd":[{"agent_id":"a","source":"herdr","tool":"herdr","state":"idle",
        "capabilities":[],"workspace":{"repo":"corral","branch":"g564-lane",
        "worktree_path":"\(lane)","dirty":true,"ahead":2,"behind":3}}]}
        """
        let delta = try JSONDecoder().decode(Delta.self, from: Data(json.utf8))
        let encoded = try JSONEncoder().encode(delta)
        let object = try XCTUnwrap(JSONSerialization.jsonObject(with: encoded) as? [String: Any])
        let facts = try XCTUnwrap(
            object["git_worktree_facts"] as? [String: [String: Any]],
            "a decoded delta must preserve a git_worktree_facts map it carries"
        )
        XCTAssertEqual((facts[lane]?["stale"] as? NSNumber)?.boolValue, true)
        XCTAssertEqual((facts[lane]?["fact_age_ms"] as? NSNumber)?.uint64Value, 121_000)

        // A facts-less wire delta decodes to "no map" and is re-encoded with
        // the key OMITTED — it makes no statement about facts at all.
        let bare = try JSONDecoder().decode(Delta.self, from: Data("""
        {"del":[],"rev":9,"upd":[]}
        """.utf8))
        let bareObject = try XCTUnwrap(
            JSONSerialization.jsonObject(with: try JSONEncoder().encode(bare)) as? [String: Any]
        )
        XCTAssertNil(bareObject["git_worktree_facts"], "a facts-less delta states nothing about facts")
    }

    // MARK: 2. The Board card (the pre-fix RED, provable at both heads)

    func testStaleFactIsNotPresentedAsCurrent() throws {
        let theme = try theme()
        let stale = try rowWithStaleFact()
        XCTAssertTrue(stale.workspace.dirty, "the fixture's positive value is the one at stake")

        let staleImage = try render(stale, theme: theme, name: "564-stale-fact")
        let absentImage = try render(absentReference, theme: theme, name: "564-absent-reference")
        XCTAssertEqual(
            staleImage.pngData(), absentImage.pngData(),
            "a stale git fact must render exactly like a row with no positive fact — no `dirty`, no ↑ahead↓behind, and no invented stale/clean affordance"
        )
        XCTAssertEqual(staleImage.size.height, absentImage.size.height, "row height is unchanged")

        let text = try recognizedText(staleImage)
        // The scan must actually READ the row for the absence below to mean
        // anything: "lane-a" (the worktree basename) survives OCR where the
        // monospaced branch name's `l` reads back as `1`.
        XCTAssertTrue(text.contains("lane-a"), "the scan must actually read the row: \(text)")
        XCTAssertFalse(text.contains("dirty"), "a stale fact must not paint `dirty`: \(text)")
    }

    /// Control + discrimination: the SAME fixture with the daemon's current
    /// verdict still presents the positive signal, so suppression is driven
    /// by freshness and not by dropping the facts themselves.
    func testFreshFactStillPresentsTheSignal() throws {
        let theme = try theme()
        let fresh = try rowWithFreshFact()
        let freshImage = try render(fresh, theme: theme, name: "564-fresh-fact")
        let absentImage = try render(absentReference, theme: theme, name: "564-fresh-reference")
        XCTAssertNotEqual(freshImage.pngData(), absentImage.pngData(),
                          "a fresh fact must still paint its positive signal")
        let text = try recognizedText(freshImage)
        XCTAssertTrue(text.contains("dirty"), "a fresh fact paints `dirty`: \(text)")
        XCTAssertTrue(text.contains("lane-a"), "identity survives: \(text)")
    }

    /// No fact (no map, or no row for the path) is equally not current.
    func testNoFactIsNotPresentedAsCurrent() throws {
        let theme = try theme()
        let absentImage = try render(absentReference, theme: theme, name: "564-nofact-reference")
        for (label, row) in [("no-map", try rowWithoutFactsMap()),
                             ("no-row", try rowWithoutARow())] {
            let image = try render(row, theme: theme, name: "564-nofact-\(label)")
            XCTAssertEqual(image.pngData(), absentImage.pngData(),
                           "\(label): a row with no fact must not present dirty/↑↓ as current")
            let text = try recognizedText(image)
            XCTAssertFalse(text.contains("dirty"), "\(label): \(text)")
        }
    }

    /// The gate removes the trailing signal band only: the row's height and
    /// its leading identity band are byte-identical across all three states.
    func testSuppressionChangesNoGeometry() throws {
        let theme = try theme()
        let freshImage = try render(try rowWithFreshFact(), theme: theme, name: "564-geometry-fresh")
        let staleImage = try render(try rowWithStaleFact(), theme: theme, name: "564-geometry-stale")
        let absentImage = try render(absentReference, theme: theme, name: "564-geometry-absent")
        print("G564_GEOMETRY heights fresh=\(freshImage.size.height) stale=\(staleImage.size.height) absent=\(absentImage.size.height) width=\(freshImage.size.width)")
        XCTAssertEqual(freshImage.size.height, absentImage.size.height)
        XCTAssertEqual(staleImage.size.height, absentImage.size.height)
        XCTAssertEqual(freshImage.size.width, absentImage.size.width)

        let leadingFresh = try leadingBand(freshImage, points: 90)
        let leadingStale = try leadingBand(staleImage, points: 90)
        let leadingAbsent = try leadingBand(absentImage, points: 90)
        XCTAssertEqual(leadingFresh, leadingAbsent,
                       "the leading identity band must not move when the fact is absent")
        XCTAssertEqual(leadingStale, leadingAbsent,
                       "the leading identity band must not move when the fact is stale")
    }

    // MARK: Render / OCR / crop helpers

    private func render(_ agent: Agent, theme: ThemeStore, name: String) throws -> UIImage {
        let renderer = ImageRenderer(content: WorkspaceLine(agent: agent)
            .padding(8).frame(width: 390)
            .background(theme.base).environmentObject(theme))
        renderer.scale = 3
        let image = try XCTUnwrap(renderer.uiImage, "ImageRenderer produced no image")
        let attachment = XCTAttachment(image: image)
        attachment.name = name
        attachment.lifetime = .keepAlways
        add(attachment)
        try save(image, name: name)
        return image
    }

    private func recognizedText(_ image: UIImage) throws -> String {
        let request = VNRecognizeTextRequest()
        request.recognitionLevel = .accurate
        request.recognitionLanguages = ["en-US"]
        try VNImageRequestHandler(cgImage: XCTUnwrap(image.cgImage)).perform([request])
        let result = (request.results ?? []).compactMap { $0.topCandidates(1).first?.string }
            .joined(separator: " ")
        print("G564_OCR \(result)")
        return result
    }

    private func leadingBand(_ image: UIImage, points: CGFloat) throws -> Data {
        let source = try XCTUnwrap(image.cgImage)
        let width = min(Int(points * image.scale), source.width)
        let cropped = try XCTUnwrap(source.cropping(to: CGRect(x: 0, y: 0, width: width,
                                                               height: source.height)))
        return try XCTUnwrap(UIImage(cgImage: cropped, scale: image.scale,
                                     orientation: .up).pngData())
    }

    private func save(_ image: UIImage, name: String) throws {
        let directory = try XCTUnwrap(FileManager.default.urls(for: .documentDirectory, in: .userDomainMask).first)
            .appendingPathComponent("g564-evidence")
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        try XCTUnwrap(image.pngData()).write(to: directory.appendingPathComponent(name + ".png"))
    }
}
