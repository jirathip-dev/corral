import Foundation

/// Decoded SSE event for the read path: a full snapshot (cursor too old or
/// lagged) or an incremental delta.
enum FleetEvent: Sendable {
    case snapshot(Snapshot)
    case delta(Delta)
}

/// The corrald read-path client (R2): `GET /snapshot` and `GET /events`
/// with `Last-Event-ID` resume, delta application, and reconnect backoff.
///
/// Backgrounded = no connection (D5): the owner cancels the stream task;
/// on foreground the stream reconnects and the daemon resumes from the last
/// seen rev (snapshot when the cursor is too old, deltas otherwise).
struct CorraldClient: Sendable {
    let host: URL
    let session: URLSession

    init(host: URL, session: URLSession = .shared) {
        self.host = host
        self.session = session
    }

    /// Full point-in-time state (schema v3).
    func fetchSnapshot() async throws -> Snapshot {
        var request = URLRequest(url: host.appendingPathComponent("/snapshot"))
        request.timeoutInterval = 15
        let (data, response) = try await session.data(for: request)
        guard let http = response as? HTTPURLResponse, http.statusCode == 200 else {
            throw DriveError.network("snapshot failed: \(response)")
        }
        // All models carry explicit snake_case CodingKeys; no strategy.
        return try JSONDecoder().decode(Snapshot.self, from: data)
    }

    /// Live SSE event stream with automatic reconnect.
    ///
    /// - Resumes from `lastEventId` via the `Last-Event-ID` header.
    /// - On disconnect (server close or network error) backs off
    ///   1s → 2s → 4s … capped at 30s, then reconnects from the latest
    ///   event id delivered so far (`onEvent` reports ids).
    /// - Ends only on cancellation.
    func stream(lastEventId: @escaping @Sendable () -> UInt64?,
                onEvent: @escaping @Sendable (SSEFrame) -> Void) async {
        var backoff: UInt64 = 1
        while !Task.isCancelled {
            do {
                var request = URLRequest(url: host.appendingPathComponent("/events"))
                request.timeoutInterval = 60
                request.setValue("text/event-stream", forHTTPHeaderField: "Accept")
                if let rev = lastEventId() {
                    request.setValue(String(rev), forHTTPHeaderField: "Last-Event-ID")
                }
                let (bytes, response) = try await session.bytes(for: request)
                guard let http = response as? HTTPURLResponse, http.statusCode == 200 else {
                    throw DriveError.network("events failed: \(response)")
                }
                backoff = 1 // connected: reset the backoff ladder
                var parser = SSEParser()
                try await Self.consumeLines(from: bytes, parser: &parser, onEvent: onEvent)
                // Clean EOF: server closed the stream; reconnect.
            } catch is CancellationError {
                return
            } catch {
                // Transient network failure; back off and retry.
            }
            if Task.isCancelled { return }
            try? await Task.sleep(nanoseconds: backoff * 1_000_000_000)
            backoff = min(backoff * 2, 30)
        }
    }

    /// Byte-consumption seam (issue #90): the loop `stream()` runs, kept
    /// callable so the regression test can drive it through the REAL
    /// `URLSession` byte path (a `URLProtocol` mock serves the payload) —
    /// a hand-built-string `parser.feed()` test is vacuous against this
    /// defect.
    ///
    /// SSE framing is byte-defined: lines end at `0x0a` and frames close
    /// on an EMPTY line — the terminator the parser needs. `AsyncLineSequence`
    /// strips terminators AND drops blank lines, so it can never deliver a
    /// frame boundary; instead consume raw bytes and hand the parser each
    /// line INCLUDING its terminator. A trailing partial line is flushed so
    /// a server that closes without a final newline still yields its frame.
    static func consumeLines(
        from bytes: URLSession.AsyncBytes,
        parser: inout SSEParser,
        onEvent: @escaping @Sendable (SSEFrame) -> Void
    ) async throws {
        var chunk = [UInt8]()
        chunk.reserveCapacity(16 * 1024)
        for try await byte in bytes {
            chunk.append(byte)
            guard byte == 0x0a else { continue }
            let frames = parser.feed(String(decoding: chunk, as: UTF8.self))
            chunk.removeAll(keepingCapacity: true)
            for frame in frames {
                onEvent(frame)
            }
        }
        if !chunk.isEmpty {
            for frame in parser.feed(String(decoding: chunk, as: UTF8.self)) {
                onEvent(frame)
            }
        }
    }

    /// What one SSE frame decoded to. `#79` defect 2: the old
    /// `FleetEvent?` shape collapsed everything into one silent `nil`,
    /// so a schema drift was a SILENT infinite spinner. `failed` carries
    /// the underlying reason so the UI can surface it and a log line can
    /// name it. Review F1: SSE keep-alives are COMMENT lines — the
    /// parser never frames them — so any `.message` frame that reaches
    /// decode carries data under an event name this client does not
    /// recognize; that is protocol drift and is REPORTED, not ignored.
    /// `.ignored` remains only for the defensively-unreachable
    /// empty-data case.
    enum DecodeOutcome {
        case event(FleetEvent)
        case ignored
        case failed(String)
    }

    /// Decode a parsed SSE frame into a fleet event, unit-testable.
    /// Errors are REPORTED, never swallowed (#79 acceptance: a malformed
    /// frame must be visible, not an endless spinner).
    static func decode(_ frame: SSEFrame) -> DecodeOutcome {
        let decoder = JSONDecoder()
        let raw = frame.data
        guard let data = raw.data(using: .utf8) else {
            return .failed("frame data is not UTF-8 (\(raw.count) chars)")
        }
        switch frame.kind {
        case .snapshot:
            do {
                return .event(.snapshot(try decoder.decode(Snapshot.self, from: data)))
            } catch {
                return .failed("snapshot frame undecodable: \(error)")
            }
        case .delta:
            do {
                return .event(.delta(try decoder.decode(Delta.self, from: data)))
            } catch {
                return .failed("delta frame undecodable: \(error)")
            }
        case .message:
            guard !raw.isEmpty else { return .ignored }
            let name = frame.eventName ?? "<none>"
            return .failed(
                "unrecognized event '\(name)' with \(raw.count) bytes of data — protocol drift?")
        }
    }
}
