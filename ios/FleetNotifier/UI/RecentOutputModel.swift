import Foundation

// MARK: - Recent output surface (#167, D7/D8/D9/D10)
//
// PURE, UI-free view-model. It takes the daemon's already-segmented blocks
// (live tail + older transcript pages) and produces the rows a SwiftUI
// `ScrollView(.vertical)` renders, PLUS the four-state machine
// (loading / empty / error / loaded) and the pagination cursor handling.
//
// The renderer stays dumb: this type owns "which divider sits where" and
// "what state am I in", so it is unit-testable without a UI host.

/// The four async states for the whole Recent-output surface (D8). A stalled
/// or nil older page shows what IS loaded plus the error row + Retry — never
/// an infinite spinner (#160).
enum RecentOutputPhase: Equatable, Sendable {
    case loading
    case empty
    case error(TranscriptFailure)
    case loaded
}

/// One renderable row. `loadEarlier` is the full-width truncated divider row
/// (tappable), carrying the `truncated_before` count when the daemon lifted a
/// `... +N lines` marker, else nil (the generic "Load earlier" divider).
enum RecentOutputRow: Equatable, Sendable {
    case block(TranscriptBlock)
    case loadEarlier(UInt32?)
    case error(TranscriptFailure)
    case loading
}

/// Immutable render plan: phase + ordered rows (oldest→newest, newest at the
/// bottom of the live tail) + the pagination affordances.
struct RecentOutputRender: Equatable, Sendable {
    let phase: RecentOutputPhase
    let rows: [RecentOutputRow]
    /// Transcript has another (older) page to load.
    let canLoadOlder: Bool
    /// The transcript walk is in flight (show a progress row, not a spin).
    let transcriptLoading: Bool
    /// Retry-able tail error (the live tail failed and has no blocks yet).
    let canRetryTail: Bool
    /// Retry-able transcript cursor error.
    let canRetryTranscript: Bool
    /// The wire cursor for the next (older) transcript page, if any.
    let nextCursor: String?
}

/// The pure transform. `tail` and `transcript` are the two panes from the
/// store; `at` is `nil` because the view-model does not know time.
enum RecentOutputModel {
    /// Build the render plan from the raw panes.
    static func render(tail: TailPane?, transcript: TranscriptPane?) -> RecentOutputRender {
        let tail = tail ?? TailPane()
        var rows: [RecentOutputRow] = []

        // ---- older transcript (top) ----
        let transcriptRows = transcript.map { rowsForTranscript($0) } ?? []
        rows.append(contentsOf: transcriptRows)

        let allBlocks = (transcript?.blocks ?? []) + tail.blocks
        let hasContent = !allBlocks.isEmpty

        // The oldest block on the surface carries the daemon-lifted
        // `... +N lines` truncation count. Live-tail blocks are oldest→newest
        // (`first` = oldest); transcript blocks are newest-first (`last` =
        // the oldest block of the oldest page loaded so far).
        let oldestTruncated = transcript?.blocks.last?.truncatedBefore
            ?? tail.blocks.first?.truncatedBefore

        // If there is no transcript page yet but the live tail says something
        // was cut off, put the tappable full-width divider at the very top.
        let transcriptHasBlocks = transcript?.blocks.isEmpty == false
        if !transcriptHasBlocks, let oldestTruncated {
            rows.append(.loadEarlier(oldestTruncated))
        }

        // ---- live tail (bottom) ----
        if tail.loading && tail.isEmpty {
            rows.append(.loading)
        }
        if let tailError = tail.error, tail.isEmpty {
            rows.append(.error(tailError))
        }
        rows.append(contentsOf: tail.blocks.map(RecentOutputRow.block))

        let phase: RecentOutputPhase
        if let tailError = tail.error, allBlocks.isEmpty {
            phase = .error(tailError)
        } else if hasContent {
            phase = .loaded
        } else if tail.loading || transcript?.loading == true {
            phase = .loading
        } else if let error = transcript?.error {
            phase = .error(error)
        } else {
            phase = .empty
        }

        let transcriptLoading = transcript?.loading ?? false
        let canLoadOlder = (transcript?.canLoadOlder ?? false)
            || (oldestTruncated != nil && !transcriptHasBlocks)
        let canRetryTail = !tail.loading && tail.error != nil
        let canRetryTranscript = transcript?.canRetry ?? false
        return RecentOutputRender(
            phase: phase,
            rows: rows,
            canLoadOlder: canLoadOlder,
            transcriptLoading: transcriptLoading,
            canRetryTail: canRetryTail,
            canRetryTranscript: canRetryTranscript,
            nextCursor: transcript?.nextCursor
        )
    }

    /// The older-transcript rows, oldest→newest (so the newest transcript
    /// block sits directly above the live tail). A `loadEarlier` divider tops
    /// the list when the walk is not exhausted or the oldest block carries a
    /// lifted truncation count. An error row is inserted when the page fetch
    /// failed but held blocks remain.
    private static func rowsForTranscript(_ pane: TranscriptPane) -> [RecentOutputRow] {
        var rows: [RecentOutputRow] = []
        if pane.canLoadOlder || pane.blocks.last?.truncatedBefore != nil {
            rows.append(.loadEarlier(pane.blocks.last?.truncatedBefore))
        }
        if pane.loading && pane.blocks.isEmpty {
            rows.append(.loading)
        }
        if let error = pane.error {
            // Show what is loaded + the failure row + Retry (#160). Even when
            // no older blocks are held, a failed "Load earlier" page must be
            // visible (the live tail remains below).
            rows.append(.error(error))
        }
        // The store holds newest-first; reverse to oldest→newest for the
        // top-of-surface layout. A single page reverses cleanly; multiple
        // pages append in load order, so the oldest page ends up at the top.
        let reversed = pane.blocks.reversed()
        rows.append(contentsOf: reversed.map(RecentOutputRow.block))
        return rows
    }
}

// MARK: - Block render helpers (pure, testable)

extension RecentOutputRender {
    /// The one-line collapsed summary for a tool block (`▸ ran cargo test`).
    static func toolSummary(_ text: String) -> String {
        let first = text.split(separator: "\n").first.map(String.init) ?? text
        let cleaned = first
            .trimmingCharacters(in: .whitespacesAndNewlines)
            .replacingOccurrences(of: "$ ", with: "")
        let prefix = cleaned.hasPrefix("cargo ") || cleaned.hasPrefix("npm ")
            ? String(cleaned.split(separator: " ").dropFirst().joined(separator: " ").prefix(48))
            : cleaned
        // Prefer a compact `<command> (exit <code>)`-style label when a
        // `test result:` line follows, otherwise the command line.
        let lines = text.split(separator: "\n").map(String.init)
        if let result = lines.first(where: { $0.hasPrefix("test result:") }) {
            let tail = result.replacingOccurrences(of: "test result: ", with: "")
            return String(tail.prefix(60))
        }
        return String(prefix.prefix(48))
    }

    /// A short block-kind accessibility label.
    static func accessibilityLabel(_ block: TranscriptBlock) -> String {
        switch block.kind {
        case .user: return "You: \(block.text)"
        case .agent: return "Agent: \(block.text)"
        case .tool: return "Tool: \(block.text)"
        case .system: return "System: \(block.text)"
        }
    }

    /// Group consecutive same-kind blocks into render sections (the SwiftUI
    /// view uses one row per block, but timestamps render only at a kind
    /// boundary).
    static func isBoundary(previous: TranscriptBlock?, current: TranscriptBlock) -> Bool {
        previous?.kind != current.kind
    }
}
