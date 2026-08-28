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

/// WS client for the tmux fallback. Authentication is the same signed Attach
/// envelope as the drive plane; the server still validates cwd confinement.
final class TerminalAttachClient: NSObject, @unchecked Sendable {
    private let host: URL
    private let session: URLSession
    private let keyId: String
    private let signer: DeviceSigner
    private var task: URLSessionWebSocketTask?
    private let lock = NSLock()

    init(host: URL, session: URLSession = .shared, keyId: String, signer: DeviceSigner) {
        self.host = host; self.session = session; self.keyId = keyId; self.signer = signer
    }

    func worktrees() async throws -> [CorralWorktree] {
        let (data, response) = try await session.data(from: host.appendingPathComponent("/v1/worktrees"))
        guard (response as? HTTPURLResponse)?.statusCode == 200 else { throw DriveError.network("worktree browser failed") }
        return try JSONDecoder().decode(CorralWorktreeResponse.self, from: data).worktrees
    }

    func connect(worktree: CorralWorktree, onFrame: @escaping @Sendable (TerminalFrame) -> Void) async throws {
        let requestId = DriveClient.newRequestId()
        let bytes = CanonicalJSON.envelopeBytes(requestId: requestId, capability: Capability.attach.rawValue,
                                                target: worktree.workspaceId, payload: .null, rev: nil)
        let signature = try signer.sign(bytes).base64EncodedString()
        let escapedKey = try JSONEncoder().encode(keyId)
        let escapedSignature = try JSONEncoder().encode(signature)
        let socket = session.webSocketTask(with: host.appendingPathComponent("/v1/terminal"))
        let escapedCwd = try JSONEncoder().encode(worktree.path)
        var body = Data("{\"auth\":{\"key_id\":".utf8); body.append(escapedKey)
        body.append(Data(",\"signature\":".utf8)); body.append(escapedSignature)
        body.append(Data(",\"envelope\":".utf8)); body.append(bytes); body.append(Data("},\"cwd\":".utf8)); body.append(escapedCwd); body.append(Data("}".utf8))
        lock.lock(); task = socket; lock.unlock()
        socket.resume()
        try await socket.send(.string(String(data: body, encoding: .utf8)!))
        while true {
            let message = try await socket.receive()
            guard case .string(let text) = message,
                  let data = text.data(using: .utf8) else { continue }
            if let frame = try? JSONDecoder().decode(TerminalFrame.self, from: data), frame.type == "frame" {
                onFrame(frame)
            }
        }
    }

    func sendInput(_ text: String) async throws { try await send(.input(text: text)) }
    func resize(cols: Int, rows: Int) async throws { try await send(.resize(cols: cols, rows: rows)) }
    func close() { lock.lock(); defer { lock.unlock() }; task?.cancel(with: .normalClosure, reason: nil); task = nil }

    private enum Command { case input(text: String); case resize(cols: Int, rows: Int) }
    private func send(_ command: Command) async throws {
        let value: [String: Any]
        switch command {
        case .input(let text): value = ["type": "input", "text": text]
        case .resize(let cols, let rows): value = ["type": "resize", "cols": cols, "rows": rows]
        }
        let data = try JSONSerialization.data(withJSONObject: value)
        lock.lock(); let socket = task; lock.unlock()
        guard let socket else { throw DriveError.network("terminal is not connected") }
        try await socket.send(.string(String(data: data, encoding: .utf8)!))
    }
}


struct TerminalFrame: Codable, Sendable {
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
        view.text = text.replacingOccurrences(of: "\\u{1B}[", with: "")
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
    let client: TerminalAttachClient
    let worktree: CorralWorktree
    @State private var output = ""
    @State private var cursor = (0, 0)
    var body: some View {
        SwiftTermTerminalView(text: output, cursor: cursor).task {
            try? await client.connect(worktree: worktree) { frame in
                Task { @MainActor in output = frame.ansi ?? output; cursor = (frame.cursorX ?? 0, frame.cursorY ?? 0) }
            }
        }.onDisappear { client.close() }.navigationTitle(worktree.branch)
    }
}
