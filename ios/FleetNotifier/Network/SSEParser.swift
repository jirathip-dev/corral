import Foundation

/// One SSE frame as emitted by corrald (src/api/mod.rs): `event` is
/// "snapshot" | "delta", `id` is the monotonic rev, `data` the JSON payload.
struct SSEFrame: Sendable {
    enum Kind: Sendable {
        case snapshot
        case delta
        case message
    }

    var kind: Kind
    var id: UInt64?
    var data: String

    var eventType: String {
        switch kind {
        case .snapshot: return "snapshot"
        case .delta: return "delta"
        case .message: return "message"
        }
    }
}

/// Incremental line-based SSE parser (RFC 8895-ish subset the daemon emits).
/// Frames are separated by blank lines; `event:`/`id:`/`data:` fields
/// accumulate; lines starting with `:` are comments (keep-alives) and are
/// ignored. Handles both `\n` and `\r\n` line endings.
struct SSEParser {
    private var buffer = ""
    private var event = SSEFrame.Kind.message
    private var id: UInt64?
    private var dataLines: [String] = []

    mutating func feed(_ chunk: String) -> [SSEFrame] {
        var frames: [SSEFrame] = []
        // CRLF is a single Swift grapheme cluster, so `firstIndex(of: "\n")`
        // would miss it; normalize to LF first.
        buffer += chunk.replacingOccurrences(of: "\r\n", with: "\n")
        while let newline = buffer.firstIndex(of: "\n") {
            var line = String(buffer[..<newline])
            buffer.removeSubrange(...newline)
            if line.hasSuffix("\r") {
                line.removeLast()
            }
            if line.isEmpty {
                if let frame = take() {
                    frames.append(frame)
                }
            } else if !line.hasPrefix(":") {
                parse(field: line)
            }
        }
        return frames
    }

    mutating func finish() -> [SSEFrame] {
        var frames: [SSEFrame] = []
        if !buffer.isEmpty {
            parse(field: buffer)
            buffer = ""
        }
        if let frame = take() {
            frames.append(frame)
        }
        return frames
    }

    private mutating func parse(field: String) {
        if field.hasPrefix("event:") {
            let value = String(field.dropFirst(6)).trimmingCharacters(in: .whitespaces)
            event = value == "snapshot" ? .snapshot : value == "delta" ? .delta : .message
        } else if field.hasPrefix("id:") {
            id = UInt64(String(field.dropFirst(3)).trimmingCharacters(in: .whitespaces))
        } else if field.hasPrefix("data:") {
            var value = String(field.dropFirst(5))
            if value.hasPrefix(" ") { value.removeFirst() }
            dataLines.append(value)
        }
    }

    private mutating func take() -> SSEFrame? {
        guard !dataLines.isEmpty else {
            event = .message
            id = nil
            return nil
        }
        let frame = SSEFrame(kind: event, id: id, data: dataLines.joined(separator: "\n"))
        event = .message
        id = nil
        dataLines = []
        return frame
    }
}
