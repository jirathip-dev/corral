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
                for try await line in bytes.lines {
                    let frames = parser.feed(line + "\n")
                    for frame in frames {
                        onEvent(frame)
                    }
                }
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

    /// Decode a parsed SSE frame into a fleet event, keyed by the current
    /// agents dictionary so delta application lives here, unit-testable.
    static func decode(_ frame: SSEFrame) -> FleetEvent? {
        let decoder = JSONDecoder()
        let raw = frame.data
        guard let data = raw.data(using: .utf8) else { return nil }
        switch frame.kind {
        case .snapshot:
            guard let snapshot = try? decoder.decode(Snapshot.self, from: data) else { return nil }
            return .snapshot(snapshot)
        case .delta:
            guard let delta = try? decoder.decode(Delta.self, from: data) else { return nil }
            return .delta(delta)
        case .message:
            return nil
        }
    }
}
