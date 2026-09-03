import Foundation
import SwiftUI

// MARK: - StateStyle

/// Read-model styling for one `AgentState`.
///
/// The LABEL is the herdr RAW state token verbatim — working / idle /
/// blocked / unknown — per the #354 spec amendment (09-02): herdr 0.8.2 has
/// NO `done` (finished Hermes panes fall back to idle), and
/// Corral-invented wording (Needs-you / Supervising / Finished) is banned on
/// the read-only board.
///
/// Ranks encode the board's attention order: blocked → working → idle →
/// unknown, with a wire-`done` record ranked with idle (its herdr fallback).
///
/// #372: COLORS no longer live here. The shared `contracts/state-tokens.json`
/// carries the pre-theming GitHub-dark light/dark hexes, which the #372
/// Catppuccin design approval supersedes for the iOS client (the whole app
/// renders through the active flavor's palette; the egui mirror is a later
/// follow-up, and the shared contract file is deliberately untouched by this
/// lane). The per-flavor state→color mapping lives in
/// `ThemeStore.stateColor(for:)` (working=teal, blocked=red, done=green,
/// idle=subtext0, unknown=surface2 — the locked mapping), and the rank/mark
/// vocabulary stays here so board ordering and glyphs keep one home.
struct StateStyle: Equatable, Sendable {
    let state: AgentState
    let label: String
    let mark: String
    let rank: Int

    static func style(for state: AgentState) -> StateStyle {
        switch state {
        case .blocked:
            return StateStyle(state: .blocked, label: "blocked", mark: "alert",
                              rank: 0)
        case .working:
            return StateStyle(state: .working, label: "working", mark: "ring",
                              rank: 1)
        case .idle:
            return StateStyle(state: .idle, label: "idle", mark: "dot",
                              rank: 2)
        case .done:
            // Finished Hermes panes fall back to idle in herdr 0.8.2; a
            // wire `done` (transitional daemon) ranks + reads as idle.
            return StateStyle(state: .done, label: "done", mark: "check",
                              rank: 2)
        case .unknown:
            return StateStyle(state: .unknown, label: "unknown", mark: "query",
                              rank: 3)
        }
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
