import Foundation

// MARK: - Recent output surface (#205 → #354 L2 recents v1)
//
// Recents v1 is LIVE TAIL ONLY: the daemon's bounded read_tail result
// (≤200 lines) renders as one unpartitioned stream of canonical blocks with
// auto-scroll. The V3 Conversation/Harness partition, session-status
// metadata, load-earlier paging, and the composer were removed with the cut;
// this file keeps the display contract (block rendering helpers) rather than
// SwiftUI, so the expensive pure work stays unit-testable.

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

    /// The canonical blocks the sheet renders from a pane: the daemon's
    /// blocks when present, else legacy raw lines mapped to honest unknown
    /// content (never reclassified). Empty/whitespace-only blocks and
    /// adjacent tool/system blocks are merged exactly like the pre-cut
    /// renderer, so the stream stays compact and stable across fetches.
    static func tailRows(from pane: TailPane?) -> [TranscriptBlock] {
        let pane = pane ?? TailPane()
        let raw: [TranscriptBlock]
        if !pane.blocks.isEmpty {
            raw = pane.blocks
        } else {
            raw = pane.lines.map { TranscriptBlock(kind: .unknown, text: $0) }
        }
        var grouped: [TranscriptBlock] = []
        for block in raw {
            let lines = RecentOutputRender.messageLines(block.text)
            guard lines.contains(where: {
                !$0.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
            }) else {
                continue
            }
            var visible = block
            visible.text = lines.joined(separator: "\n")
            if let last = grouped.last,
               (last.kind == .tool || last.kind == .system),
               last.kind == visible.kind {
                grouped[grouped.count - 1].text += "\n" + visible.text
            } else {
                grouped.append(visible)
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

/// Pure block-rendering helpers (row cards, code/diff highlighting,
/// divider classification, timestamps, accessibility labels).
enum RecentOutputRender {
    private static func makeTimestampFormatter(timeZone: TimeZone) -> DateFormatter {
        let formatter = DateFormatter()
        formatter.locale = Locale(identifier: "en_US_POSIX")
        formatter.calendar = Calendar(identifier: .gregorian)
        formatter.timeZone = timeZone
        formatter.dateFormat = "HH:mm:ss"
        return formatter
    }

    private static let localTimestampFormatter = makeTimestampFormatter(
        timeZone: .autoupdatingCurrent)

    static func timestamp(_ ms: UInt64, timeZone: TimeZone? = nil) -> String {
        let date = Date(timeIntervalSince1970: Double(ms) / 1000)
        if let timeZone {
            let formatter = localTimestampFormatter.copy() as! DateFormatter
            formatter.timeZone = timeZone
            return formatter.string(from: date)
        }
        return localTimestampFormatter.string(from: date)
    }

    static func messageLines(_ text: String) -> [String] {
        text.components(separatedBy: .newlines)
    }

    static func toolSummary(_ text: String) -> String {
        let first = messageLines(text).first(where: {
            !$0.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
        }) ?? text
        let cleaned = first
            .trimmingCharacters(in: .whitespacesAndNewlines)
        let command = cleaned.hasPrefix("$ ")
            ? String(cleaned.dropFirst(2))
            : cleaned
        return String(command.trimmingCharacters(in: .whitespacesAndNewlines).prefix(48))
    }

    /// Accessible role naming is locked (`You said…`, `Assistant`, `Tool`,
    /// `Diagnostic`, `Unknown activity`).
    static func accessibilityLabel(_ block: TranscriptBlock) -> String {
        switch block.kind {
        case .user: return "You said: \(block.text)"
        case .agent: return "Assistant: \(block.text)"
        case .tool: return "Tool: \(block.text)"
        case .system: return "Diagnostic: \(block.text)"
        case .unknown: return "Unknown activity: \(block.text)"
        }
    }

    static func disclosureAccessibilityLabel(_ block: TranscriptBlock) -> String {
        let role: String
        switch block.kind {
        case .system: role = "Diagnostic"
        case .unknown: role = "Unknown activity"
        default: role = "Tool"
        }
        return "\(role): \(toolSummary(block.text))"
    }

    static let disclosureAccessibilityHint = "Double tap to expand or collapse"

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
