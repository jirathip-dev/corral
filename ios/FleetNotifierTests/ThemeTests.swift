import XCTest
import SwiftUI
@testable import FleetNotifier

// MARK: - #372 Catppuccin theme foundation tests
//
// Locks the theme layer against the approved design-round data:
// - the four locked palettes (hexes verbatim from the approved
//   catppuccin-palette.json; no invented colors),
// - the per-flavor ANSI slot table (the design round's ANSI map),
// - the ported repo-hue function (fnv1a32 % 8 + linear probe),
// - ThemeStore defaults + persistence, and the Reduce Motion plumbing.

@MainActor
final class ThemePaletteTests: XCTestCase {
    func testFlavorVocabularyAndOrderMatchTheAppearanceFrame() {
        XCTAssertEqual(CatppuccinFlavor.allCases.map(\.rawValue),
                       ["latte", "frappe", "macchiato", "mocha"],
                       "Settings row order is Latte → Frappé → Macchiato → Mocha (approved frame)")
        XCTAssertEqual(CatppuccinFlavor.default, .mocha,
                       "the default flavor is Mocha (spec lock)")
        XCTAssertTrue(CatppuccinFlavor.latte.isLight)
        XCTAssertFalse(CatppuccinFlavor.mocha.isLight)
        XCTAssertFalse(CatppuccinFlavor.frappe.isLight)
        XCTAssertFalse(CatppuccinFlavor.macchiato.isLight)
        XCTAssertEqual(CatppuccinFlavor.latte.meta, "Light")
        XCTAssertEqual(CatppuccinFlavor.mocha.meta, "Dark · default")
        XCTAssertEqual(CatppuccinFlavor.mocha.displayName, "Mocha")
    }

    func testPaletteDefinesAllTwentySixTokensPerFlavor() {
        for flavor in CatppuccinFlavor.allCases {
            let palette = CatppuccinPalette.palette(for: flavor)
            for token in CatppuccinToken.allCases {
                let hex = palette.hex(token)
                XCTAssertTrue(hex.hasPrefix("#") && hex.count == 7,
                              "\(flavor.rawValue).\(token.rawValue) must be a #RRGGBB hex, got \(hex)")
            }
        }
    }

    /// The semantic tokens the UI actually consumes, pinned to the LOCKED
    /// hex values of the approved palette (catppuccin-palette.json).
    func testSemanticTokenHexesMatchTheLockedPalette() {
        let locked: [CatppuccinFlavor: [CatppuccinToken: String]] = [
            .latte: [
                .base: "#eff1f5", .mantle: "#e6e9ef", .crust: "#dce0e8",
                .surface0: "#ccd0da", .surface1: "#bcc0cc", .surface2: "#acb0be",
                .overlay0: "#9ca0b0", .overlay1: "#8c8fa1", .overlay2: "#7c7f93",
                .text: "#4c4f69", .subtext1: "#5c5f77", .subtext0: "#6c6f85",
                .mauve: "#8839ef", .red: "#d20f39", .peach: "#fe640b",
                .yellow: "#df8e1d", .green: "#40a02b", .teal: "#179299",
                .blue: "#1e66f5", .pink: "#ea76cb", .sapphire: "#209fb5",
            ],
            .frappe: [
                .base: "#303446", .mantle: "#292c3c", .crust: "#232634",
                .surface0: "#414559", .surface1: "#51576d", .surface2: "#626880",
                .overlay0: "#737994", .overlay1: "#838ba7", .overlay2: "#949cbb",
                .text: "#c6d0f5", .subtext1: "#b5bfe2", .subtext0: "#a5adce",
                .mauve: "#ca9ee6", .red: "#e78284", .peach: "#ef9f76",
                .yellow: "#e5c890", .green: "#a6d189", .teal: "#81c8be",
                .blue: "#8caaee", .pink: "#f4b8e4", .sapphire: "#85c1dc",
            ],
            .macchiato: [
                .base: "#24273a", .mantle: "#1e2030", .crust: "#181926",
                .surface0: "#363a4f", .surface1: "#494d64", .surface2: "#5b6078",
                .overlay0: "#6e738d", .overlay1: "#8087a2", .overlay2: "#939ab7",
                .text: "#cad3f5", .subtext1: "#b8c0e0", .subtext0: "#a5adcb",
                .mauve: "#c6a0f6", .red: "#ed8796", .peach: "#f5a97f",
                .yellow: "#eed49f", .green: "#a6da95", .teal: "#8bd5ca",
                .blue: "#8aadf4", .pink: "#f5bde6", .sapphire: "#7dc4e4",
            ],
            .mocha: [
                .base: "#1e1e2e", .mantle: "#181825", .crust: "#11111b",
                .surface0: "#313244", .surface1: "#45475a", .surface2: "#585b70",
                .overlay0: "#6c7086", .overlay1: "#7f849c", .overlay2: "#9399b2",
                .text: "#cdd6f4", .subtext1: "#bac2de", .subtext0: "#a6adc8",
                .mauve: "#cba6f7", .red: "#f38ba8", .peach: "#fab387",
                .yellow: "#f9e2af", .green: "#a6e3a1", .teal: "#94e2d5",
                .blue: "#89b4fa", .pink: "#f5c2e7", .sapphire: "#74c7ec",
            ],
        ]
        for flavor in CatppuccinFlavor.allCases {
            let palette = CatppuccinPalette.palette(for: flavor)
            for (token, expected) in locked[flavor] ?? [:] {
                XCTAssertEqual(palette.hex(token), expected,
                               "\(flavor.rawValue).\(token.rawValue)")
            }
        }
    }

    func testAccentIsMauvePerFlavorAndNeverTeal() {
        for flavor in CatppuccinFlavor.allCases {
            // SAFETY: a fresh UUID-based suite name is always a valid suite.
            let store = ThemeStore(defaults: UserDefaults(suiteName: "accent-\(flavor.rawValue)-\(UUID().uuidString)")!,
                                   reduceMotionProvider: { false })
            store.setFlavor(flavor)
            XCTAssertEqual(store.accent.hexDescription, store.palette.hex(.mauve),
                           "the UI accent must be the flavor's mauve token")
            XCTAssertNotEqual(store.accent.hexDescription, store.palette.hex(.teal),
                              "the UI accent (mauve) must never be teal (teal = working state only)")
        }
    }

    func testAnsiSlotsResolvePerFlavor() {
        let theme = { (flavor: CatppuccinFlavor) in
            // SAFETY: a fresh UUID-based suite name is always a valid suite.
            let store = ThemeStore(defaults: UserDefaults(suiteName: "ansi-\(flavor.rawValue)-\(UUID().uuidString)")!,
                                   reduceMotionProvider: { false })
            store.setFlavor(flavor)
            return store
        }
        // Latte arm (neutral slots step darker for the light panel).
        XCTAssertEqual(theme(.latte).ansiHex(.black), "#5c5f77")   // subtext1
        XCTAssertEqual(theme(.latte).ansiHex(.red), "#d20f39")
        XCTAssertEqual(theme(.latte).ansiHex(.green), "#40a02b")
        XCTAssertEqual(theme(.latte).ansiHex(.yellow), "#df8e1d")
        XCTAssertEqual(theme(.latte).ansiHex(.blue), "#1e66f5")
        XCTAssertEqual(theme(.latte).ansiHex(.magenta), "#ea76cb") // pink
        XCTAssertEqual(theme(.latte).ansiHex(.cyan), "#179299")    // teal
        XCTAssertEqual(theme(.latte).ansiHex(.white), "#6c6f85")   // subtext0
        XCTAssertEqual(theme(.latte).ansiHex(.brightBlack), "#8c8fa1") // overlay1
        XCTAssertEqual(theme(.latte).ansiHex(.brightWhite), "#5c5f77") // subtext1
        // Dark arm (Mocha; Frappé + Macchiato share the mapping but each
        // resolves its OWN surface1/neutral hexes).
        XCTAssertEqual(theme(.mocha).ansiHex(.black), "#45475a")   // surface1
        XCTAssertEqual(theme(.mocha).ansiHex(.red), "#f38ba8")
        XCTAssertEqual(theme(.mocha).ansiHex(.green), "#a6e3a1")
        XCTAssertEqual(theme(.mocha).ansiHex(.yellow), "#f9e2af")
        XCTAssertEqual(theme(.mocha).ansiHex(.blue), "#89b4fa")
        XCTAssertEqual(theme(.mocha).ansiHex(.magenta), "#f5c2e7") // pink
        XCTAssertEqual(theme(.mocha).ansiHex(.cyan), "#94e2d5")    // teal
        XCTAssertEqual(theme(.mocha).ansiHex(.white), "#bac2de")   // subtext1
        XCTAssertEqual(theme(.mocha).ansiHex(.brightBlack), "#7f849c") // overlay1
        XCTAssertEqual(theme(.mocha).ansiHex(.brightWhite), "#a6adc8") // subtext0
        XCTAssertEqual(theme(.frappe).ansiHex(.black), "#51576d",
                       "frappe black = its own surface1")
        XCTAssertEqual(theme(.macchiato).ansiHex(.black), "#494d64",
                       "macchiato black = its own surface1")
    }

    func testTailSemanticTokensFollowTheLockedFlavorRules() {
        // SAFETY: a fresh UUID-based suite name is always a valid suite.
        let store = ThemeStore(defaults: UserDefaults(suiteName: "tail-semantics-\(UUID().uuidString)")!,
                               reduceMotionProvider: { false })
        store.setFlavor(.mocha)
        XCTAssertEqual(store.tailBackground.hexDescription, "#181825",
                       "dark output panel recesses toward mantle")
        XCTAssertEqual(store.tailInk.hexDescription, "#cdd6f4", "ink = text")
        XCTAssertEqual(store.tailMuted.hexDescription, "#a6adc8", "dark muted = subtext0")
        XCTAssertEqual(store.codeDeletion.hexDescription, "#f38ba8", "deletion = ANSI red")
        XCTAssertEqual(store.codeAddition.hexDescription, "#a6e3a1", "addition = ANSI green")
        XCTAssertEqual(store.codeKeyword.hexDescription, "#f9e2af", "hunks/keywords = ANSI yellow")
        XCTAssertEqual(store.codeString.hexDescription, "#89b4fa", "strings = ANSI blue")
        XCTAssertEqual(store.codeComment.hexDescription, "#7f849c", "comments = ANSI bright-black")
        store.setFlavor(.latte)
        XCTAssertEqual(store.tailBackground.hexDescription, "#eff1f5",
                       "Latte output panel goes LIGHTER (base) — the accepted recess rule")
        XCTAssertEqual(store.tailMuted.hexDescription, "#5c5f77",
                       "Latte muted tier = subtext1 (binding; not subtext0)")
        XCTAssertEqual(store.codeDeletion.hexDescription, "#d20f39")
        XCTAssertEqual(store.codeAddition.hexDescription, "#40a02b")
        XCTAssertEqual(store.codeKeyword.hexDescription, "#df8e1d")
        XCTAssertEqual(store.codeString.hexDescription, "#1e66f5")
        XCTAssertEqual(store.codeComment.hexDescription, "#8c8fa1")
    }

    func testRoleColorsUseThePaletteRoles() {
        // SAFETY: a fresh UUID-based suite name is always a valid suite.
        let store = ThemeStore(defaults: UserDefaults(suiteName: "roles-\(UUID().uuidString)")!,
                               reduceMotionProvider: { false })
        store.setFlavor(.mocha)
        XCTAssertEqual(store.roleColor(for: .agent).hexDescription, "#cba6f7", "assistant = accent (mauve)")
        XCTAssertEqual(store.roleColor(for: .user).hexDescription, "#89b4fa", "you = blue")
        XCTAssertEqual(store.roleColor(for: .tool).hexDescription, "#fab387", "tool = peach")
        XCTAssertEqual(store.roleColor(for: .system).hexDescription, "#a6adc8")
        XCTAssertEqual(store.roleColor(for: .unknown).hexDescription, "#a6adc8")
    }
}

// MARK: - Repo hue function (#371 Q15 — ported, not tabled)

final class RepoHueTests: XCTestCase {

    func testFnv1a32KnownVectors() {
        XCTAssertEqual(RepoHue.fnv1a32(""), 0x811C9DC5)
        XCTAssertEqual(RepoHue.fnv1a32("a"), 0xE40C292C)
        XCTAssertEqual(RepoHue.fnv1a32("hello"), 0x4F9F2CAB)
        // ASCII repo names: hash mod 8 matches the approved fixture.py port.
        XCTAssertEqual(RepoHue.fnv1a32("demo-atlas") % 8, 0)
    }

    func testDemoFleetHuesAreDeterministicAndOrderIndependent() {
        let repos = ["demo-atlas", "demo-garden", "demo-ledger", "demo-orbit"]
        let expected: [String: CatppuccinToken] = [
            "demo-atlas": .blue, "demo-garden": .sapphire,
            "demo-ledger": .teal, "demo-orbit": .green,
        ]
        XCTAssertEqual(RepoHue.hues(for: repos), expected)
        XCTAssertEqual(RepoHue.hues(for: repos.shuffled()), expected,
                       "assignment is independent of fleet arrival order")
        XCTAssertEqual(RepoHue.hues(for: ["demo-atlas"]),
                       ["demo-atlas": .blue],
                       "a lone repo keeps its fnv slot")
    }

    func testCollidingReposLinearProbeToDistinctSlots() {
        // repo0..repo7 contain fnv%8 collisions; the linear probe must give
        // every repo its own ring slot (vector computed from fixture.py).
        let repos = (0..<8).map { "repo\($0)" }
        XCTAssertEqual(RepoHue.hues(for: repos), [
            "repo0": .pink, "repo1": .yellow, "repo2": .peach,
            "repo3": .teal, "repo4": .green, "repo5": .blue,
            "repo6": .sapphire, "repo7": .mauve,
        ])
    }

    func testMoreReposThanRingSlotsStillAssignEveryRepo() {
        let repos = (0..<12).map { "repo\($0)" }
        let hues = RepoHue.hues(for: repos)
        XCTAssertEqual(hues.count, 12, "every repo must receive a hue")
        XCTAssertTrue(hues.values.allSatisfy(RepoHue.ring.contains),
                      "hues stay inside the locked accent ring")
    }

    func testRepoHueRingIsTheLockedAccentRing() {
        XCTAssertEqual(RepoHue.ring.map(\.rawValue),
                       ["blue", "sapphire", "teal", "green",
                        "yellow", "peach", "mauve", "pink"])
    }
}

// MARK: - ThemeStore behavior

@MainActor
final class ThemeStoreTests: XCTestCase {

    private func suite(_ name: String) -> UserDefaults {
        let suiteName = "theme-store-\(name)-\(UUID().uuidString)"
        // SAFETY: a fresh UUID-based suite name is always a valid suite.
        let defaults = UserDefaults(suiteName: suiteName)!
        defaults.removePersistentDomain(forName: suiteName)
        return defaults
    }

    func testDefaultFlavorIsMocha() {
        let store = ThemeStore(defaults: suite("default"), reduceMotionProvider: { false })
        XCTAssertEqual(store.flavor, .mocha)
    }

    func testSelectionPersistsAcrossStores() {
        let defaults = suite("persist")
        let store = ThemeStore(defaults: defaults, reduceMotionProvider: { false })
        store.setFlavor(.latte)
        let reloaded = ThemeStore(defaults: defaults, reduceMotionProvider: { false })
        XCTAssertEqual(reloaded.flavor, .latte,
                       "the Settings → Appearance choice must survive relaunch")
    }

    func testReduceMotionPlumbingReflectsTheProvider() {
        let reduced = ThemeStore(defaults: suite("rm-true"),
                                 reduceMotionProvider: { true })
        XCTAssertTrue(reduced.reduceMotion)
        let normal = ThemeStore(defaults: suite("rm-false"),
                                reduceMotionProvider: { false })
        XCTAssertFalse(normal.reduceMotion)
    }
}

// MARK: - Color hexDescription (test-side: Color → #RRGGBB for assertions)

extension Color {
    /// The sRGB hex of this color in the current trait environment — used
    /// ONLY by the theme tests to assert token values (an opaque Color from
    /// a #RRGGBB source round-trips exactly). Lowercase, matching the
    /// palette hex strings.
    var hexDescription: String {
        let resolved = self.resolve(in: EnvironmentValues())
        let r = Int(round(resolved.red * 255))
        let g = Int(round(resolved.green * 255))
        let b = Int(round(resolved.blue * 255))
        return String(format: "#%02x%02x%02x", r, g, b)
    }
}
