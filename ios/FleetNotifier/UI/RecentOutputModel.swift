import Foundation

// MARK: - Recent output surface (#205 → #354 L2 recents v1 → #373 block-per-run)
//
// Recents is LIVE TAIL ONLY: the daemon's bounded read_tail result
// (≤200 lines) renders as a transcript of RUNS, one block per semantic
// role run (#373). A role change (You / Assistant / Tool run / Status)
// starts a new block; consecutive same-role material stays ONE block, and
// a tool run renders one COMPACT line per invocation with the command
// output inline on a subtle tinted panel. #361's continuous rail (one
// undivided stream) was REJECTED in the design round and is gone. This
// file keeps the display contract (pure block helpers) rather than
// SwiftUI, so the expensive pure work stays unit-testable.

/// A bounded tail pane mapped to the block sequence the sheet renders.
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

    // MARK: #373 block-per-run display model
    //
    // The sheet renders display blocks. Each block is one ROLE RUN: role
    // changes start a block (You / Assistant / Tool run / Status), and
    // consecutive same-role canonical blocks merge into ONE run so a
    // growing live tail appends INTO the current semantic block instead of
    // stacking duplicates. Divider-only and empty material is dropped
    // before runs form (the #253/#361 furniture rule). The daemon wire
    // carries kind + text only — tool identity is not on the wire, so a
    // tool run's invocations are classified from its text (documented
    // below) and its icon is the best-effort shape-derived kind with a
    // generic fallback.

    /// The row vocabulary INSIDE a display block. Every row is exactly one
    /// line of the canonical block stream (except `.waiting`, which is a
    /// placeholder row, not stream content).
    enum BlockRowKind: String, Equatable, Sendable {
        /// User/assistant body copy — sans, on the block surface.
        case prose
        /// Status material — quiet muted mono.
        case meta
        /// ONE tool invocation line (compact; the tool name lives here).
        case call
        /// Tool output / raw terminal line — mono, tinted panel inside a
        /// tool block.
        case output
        /// "waiting for output…" placeholder on a run that has started but
        /// produced no output yet (never its own block).
        case waiting
    }

    struct BlockRow: Equatable, Sendable {
        let kind: BlockRowKind
        let text: String

        init(_ kind: BlockRowKind, _ text: String) {
            self.kind = kind
            self.text = text
        }
    }

    /// The tool icon vocabulary (design lock): terminal / doc (read_file) /
    /// code (edit) / search + a generic fallback for shapes we cannot
    /// classify. Because tool identity is absent from the wire, the kind is
    /// derived from the first call line's command word (see
    /// `toolKind(forCallLine:)`) — documented best-effort, deterministic
    /// for tests, honest (generic) for unrecognized shapes.
    enum ToolKind: String, Equatable, Sendable {
        case terminal, doc, code, search, generic
    }

    /// One rendered block: a role run plus its content rows and the
    /// shape-derived tool icon (tool runs only; nil for other roles).
    struct DisplayBlock: Equatable, Sendable, Identifiable {
        let id: String
        let kind: TranscriptBlockKind
        let tool: ToolKind?
        let rows: [BlockRow]

        /// The block's full text (used for append-scroll change detection).
        var text: String { rows.map(\.text).joined(separator: "\n") }

        /// The first content row — the collapsed header preview line
        /// (never a role word; for a tool run this is the invocation).
        var firstLine: String { rows.first?.text ?? "" }

        /// Rows hidden by the per-block line cap (0 when at/below it).
        var cappedLineCount: Int { max(0, rows.count - RecentOutputModel.lineCap) }
    }

    /// Locked: a block body shows at most 20 LINES, then a "Show all"
    /// control reveals the rest inline (design round, #373).
    static let lineCap = 20

    /// The muted inline placeholder for a run that has started but has no
    /// output yet (spec: NO block of its own).
    static let waitingRowText = "waiting for output…"

    /// The canonical display blocks the sheet renders from a pane: the
    /// daemon's blocks when present, else legacy raw lines mapped to honest
    /// unknown content (never reclassified). Role runs form over the
    /// RENDERED sequence (empty + divider-only material already dropped),
    /// so a role change starts a block and same-role adjacency — including
    /// across fetches as a live tail grows — stays ONE block.
    static func displayBlocks(from pane: TailPane?) -> [DisplayBlock] {
        let pane = pane ?? TailPane()
        let raw: [TranscriptBlock]
        if !pane.blocks.isEmpty {
            raw = pane.blocks
        } else {
            raw = pane.lines.map { TranscriptBlock(kind: .unknown, text: $0) }
        }
        var runs: [TranscriptBlock] = []
        for block in raw {
            let lines = RecentOutputRender.messageLines(block.text)
            guard lines.contains(where: {
                !$0.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
            }) else {
                continue
            }
            var visible = block
            visible.text = lines.joined(separator: "\n")
            // Divider-only material is presentation furniture — it never
            // starts or rides inside a run.
            guard !RecentOutputRender.isDividerBlock(visible) else { continue }
            if let last = runs.last, last.kind == visible.kind {
                runs[runs.count - 1].text += "\n" + visible.text
            } else {
                runs.append(visible)
            }
        }
        var blocks = runs.enumerated().map { index, run in
            let rows = rows(for: run)
            let tool = run.kind == .tool
                ? rows.first(where: { $0.kind == .call })
                    .map { toolKind(forCallLine: $0.text) } ?? .generic
                : nil
            return DisplayBlock(id: "rb-\(index)", kind: run.kind,
                                tool: tool, rows: rows)
        }
        // A TRAILING tool run that has started (at least one invocation)
        // but produced no output yet shows the muted inline waiting line —
        // appended into the run's block, never a block of its own.
        if let last = blocks.last,
           last.kind == .tool,
           last.rows.contains(where: { $0.kind == .call }),
           !last.rows.contains(where: { $0.kind == .output }) {
            blocks[blocks.count - 1] = DisplayBlock(
                id: last.id, kind: last.kind, tool: last.tool,
                rows: last.rows + [BlockRow(.waiting, waitingRowText)])
        }
        return blocks
    }

    /// Content rows for one role run.
    static func rows(for run: TranscriptBlock) -> [BlockRow] {
        switch run.kind {
        case .user, .agent:
            return nonEmptyLines(run.text).map { BlockRow(.prose, $0) }
        case .system:
            return nonEmptyLines(run.text).map { BlockRow(.meta, $0) }
        case .tool:
            return toolRows(run.text)
        case .unknown:
            return nonEmptyLines(run.text).map { BlockRow(.output, $0) }
        }
    }

    /// A tool run's invocation/output rows. Invocations ("call lines") are
    /// shell echoes (`$ cmd`) or — only as the run's FIRST content line —
    /// a bare tool invocation whose first word is a known tool verb
    /// (structured tool runs have no shell-prompt echo). Every other line
    /// is output, kept verbatim (leading/trailing blanks trimmed, interior
    /// blanks kept as output spacing).
    static func toolRows(_ text: String) -> [BlockRow] {
        var lines = RecentOutputRender.messageLines(text)
        while lines.first.map({ $0.trimmingCharacters(in: .whitespaces).isEmpty }) == true {
            lines.removeFirst()
        }
        while lines.last.map({ $0.trimmingCharacters(in: .whitespaces).isEmpty }) == true {
            lines.removeLast()
        }
        var rows: [BlockRow] = []
        for (index, line) in lines.enumerated() {
            if isCallLine(line, firstContentLine: index == 0) {
                rows.append(BlockRow(.call,
                                     line.trimmingCharacters(in: .whitespaces)))
            } else {
                rows.append(BlockRow(.output, line))
            }
        }
        return rows
    }

    /// Whether one tool line is an invocation: a shell echo (`$ cmd`), or
    /// the run's first content line when it starts with a known tool verb.
    static func isCallLine(_ line: String, firstContentLine: Bool) -> Bool {
        let trimmed = line.trimmingCharacters(in: .whitespaces)
        if trimmed == "$" || trimmed.hasPrefix("$ ") { return true }
        guard firstContentLine else { return false }
        return Self.bareToolVerbs.contains(firstWord(of: trimmed))
    }

    /// The locked icon-kind vocabulary for one call line: the first command
    /// word decides (read_file → doc, edit → code, grep family → search,
    /// anything else that actually invoked → terminal). Only a run with NO
    /// recognizable invocation falls back to `.generic`.
    static func toolKind(forCallLine line: String) -> ToolKind {
        var command = line.trimmingCharacters(in: .whitespaces)
        if command.hasPrefix("$") {
            command = command.dropFirst().trimmingCharacters(in: .whitespaces)
        }
        switch firstWord(of: command) {
        case "read_file", "write_file", "view":
            return .doc
        case "edit", "apply_patch", "patch":
            return .code
        case "grep", "rg", "ag", "ack", "find", "search_files":
            return .search
        case let verb where !verb.isEmpty:
            return .terminal
        default:
            return .generic
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

    // MARK: Private row-building helpers

    /// Tool verbs that may open a bare (no `$`) invocation line — the same
    /// vocabulary `toolKind(forCallLine:)` classifies, so a bare first line
    /// is a call only when its icon kind is knowable.
    private static let bareToolVerbs: Set<String> = [
        "read_file", "write_file", "view",
        "edit", "apply_patch", "patch",
        "grep", "rg", "ag", "ack", "find", "search_files",
    ]

    private static func firstWord(of line: String) -> String {
        line.split(whereSeparator: { $0.isWhitespace }).first.map(String.init)?
            .lowercased() ?? ""
    }

    private static func nonEmptyLines(_ text: String) -> [String] {
        RecentOutputRender.messageLines(text).filter {
            !$0.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
        }
    }
}

// MARK: - Code/diff highlighting (pure block-rendering helpers)

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

/// Pure block-rendering helpers (code/diff line classification, divider
/// classification, line splitting). The block renderer consumes these; no
/// SwiftUI lives here.
enum RecentOutputRender {
    static func messageLines(_ text: String) -> [String] {
        text.components(separatedBy: .newlines)
    }

    static func isBoundary(previous: TranscriptBlock?, current: TranscriptBlock) -> Bool {
        previous?.kind != current.kind
    }

    /// Highlight only a tool run that clearly contains source or diff
    /// syntax. Prose and ordinary command output stay plain monospace text.
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

    /// One line's syntax segments (keywords, strings, diff marks,
    /// comments). Public so the block renderer can color output lines with
    /// the theme's ANSI-slot segment colors.
    static func highlightSegments(in line: String) -> [RecentCodeSegment] {
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
    /// never content inside a run.
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
