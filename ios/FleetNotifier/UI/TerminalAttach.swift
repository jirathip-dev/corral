import Foundation
import SwiftUI
import UIKit

/// A bounded worktree entry returned by corrald's read-only browser route.
struct CorralWorktree: Codable, Identifiable, Equatable, Sendable {
    let repo: String
    let branch: String
    let path: String
    let workspaceId: String
    let paneId: String?
    let isPrunable: Bool
    let dirty: Bool
    let agentAttached: Bool
    let currentFocus: Bool
    var id: String { workspaceId }

    enum CodingKeys: String, CodingKey {
        case repo, branch, path, workspaceId = "workspace_id", paneId = "pane_id"
        case isPrunable = "is_prunable", dirty, agentAttached = "agent_attached"
        case currentFocus = "current_focus"
    }
}

struct CorralWorktreeResponse: Codable, Sendable { let worktrees: [CorralWorktree] }

enum TerminalAttachError: LocalizedError, Equatable, Sendable {
    case unavailable
    case invalidHost
    case encoding
    case handshakeTimedOut
    case protocolError(String)
    case server(kind: String, message: String)
    case network

    var errorDescription: String? {
        switch self {
        case .unavailable:
            return "Terminal worktree is unavailable."
        case .invalidHost:
            return "Terminal host is invalid."
        case .encoding:
            return "Terminal request could not be encoded."
        case .handshakeTimedOut:
            return "Terminal handshake timed out."
        case .protocolError(let message):
            return message
        case .server(_, let message):
            return message
        case .network:
            return "Terminal connection failed."
        }
    }
}

protocol TerminalAttachSession: AnyObject {
    func connect(worktree: CorralWorktree,
                 onFrame: @escaping @Sendable (TerminalFrame) -> Void) async throws
    func close()
}

/// WS client for the tmux fallback. Authentication is the same signed Attach
/// envelope as the drive plane; the server still validates cwd confinement.
final class TerminalAttachClient: NSObject, @unchecked Sendable, TerminalAttachSession {
    private let host: URL
    private let session: URLSession
    private let keyId: String
    private let signer: DeviceSigner
    private var task: URLSessionWebSocketTask?
    private var connectionGeneration: UInt64 = 0
    private let lock = NSLock()
    private static let handshakeTimeoutNanoseconds: UInt64 = 5_000_000_000

    init(host: URL, session: URLSession = .shared, keyId: String, signer: DeviceSigner) {
        self.host = host; self.session = session; self.keyId = keyId; self.signer = signer
    }

    func worktrees() async throws -> [CorralWorktree] {
        let (data, response) = try await session.data(from: host.appendingPathComponent("/v1/worktrees"))
        guard (response as? HTTPURLResponse)?.statusCode == 200 else { throw DriveError.network("worktree browser failed") }
        return try JSONDecoder().decode(CorralWorktreeResponse.self, from: data).worktrees
    }

    static func websocketURL(for host: URL) -> URL? {
        guard var components = URLComponents(url: host, resolvingAgainstBaseURL: false),
              let scheme = components.scheme?.lowercased() else { return nil }
        switch scheme {
        case "http": components.scheme = "ws"
        case "https": components.scheme = "wss"
        case "ws", "wss": break
        default: return nil
        }
        return components.url?.appendingPathComponent("v1/terminal")
    }

    enum Message: Equatable, Sendable {
        case opened
        case frame(TerminalFrame)
    }

    static func parseMessage(_ text: String, afterOpen: Bool) throws -> Message {
        guard let data = text.data(using: .utf8) else {
            throw TerminalAttachError.protocolError("Terminal sent invalid text.")
        }
        let wire: TerminalWireMessage
        do {
            wire = try JSONDecoder().decode(TerminalWireMessage.self, from: data)
        } catch {
            throw TerminalAttachError.protocolError("Terminal sent a malformed frame.")
        }
        switch wire.type {
        case "opened":
            return .opened
        case "frame":
            guard afterOpen else {
                throw TerminalAttachError.protocolError("Terminal frame arrived before handshake.")
            }
            return .frame(TerminalFrame(type: wire.type, ansi: wire.ansi,
                                        cursorX: wire.cursorX, cursorY: wire.cursorY))
        case "error":
            throw TerminalAttachError.server(
                kind: wire.kind ?? "terminal_error",
                message: wire.message ?? "Terminal request was refused.")
        default:
            throw TerminalAttachError.protocolError("Terminal sent an unexpected frame.")
        }
    }

    func connect(worktree: CorralWorktree, onFrame: @escaping @Sendable (TerminalFrame) -> Void) async throws {
        guard !worktree.path.isEmpty, !worktree.workspaceId.isEmpty else {
            throw TerminalAttachError.unavailable
        }
        try Task.checkCancellation()
        guard let socketURL = Self.websocketURL(for: host) else {
            throw TerminalAttachError.invalidHost
        }
        let requestId = DriveClient.newRequestId()
        let payload: CanonicalJSON.Value = .object([
            (key: "cwd", value: .string(worktree.path)),
            (key: "workspace_id", value: .string(worktree.workspaceId)),
        ])
        let bytes = CanonicalJSON.envelopeBytes(requestId: requestId, capability: Capability.attach.rawValue,
                                                target: worktree.workspaceId, payload: payload, rev: nil)
        let signature: String
        do {
            signature = try signer.sign(bytes).base64EncodedString()
        } catch {
            throw TerminalAttachError.encoding
        }
        let auth = CanonicalJSON.signedDriveBody(keyId: keyId, signatureB64: signature,
                                                  envelopeBytes: bytes)
        let escapedCwd: Data
        do {
            escapedCwd = try JSONEncoder().encode(worktree.path)
        } catch {
            throw TerminalAttachError.encoding
        }
        var body = Data("{\"auth\":".utf8)
        body.append(auth)
        body.append(Data(",\"cwd\":".utf8))
        body.append(escapedCwd)
        body.append(Data("}".utf8))
        guard let bodyText = String(data: body, encoding: .utf8) else {
            throw TerminalAttachError.encoding
        }

        let socket = session.webSocketTask(with: socketURL)
        let generation = install(socket)
        defer { clear(socket, generation: generation) }
        do {
            socket.resume()
            try await socket.send(.string(bodyText))
            var didOpen = false
            while true {
                let message: URLSessionWebSocketTask.Message
                if didOpen {
                    message = try await socket.receive()
                } else {
                    message = try await Self.receiveHandshake(from: socket)
                }
                switch message {
                case .string(let text):
                    switch try Self.parseMessage(text, afterOpen: didOpen) {
                    case .opened:
                        didOpen = true
                    case .frame(let frame):
                        onFrame(frame)
                    }
                case .data:
                    throw TerminalAttachError.protocolError("Terminal sent a binary frame.")
                @unknown default:
                    throw TerminalAttachError.protocolError("Terminal sent an unsupported frame.")
                }
            }
        } catch {
            if Task.isCancelled || !isCurrent(socket, generation: generation) {
                throw CancellationError()
            }
            if let terminalError = error as? TerminalAttachError {
                throw terminalError
            }
            throw TerminalAttachError.network
        }
    }

    func sendInput(_ text: String) async throws {
        guard !text.contains("\0") else {
            throw TerminalAttachError.protocolError("Terminal input is invalid.")
        }
        try await send(.input(text: text))
    }

    func resize(cols: Int, rows: Int) async throws {
        guard cols > 0, rows > 0 else {
            throw TerminalAttachError.protocolError("Terminal dimensions must be positive.")
        }
        try await send(.resize(cols: cols, rows: rows))
    }

    func close() {
        lock.lock()
        let socket = task
        task = nil
        connectionGeneration &+= 1
        lock.unlock()
        socket?.cancel(with: .normalClosure, reason: nil)
    }

    private enum Command { case input(text: String); case resize(cols: Int, rows: Int) }

    private func send(_ command: Command) async throws {
        let value: [String: Any]
        switch command {
        case .input(let text): value = ["type": "input", "text": text]
        case .resize(let cols, let rows): value = ["type": "resize", "cols": cols, "rows": rows]
        }
        let data: Data
        do {
            data = try JSONSerialization.data(withJSONObject: value)
        } catch {
            throw TerminalAttachError.encoding
        }
        guard let text = String(data: data, encoding: .utf8) else {
            throw TerminalAttachError.encoding
        }
        lock.lock(); let socket = task; lock.unlock()
        guard let socket else { throw TerminalAttachError.network }
        try Task.checkCancellation()
        try await socket.send(.string(text))
    }

    private func install(_ socket: URLSessionWebSocketTask) -> UInt64 {
        lock.lock()
        let previous = task
        connectionGeneration &+= 1
        let generation = connectionGeneration
        task = socket
        lock.unlock()
        previous?.cancel(with: .normalClosure, reason: nil)
        return generation
    }

    private func clear(_ socket: URLSessionWebSocketTask, generation: UInt64) {
        lock.lock()
        if task === socket, connectionGeneration == generation {
            task = nil
        }
        lock.unlock()
    }

    private func isCurrent(_ socket: URLSessionWebSocketTask, generation: UInt64) -> Bool {
        lock.lock()
        defer { lock.unlock() }
        return task === socket && connectionGeneration == generation
    }

    private static func receiveHandshake(from socket: URLSessionWebSocketTask) async throws -> URLSessionWebSocketTask.Message {
        try await withThrowingTaskGroup(of: URLSessionWebSocketTask.Message.self) { group in
            group.addTask { try await socket.receive() }
            group.addTask {
                try await Task.sleep(nanoseconds: Self.handshakeTimeoutNanoseconds)
                socket.cancel(with: .goingAway, reason: nil)
                throw TerminalAttachError.handshakeTimedOut
            }
            defer { group.cancelAll() }
            guard let message = try await group.next() else {
                throw TerminalAttachError.handshakeTimedOut
            }
            return message
        }
    }
}

private struct TerminalWireMessage: Decodable {
    let type: String
    let ansi: String?
    let cursorX: Int?
    let cursorY: Int?
    let kind: String?
    let message: String?

    enum CodingKeys: String, CodingKey {
        case type, ansi, kind, message
        case cursorX = "cursor_x"
        case cursorY = "cursor_y"
    }
}

@MainActor
final class TerminalAttachSessionModel: ObservableObject {
    enum State: Equatable, Sendable {
        case connecting
        case connected
        case failed(String)
    }

    @Published private(set) var state: State = .connecting
    @Published private(set) var output = ""
    @Published private(set) var cursor = (0, 0)

    let client: (any TerminalAttachSession)?
    let worktree: CorralWorktree?
    private var connectionGeneration: UInt64 = 0

    init(client: (any TerminalAttachSession)?, worktree: CorralWorktree?) {
        self.client = client
        self.worktree = worktree
    }

    func start() async {
        let generation = connectionGeneration
        state = .connecting
        guard let client, let worktree else {
            state = .failed(TerminalAttachError.unavailable.localizedDescription)
            return
        }
        do {
            try await client.connect(worktree: worktree) { [weak self] frame in
                Task { @MainActor [weak self] in
                    guard let self, self.connectionGeneration == generation else { return }
                    self.output = frame.ansi ?? self.output
                    self.cursor = (frame.cursorX ?? 0, frame.cursorY ?? 0)
                    self.state = .connected
                }
            }
            guard connectionGeneration == generation, !Task.isCancelled else { return }
            state = .failed("Terminal connection closed.")
        } catch is CancellationError {
            return
        } catch {
            guard connectionGeneration == generation, !Task.isCancelled else { return }
            state = .failed(error.localizedDescription)
        }
    }

    func retry() {
        connectionGeneration &+= 1
        client?.close()
    }

    func stop() {
        connectionGeneration &+= 1
        client?.close()
    }
}

struct TerminalFrame: Codable, Equatable, Sendable {
    let type: String
    let ansi: String?
    let cursorX: Int?
    let cursorY: Int?
    enum CodingKeys: String, CodingKey { case type, ansi, cursorX = "cursor_x", cursorY = "cursor_y" }
}

/// Lightweight SwiftTerm-compatible screen surface. The daemon supplies ANSI
/// frames and cursor metadata; the view keeps one text buffer, so frames never
/// stack as duplicate SwiftUI rows or flicker during refresh.
struct SwiftTermTerminalView: UIViewRepresentable {
    let text: String
    let cursor: (Int, Int)
    func makeUIView(context: Context) -> UITextView {
        let view = UITextView(); view.isEditable = false; view.isSelectable = true
        view.font = UIFont.monospacedSystemFont(ofSize: 12, weight: .regular)
        view.backgroundColor = .black; view.textColor = .white; return view
    }
    func updateUIView(_ view: UITextView, context: Context) {
        view.text = text.replacingOccurrences(of: "\u{1B}\\[[0-9;]*m", with: "", options: .regularExpression)
        view.accessibilityLabel = "Terminal cursor column \(cursor.0), row \(cursor.1)"
    }
}

struct WorktreeBrowserView: View {
    let client: TerminalAttachClient
    @State private var rows: [CorralWorktree] = []
    @State private var selected: CorralWorktree?
    @State private var error: String?
    var body: some View {
        NavigationSplitView {
            List(rows) { row in
                Button { selected = row } label: {
                    VStack(alignment: .leading) { Text(row.branch); Text(row.repo).font(.caption).foregroundStyle(.secondary) }
                }
            }.navigationTitle("Worktrees").task { do { rows = try await client.worktrees() } catch let failure { error = failure.localizedDescription } }
        } detail: {
            if let selected { TerminalAttachView(client: client, worktree: selected) } else { Text("Select a worktree") }
        }.alert("Terminal", isPresented: .constant(error != nil)) { Button("OK") { error = nil } } message: { Text(error ?? "") }
    }
}

struct TerminalAttachView: View {
    let client: (any TerminalAttachSession)?
    let worktree: CorralWorktree?
    @Environment(\.dismiss) private var dismiss
    @StateObject private var sessionModel: TerminalAttachSessionModel
    @State private var attempt = 0

    init(client: (any TerminalAttachSession)?, worktree: CorralWorktree?) {
        self.client = client
        self.worktree = worktree
        _sessionModel = StateObject(wrappedValue: TerminalAttachSessionModel(
            client: client, worktree: worktree))
    }

    var body: some View {
        Group {
            switch sessionModel.state {
            case .connecting:
                VStack(alignment: .leading, spacing: 12) {
                    ProgressView("Connecting to Terminal…")
                    if !sessionModel.output.isEmpty {
                        terminalSurface
                    }
                }
            case .connected:
                terminalSurface
            case .failed(let message):
                VStack(alignment: .leading, spacing: 12) {
                    Label("Terminal unavailable", systemImage: "exclamationmark.triangle")
                        .font(.headline)
                    Text(message)
                        .foregroundStyle(.secondary)
                        .textSelection(.enabled)
                    HStack(spacing: 12) {
                        Button("Retry") {
                            sessionModel.retry()
                            attempt &+= 1
                        }
                        .buttonStyle(.borderedProminent)
                        Button("Done", role: .cancel) {
                            sessionModel.stop()
                            dismiss()
                        }
                        .buttonStyle(.bordered)
                    }
                }
                .accessibilityElement(children: .contain)
                .accessibilityLabel("Terminal error: \(message)")
            }
        }
        .padding()
        .task(id: attempt) {
            await sessionModel.start()
        }
        .onDisappear {
            sessionModel.stop()
        }
        .navigationTitle(worktree?.branch ?? "Terminal")
    }

    private var terminalSurface: some View {
        VStack(alignment: .leading, spacing: 8) {
            Text("cursor: \(sessionModel.cursor.0),\(sessionModel.cursor.1)")
                .font(.caption.monospaced())
                .accessibilityLabel("Terminal cursor column \(sessionModel.cursor.0), row \(sessionModel.cursor.1)")
            SwiftTermTerminalView(text: sessionModel.output, cursor: sessionModel.cursor)
                .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
        }
    }
}

#if DEBUG
struct TerminalAttachDemoView: View {
    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            Text("cursor: 80,23")
                .font(.caption.monospaced())
            SwiftTermTerminalView(
                text: "\u{1b}[1;34mLAZYGIT\u{1b}[0m  worktree\n\n▸ Files\n  src/tmux.rs\n  ios/FleetNotifier/UI/TerminalAttach.swift\n\n  2 changed files  •  ready",
                cursor: (80, 23))
        }
        .padding()
        .navigationTitle("Terminal Preview")
        .accessibilityElement(children: .contain)
        .accessibilityLabel("Terminal ANSI preview; cursor column 80, row 23")
    }
}
#endif
