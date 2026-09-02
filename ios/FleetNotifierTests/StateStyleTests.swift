import XCTest
@testable import FleetNotifier

// MARK: - State token contract drift guard (#354 L2 board vocabulary)

/// Mirrors `contracts/state-tokens.json` (the shared state→color/mark
/// vocabulary). LABELS deliberately diverge: the #354 v2 board shows herdr's
/// RAW tokens verbatim (working / idle / blocked / unknown — no done, no
/// Corral-invented wording), and the attention ranks follow the v2 order
/// (blocked → working → idle → unknown; a wire `done` ranks with idle as its
/// herdr fallback). Colors, glyph marks, and hexes stay contract-bound so
/// the two clients cannot drift visually.
private struct StateToken: Codable, Equatable {
    let state: String
    let rank: Int
    let label: String
    let dark: String
    let light: String
    let mark: String
}

final class StateStyleTests: XCTestCase {

    /// Loads the contract relative to `#filePath` and asserts `StateStyle`
    /// keeps the light/dark hexes + mark tokens in sync with it.
    func testStateStyleMatchesContractColorsAndMarks() throws {
        let contractURL = URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent()
            .appendingPathComponent("../../contracts/state-tokens.json")
            .standardizedFileURL
        let data = try Data(contentsOf: contractURL)
        let tokens = try JSONDecoder().decode([StateToken].self, from: data)

        XCTAssertEqual(tokens.count, 5, "contract has exactly five states")

        for token in tokens {
            guard let state = AgentState(rawValue: token.state) else {
                XCTFail("unknown state in contract: \(token.state)")
                continue
            }
            let style = StateStyle.style(for: state)
            XCTAssertEqual(style.mark, token.mark, "mark token drifted for \(token.state)")
            XCTAssertEqual(style.darkHex, token.dark, "dark hex drifted for \(token.state)")
            XCTAssertEqual(style.lightHex, token.light, "light hex drifted for \(token.state)")
            XCTAssertFalse(style.glyph.isEmpty, "glyph must be present for \(token.state)")
        }
    }

    /// v2: the board renders the raw herdr state tokens verbatim.
    func testLabelsAreRawHerdrTokens() {
        XCTAssertEqual(StateStyle.style(for: .blocked).label, "blocked")
        XCTAssertEqual(StateStyle.style(for: .working).label, "working")
        XCTAssertEqual(StateStyle.style(for: .idle).label, "idle")
        XCTAssertEqual(StateStyle.style(for: .unknown).label, "unknown")
        // `done` is not part of herdr 0.8.2's vocabulary; if a transitional
        // daemon still emits it the board names it honestly, never with
        // Corral-invented wording.
        XCTAssertEqual(StateStyle.style(for: .done).label, "done")
    }

    /// v2 attention order: blocked(0) → working(1) → idle/done(2) → unknown(3).
    func testRanksEncodeTheV2AttentionOrder() {
        let blocked = StateStyle.style(for: .blocked).rank
        let working = StateStyle.style(for: .working).rank
        let idle = StateStyle.style(for: .idle).rank
        let done = StateStyle.style(for: .done).rank
        let unknown = StateStyle.style(for: .unknown).rank
        XCTAssertLessThan(blocked, working)
        XCTAssertLessThan(working, idle)
        XCTAssertEqual(idle, done, "a wire done ranks with idle (herdr fallback)")
        XCTAssertLessThan(done, unknown)
    }
}
