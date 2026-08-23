import Foundation

// MARK: - Time in state (#166 item 6)

/// Relative-duration formatting for the "time in state" chip. Pure and
/// locale-independent (the prototype shows `42m`, `1h 10m`, `3h 02m`), so it
/// is unit-testable without a rendering harness.
enum RelativeTime {
    /// Format an elapsed milliseconds span the way the prototype does:
    ///
    /// - < 60s → `42s`
    /// - < 60m → `42m`
    /// - < 24h → `1h 10m`, `3h 02m` (minutes always two digits so a
    ///   non-truncated hour line keeps a fixed-width column, matching the
    ///   prototype)
    /// - >= 24h → `1d 4h`
    static func duration(milliseconds: UInt64) -> String {
        duration(seconds: Double(milliseconds) / 1000)
    }

    static func duration(seconds: Double) -> String {
        let total = max(0, Int(seconds.rounded()))
        if total < 60 { return "\(total)s" }
        if total < 3600 { return "\(total / 60)m" }
        if total < 86400 {
            let h = total / 3600
            let m = (total % 3600) / 60
            return "\(h)h \(String(format: "%02d", m))m"
        }
        let d = total / 86400
        let h = (total % 86400) / 3600
        return "\(d)d \(h)h"
    }
}

/// Time-in-state derivation. The daemon snapshot carries `agent.ts`
/// ("wall-clock when this record was last changed", epoch millis) but NOT a
/// dedicated state-change timestamp, so `ts` is used here as a proxy for
/// state-entered time.
///
/// LIMITATION (issue #166 deliberately does NOT change the daemon): a
/// reason/tool update that re-writes the record mid-state advances `ts` and
/// under-reports the true time in state. Deriving from observed transitions
/// is a possible follow-up; the board does not do that here.
enum TimeInState {
    /// Milliseconds the agent has been in its current state, or `nil` when
    /// the record carries no usable `ts` (the UI then omits the duration).
    static func milliseconds(for agent: Agent, now: UInt64) -> UInt64? {
        guard agent.ts > 0 else { return nil }
        return now >= agent.ts ? now - agent.ts : 0
    }
}
