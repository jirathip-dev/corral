import Foundation
import SwiftUI
import UIKit

// MARK: - #372 Catppuccin theming foundation
//
// The whole app renders through one `ThemeStore` (environment object) which
// owns the active Catppuccin flavor (Latte / Frappé / Macchiato / Mocha,
// default Mocha) and exposes every semantic token the UI consumes. Hexes are
// the LOCKED values from the approved 371-372 design round's
// `catppuccin-palette.json` — real Catppuccin colors only, no invented hexes,
// and no legacy GitHub-dark literals anywhere in the UI layer (audit gate in
// the #372 report greps for them).
//
// #371 (board v2) and #373 (recents block-per-run) consume these tokens later;
// this lane lands the foundation + the complete token swap of the CURRENT
// surfaces (board, rows, chips, recents rail, settings, sheet chrome).

// MARK: - Flavor

/// The four locked Catppuccin flavors, in the Settings picker order
/// (Latte → Frappé → Macchiato → Mocha, matching the approved Appearance
/// frame). Mocha is the default.
enum CatppuccinFlavor: String, CaseIterable, Codable, Hashable, Sendable {
    case latte
    case frappe
    case macchiato
    case mocha

    /// Default flavor: Mocha (spec lock).
    static let `default` = CatppuccinFlavor.mocha

    var displayName: String {
        switch self {
        case .latte: return "Latte"
        case .frappe: return "Frappé"
        case .macchiato: return "Macchiato"
        case .mocha: return "Mocha"
        }
    }

    /// The Appearance-row meta line (prototype lock: "Light", "Dark",
    /// "Dark", "Dark · default").
    var meta: String {
        switch self {
        case .latte: return "Light"
        case .frappe, .macchiato: return "Dark"
        case .mocha: return "Dark · default"
        }
    }

    /// Latte is the only light flavor; the app forces the matching system
    /// color scheme so native chrome (navigation bars, materials, form
    /// cells) rides the same light/dark axis as the palette.
    var isLight: Bool { self == .latte }
}

// MARK: - Palette tokens

/// The 26-token Catppuccin palette vocabulary (one per flavor).
enum CatppuccinToken: String, CaseIterable, Hashable, Sendable {
    case rosewater, flamingo, pink, mauve, red, maroon, peach, yellow
    case green, teal, sky, sapphire, blue, lavender
    case text, subtext1, subtext0, overlay2, overlay1, overlay0
    case surface2, surface1, surface0, base, mantle, crust
}

/// One flavor's palette: hex strings (the locked values; tests assert them
/// verbatim) plus the `Color` accessor the views consume.
struct CatppuccinPalette: Equatable, Sendable {
    let flavor: CatppuccinFlavor
    private let hexes: [CatppuccinToken: String]

    static func palette(for flavor: CatppuccinFlavor) -> CatppuccinPalette {
        CatppuccinPalette(flavor: flavor, hexes: Self.table[flavor] ?? [:])
    }

    /// The Catppuccin hex for one token (verbatim from the locked palette).
    func hex(_ token: CatppuccinToken) -> String {
        // Palette-table completeness is contract-tested for every flavor, so
        // the dictionary lookup below can never miss; the fallback still
        // returns a real Catppuccin hex rather than trapping.
        hexes[token] ?? Self.fallbackHex(for: token)
    }

    func color(_ token: CatppuccinToken) -> Color {
        Color(uiColor: UIColor(catppuccinHex: hex(token)))
    }

    // MARK: Locked table (#372; source = approved catppuccin-palette.json)

    private static let table: [CatppuccinFlavor: [CatppuccinToken: String]] = [
        .latte: [
            .rosewater: "#dc8a78", .flamingo: "#dd7878", .pink: "#ea76cb",
            .mauve: "#8839ef", .red: "#d20f39", .maroon: "#e64553",
            .peach: "#fe640b", .yellow: "#df8e1d", .green: "#40a02b",
            .teal: "#179299", .sky: "#04a5e5", .sapphire: "#209fb5",
            .blue: "#1e66f5", .lavender: "#7287fd", .text: "#4c4f69",
            .subtext1: "#5c5f77", .subtext0: "#6c6f85", .overlay2: "#7c7f93",
            .overlay1: "#8c8fa1", .overlay0: "#9ca0b0", .surface2: "#acb0be",
            .surface1: "#bcc0cc", .surface0: "#ccd0da", .base: "#eff1f5",
            .mantle: "#e6e9ef", .crust: "#dce0e8",
        ],
        .frappe: [
            .rosewater: "#f2d5cf", .flamingo: "#eebebe", .pink: "#f4b8e4",
            .mauve: "#ca9ee6", .red: "#e78284", .maroon: "#ea999c",
            .peach: "#ef9f76", .yellow: "#e5c890", .green: "#a6d189",
            .teal: "#81c8be", .sky: "#99d1db", .sapphire: "#85c1dc",
            .blue: "#8caaee", .lavender: "#babbf1", .text: "#c6d0f5",
            .subtext1: "#b5bfe2", .subtext0: "#a5adce", .overlay2: "#949cbb",
            .overlay1: "#838ba7", .overlay0: "#737994", .surface2: "#626880",
            .surface1: "#51576d", .surface0: "#414559", .base: "#303446",
            .mantle: "#292c3c", .crust: "#232634",
        ],
        .macchiato: [
            .rosewater: "#f4dbd6", .flamingo: "#f0c6c6", .pink: "#f5bde6",
            .mauve: "#c6a0f6", .red: "#ed8796", .maroon: "#ee99a0",
            .peach: "#f5a97f", .yellow: "#eed49f", .green: "#a6da95",
            .teal: "#8bd5ca", .sky: "#91d7e3", .sapphire: "#7dc4e4",
            .blue: "#8aadf4", .lavender: "#b7bdf8", .text: "#cad3f5",
            .subtext1: "#b8c0e0", .subtext0: "#a5adcb", .overlay2: "#939ab7",
            .overlay1: "#8087a2", .overlay0: "#6e738d", .surface2: "#5b6078",
            .surface1: "#494d64", .surface0: "#363a4f", .base: "#24273a",
            .mantle: "#1e2030", .crust: "#181926",
        ],
        .mocha: [
            .rosewater: "#f5e0dc", .flamingo: "#f2cdcd", .pink: "#f5c2e7",
            .mauve: "#cba6f7", .red: "#f38ba8", .maroon: "#eba0ac",
            .peach: "#fab387", .yellow: "#f9e2af", .green: "#a6e3a1",
            .teal: "#94e2d5", .sky: "#89dceb", .sapphire: "#74c7ec",
            .blue: "#89b4fa", .lavender: "#b4befe", .text: "#cdd6f4",
            .subtext1: "#bac2de", .subtext0: "#a6adc8", .overlay2: "#9399b2",
            .overlay1: "#7f849c", .overlay0: "#6c7086", .surface2: "#585b70",
            .surface1: "#45475a", .surface0: "#313244", .base: "#1e1e2e",
            .mantle: "#181825", .crust: "#11111b",
        ],
    ]

    /// A real Catppuccin hex for a token that is somehow absent (never hit
    /// while the completeness test is green). Uses the flavor's own values
    /// when the dictionary exists but the key does not.
    private static func fallbackHex(for token: CatppuccinToken) -> String {
        table[.mocha]?[token] ?? "#cdd6f4"
    }
}

// MARK: - ANSI remap (per-flavor)

/// The ANSI slots the recents tail resolves. The #373 recents-stance defines
/// Catppuccin's per-flavor ANSI mapping over these ten slots (its ANSI map
/// "complete for both flavors" gate); the dark flavors share the Mocha arm.
/// ANSI codes 0-7 are black/red/green/yellow/blue/magenta/cyan/white, 8-15
/// their bright variants (bright black / bright white defined below; the
/// other six brights resolve to the same accent as their base slot, which is
/// the Catppuccin ANSI set's behavior — one accent per hue).
enum CatppuccinAnsiSlot: Int, CaseIterable, Sendable {
    case black = 0, red, green, yellow, blue, magenta, cyan, white
    case brightBlack, brightWhite

    /// The palette token an ANSI slot maps to for one flavor. Verbatim from
    /// the design round's ANSI table (Mocha arm = Frappé/Macchiato/Mocha;
    /// the Latte arm differs only in the neutral slots, which need darker
    /// steps on the light panel).
    static func token(for slot: CatppuccinAnsiSlot,
                      flavor: CatppuccinFlavor) -> CatppuccinToken {
        if flavor == .latte {
            switch slot {
            case .black: return .subtext1
            case .red: return .red
            case .green: return .green
            case .yellow: return .yellow
            case .blue: return .blue
            case .magenta: return .pink
            case .cyan: return .teal
            case .white: return .subtext0
            case .brightBlack: return .overlay1
            case .brightWhite: return .subtext1
            }
        }
        switch slot {
        case .black: return .surface1
        case .red: return .red
        case .green: return .green
        case .yellow: return .yellow
        case .blue: return .blue
        case .magenta: return .pink
        case .cyan: return .teal
        case .white: return .subtext1
        case .brightBlack: return .overlay1
        case .brightWhite: return .subtext0
        }
    }
}

extension UIColor {
    /// `#RRGGBB` → opaque `UIColor`. Feeds the theme only (all hexes come
    /// from the locked Catppuccin tables above — never legacy literals).
    convenience init(catppuccinHex: String) {
        let hexString = catppuccinHex.hasPrefix("#")
            ? String(catppuccinHex.dropFirst()) : catppuccinHex
        var value: UInt64 = 0
        Scanner(string: hexString).scanHexInt64(&value)
        self.init(red: CGFloat((value & 0xFF0000) >> 16) / 255,
                  green: CGFloat((value & 0x00FF00) >> 8) / 255,
                  blue: CGFloat(value & 0x0000FF) / 255,
                  alpha: 1)
    }
}

// MARK: - Repo hue assignment (#371 Q15, foundation)

/// Deterministic repo → accent-token assignment, ported from the approved
/// prototype's `fixture.py`:
///
///     fnv1a32(repo) % 8 indexes the hue ring; collisions linear-probe
///     forward. Repos are processed in alphabetical order so the result is
///     stable for a given repo set and independent of fleet arrival order.
///
/// The function is ported (never a table) so #371 board-v2 subgroups and any
/// future repo surface share the exact same deterministic hues.
enum RepoHue {
    /// The locked accent ring in fixed order (blue → pink).
    static let ring: [CatppuccinToken] = [
        .blue, .sapphire, .teal, .green, .yellow, .peach, .mauve, .pink,
    ]

    /// FNV-1a 32-bit (the prototype's `fnv1a32` verbatim).
    static func fnv1a32(_ value: String) -> UInt32 {
        var hash: UInt32 = 0x811C9DC5
        for byte in value.utf8 {
            hash ^= UInt32(byte)
            hash = hash &* 0x01000193
        }
        return hash
    }

    /// Assign hues for a repo set (sorted input, linear-probe collision
    /// handling). `Other` (no repo / unknown) is NOT in this set — it is
    /// always the palette's surface2 gray, never an accent.
    static func hues(for repos: [String]) -> [String: CatppuccinToken] {
        var taken: [Int: String] = [:]
        var out: [String: CatppuccinToken] = [:]
        for repo in repos.sorted() {
            let start = Int(fnv1a32(repo) % UInt32(ring.count))
            var assigned = ring[start]
            for step in 0..<ring.count {
                let index = (start + step) % ring.count
                if taken[index] == nil {
                    taken[index] = repo
                    assigned = ring[index]
                    break
                }
            }
            out[repo] = assigned
        }
        return out
    }
}

// MARK: - ThemeStore (the observable theme)

/// The app-wide theme: the active flavor plus the Reduce Motion plumbing.
/// Lives as an environment object above the root view so every surface
/// re-renders when the flavor flips (Settings → Appearance is the ONLY
/// picker — placement lock) or when the system Reduce Motion setting
/// changes.
@MainActor
final class ThemeStore: ObservableObject {
    /// Persisted selection key (`UserDefaults`), mirroring the
    /// notifications-pairing key convention.
    static let flavorKey = "fleetnotifier.themeFlavor"

    @Published var flavor: CatppuccinFlavor {
        didSet {
            defaults.set(flavor.rawValue, forKey: Self.flavorKey)
        }
    }

    /// System Reduce Motion state (plumbing for the theme layer; the board's
    /// working-motion chip — #371 — consumes this; this lane already gates
    /// the recents auto-scroll animation on it).
    @Published private(set) var reduceMotion: Bool

    private let defaults: UserDefaults
    private let reduceMotionProvider: @Sendable () -> Bool
    private var reduceMotionObserver: NSObjectProtocol?

    /// - Parameters:
    ///   - defaults: persistence store (inject a suite in tests).
    ///   - reduceMotionProvider: injectable read of the system setting
    ///     (defaults to `UIAccessibility.isReduceMotionEnabled`).
    init(defaults: UserDefaults = .standard,
         reduceMotionProvider: @escaping @Sendable () -> Bool = {
             UIAccessibility.isReduceMotionEnabled
         }) {
        self.defaults = defaults
        self.reduceMotionProvider = reduceMotionProvider
        self.flavor = CatppuccinFlavor(
            rawValue: defaults.string(forKey: Self.flavorKey) ?? ""
        ) ?? .default
        self.reduceMotion = reduceMotionProvider()
        reduceMotionObserver = NotificationCenter.default.addObserver(
            forName: UIAccessibility.reduceMotionStatusDidChangeNotification,
            object: nil,
            queue: .main
        ) { [weak self] _ in
            Task { @MainActor in
                self?.reduceMotion = self?.reduceMotionProvider() ?? false
            }
        }
    }

    deinit {
        if let reduceMotionObserver {
            NotificationCenter.default.removeObserver(reduceMotionObserver)
        }
    }

    /// Settings → Appearance selection. Persists immediately (didSet).
    func setFlavor(_ flavor: CatppuccinFlavor) {
        self.flavor = flavor
    }

    var palette: CatppuccinPalette {
        CatppuccinPalette.palette(for: flavor)
    }
}

// MARK: - Semantic tokens (single place views read)

extension ThemeStore {
    // Hierarchy (base / mantle / crust / surface / overlay / text /
    // subtext) plus the UI accent — Mauve per the DESIGN APPROVED binding
    // (teal is reserved for the working state ONLY; a selected filter chip
    // is never teal).
    var base: Color { color(.base) }
    var mantle: Color { color(.mantle) }
    var crust: Color { color(.crust) }
    var surface0: Color { color(.surface0) }
    var surface1: Color { color(.surface1) }
    var surface2: Color { color(.surface2) }
    var overlay0: Color { color(.overlay0) }
    var overlay1: Color { color(.overlay1) }
    var overlay2: Color { color(.overlay2) }
    var text: Color { color(.text) }
    var subtext1: Color { color(.subtext1) }
    var subtext0: Color { color(.subtext0) }
    var accent: Color { color(.mauve) }
    var blue: Color { color(.blue) }
    var teal: Color { color(.teal) }
    var green: Color { color(.green) }
    var yellow: Color { color(.yellow) }
    var peach: Color { color(.peach) }
    var red: Color { color(.red) }
    var pink: Color { color(.pink) }

    func color(_ token: CatppuccinToken) -> Color {
        palette.color(token)
    }

    /// The LOCKED state→palette-token mapping: working=teal, blocked=red,
    /// done=green, idle=subtext0, unknown=surface2 (issue spec + the
    /// approved design round). ONE shared mapping: both the `Color` the
    /// views consume (`stateColor(for:)`) and the hex accessor
    /// (`stateHex(for:)`) resolve through it, so the contract cannot
    /// regress through a parallel hand-written switch.
    private static func stateToken(for state: AgentState) -> CatppuccinToken {
        switch state {
        case .blocked: return .red
        case .working: return .teal
        case .done: return .green
        case .idle: return .subtext0
        case .unknown: return .surface2
        }
    }

    /// The state color the whole UI consumes (row dots, badges, sheet
    /// headers) — resolved from the single shared state mapping.
    func stateColor(for state: AgentState) -> Color {
        color(Self.stateToken(for: state))
    }

    /// The hex view of the same shared mapping (used by tests and any
    /// hex-literal consumer — never a parallel switch).
    func stateHex(for state: AgentState) -> String {
        palette.hex(Self.stateToken(for: state))
    }

    /// The accent-ring hue for one repo (`Other`/no-repo = surface2 gray).
    func repoHue(for repo: String, among repos: [String]) -> CatppuccinToken {
        RepoHue.hues(for: repos)[repo] ?? .surface2
    }

    func repoHueColor(for repo: String, among repos: [String]) -> Color {
        color(repoHue(for: repo, among: repos))
    }

    // MARK: #371 board-v2 chip surfaces (derived mixes, single palette source)
    //
    // The approved board renders repo/state hues on TINTED chip surfaces.
    // The design round derives every tint/ink as an sRGB mix of a VERBATIM
    // palette hue over a palette surface (tune.py in the approved
    // 371-372 board-theming bundle):
    //   repo label chip fill = mix(hue 15 %, base);  border = mix(hue 38 %, base)
    //   repo subgroup band   = mix(hue  9 %, mantle)
    //   state chip fill      = mix(state 17 %, base); border = mix(state 34 %, base)
    // Label ink = mix(hue X %, text) with X per flavor — Latte 29 %,
    // Frappé 76 %, Macchiato/Mocha 100 % — the strongest hue share whose
    // worst-case contrast still clears 4.5:1 on the chip fill for every
    // accent-ring hue (derived by tune.py, not guessed). `Other`
    // (surface2) can never reach AA mixed into its own tint, so its ink is
    // subtext1 verbatim (design lock) — all resolved from the SAME palette
    // tables, never a parallel hex set.

    /// The locked per-flavor label-ink mix ratio (hue share of the mix).
    private static let repoInkMixRatio: [CatppuccinFlavor: Double] = [
        .latte: 0.29, .frappe: 0.76, .macchiato: 1.0, .mocha: 1.0,
    ]

    /// The repo/subgroup label ink for one hue token: the hue mixed toward
    /// the flavor's text token at the locked ratio; `Other` (surface2)
    /// falls back to subtext1 (design lock — gray can never clear AA
    /// mixed, so it stays a neutral tier).
    func repoInk(for hue: CatppuccinToken) -> Color {
        if hue == .surface2 { return color(.subtext1) }
        let ratio = Self.repoInkMixRatio[flavor] ?? 1.0
        return Color(uiColor: UIColor(
            catppuccinHex: Self.mixedHex(palette.hex(hue),
                                         at: ratio,
                                         over: palette.hex(.text))))
    }

    /// The tinted chip surfaces of one state (working=teal, blocked=red,
    /// done=green, idle=subtext0, unknown=surface2 — the SAME single
    /// state mapping `stateColor(for:)` uses, never a parallel switch).
    func stateChipFill(for state: AgentState) -> Color {
        mixed(Self.stateToken(for: state), at: 0.17, over: .base)
    }

    func stateChipBorder(for state: AgentState) -> Color {
        mixed(Self.stateToken(for: state), at: 0.34, over: .base)
    }

    /// The repo subgroup header band (hue 9 % over mantle) — the full-width
    /// tinted strip under the status header.
    func repoBand(for hue: CatppuccinToken) -> Color {
        mixed(hue, at: 0.09, over: .mantle)
    }

    /// The repo label chip fill/border (hue over base) on agent rows.
    func repoChipFill(for hue: CatppuccinToken) -> Color {
        mixed(hue, at: 0.15, over: .base)
    }

    func repoChipBorder(for hue: CatppuccinToken) -> Color {
        mixed(hue, at: 0.38, over: .base)
    }

    /// One sRGB mix of two palette tokens: `hueFraction` of the hue token
    /// over the surface token (0…1).
    func mixed(_ hue: CatppuccinToken, at hueFraction: Double,
               over surface: CatppuccinToken) -> Color {
        Color(uiColor: UIColor(
            catppuccinHex: Self.mixedHex(palette.hex(hue),
                                         at: hueFraction,
                                         over: palette.hex(surface))))
    }

    /// `#RRGGBB` × `#RRGGBB` sRGB linear mix (component-wise lerp),
    /// returning the `#rrggbb` hex — palette-token hexes only.
    ///
    /// Quantization lock: half-boundary components round to EVEN
    /// (`.toNearestOrEven`), matching the approved prototype's CSS
    /// `color-mix(in srgb, …)` render — pixel-sampled in the design round
    /// (e.g. Mocha blue-15 %-over-base = #2e344d, never #2e354d). A plain
    /// half-away-from-zero `.rounded()` diverges at exact x.5 channels.
    private static func mixedHex(_ hexA: String, at hueFraction: Double,
                                 over hexB: String) -> String {
        let (ar, ag, ab) = rgbComponents(hexA)
        let (br, bg, bb) = rgbComponents(hexB)
        let t = min(max(hueFraction, 0), 1)
        func lerp(_ a: Double, _ b: Double) -> Int {
            Int((a * t + b * (1 - t)).rounded(.toNearestOrEven))
        }
        return String(format: "#%02x%02x%02x", lerp(ar, br), lerp(ag, bg),
                      lerp(ab, bb))
    }

    /// `#RRGGBB` → sRGB components (0…255).
    private static func rgbComponents(_ hex: String) -> (Double, Double, Double) {
        let body = hex.hasPrefix("#") ? String(hex.dropFirst()) : hex
        var value: UInt64 = 0
        Scanner(string: body).scanHexInt64(&value)
        return (Double((value >> 16) & 0xFF),
                Double((value >> 8) & 0xFF),
                Double(value & 0xFF))
    }

    // MARK: Recents tail tokens
    //
    // The REAL recents tail is themed: text/subtext tokens + ANSI remap to
    // the active flavor (no legacy GitHub-dark hexes anywhere, incl. the
    // tail). The output panel recesses toward mantle on the dark flavors
    // and goes LIGHTER (base) on Latte — the accepted recess rule, because
    // recessing a light theme the same way sinks the ANSI hues into the
    // panel (measured in the design round's explore_panel.py).

    /// Output-panel background: base on Latte (recess rule), mantle on the
    /// dark flavors.
    var tailBackground: Color {
        flavor == .latte ? base : mantle
    }

    /// Tail body text (the daemon's raw output lines).
    var tailInk: Color { text }

    /// Tail secondary text. The Latte muted tier is subtext1 (binding:
    /// "muted tier on Latte = subtext1 not subtext0"); dark flavors use
    /// subtext0.
    var tailMuted: Color {
        flavor == .latte ? subtext1 : subtext0
    }

    /// Dim quiet text inside the tail (code line numbers, dim punctuation):
    /// overlay0 on the dark flavors, overlay2 on Latte (design QUIET map).
    var tailQuiet: Color {
        flavor == .latte ? overlay2 : overlay0
    }

    /// The continuous rail spine behind the tail rows — derived from the
    /// accent token at low opacity (the #361 R1 hairline rule, now mauve).
    var tailLine: Color { accent.opacity(0.18) }

    // MARK: Tail ANSI remap (per-flavor semantic colors)
    //
    // The tail's semantic colors resolve through the ACTIVE flavor's ANSI
    // hex sets: addition = ANSI green, deletion = ANSI red, hunk headers /
    // code keywords = ANSI yellow, string literals = ANSI blue, comments =
    // ANSI bright-black. Documented Latte exception (design accepted): on
    // the Latte base panel ANSI green (#40a02b) measures 2.96:1, ANSI yellow
    // (#df8e1d) 2.31:1, and ANSI bright-black (overlay1 #8c8fa1) 2.83:1 —
    // all < 3.0:1 (contrast.py in the design round). Semantics are
    // preserved: green stays green for a pass, yellow stays yellow for a
    // warning, and the +/- prefixes carry the diff meaning without hue; no
    // hexes were invented to force AA.

    /// One ANSI slot resolved for the active flavor.
    func ansiColor(_ slot: CatppuccinAnsiSlot) -> Color {
        color(CatppuccinAnsiSlot.token(for: slot, flavor: flavor))
    }

    func ansiHex(_ slot: CatppuccinAnsiSlot) -> String {
        palette.hex(CatppuccinAnsiSlot.token(for: slot, flavor: flavor))
    }

    var codeKeyword: Color { ansiColor(.yellow) }
    var codeString: Color { ansiColor(.blue) }
    var codeComment: Color { ansiColor(.brightBlack) }
    var codeAddition: Color { ansiColor(.green) }
    var codeDeletion: Color { ansiColor(.red) }

    /// Role colors for the rail transition markers (#361 lock remapped to
    /// the palette): Agent = accent (mauve), You = blue, Tool = peach.
    func roleColor(for kind: TranscriptBlockKind) -> Color {
        switch kind {
        case .user: return blue
        case .agent: return accent
        case .tool: return peach
        case .system, .unknown: return tailMuted
        }
    }

    /// Segment colors for the tail's code/diff highlighter.
    func segmentColor(for kind: RecentCodeSegmentKind) -> Color {
        switch kind {
        case .plain: return tailInk
        case .keyword: return codeKeyword
        case .string: return codeString
        case .addition: return codeAddition
        case .deletion: return codeDeletion
        case .comment: return codeComment
        }
    }
}

// MARK: - #385 translucent sheet backdrop (constants + WCAG math)

/// The #385 translucent-sheet contract: RecentOutputSheet and the Settings
/// sheet sit over a backdrop that lets the board content show through
/// softly (the approved terminal-transparency look; #373 AC that #378/#381
/// missed). Below iOS 26 the backdrop is the active flavor's base tinted at
/// `fallbackTintAlpha` over an ultra-thin material blur; iOS 26+ renders the
/// NATIVE Liquid Glass surface instead (see FleetViews.swift
/// `TranslucentSheetBackdrop`). Text layers that need guaranteed AA keep
/// their opaque token backing; the backdrop itself is verified against the
/// spec's 4.5:1 minimum in the worst underlying-content case by
/// `SheetBackdropTests`.
enum SheetBackdrop {
    /// Spec lock (#385): the fallback tint alpha must sit in this
    /// terminal-transparency band.
    static let fallbackTintAlphaRange: ClosedRange<Double> = 0.85...0.90

    /// The locked fallback tint alpha: theme base at 88 % over the backdrop
    /// blur material (inside the 0.85–0.90 spec band; tests assert both).
    static let fallbackTintAlpha: Double = 0.88

    /// The iOS 26+ Native Liquid Glass tint strength: the flavor's base
    /// token is applied to the glass at this opacity. A FULLY opaque tint
    /// paints the glass into a solid flat color (no board visible through
    /// the sheet — measured on the iOS 26.5 sim), so the theme hook stays
    /// a tint over the still-translucent glass material. Tests lock it to
    /// the 0.2–0.4 band and re-verify the sheet AC.
    static let glassTintOpacity: Double = 0.3
    static let glassTintOpacityRange: ClosedRange<Double> = 0.2...0.4

    /// The spec's minimum contrast for sheet content over the translucent
    /// backdrop (WCAG AA).
    static let minimumContrast: Double = 4.5

    /// `top` at `alpha` over `bottom` (sRGB component lerp), returning the
    /// composite `#rrggbb`. Rounding matches the palette mix helpers
    /// (half-boundary components round to even), so the tests' worst-case
    /// math equals what the renderer composites.
    static func blend(_ top: String, alpha: Double, over bottom: String) -> String {
        func component(_ hex: String, _ index: Int) -> Double {
            let body = hex.hasPrefix("#") ? String(hex.dropFirst()) : hex
            let start = body.index(body.startIndex, offsetBy: index * 2)
            let end = body.index(start, offsetBy: 2)
            return Double(UInt32(body[start..<end], radix: 16) ?? 0)
        }
        let t = min(max(alpha, 0), 1)
        var out = "#"
        for index in 0..<3 {
            let value = (component(top, index) * t
                         + component(bottom, index) * (1 - t))
                .rounded(.toNearestOrEven)
            out += String(format: "%02x", Int(value))
        }
        return out
    }

    /// WCAG 2.x relative luminance of a `#RRGGBB` hex.
    static func relativeLuminance(_ hex: String) -> Double {
        func channel(_ value: Double) -> Double {
            let s = value / 255
            return s <= 0.04045 ? s / 12.92 : pow((s + 0.055) / 1.055, 2.4)
        }
        func component(_ index: Int) -> Double {
            let body = hex.hasPrefix("#") ? String(hex.dropFirst()) : hex
            let start = body.index(body.startIndex, offsetBy: index * 2)
            let end = body.index(start, offsetBy: 2)
            return Double(UInt32(body[start..<end], radix: 16) ?? 0)
        }
        return 0.2126 * channel(component(0))
            + 0.7152 * channel(component(1))
            + 0.0722 * channel(component(2))
    }

    /// WCAG contrast ratio (1…21) between two `#RRGGBB` hexes.
    static func contrastRatio(_ first: String, _ second: String) -> Double {
        let a = relativeLuminance(first)
        let b = relativeLuminance(second)
        let lighter = max(a, b)
        let darker = min(a, b)
        return (lighter + 0.05) / (darker + 0.05)
    }

    /// The worst contrast `ink` holds over the translucent sheet tint: the
    /// tint composite is `tint` at `fallbackTintAlpha` over each candidate
    /// underlying hex, and the worst (lowest) ratio wins. Used by the tests
    /// to prove the spec's 4.5:1 minimum on the darkest underlying-content
    /// case for every palette token the sheet could float over.
    static func worstContrast(ink: String, tint: String,
                              over candidates: [String]) -> Double {
        candidates
            .map { contrastRatio(ink, blend(tint, alpha: fallbackTintAlpha,
                                            over: $0)) }
            .min() ?? 0
    }
}
