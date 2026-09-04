import XCTest
@testable import FleetNotifier

// MARK: - State token vocabulary (#354 L2 board labels, marks, ranks)
/// #372: the shared `contracts/state-tokens.json` drift guard previously
/// also pinned the LIGHT/DARK COLORS of each state. The #372 Catppuccin
/// design approval supersedes those pre-theming GitHub-dark hexes for the
/// iOS client (the whole app now renders the ACTIVE flavor's palette; the
/// egui mirror and the shared contract file are a later follow-up and were
/// deliberately NOT touched by this lane). The per-flavor state→color
/// mapping is LOCKED as ONE shared mapping inside `ThemeStore`
/// (`stateToken(for:)`); both accessors — the production `stateColor(for:)`
/// the views consume and the hex view `stateHex(for:)` — resolve through
/// it, and this suite pins the mapping against the approved palette hexes
/// THROUGH `stateColor(for:)` — every flavor, every state. Marks and ranks
/// still mirror the shared contract (they drive the glyphs and the board
/// ordering in both clients).
private struct StateToken: Codable, Equatable {
    let state: String
    let rank: Int
    let label: String
    let dark: String
    let light: String
    let mark: String
}

final class StateStyleTests: XCTestCase {

    /// The LOCKED per-flavor state mapping (issue spec + the approved
    /// design round): working=teal, blocked=red, done=green, idle=subtext0,
    /// unknown=surface2. Hexes are the approved Catppuccin palette values.
    private let lockedStateTokenHex: [CatppuccinFlavor: [AgentState: String]] = [
        .latte: [
            .blocked: "#d20f39", .working: "#179299", .done: "#40a02b",
            .idle: "#6c6f85", .unknown: "#acb0be",
        ],
        .frappe: [
            .blocked: "#e78284", .working: "#81c8be", .done: "#a6d189",
            .idle: "#a5adce", .unknown: "#626880",
        ],
        .macchiato: [
            .blocked: "#ed8796", .working: "#8bd5ca", .done: "#a6da95",
            .idle: "#a5adcb", .unknown: "#5b6078",
        ],
        .mocha: [
            .blocked: "#f38ba8", .working: "#94e2d5", .done: "#a6e3a1",
            .idle: "#a6adc8", .unknown: "#585b70",
        ],
    ]

    /// Loads the contract relative to `#filePath` and asserts `StateStyle`
    /// keeps the mark + rank tokens in sync with it (the color columns are
    /// superseded by the #372 palette mapping — see the type comment).
    func testStateStyleMatchesContractMarksAndRanks() throws {
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
            XCTAssertEqual(style.rank, token.rank, "rank drifted for \(token.state)")
            XCTAssertFalse(style.glyph.isEmpty, "glyph must be present for \(token.state)")
        }
    }

    /// #372: every state resolves to the LOCKED palette token per flavor
    /// with the approved hex values, pinned THROUGH THE PRODUCTION
    /// ACCESSOR the views consume — `ThemeStore.stateColor(for:)` — not a
    /// test-only hex helper (a `working→red` drift in the shared mapping
    /// goes RED here; both accessors resolve through the ONE mapping,
    /// `stateToken(for:)`). A palette swap that forgets a flavor also goes
    /// RED.
    @MainActor
    func testStateColorsResolveThroughTheLockedPerFlavorMapping() {
        for flavor in CatppuccinFlavor.allCases {
            // SAFETY: a fresh UUID-based suite name is always a valid suite.
            let theme = ThemeStore(defaults: UserDefaults(suiteName: "theme-\(flavor.rawValue)-state-\(UUID().uuidString)")!,
                                   reduceMotionProvider: { false })
            theme.setFlavor(flavor)
            for state in AgentState.allCases {
                let lockedHex = lockedStateTokenHex[flavor]?[state]
                XCTAssertEqual(
                    theme.stateColor(for: state).hexDescription,
                    lockedHex,
                    "state \(state) color drifted under \(flavor.rawValue) "
                    + "(the accessor every view renders through)")
                XCTAssertEqual(
                    theme.stateHex(for: state),
                    lockedHex,
                    "state \(state) hex drifted under \(flavor.rawValue)")
            }
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
