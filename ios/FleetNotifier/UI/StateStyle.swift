import Foundation
import SwiftUI

// MARK: - Color(light:dark:)

extension UIColor {
    /// `#RRGGBB` → opaque `UIColor`. Hexes come from
    /// `contracts/state-tokens.json` (single source of truth).
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

/// Read-model styling for one `AgentState`, mirroring
/// `contracts/state-tokens.json` (label + mark token + light/dark hex).
/// Both clients consume that contract so the state→color/label vocabulary
/// can never diverge again. The color is a dynamic `Color(light:dark:)` so
/// it participates in dark mode and accessibility contrast.
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
            return StateStyle(state: .blocked, label: "Needs you", mark: "alert",
                              rank: 0, darkHex: "#F85149", lightHex: "#CF222E")
        case .done:
            return StateStyle(state: .done, label: "Review", mark: "check",
                              rank: 1, darkHex: "#D29922", lightHex: "#9A6700")
        case .working:
            return StateStyle(state: .working, label: "Working", mark: "ring",
                              rank: 2, darkHex: "#58A6FF", lightHex: "#0969DA")
        case .idle:
            return StateStyle(state: .idle, label: "Idle", mark: "dot",
                              rank: 3, darkHex: "#8B949E", lightHex: "#6E7781")
        case .unknown:
            return StateStyle(state: .unknown, label: "Unknown", mark: "query",
                              rank: 4, darkHex: "#6E7681", lightHex: "#8C959F")
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
