import Foundation

// MARK: - Recent output surface (#205)
//
// This file deliberately contains the display contract rather than SwiftUI.
// The daemon already knows the semantic block boundaries; the model keeps
// those boundaries intact, removes only metadata lines, and exposes a
// bounded, testable line/token representation for the view.

enum RecentOutputPhase: Equatable, Sendable {
    case loading
    case empty
    case error(TranscriptFailure)
    case loaded
}

enum RecentOutputRow: Equatable, Sendable {
    case block(TranscriptBlock)
    case loadEarlier(UInt32?)
    case error(TranscriptFailure)
    case loading

    /// The content portion of a row identity. The view adds an occurrence
    /// ordinal so equal lines/blocks never collide in a `ForEach`.
    fileprivate var contentID: String {
        switch self {
        case .block(let block):
            return "block|\(block.kind.rawValue)|\(block.at ?? 0)|\(block.text)"
        case .loadEarlier(let count):
            return "load-earlier|\(count.map(String.init) ?? "none")"
        case .error(let failure):
            return "error|\(failure.kind)|\(failure.message)"
        case .loading:
            return "loading"
        }
    }
}

struct RecentOutputIdentifiedRow: Equatable, Sendable, Identifiable {
    let id: String
    let row: RecentOutputRow
}

struct RecentOutputMetadata: Equatable, Sendable {
    let model: String?
    let effort: String?
    let worktree: String?

    init(model: String? = nil, effort: String? = nil, worktree: String? = nil) {
        self.model = model
        self.effort = effort
        self.worktree = worktree
    }

    var isEmpty: Bool {
        model == nil && effort == nil && worktree == nil
    }

    /// Extract metadata from both structured blocks and legacy tail lines.
    /// The wire format's only display metadata marker is the canonical
    /// `model effort · path` line. Key-looking prose such as `path: ...` is
    /// still output content and must not be consumed as a badge.
    static func extract(from blocks: [TranscriptBlock],
                        fallbackLines: [String] = []) -> RecentOutputMetadata {
        var found = RecentOutputMetadata()
        for block in blocks {
            for line in RecentOutputRender.messageLines(block.text) {
                found = found.merged(with: parse(line))
            }
        }
        for line in fallbackLines {
            found = found.merged(with: parse(line))
        }
        return found
    }

    private func merged(with other: RecentOutputMetadata?) -> RecentOutputMetadata {
        guard let other else { return self }
        return RecentOutputMetadata(
            model: model ?? other.model,
            effort: effort ?? other.effort,
            worktree: worktree ?? other.worktree)
    }

    fileprivate static func isMetadataLine(_ line: String) -> Bool {
        parse(line) != nil
    }

    fileprivate static func parse(_ line: String) -> RecentOutputMetadata? {
        let value = line.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !value.isEmpty else { return nil }

        let pieces = value.components(separatedBy: "·")
        guard pieces.count >= 2,
              let path = pieces.last?.trimmingCharacters(in: .whitespaces),
              isWorktreePath(path) else {
            return nil
        }
        let left = pieces.dropLast()
            .joined(separator: " · ")
            .trimmingCharacters(in: .whitespaces)
        let words = left.split(whereSeparator: { $0 == " " || $0 == "\t" })
        guard !words.isEmpty else { return nil }
        let last = String(words.last!).lowercased()
        let effort = effortValues.contains(last) ? last : nil
        let modelWords = effort == nil ? words : words.dropLast()
        let model = modelWords.joined(separator: " ")
        guard !model.isEmpty, isModelName(model) else { return nil }
        return RecentOutputMetadata(model: model, effort: effort, worktree: path)
    }

    private static let effortValues: Set<String> = [
        "minimal", "low", "medium", "high", "max"
    ]

    private static func isWorktreePath(_ value: String) -> Bool {
        let lower = value.lowercased()
        return value.hasPrefix("~")
            || value.hasPrefix("/")
            || lower.contains("worktree")
            || value.contains("/")
    }

    private static func isModelName(_ value: String) -> Bool {
        let lower = value.lowercased()
        return lower.contains("gpt")
            || lower.contains("claude")
            || lower.contains("gemini")
            || lower.contains("sonnet")
            || lower.contains("opus")
            || lower.contains("luna")
            || lower.hasPrefix("o1")
            || lower.hasPrefix("o3")
            || lower.hasPrefix("o4")
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

struct RecentOutputRender: Equatable, Sendable {
    let phase: RecentOutputPhase
    let rows: [RecentOutputRow]
    let canLoadOlder: Bool
    let canRetryTail: Bool
    let metadata: RecentOutputMetadata
}

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

    /// A single render/pairing snapshot for one SwiftUI body pass. Keeping
    /// this pure makes it possible to test the expensive work independently
    /// of SwiftUI and prevents each child section from rebuilding it.
    static func snapshot(tail: TailPane?) -> RecentOutputSnapshot {
        let render = render(tail: tail)
        let visibleRows = render.rows.filter { row in
            if case .loadEarlier = row { return false }
            return true
        }
        return RecentOutputSnapshot(
            render: render,
            visibleRows: visibleRows,
            identifiedRows: identifiedRows(for: visibleRows))
    }

    static func identifiedRows(for rows: [RecentOutputRow]) -> [RecentOutputIdentifiedRow] {
        var occurrences: [String: Int] = [:]
        return rows.map { row in
            let contentID = row.contentID
            let occurrence = occurrences[contentID, default: 0]
            occurrences[contentID] = occurrence + 1
            return RecentOutputIdentifiedRow(
                id: "\(contentID)|occurrence:\(occurrence)",
                row: row)
        }
    }

    /// Follow the newest block when the surface is first populated or when
    /// blocks were appended at the tail. A page inserted at the top is
    /// history loading and must preserve the reader's anchor instead.
    static func shouldFollowLatest(from oldRows: [RecentOutputRow],
                                   to newRows: [RecentOutputRow]) -> Bool {
        let oldIDs = oldRows.compactMap { row -> String? in
            if case .block = row { return row.contentID }
            return nil
        }
        let newIDs = newRows.compactMap { row -> String? in
            if case .block = row { return row.contentID }
            return nil
        }
        guard !newIDs.isEmpty else { return false }
        guard !oldIDs.isEmpty else { return true }
        guard oldIDs != newIDs else { return false }

        // A real history prepend leaves every previously rendered block as a
        // suffix. This is the one mutation that must preserve the reader's
        // position rather than follow the newest tail.
        if newIDs.count > oldIDs.count,
           hasSuffix(newIDs, matching: oldIDs) {
            return false
        }

        // Normal append and a bounded tail slide both expose the old tail at
        // the front of the new sequence (the latter only partially).
        if newIDs.count >= oldIDs.count,
           hasPrefix(newIDs, matching: oldIDs) {
            return true
        }

        // A bounded tail can slide while the previous last block is still
        // being completed. Compare the old sequence without its last block
        // with the new sequence without its last block; an exact suffix /
        // prefix overlap proves that the change is at the tail even when the
        // overlapping last block itself changed. The scan is linear and uses
        // one prefix table rather than allocating arrays for each candidate.
        let oldBeforeLast = max(oldIDs.count - 1, 0)
        let newBeforeLast = max(newIDs.count - 1, 0)
        if suffixPrefixOverlapLength(
            old: oldIDs,
            oldCount: oldBeforeLast,
            new: newIDs,
            newCount: newBeforeLast) > 0 {
            return true
        }

        // A one-block stream has no unchanged prefix to overlap, but a
        // changed sole block is still tail growth rather than a replacement.
        if oldIDs.count == newIDs.count,
           oldIDs.count == 1 {
            return true
        }
        return false
    }

    private static func hasPrefix(_ values: [String], matching prefix: [String]) -> Bool {
        guard prefix.count <= values.count else { return false }
        for index in prefix.indices where values[index] != prefix[index] {
            return false
        }
        return true
    }

    private static func hasSuffix(_ values: [String], matching suffix: [String]) -> Bool {
        guard suffix.count <= values.count else { return false }
        let start = values.count - suffix.count
        for index in suffix.indices where values[start + index] != suffix[index] {
            return false
        }
        return true
    }

    /// Return the longest suffix of `old[0..<oldCount]` that is also a
    /// prefix of `new[0..<newCount]` in O(oldCount + newCount) time.
    private static func suffixPrefixOverlapLength(old: [String],
                                                  oldCount: Int,
                                                  new: [String],
                                                  newCount: Int) -> Int {
        guard oldCount > 0, newCount > 0 else { return 0 }

        var failure = Array(repeating: 0, count: newCount)
        var prefixLength = 0
        var patternIndex = 1
        while patternIndex < newCount {
            if new[patternIndex] == new[prefixLength] {
                prefixLength += 1
                failure[patternIndex] = prefixLength
                patternIndex += 1
            } else if prefixLength > 0 {
                prefixLength = failure[prefixLength - 1]
            } else {
                patternIndex += 1
            }
        }

        var matched = 0
        var textIndex = 0
        while textIndex < oldCount {
            while matched > 0 && old[textIndex] != new[matched] {
                matched = failure[matched - 1]
            }
            if old[textIndex] == new[matched] {
                matched += 1
            }
            if matched == newCount, textIndex < oldCount - 1 {
                matched = failure[matched - 1]
            }
            textIndex += 1
        }
        return matched
    }

    /// Keep the top reader anchor only when a successful page really inserted
    /// older blocks before the existing sequence. A failed/no-op request must
    /// clear the pending anchor so the next live append can follow the tail.
    static func shouldPreservePaginationAnchor(_ anchorID: String,
                                                from oldRows: [RecentOutputRow],
                                                to newRows: [RecentOutputRow]) -> Bool {
        let oldBlocks = oldRows.compactMap { row -> String? in
            if case .block = row { return row.contentID }
            return nil
        }
        let newBlocks = newRows.compactMap { row -> String? in
            if case .block = row { return row.contentID }
            return nil
        }
        guard newBlocks.count > oldBlocks.count,
              Array(newBlocks.suffix(oldBlocks.count)) == oldBlocks else {
            return false
        }
        return identifiedRows(for: newRows).contains { $0.id == anchorID }
    }

    static func render(tail: TailPane?) -> RecentOutputRender {
        let tail = tail ?? TailPane()
        var rows: [RecentOutputRow] = []

        let tailRaw = tailBlocks(from: tail)
        let tailBlocks = visibleBlocks(tailRaw)
        let hasContent = !tailBlocks.isEmpty
        let oldestTruncated = tailRaw.first?.truncatedBefore

        // A bounded tail can advertise older content the daemon elided (the
        // `+N lines` marker lifted from the pane). Keep this one compact
        // affordance at the top.
        if let oldestTruncated,
           !rows.contains(where: { row in
               if case .loadEarlier = row { return true }
               return false
           }) {
            rows.append(.loadEarlier(oldestTruncated))
        }

        if tail.loading && tail.isEmpty {
            rows.append(.loading)
        }
        if let tailError = tail.error, tail.isEmpty {
            rows.append(.error(tailError))
        }
        rows.append(contentsOf: tailBlocks.map(RecentOutputRow.block))

        let phase: RecentOutputPhase
        if let tailError = tail.error, !hasContent {
            phase = .error(tailError)
        } else if hasContent {
            phase = .loaded
        } else if tail.loading {
            phase = .loading
        } else {
            phase = .empty
        }

        let metadata = RecentOutputMetadata.extract(
            from: tailRaw,
            fallbackLines: tail.lines)
        return RecentOutputRender(
            phase: phase,
            rows: rows,
            canLoadOlder: oldestTruncated != nil,
            canRetryTail: !tail.loading && tail.error != nil,
            metadata: metadata)
    }

    private static func tailBlocks(from pane: TailPane) -> [TranscriptBlock] {
        if !pane.blocks.isEmpty {
            return pane.blocks
        }
        return pane.lines.map { line in
            TranscriptBlock(kind: kind(for: line), text: line)
        }
    }

    private static func kind(for role: String) -> TranscriptBlockKind {
        switch role.trimmingCharacters(in: .whitespacesAndNewlines).lowercased() {
        case "user", "you", "prompt":
            return .user
        case "tool", "system", "command":
            return .tool
        default:
            return .agent
        }
    }

    private static func visibleBlocks(_ blocks: [TranscriptBlock]) -> [TranscriptBlock] {
        var grouped: [TranscriptBlock] = []
        for block in blocks {
            let lines = visibleMessageLines(block.text)
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

    private static func visibleMessageLines(_ text: String) -> [String] {
        let lines = RecentOutputRender.messageLines(text)
        guard let firstContentIndex = lines.firstIndex(where: {
            !$0.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
        }) else {
            return lines
        }
        guard let lastContentIndex = lines.lastIndex(where: {
            !$0.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
        }), firstContentIndex != lastContentIndex,
                  RecentOutputMetadata.isMetadataLine(lines[lastContentIndex]) else {
            return lines
        }
        return lines.enumerated().compactMap { index, line in
            index == lastContentIndex ? nil : line
        }
    }
}

struct RecentOutputSnapshot: Equatable, Sendable {
    let render: RecentOutputRender
    let visibleRows: [RecentOutputRow]
    let identifiedRows: [RecentOutputIdentifiedRow]
}

// MARK: - Pure block and code helpers

extension RecentOutputRender {
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

    static func accessibilityLabel(_ block: TranscriptBlock) -> String {
        switch block.kind {
        case .user: return "You: \(block.text)"
        case .agent: return "Agent: \(block.text)"
        case .tool: return "Tool: \(block.text)"
        case .system: return "System: \(block.text)"
        }
    }

    static func disclosureAccessibilityLabel(_ block: TranscriptBlock) -> String {
        let role = block.kind == .system ? "System" : "Tool"
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

    private static func isDividerScalar(_ scalar: UnicodeScalar) -> Bool {
        (0x2500...0x259F).contains(scalar.value)
    }

    private static let keywords: Set<String> = [
        "actor", "class", "const", "else", "enum", "fn", "for", "func",
        "if", "impl", "import", "in", "let", "match", "mut", "pub",
        "return", "struct", "switch", "var", "where", "while"
    ]
}
