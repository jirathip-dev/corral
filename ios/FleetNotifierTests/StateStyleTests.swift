import XCTest
@testable import FleetNotifier

// MARK: - State token contract drift guard

/// Mirrors `contracts/state-tokens.json` (the single authoritative
/// state→color/label vocabulary shared by the egui board and this app).
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
    /// keeps both the light/dark hexes and the label/mark/rank in sync.
    func testStateStyleMatchesContract() throws {
        let contractURL = URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent()
            .appendingPathComponent("../../contracts/state-tokens.json")
            .standardizedFileURL
        let data = try Data(contentsOf: contractURL)
        let tokens = try JSONDecoder().decode([StateToken].self, from: data)

        XCTAssertEqual(tokens.count, 5, "contract has exactly five states")

        var seen = Set<String>()
        for token in tokens {
            guard let state = AgentState(rawValue: token.state) else {
                XCTFail("unknown state in contract: \(token.state)")
                continue
            }
            let style = StateStyle.style(for: state)
            XCTAssertEqual(style.label, token.label, "label drifted for \(token.state)")
            XCTAssertEqual(style.mark, token.mark, "mark token drifted for \(token.state)")
            XCTAssertEqual(style.rank, token.rank, "rank drifted for \(token.state)")
            XCTAssertEqual(style.darkHex, token.dark, "dark hex drifted for \(token.state)")
            XCTAssertEqual(style.lightHex, token.light, "light hex drifted for \(token.state)")
            XCTAssertFalse(style.glyph.isEmpty, "glyph must be present for \(token.state)")
            // AC5: distinct label + mark per state (color never the only channel).
            XCTAssertTrue(seen.insert("\(style.label)|\(style.mark)").inserted,
                          "duplicate label/mark for \(token.state)")
        }
    }
}
