import Foundation
import SwiftUI

// MARK: - Color(light:dark:)

extension UIColor {
    /// `#RRGGBB` → opaque `UIColor`. Hexes come from
    /// `contracts/state-tokens.json` (single source of truth for colors and
    /// marks; the #354 L2 board labels deliberately diverge from that
    /// contract's display words — see below).
    convenience init(hex: String) {
        let clean = hex.trimmingCharacters(in: .whitespacesAndNewlines)
        let hexString = clean.hasPrefix("#") ? String(clean.dropFirst()) : clean
        var value: UInt64 = 0
        Scanner(string: hexString).scanHexInt64(&value)
        self.init(red: CGFloat((value & 0xFF0000) >> 16) / 255,
                  green: CGFloat((value & 0x00FF00) >> 8) / 255,
                  blue: CGFloat(value & 0x0000FF) / 255,
                  alpha: 1)
    }
}

extension Color {
    /// Dynamic color from light/dark `UIColor`s. SwiftUI has no built-in
    /// `Color(light:dark:)`, so this wraps the UIKit dynamic provider.
    init(light: UIColor, dark: UIColor) {
        self.init(uiColor: UIColor { traits in
            traits.userInterfaceStyle == .dark ? dark : light
        })
    }
}

// MARK: - StateStyle

/// Read-model styling for one `AgentState`.
///
/// Colors + glyph marks mirror `contracts/state-tokens.json` (both clients
/// consume that contract so state→color can never diverge). The LABEL is the
/// herdr RAW state token verbatim — working / idle / blocked / unknown —
/// per the #354 spec amendment (09-02): herdr 0.8.2 has NO `done` (finished
/// Hermes panes fall back to idle), and Corral-invented wording (Needs-you /
/// Supervising / Finished) is banned on the read-only board. `contracts/
/// state-tokens.json` itself is shared with the egui client (L3-owned at the
/// cut base), so this divergence is iOS-local until egui lands its own cut.
///
/// Ranks encode the board's attention order: blocked → working → idle →
/// unknown, with a wire-`done` record ranked with idle (its herdr fallback).
struct StateStyle: Equatable, Sendable {
    let state: AgentState
    let label: String
    let mark: String
    let rank: Int
    let darkHex: String
    let lightHex: String

    static func style(for state: AgentState) -> StateStyle {
        switch state {
        case .blocked:
            return StateStyle(state: .blocked, label: "blocked", mark: "alert",
                              rank: 0, darkHex: "#F85149", lightHex: "#CF222E")
        case .working:
            return StateStyle(state: .working, label: "working", mark: "ring",
                              rank: 1, darkHex: "#58A6FF", lightHex: "#0969DA")
        case .idle:
            return StateStyle(state: .idle, label: "idle", mark: "dot",
                              rank: 2, darkHex: "#8B949E", lightHex: "#6E7781")
        case .done:
            // Finished Hermes panes fall back to idle in herdr 0.8.2; a
            // wire `done` (transitional daemon) ranks + reads as idle.
            return StateStyle(state: .done, label: "done", mark: "check",
                              rank: 2, darkHex: "#D29922", lightHex: "#9A6700")
        case .unknown:
            return StateStyle(state: .unknown, label: "unknown", mark: "query",
                              rank: 3, darkHex: "#6E7681", lightHex: "#8C959F")
        }
    }

    /// Dynamic `Color` driven by the contract's light/dark hexes.
    var color: Color {
        Color(light: UIColor(hex: lightHex), dark: UIColor(hex: darkHex))
    }

    /// Display glyph for the mark (AC5: color is never the only channel).
    var glyph: String {
        switch mark {
        case "alert": return "!"
        case "check": return "\u{2713}"
        case "ring": return "\u{25CB}"
        case "dot": return "\u{25E6}"
        case "query": return "?"
        default: return ""
        }
    }

    /// True when the state renders as an open ring (working) rather than a
    /// filled dot, matching the contract's `mark` column.
    var isRing: Bool { mark == "ring" }

    var accessibilityLabel: String { "State: \(label)" }
}
