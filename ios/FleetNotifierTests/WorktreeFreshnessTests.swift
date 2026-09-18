import XCTest
@testable import FleetNotifier

// MARK: - #564: per-worktree git-fact decode, retention and the three states

/// The wire shape under test is the daemon's own (#492, read-only):
/// `Snapshot.git_worktree_facts: BTreeMap<String, GitFactAge>` keyed by the
/// canonical worktree path, whose value is exactly
/// `{fact_age_ms: Option<u64>, stale: bool}`
/// (src/core/model.rs:226 / :233-235). This suite pins decode + retention on
/// Snapshot AND Delta, and the three consumer states:
/// fresh / stale / no fact.
final class WorktreeFreshnessTests: XCTestCase {
    private let laneA = "/hosts/a/worktrees/corral/lane-a"
    private let laneB = "/hosts/b/worktrees/corral/lane-b"
    private let laneC = "/hosts/c/worktrees/corral/lane-c"
    private let laneD = "/hosts/d/worktrees/corral/lane-d"

    // MARK: Fixtures

    /// One agent row as the daemon serializes it (flat record + workspace).
    private func agent(_ id: String, lane: String, dirty: Bool, ahead: UInt64, behind: UInt64) -> String {
        """
        "\(id)":{"agent_id":"\(id)","source":"herdr","tool":"herdr","state":"working",
        "capabilities":[],"workspace":{"repo":"corral","branch":"g564-\(id)",
        "worktree_path":"\(lane)","dirty":\(dirty),"ahead":\(ahead),"behind":\(behind)}}
        """
    }

    /// A full frame carrying the facts map: the daemon-side spelling of a
    /// snapshot body (extra daemon keys are ignored by this client, exactly
    /// as they are on the wire).
    private func snapshotJSON(agents: String, facts: String, rev: UInt64 = 7) -> String {
        """
        {"agents":{\(agents)},"epoch":"564-fixture","generated_at":1789578054645,
        "git_plane_alive":true,"git_plane_backlog":false,
        "git_plane_last_event_age_ms":null,"git_plane_skipped":0,
        "git_worktree_facts":{\(facts)},"rev":\(rev),"schema_version":5}
        """
    }

    /// The daemon's emission for a stopped plane: rows exist, ages may be
    /// real or null, every `stale` is true (src/core/store.rs:113-116).
    private var stoppedFacts: String {
        """
        "\(laneA)":{"fact_age_ms":7,"stale":true},
        "\(laneB)":{"fact_age_ms":120000,"stale":true},
        "\(laneC)":{"fact_age_ms":null,"stale":true}
        """
    }

    private var freshFacts: String {
        """
        "\(laneA)":{"fact_age_ms":7,"stale":false},
        "\(laneB)":{"fact_age_ms":120000,"stale":true},
        "\(laneC)":{"fact_age_ms":null,"stale":true}
        """
    }

    private var agentsABCD: String {
        [
            agent("a", lane: laneA, dirty: true, ahead: 2, behind: 3),
            agent("b", lane: laneB, dirty: true, ahead: 1, behind: 1),
            agent("c", lane: laneC, dirty: false, ahead: 0, behind: 4),
            agent("d", lane: laneD, dirty: true, ahead: 0, behind: 0),
        ].joined(separator: ",")
    }

    private func decodedSnapshot(_ json: String) throws -> Snapshot {
        try JSONDecoder().decode(Snapshot.self, from: Data(json.utf8))
    }

    private func decodedDelta(_ json: String) throws -> Delta {
        try JSONDecoder().decode(Delta.self, from: Data(json.utf8))
    }

    // MARK: Snapshot decode + preserve

    func testSnapshotDecodePreservesFactRowsAndResolvesEveryRow() throws {
        let snapshot = try decodedSnapshot(snapshotJSON(agents: agentsABCD, facts: freshFacts))

        // The map itself is PRESERVED, verbatim, keyed by canonical path.
        XCTAssertEqual(snapshot.gitWorktreeFacts.count, 3)
        let a = try XCTUnwrap(snapshot.gitWorktreeFacts[laneA])
        XCTAssertEqual(a.factAgeMs, 7)
        XCTAssertFalse(a.stale)
        let b = try XCTUnwrap(snapshot.gitWorktreeFacts[laneB])
        XCTAssertEqual(b.factAgeMs, 120_000)
        XCTAssertTrue(b.stale)
        let c = try XCTUnwrap(snapshot.gitWorktreeFacts[laneC])
        XCTAssertNil(c.factAgeMs, "an unobserved fact keeps its null age")
        XCTAssertTrue(c.stale)

        // Each row's verdict comes from ITS OWN canonical path.
        XCTAssertEqual(snapshot.agents["a"]?.workspace.gitFactFreshness, .fresh)
        XCTAssertEqual(snapshot.agents["b"]?.workspace.gitFactFreshness, .stale)
        XCTAssertEqual(snapshot.agents["c"]?.workspace.gitFactFreshness, .stale,
                       "null-age rows are the daemon's stale verdict, not 'no fact'")
        XCTAssertEqual(snapshot.agents["d"]?.workspace.gitFactFreshness, .noFact,
                       "a path with no row has no fact to report")

        // The rest of the row is untouched by the projection.
        XCTAssertEqual(snapshot.agents["a"]?.workspace.dirty, true)
        XCTAssertEqual(snapshot.agents["a"]?.workspace.behind, 3)
        XCTAssertEqual(snapshot.agents["a"]?.workspace.worktreePath, laneA)
    }

    func testPreFactsPayloadDecodesToNoFactNeverFresh() throws {
        let json = """
        {"agents":{\(agent("a", lane: laneA, dirty: true, ahead: 2, behind: 3))},
        "generated_at":1789578054645,"rev":7,"schema_version":4}
        """
        let snapshot = try decodedSnapshot(json)
        XCTAssertEqual(snapshot.gitWorktreeFacts, [:])
        XCTAssertEqual(snapshot.agents["a"]?.workspace.gitFactFreshness, .noFact,
                       "a frame with no facts map can never resolve a fresh fact")
        XCTAssertEqual(snapshot.agents["a"]?.workspace.dirty, true,
                       "the retained workspace values still decode — they are just unbacked")
    }

    // MARK: The three states, deterministically

    func testThreeStatesAreDistinctAndCarryTheirOwnInputs() {
        let table = WorktreeFactTable(rows: [
            laneA: GitFactAge(factAgeMs: 7, stale: false),
            laneB: GitFactAge(factAgeMs: 120_000, stale: true),
            laneC: GitFactAge(factAgeMs: nil, stale: true),
            laneD: GitFactAge(factAgeMs: 7, stale: true),
        ])
        XCTAssertEqual(table.freshness(forWorktreePath: laneA), .fresh,
                       "young age + stale:false is the daemon's current verdict")
        XCTAssertEqual(table.freshness(forWorktreePath: laneB), .stale,
                       "present-but-stale: an aged row is stale")
        XCTAssertEqual(table.freshness(forWorktreePath: laneC), .stale,
                       "absent age (never observed) is the daemon's stale verdict")
        XCTAssertEqual(table.freshness(forWorktreePath: laneD), .stale,
                       "a young age with stale:true stays stale — the verdict is authoritative, never re-derived")
        XCTAssertEqual(table.freshness(forWorktreePath: "/hosts/x/worktrees/corral/lane-x"), .noFact,
                       "no row for the path means no fact")
        XCTAssertEqual(table.freshness(forWorktreePath: nil), .noFact,
                       "no canonical path means no fact")

        XCTAssertNotEqual(WorktreeFactFreshness.fresh, .stale)
        XCTAssertNotEqual(WorktreeFactFreshness.stale, .noFact)
        XCTAssertNotEqual(WorktreeFactFreshness.noFact, .fresh)
        XCTAssertTrue(WorktreeFactFreshness.fresh.isCurrent)
        XCTAssertFalse(WorktreeFactFreshness.stale.isCurrent)
        XCTAssertFalse(WorktreeFactFreshness.noFact.isCurrent)
    }

    func testFactRowWithoutAVerdictFailsClosed() throws {
        let json = #"{"fact_age_ms":9}"#
        let row = try JSONDecoder().decode(GitFactAge.self, from: Data(json.utf8))
        XCTAssertEqual(row.factAgeMs, 9)
        XCTAssertTrue(row.stale, "a row that carries no daemon verdict is never treated as current")
    }

    // MARK: Delta decode + retain

    func testDeltaDecodePreservesFactRowsAndResolvesItsRows() throws {
        let json = """
        {"del":[],"epoch":"564-fixture",
        "git_worktree_facts":{"\(laneA)":{"fact_age_ms":130000,"stale":true}},
        "rev":8,
        "upd":[{"agent_id":"a","source":"herdr","tool":"herdr","state":"working",
        "capabilities":[],"workspace":{"repo":"corral","branch":"g564-a",
        "worktree_path":"\(laneA)","dirty":true,"ahead":2,"behind":3}}]}
        """
        let delta = try decodedDelta(json)
        XCTAssertEqual(delta.gitWorktreeFacts?[laneA]?.stale, true)
        XCTAssertEqual(delta.gitWorktreeFacts?[laneA]?.factAgeMs, 130_000)
        XCTAssertEqual(delta.upd.first?.workspace.gitFactFreshness, .stale,
                       "a delta that carries facts resolves its own rows from them")
    }

    func testWireDeltaWithoutFactsCarriesNoneAndRetains() throws {
        // Exactly the daemon's delta frame body (#492 model.rs:242-246 has no
        // facts field; the SSE delta is `{epoch, rev, upd, del}`).
        let json = """
        {"del":["gone"],"epoch":"564-fixture","rev":8,
        "upd":[{"agent_id":"a","source":"herdr","tool":"herdr","state":"working",
        "capabilities":[],"workspace":{"repo":"corral","branch":"g564-a",
        "worktree_path":"\(laneA)","dirty":true,"ahead":2,"behind":3}}]}
        """
        let delta = try decodedDelta(json)
        XCTAssertNil(delta.gitWorktreeFacts,
                     "a wire delta carries no facts map — the frame says nothing about facts")
        XCTAssertEqual(delta.upd.first?.workspace.gitFactFreshness, .noFact,
                       "a row replaced by a facts-less frame is fail-closed, never fresh")

        let retained = WorktreeFactTable(rows: [laneA: GitFactAge(factAgeMs: 7, stale: false)])
        XCTAssertEqual(retained.merging(delta), retained,
                       "a facts-less delta must not disturb the retained rows")
    }

    // MARK: Multi-host delta sequence: retained rows, fresh -> stale -> no fact

    func testMultiHostDeltaSequenceRetainsEachRowsOwnState() throws {
        // One retained table per host stream (the board merges their rows).
        var tableA = WorktreeFactTable(rows: [laneA: GitFactAge(factAgeMs: 7, stale: false)])
        var tableB = WorktreeFactTable(rows: [laneB: GitFactAge(factAgeMs: 7, stale: false)])
        XCTAssertEqual(tableA.freshness(forWorktreePath: laneA), .fresh)
        XCTAssertEqual(tableB.freshness(forWorktreePath: laneB), .fresh)

        // Delta 1: host A's worktree ages out. Host A's frame carries the
        // row; host B's stream is a DIFFERENT table and never sees it.
        let deltaA1 = try decodedDelta("""
        {"del":[],"rev":8,
        "git_worktree_facts":{"\(laneA)":{"fact_age_ms":121000,"stale":true}},
        "upd":[{"agent_id":"a","source":"herdr","tool":"herdr","state":"idle",
        "capabilities":[],"workspace":{"repo":"corral","branch":"g564-a",
        "worktree_path":"\(laneA)","dirty":true,"ahead":2,"behind":3}}]}
        """)
        tableA = tableA.merging(deltaA1)
        XCTAssertEqual(tableA.freshness(forWorktreePath: laneA), .stale)
        XCTAssertEqual(deltaA1.upd.first?.workspace.gitFactFreshness, .stale)
        XCTAssertEqual(tableB.freshness(forWorktreePath: laneB), .fresh,
                       "another host's row stays fresh")

        // Delta 2 (host A, facts-less): retention must not promote the row.
        let deltaA2 = try decodedDelta("""
        {"del":[],"rev":9,
        "upd":[{"agent_id":"a","source":"herdr","tool":"herdr","state":"idle",
        "capabilities":[],"workspace":{"repo":"corral","branch":"g564-a",
        "worktree_path":"\(laneA)","dirty":true,"ahead":2,"behind":3}}]}
        """)
        let before = tableA
        tableA = tableA.merging(deltaA2)
        XCTAssertEqual(tableA, before, "a facts-less delta changes nothing")
        XCTAssertEqual(tableA.freshness(forWorktreePath: laneA), .stale,
                       "omission is never a freshness claim")

        // Delta 3 (host A): a full frame that no longer carries the row is
        // the ONLY transition to no fact — and it replaces, never merges.
        let snapshotA3 = try decodedSnapshot("""
        {"agents":{\(agent("a", lane: laneA, dirty: true, ahead: 2, behind: 3))},
        "generated_at":1789578054645,"git_worktree_facts":{},"rev":10,"schema_version":5}
        """)
        tableA = tableA.replacing(with: snapshotA3)
        XCTAssertEqual(tableA.freshness(forWorktreePath: laneA), .noFact)
        XCTAssertEqual(snapshotA3.agents["a"]?.workspace.gitFactFreshness, .noFact,
                       "the replacement frame resolves the row to no fact too")
        XCTAssertEqual(tableB.freshness(forWorktreePath: laneB), .fresh,
                       "host B's retained row survives host A's replacement")
    }

    func testDeltaUpsertRetainsUnmentionedRowsInOneTable() throws {
        // One composite table holding two hosts' rows: a delta upserts the
        // rows it names and retains the rest with their own verdicts.
        var table = WorktreeFactTable(rows: [
            laneA: GitFactAge(factAgeMs: 7, stale: false),
            laneB: GitFactAge(factAgeMs: 7, stale: false),
        ])
        let delta = try decodedDelta("""
        {"del":[],"rev":8,
        "git_worktree_facts":{"\(laneB)":{"fact_age_ms":130000,"stale":true}},
        "upd":[]}
        """)
        table = table.merging(delta)
        XCTAssertEqual(table.freshness(forWorktreePath: laneA), .fresh, "unmentioned row retained")
        XCTAssertEqual(table.freshness(forWorktreePath: laneB), .stale, "named row upserted")
        XCTAssertEqual(table.rows.count, 2, "an upsert never drops a row")
    }

    func testReplacementIsAuthoritativeInBothDirections() throws {
        // A full frame's empty map is a replacement: nothing is retained and
        // nothing becomes stale-by-omission — the rows are simply gone.
        let table = WorktreeFactTable(rows: [laneA: GitFactAge(factAgeMs: 7, stale: false)])
        let empty = try decodedSnapshot("""
        {"agents":{},"generated_at":1789578054645,"git_worktree_facts":{},"rev":11,"schema_version":5}
        """)
        let replaced = table.replacing(with: empty)
        XCTAssertEqual(replaced.rows.count, 0)
        XCTAssertEqual(replaced.freshness(forWorktreePath: laneA), .noFact)
        // And a replacement that DOES carry the row is authoritative too.
        let carried = try decodedSnapshot(snapshotJSON(agents: agentsABCD, facts: stoppedFacts, rev: 12))
        let next = replaced.replacing(with: carried)
        XCTAssertEqual(next.freshness(forWorktreePath: laneA), .stale)
        XCTAssertEqual(next.freshness(forWorktreePath: laneB), .stale)
        XCTAssertEqual(next.freshness(forWorktreePath: laneD), .noFact)
    }
}
