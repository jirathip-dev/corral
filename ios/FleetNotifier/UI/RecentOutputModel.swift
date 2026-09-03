import Foundation

// MARK: - Recent output surface (#205 → #354 L2 recents v1 → #361 rail)
//
// Recents is LIVE TAIL ONLY: the daemon's bounded read_tail result
// (≤200 lines) renders as ONE continuous chronological rail of raw output
// with auto-scroll. #361 removed the V3-era grouping chrome — divider-only
// rows, role-grouped card chrome, section headers, and per-row role labels —
// so role appears ONLY as a transition marker (shape + role token color) at
// semantic role changes. This file keeps the display contract (pure block /
// rail helpers) rather than SwiftUI, so the expensive pure work stays
// unit-testable.

/// A bounded tail pane mapped to the row sequence the sheet renders.
enum RecentOutputModel {
    static let liveTailFreshness: TimeInterval = 15

    static func hasFreshNonErrorTail(_ tail: TailPane?,
                                     now: Date = Date()) -> Bool {
        guard let tail,
              !tail.isEmpty,
              tail.error == nil,
              let updatedAt = tail.updatedAt else {
            return false
        }
        let age = now.timeIntervalSince(updatedAt)
        return age >= 0 && age <= liveTailFreshness
    }

    static func shouldShowLiveIndicator(isLiveMode: Bool,
                                        hasFreshNonErrorTail: Bool) -> Bool {
        isLiveMode && hasFreshNonErrorTail
    }

    /// The canonical rows the sheet renders from a pane: the daemon's
    /// blocks when present, else legacy raw lines mapped to honest unknown
    /// content (never reclassified). Empty/whitespace-only blocks are
    /// dropped, divider-only rows are dropped BEFORE adjacent tool/system
    /// merging (#361: the rail renders ZERO divider rows — a divider is
    /// never an event card and never rides inside a merged content row;
    /// content that merely CONTAINS a run stays text), and adjacent
    /// tool/system blocks are merged exactly like the pre-cut renderer, so
    /// the stream stays compact and stable across fetches.
    static func tailRows(from pane: TailPane?) -> [TranscriptBlock] {
        let pane = pane ?? TailPane()
        let raw: [TranscriptBlock]
        if !pane.blocks.isEmpty {
            raw = pane.blocks
        } else {
            raw = pane.lines.map { TranscriptBlock(kind: .unknown, text: $0) }
        }
        var rows: [TranscriptBlock] = []
        for block in raw {
            let lines = RecentOutputRender.messageLines(block.text)
            guard lines.contains(where: {
                !$0.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
            }) else {
                continue
            }
            var visible = block
            visible.text = lines.joined(separator: "\n")
            if !RecentOutputRender.isDividerBlock(visible) {
                rows.append(visible)
            }
        }
        var grouped: [TranscriptBlock] = []
        for block in rows {
            if let last = grouped.last,
               (last.kind == .tool || last.kind == .system),
               last.kind == block.kind {
                grouped[grouped.count - 1].text += "\n" + block.text
            } else {
                grouped.append(block)
            }
        }
        return grouped
    }

    struct IdentifiedBlock: Equatable, Sendable, Identifiable {
        let id: String
        let block: TranscriptBlock
    }

    /// Occurrence-suffixed identities so equal blocks never collide in a
    /// `ForEach` (mirrors the pre-cut row identity discipline).
    static func identifiedBlocks(_ blocks: [TranscriptBlock]) -> [IdentifiedBlock] {
        var occurrences: [String: Int] = [:]
        return blocks.map { block in
            let content = "block|\(block.kind.rawValue)|\(block.at ?? 0)|\(block.text)"
            let occurrence = occurrences[content, default: 0]
            occurrences[content] = occurrence + 1
            return IdentifiedBlock(id: "\(content)|occurrence:\(occurrence)", block: block)
        }
    }

    /// The rail marker vocabulary (#361 DESIGN LOCK; #316 RATIONALE V1):
    /// Assistant = circle, You = diamond, Tool = square. A marker appears
    /// ONLY at a semantic role transition — never repeated per row, and
    /// never as role text.
    enum RailMarker: Equatable, Sendable {
        case circle
        case diamond
        case square
    }

    /// The locked shape for one block kind. `system`/`unknown` rows are
    /// raw output with no known role, so they never receive a marker.
    static func marker(for kind: TranscriptBlockKind) -> RailMarker? {
        switch kind {
        case .user: return .diamond
        case .agent: return .circle
        case .tool: return .square
        case .system, .unknown: return nil
        }
    }

    /// One rendered rail row: a canonical block plus whether THIS row is
    /// the first row of a role run (its kind differs from the previous
    /// rendered row) and therefore shows the role transition marker.
    /// Continuation rows carry no marker and no label.
    struct RailRow: Equatable, Sendable, Identifiable {
        let id: String
        let block: TranscriptBlock
        let showsTransitionMarker: Bool
    }

    /// The continuous chronological rail the sheet renders: `tailRows` in
    /// daemon order with transition markers computed over the RENDERED
    /// sequence (divider rows already dropped), so a marker appears exactly
    /// where the visible role changes.
    static func railRows(from pane: TailPane?) -> [RailRow] {
        let identified = identifiedBlocks(tailRows(from: pane))
        var previousKind: TranscriptBlockKind?
        return identified.map { item in
            let kind = item.block.kind
            let showsMarker = marker(for: kind) != nil && kind != previousKind
            previousKind = kind
            return RailRow(id: item.id, block: item.block,
                           showsTransitionMarker: showsMarker)
        }
    }

    /// The sheet's four-state display phase, derived from the pane.
    static func phase(for pane: TailPane?) -> Phase {
        let pane = pane ?? TailPane()
        if let error = pane.error, pane.isEmpty {
            return .error(error)
        }
        if !pane.isEmpty {
            return .loaded
        }
        if pane.loading {
            return .loading
        }
        return .empty
    }

    enum Phase: Equatable, Sendable {
        case loading
        case empty
        case error(TranscriptFailure)
        case loaded
    }
}

enum RecentCodeSegmentKind: Equatable, Sendable {
    case plain
    case keyword
    case string
    case addition
    case deletion
    case comment
}

struct RecentCodeSegment: Equatable, Sendable {
    let text: String
    let kind: RecentCodeSegmentKind
}

struct RecentCodeLine: Equatable, Sendable {
    let number: Int?
    let text: String
    let segments: [RecentCodeSegment]
    let isHighlighted: Bool
}

/// Pure block-rendering helpers (code/diff highlighting, divider
/// classification, accessibility labels). The rail drops divider-only rows
/// and shows no timestamps or per-row chrome (#361).
enum RecentOutputRender {
    static func messageLines(_ text: String) -> [String] {
        text.components(separatedBy: .newlines)
    }

    /// Accessible role naming is locked (`You said…`, `Assistant`, `Tool`,
    /// `Diagnostic`, `Unknown activity`). Visible role text is banned on
    /// the rail (#361); the accessible layer keeps the roles attributable.
    static func accessibilityLabel(_ block: TranscriptBlock) -> String {
        switch block.kind {
        case .user: return "You said: \(block.text)"
        case .agent: return "Assistant: \(block.text)"
        case .tool: return "Tool: \(block.text)"
        case .system: return "Diagnostic: \(block.text)"
        case .unknown: return "Unknown activity: \(block.text)"
        }
    }

    static func isBoundary(previous: TranscriptBlock?, current: TranscriptBlock) -> Bool {
        previous?.kind != current.kind
    }

    /// Highlight only a tool block that clearly contains source or diff
    /// syntax. Prose and ordinary command output stay plain monospace text.
    static func codeLines(for block: TranscriptBlock) -> [RecentCodeLine] {
        let lines = messageLines(block.text)
        let highlighted = isCodeOrDiff(block.text)
        return lines.enumerated().map { index, line in
            RecentCodeLine(
                number: highlighted ? index + 1 : nil,
                text: line,
                segments: highlighted ? highlight(line) : [
                    RecentCodeSegment(text: line, kind: .plain)
                ],
                isHighlighted: highlighted)
        }
    }

    static func isCodeOrDiff(_ text: String) -> Bool {
        let lines = messageLines(text)
        var hasGitHeader = false
        var hasFileHeader = false
        var hasHunk = false
        var hasChange = false
        var hasFence = false
        for line in lines {
            let trimmed = line.trimmingCharacters(in: .whitespaces)
            let lower = trimmed.lowercased()
            hasGitHeader = hasGitHeader
                || lower.hasPrefix("git diff")
                || lower.hasPrefix("diff --git")
            hasFileHeader = hasFileHeader
                || lower.hasPrefix("+++ ")
                || lower.hasPrefix("--- ")
            hasHunk = hasHunk || lower.hasPrefix("@@")
            hasChange = hasChange
                || (trimmed.hasPrefix("+") && !trimmed.hasPrefix("+++"))
                || (trimmed.hasPrefix("-") && !trimmed.hasPrefix("---"))
            hasFence = hasFence || trimmed.hasPrefix("\u{60}\u{60}\u{60}")
        }
        let hasDiffEvidence = (hasGitHeader && (hasHunk || (hasFileHeader && hasChange)))
            || (hasHunk && hasChange)
            || (hasFileHeader && hasHunk)
        let hasCodeSignal = lines.contains { line in
            let trimmed = line.trimmingCharacters(in: .whitespaces)
            let lower = trimmed.lowercased()
            return trimmed.hasPrefix("#!")
                || trimmed.hasPrefix("$ ")
                || lower.hasPrefix("def ")
                || lower.hasPrefix("class ")
                || lower.hasPrefix("import ")
                || lower.hasPrefix("from ")
                || lower.hasPrefix("echo ")
                || lower.hasPrefix("export ")
                || lower.hasPrefix("if ")
                || lower.hasPrefix("for ")
        }
        return hasFence || hasDiffEvidence || hasCodeSignal
    }

    private static func highlight(_ line: String) -> [RecentCodeSegment] {
        let trimmed = line.trimmingCharacters(in: .whitespaces)
        if trimmed.hasPrefix("+") && !trimmed.hasPrefix("+++") {
            return [RecentCodeSegment(text: line, kind: .addition)]
        }
        if trimmed.hasPrefix("-") && !trimmed.hasPrefix("---") {
            return [RecentCodeSegment(text: line, kind: .deletion)]
        }
        if trimmed.hasPrefix("@@") {
            return [RecentCodeSegment(text: line, kind: .keyword)]
        }

        let characters = Array(line)
        let firstNonWhitespace = characters.firstIndex(where: { !$0.isWhitespace })
        var segments: [RecentCodeSegment] = []
        var index = 0
        while index < characters.count {
            let character = characters[index]
            if character == "\"" || character == "'" {
                let quote = character
                var end = index + 1
                var escaped = false
                while end < characters.count {
                    let candidate = characters[end]
                    if escaped {
                        escaped = false
                    } else if candidate == "\\" {
                        escaped = true
                    } else if candidate == quote {
                        end += 1
                        break
                    }
                    end += 1
                }
                append(&segments, String(characters[index..<end]), .string)
                index = end
            } else if (character == "#" && firstNonWhitespace == index)
                        || (character == "/" && index + 1 < characters.count
                            && characters[index + 1] == "/") {
                append(&segments, String(characters[index...]), .comment)
                break
            } else if character.isLetter || character == "_" {
                var end = index + 1
                while end < characters.count
                        && (characters[end].isLetter
                            || characters[end].isNumber
                            || characters[end] == "_") {
                    end += 1
                }
                let word = String(characters[index..<end])
                append(&segments, word, keywords.contains(word) ? .keyword : .plain)
                index = end
            } else {
                append(&segments, String(character), .plain)
                index += 1
            }
        }
        return segments
    }

    private static func append(_ segments: inout [RecentCodeSegment],
                               _ text: String,
                               _ kind: RecentCodeSegmentKind) {
        guard !text.isEmpty else { return }
        if let last = segments.last, last.kind == kind {
            segments[segments.count - 1] = RecentCodeSegment(
                text: last.text + text,
                kind: kind)
        } else {
            segments.append(RecentCodeSegment(text: text, kind: kind))
        }
    }

    /// #253 fallback: a block that is purely a box-drawing / block-element
    /// run (residual TUI furniture the daemon missed — or the daemon's own
    /// short `───` divider marker) renders as a real divider instead of
    /// dash-run text. Every non-empty line must be a run; content lines
    /// that merely contain a run (e.g. `let sep = "────";`) stay text.
    static func isDividerRun(_ text: String) -> Bool {
        var sawRun = false
        for line in messageLines(text) {
            let scalars = line.unicodeScalars
                .filter { !CharacterSet.whitespaces.contains($0) }
            if scalars.isEmpty { continue }
            guard scalars.count >= 2,
                  scalars.allSatisfy({ isDividerScalar($0) }) else {
                return false
            }
            sawRun = true
        }
        return sawRun
    }

    /// The ONE divider-vs-content classification seam shared by every
    /// render path — a divider-only block is a presentation separator,
    /// never an event card.
    static func isDividerBlock(_ block: TranscriptBlock) -> Bool {
        isDividerRun(block.text)
    }

    private static func isDividerScalar(_ scalar: UnicodeScalar) -> Bool {
        (0x2500...0x259F).contains(scalar.value)
    }

    private static let keywords: Set<String> = [
        "actor", "class", "const", "else", "enum", "fn", "for", "func",
        "if", "impl", "import", "in", "let", "match", "mut", "pub",
        "return", "struct", "switch", "var", "where", "while"
    ]
}
