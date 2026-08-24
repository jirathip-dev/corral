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
/// dedicated state-change timestamp. The store therefore tracks
/// `stateEnteredAt` client-side: seeded from `ts` at first sight and updated
/// ONLY when `state` actually changes (never on title/reason churn). This
/// accessor reads that client-side value, falling back to `agent.ts` for
/// callers without a store (e.g. pure tests or a pre-tracking snapshot).
///
/// LIMITATION (issue #166 deliberately does NOT change the daemon): the
/// client-side seed uses the first-seen record's `ts`, which may be later
/// than the true state-entry time for an agent already mid-state at app
/// launch. The store marks this in `ios/README.md`.
enum TimeInState {
    /// Milliseconds the agent has been in its current state, or `nil` when
    /// neither `stateEnteredAt` nor `agent.ts` carries a usable timestamp
    /// (the UI then omits the duration).
    static func milliseconds(for agent: Agent, stateEnteredAt: UInt64? = nil,
                             now: UInt64) -> UInt64? {
        let entered = stateEnteredAt ?? agent.ts
        guard entered > 0 else { return nil }
        return now >= entered ? now - entered : 0
    }
}
