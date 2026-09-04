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

    /// Full point-in-time state (schema v5).
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

    /// #399 B3/B4: the host's stable public identity —
    /// `GET /host-key` `{algorithm, public_key}`. Consumed BEFORE pairing
    /// (fingerprint confirmation) and re-checked before opening a live
    /// stream after launch; a mismatch fails closed. Shape validation
    /// (X25519 base64 of 32 bytes) lives in `HostKeyTrust` — this is the
    /// transport only.
    func fetchHostKey() async throws -> HostKeyResponse {
        var request = URLRequest(url: host.appendingPathComponent("/host-key"))
        request.timeoutInterval = 15
        let (data, response) = try await session.data(for: request)
        guard let http = response as? HTTPURLResponse, http.statusCode == 200 else {
            throw DriveError.network("host-key failed: \(response)")
        }
        return try JSONDecoder().decode(HostKeyResponse.self, from: data)
    }

    /// Live SSE event stream with automatic reconnect.
    ///
    /// - Resumes from `lastEventId` via the `Last-Event-ID` header.
    /// - On disconnect (server close or network error) backs off
    ///   1s → 2s → 4s … capped at 30s, then reconnects from the latest
    ///   event id delivered so far (`onEvent` reports ids).
    /// - Genuine failures (non-200, URLError, request errors) report via
    ///   `onConnectionError`; clean server EOF and task cancellation are
    ///   NOT reported — the former is the daemon's normal reconnect path,
    ///   the latter is the owner ending the stream.
    /// - A successful 200 (including reconnects) reports via `onConnected`
    ///   — an idle fleet emits no frames (keep-alives are comments, never
    ///   framed), so the owner needs this to clear a stale `.error`
    ///   indicator (review F2).
    /// - Ends only on cancellation.
    func stream(lastEventId: @escaping @Sendable () -> UInt64?,
                onEvent: @escaping @Sendable (SSEFrame) -> Void,
                onConnected: (@Sendable () -> Void)? = nil,
                onConnectionError: (@Sendable (String) -> Void)? = nil) async {
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
                guard let http = response as? HTTPURLResponse else {
                    throw DriveError.network("events failed: not an HTTP response")
                }
                guard http.statusCode == 200 else {
                    // F1: name the status and URL — a bare
                    // `error.localizedDescription` used to discard this
                    // entirely (DriveError had no LocalizedError conformance).
                    throw DriveError.network(
                        "events failed: HTTP \(http.statusCode) from \(request.url?.path ?? "/events")")
                }
                backoff = 1 // connected: reset the backoff ladder
                onConnected?()
                var parser = SSEParser()
                try await Self.consumeLines(from: bytes, parser: &parser, onEvent: onEvent)
                // Clean EOF: server closed the stream; reconnect.
            } catch is CancellationError {
                return
            } catch {
                // #92: connection failures used to VANISH in this bare
                // catch — the spinner spun forever with no banner, no log
                // line, no diagnosis (the #79/#82 decode-failure treatment
                // never reached the connection path). Report the reason;
                // the backoff/retry ladder below still runs and a later
                // good frame's apply() returns the state to .connected, so
                // a transient failure is visible but not fatal.
                //
                // Owner cancellation while awaiting bytes(for:) surfaces as
                // a URLError.cancelled — end the stream then, but ONLY when
                // the task is actually cancelled; a stray .cancelled without
                // task cancellation must fall through to reporting (review
                // F6), or the stream could end silently forever.
                if (error as? URLError)?.code == .cancelled, Task.isCancelled { return }
                onConnectionError?(error.localizedDescription)
            }
            if Task.isCancelled { return }
            try? await Task.sleep(nanoseconds: backoff * 1_000_000_000)
            backoff = min(backoff * 2, 30)
        }
    }

    /// Byte-consumption path for `stream()`, factored out of the reconnect
    /// loop for clarity. The regression test drives `stream()` (never
    /// `parser.feed()` with hand-built strings), so the REAL `URLSession`
    /// byte path — including this loop — is what the suite exercises.
    ///
    /// SSE framing is byte-defined: lines end at `0x0a` and frames close
    /// on an EMPTY line — the terminator the parser needs. `AsyncLineSequence`
    /// strips terminators AND drops blank lines, so it can never deliver a
    /// frame boundary; instead consume raw bytes and hand the parser each
    /// line INCLUDING its terminator.
    ///
    /// A partial final line at EOF is DISCARDED (WHATWG EventSource
    /// semantics): `SSEParser.feed` only completes frames at a newline
    /// boundary, so emitting it would hand `decode()` half a JSON object
    /// and raise a spurious decode-failure banner. `stream()` reconnects
    /// from `Last-Event-ID`, so the daemon replays whatever was lost.
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
