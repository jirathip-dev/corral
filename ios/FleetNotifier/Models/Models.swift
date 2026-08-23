import Foundation

// MARK: - Read model (mirror of src/core/model.rs, schema v5)

/// Coarse agent lifecycle state. Deliberately small: per-tool nuance lives in
/// `reason` / `waitingOn`, not here.
enum AgentState: String, Codable, CaseIterable, Sendable {
    case idle, working, blocked, done, unknown

    /// Human-facing state text. The board intentionally renders this beside
    /// the color cue so state is never communicated by color alone.
    var displayName: String {
        switch self {
        case .idle: return "Idle"
        case .working: return "Working"
        case .blocked: return "Blocked"
        case .done: return "Done"
        case .unknown: return "Unknown"
        }
    }

    var accessibilityLabel: String { "State: \(displayName)" }
}

/// Why an agent is blocked. "Blocked" is not one UI: an approve-tool prompt,
/// a free-form question, a menu, and a crash each render differently.
enum WaitingOnKind: String, Codable, CaseIterable, Equatable, Sendable {
    case approveTool = "approve_tool"
    case answerQuestion = "answer_question"
    case menu
    case crash
}

enum CiStatus: String, Codable, CaseIterable, Sendable {
    case success, failure, pending, unknown
}

/// The six canonical drive capabilities (D7). UI buttons are rendered from
/// grants + agent capabilities — never hardcoded per tool.
enum Capability: String, Codable, CaseIterable, Sendable {
    case prompt, interrupt, approve, readTail = "read_tail", kill, attach

    var displayName: String {
        switch self {
        case .prompt: return "Prompt"
        case .interrupt: return "Interrupt"
        case .approve: return "Approve"
        case .readTail: return "Tail"
        case .kill: return "Kill"
        case .attach: return "Attach"
        }
    }
}

/// Structured "what is this agent waiting for". `promptHash` lets clients
/// dedupe across polls; `choices` are populated when the prompt exposes a
/// menu. The drive path re-derives the claim from agent_id + prompt_hash and
/// never trusts the stored copy for validation — the client echoes it
/// byte-for-byte from the snapshot anyway (D8).
struct WaitingOn: Codable, Equatable, Sendable {
    var kind: WaitingOnKind
    var prompt: String
    var promptHash: String
    /// Claim identity for the live approval (P3 D8): `"<agent_id>:<prompt_hash>"`.
    var approvalId: String?
    var choices: [String]

    enum CodingKeys: String, CodingKey {
        case kind, prompt
        case promptHash = "prompt_hash"
        case approvalId = "approval_id"
        case choices
    }

    init(kind: WaitingOnKind, prompt: String, promptHash: String, approvalId: String? = nil, choices: [String] = []) {
        self.kind = kind
        self.prompt = prompt
        self.promptHash = promptHash
        self.approvalId = approvalId
        self.choices = choices
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        kind = try c.decode(WaitingOnKind.self, forKey: .kind)
        prompt = try c.decode(String.self, forKey: .prompt)
        promptHash = try c.decode(String.self, forKey: .promptHash)
        approvalId = try c.decodeIfPresent(String.self, forKey: .approvalId)
        choices = try c.decodeIfPresent([String].self, forKey: .choices) ?? []
    }
}

/// Git topology + task-centric read-model fields (P2). Every field defaults
/// so P1-shaped payloads still decode.
struct Workspace: Codable, Equatable, Sendable {
    var repo: String?
    var branch: String?
    var worktreePath: String?
    var prNumber: UInt64?
    var ciStatus: CiStatus?
    var dirty: Bool
    var ahead: UInt64
    var behind: UInt64
    /// Issues the bound PR closes (introduced in schema v4, G23) — this is the wire
    /// location the daemon emits (`src/core/model.rs` puts `issues` on
    /// `Workspace`, not on `Agent`; pinned there by `tests/model.rs`).
    /// Serde-defaulted on the daemon, so absent decodes as empty.
    var issues: [GhIssueRef]

    enum CodingKeys: String, CodingKey {
        case repo, branch
        case worktreePath = "worktree_path"
        case prNumber = "pr_number"
        case ciStatus = "ci_status"
        case dirty, ahead, behind, issues
    }

    init(repo: String? = nil, branch: String? = nil, worktreePath: String? = nil,
         prNumber: UInt64? = nil, ciStatus: CiStatus? = nil, dirty: Bool = false,
         ahead: UInt64 = 0, behind: UInt64 = 0, issues: [GhIssueRef] = []) {
        self.repo = repo
        self.branch = branch
        self.worktreePath = worktreePath
        self.prNumber = prNumber
        self.ciStatus = ciStatus
        self.dirty = dirty
        self.ahead = ahead
        self.behind = behind
        self.issues = issues
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        repo = try c.decodeIfPresent(String.self, forKey: .repo)
        branch = try c.decodeIfPresent(String.self, forKey: .branch)
        worktreePath = try c.decodeIfPresent(String.self, forKey: .worktreePath)
        prNumber = try c.decodeIfPresent(UInt64.self, forKey: .prNumber)
        ciStatus = try c.decodeIfPresent(CiStatus.self, forKey: .ciStatus)
        dirty = try c.decodeIfPresent(Bool.self, forKey: .dirty) ?? false
        ahead = try c.decodeIfPresent(UInt64.self, forKey: .ahead) ?? 0
        behind = try c.decodeIfPresent(UInt64.self, forKey: .behind) ?? 0
        issues = try c.decodeIfPresent([GhIssueRef].self, forKey: .issues) ?? []
    }
}

/// Issue reference joined into the agent model (G23): mirrors corrald's
/// `GhIssueRef` — the bound PR's authoritative `closingIssuesReferences`.
/// Authoritative linkage only; branch-name inference lives in BoardModel
/// and is display-only (D21).
struct GhIssueRef: Codable, Equatable, Sendable {
    var repo: String
    var number: UInt64
    var state: String
    var title: String
}

/// Link back to the source's own identity for this agent (e.g. herdr pane).
struct Attachment: Codable, Equatable, Sendable {
    var kind: String
    var reference: String

    enum CodingKeys: String, CodingKey {
        case kind
        case reference = "ref"
    }
}

/// Canonical agent record. Flat keyed record in snapshot/delta payloads.
struct Agent: Codable, Equatable, Identifiable, Sendable {
    var id: String { agentId }

    /// Opaque, source-stable identity. NOT a pane id.
    var agentId: String
    /// Adapter/source name, e.g. "herdr".
    var source: String
    /// Underlying tool binary, e.g. "claude", "codex", "opencode".
    var tool: String
    var state: AgentState
    /// Free-form human reason for the current state.
    var reason: String?
    /// Per-source monotonic ordering. `ts` is display-only.
    var seq: UInt64
    /// Wall-clock when this record was last changed (epoch millis).
    var ts: UInt64
    var capabilities: [String]
    var waitingOn: WaitingOn?
    /// Topology: reviewer belongs to its implementation agent (P2+).
    var parentId: String?
    /// Host public-key identity (D10).
    var host: String?
    var workspace: Workspace
    var attachment: Attachment?
    var displayName: String?
    var title: String?

    enum CodingKeys: String, CodingKey {
        case agentId = "agent_id"
        case source, tool, state, reason, seq, ts, capabilities
        case waitingOn = "waiting_on"
        case parentId = "parent_id"
        case host, workspace, attachment
        case displayName = "display_name"
        case title
    }

    init(agentId: String, source: String = "herdr", tool: String = "claude", state: AgentState = .unknown,
         reason: String? = nil, seq: UInt64 = 0, ts: UInt64 = 0, capabilities: [String] = [],
         waitingOn: WaitingOn? = nil, parentId: String? = nil,
         host: String? = nil, workspace: Workspace = Workspace(), attachment: Attachment? = nil,
         displayName: String? = nil, title: String? = nil) {
        self.agentId = agentId
        self.source = source
        self.tool = tool
        self.state = state
        self.reason = reason
        self.seq = seq
        self.ts = ts
        self.capabilities = capabilities
        self.waitingOn = waitingOn
        self.parentId = parentId
        self.host = host
        self.workspace = workspace
        self.attachment = attachment
        self.displayName = displayName
        self.title = title
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        agentId = try c.decode(String.self, forKey: .agentId)
        source = try c.decode(String.self, forKey: .source)
        tool = try c.decode(String.self, forKey: .tool)
        state = try c.decode(AgentState.self, forKey: .state)
        reason = try c.decodeIfPresent(String.self, forKey: .reason)
        seq = try c.decodeIfPresent(UInt64.self, forKey: .seq) ?? 0
        ts = try c.decodeIfPresent(UInt64.self, forKey: .ts) ?? 0
        capabilities = try c.decodeIfPresent([String].self, forKey: .capabilities) ?? []
        waitingOn = try c.decodeIfPresent(WaitingOn.self, forKey: .waitingOn)
        parentId = try c.decodeIfPresent(String.self, forKey: .parentId)
        host = try c.decodeIfPresent(String.self, forKey: .host)
        workspace = try c.decodeIfPresent(Workspace.self, forKey: .workspace) ?? Workspace()
        attachment = try c.decodeIfPresent(Attachment.self, forKey: .attachment)
        displayName = try c.decodeIfPresent(String.self, forKey: .displayName)
        title = try c.decodeIfPresent(String.self, forKey: .title)
    }

    var grantedCapabilities: Set<Capability> {
        Set(capabilities.compactMap(Capability.init(rawValue:)))
    }

    var isBlocked: Bool { state == .blocked }

    /// The bound PR's authoritative closing-issue refs (G23), forwarded from
    /// their wire location on `workspace`.
    var issues: [GhIssueRef] { workspace.issues }

    /// The authoritative issue-number set the D21 inference validates
    /// against (mirrors egui's `known_issue_numbers`).
    var knownIssueNumbers: Set<UInt64> {
        Set(workspace.issues.map(\.number))
    }
}

/// Full point-in-time state, served by `GET /snapshot` and by SSE when a
/// client's cursor is too old.
struct Snapshot: Codable, Sendable {
    var schemaVersion: UInt32
    /// Monotonic cursor; a client's `Last-Event-ID` is compared against this.
    var rev: UInt64
    /// Epoch millis when this snapshot was assembled.
    var generatedAt: UInt64
    /// Flat keyed records (NOT JSON Patch; JSON, not CBOR).
    var agents: [String: Agent]

    enum CodingKeys: String, CodingKey {
        case schemaVersion = "schema_version"
        case rev
        case generatedAt = "generated_at"
        case agents
    }
}

/// Incremental change batch, the unit of SSE delivery.
struct Delta: Codable, Equatable, Sendable {
    var rev: UInt64
    /// Full records to upsert.
    var upd: [Agent]
    /// agent_ids to delete.
    var del: [String]
}

/// Response to a drive write: `{request_id, ok, error?, error_kind?, rev, result?}`.
struct DriveResponse: Codable, Equatable, Sendable {
    var requestId: String
    var ok: Bool
    var error: String?
    var errorKind: String?
    var rev: UInt64
    var result: CodableValue?

    enum CodingKeys: String, CodingKey {
        case requestId = "request_id"
        case ok, error
        case errorKind = "error_kind"
        case rev, result
    }
}

/// Typed pre-dispatch refusal body: `{kind, message, request_id?}`.
struct DriveErrorBody: Codable, Equatable, Sendable {
    var kind: String
    var message: String
    var requestId: String?

    enum CodingKeys: String, CodingKey {
        case kind, message
        case requestId = "request_id"
    }
}

/// `POST /register` response: `{key_id, grants, expiry_ts, revoked, algorithm, note}`.
struct RegisterResponse: Codable, Equatable, Sendable {
    var keyId: String
    var grants: [String]
    var expiryTs: UInt64
    var revoked: Bool?
    var algorithm: String?
    var note: String?

    enum CodingKeys: String, CodingKey {
        case keyId = "key_id"
        case grants
        case expiryTs = "expiry_ts"
        case revoked, algorithm, note
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        keyId = try c.decode(String.self, forKey: .keyId)
        grants = try c.decodeIfPresent([String].self, forKey: .grants) ?? []
        expiryTs = try c.decodeIfPresent(UInt64.self, forKey: .expiryTs) ?? 0
        revoked = try c.decodeIfPresent(Bool.self, forKey: .revoked)
        algorithm = try c.decodeIfPresent(String.self, forKey: .algorithm)
        note = try c.decodeIfPresent(String.self, forKey: .note)
    }
}

/// `POST /step-up` response: `{token, key_id, ttl_secs, expires_ts}`.
struct StepUpResponse: Codable, Equatable, Sendable {
    var token: String
    var keyId: String
    var ttlSecs: UInt64
    var expiresTs: UInt64

    enum CodingKeys: String, CodingKey {
        case token
        case keyId = "key_id"
        case ttlSecs = "ttl_secs"
        case expiresTs = "expires_ts"
    }
}

/// `POST /device-token` response (D16): `{ok, key_id, push_registered}`.
struct DeviceTokenResponse: Codable, Equatable, Sendable {
    var ok: Bool
    var keyId: String
    var pushRegistered: Bool

    enum CodingKeys: String, CodingKey {
        case ok
        case keyId = "key_id"
        case pushRegistered = "push_registered"
    }
}

/// `POST /grants-read` response (#101): `{ok, key_id, grants, expiry_ts,
/// revoked}` — the key's CURRENT grants, so a host-side promotion reaches
/// the phone without a device reset.
struct GrantsReadResponse: Codable, Equatable, Sendable {
    var ok: Bool
    var keyId: String
    var grants: [String]
    var expiryTs: UInt64

    enum CodingKeys: String, CodingKey {
        case ok
        case keyId = "key_id"
        case grants
        case expiryTs = "expiry_ts"
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        ok = try c.decodeIfPresent(Bool.self, forKey: .ok) ?? true
        keyId = try c.decode(String.self, forKey: .keyId)
        grants = try c.decodeIfPresent([String].self, forKey: .grants) ?? []
        expiryTs = try c.decodeIfPresent(UInt64.self, forKey: .expiryTs) ?? 0
    }
}

/// JSON value for `DriveResponse.result`. The daemon's read_tail result is
/// intentionally small and shaped as `{"lines": [String]}`; other result
/// fields remain opaque to this client.
enum CodableValue: Codable, Equatable, Sendable {
    case null
    case bool(Bool)
    case int(Int64)
    case uint(UInt64)
    case double(Double)
    case string(String)
    case array([CodableValue])
    case object([String: CodableValue])

    var tailLines: [String]? {
        guard case .object(let object) = self,
              case .array(let values) = object["lines"] else {
            return nil
        }
        let lines = values.compactMap { value -> String? in
            guard case .string(let line) = value else { return nil }
            return line
        }
        return lines.count == values.count ? lines : nil
    }

    init(from decoder: Decoder) throws {
        let container = try decoder.singleValueContainer()
        if container.decodeNil() {
            self = .null
        } else if let b = try? container.decode(Bool.self) {
            self = .bool(b)
        } else if let i = try? container.decode(Int64.self) {
            self = .int(i)
        } else if let u = try? container.decode(UInt64.self) {
            self = .uint(u)
        } else if let d = try? container.decode(Double.self) {
            self = .double(d)
        } else if let s = try? container.decode(String.self) {
            self = .string(s)
        } else if let a = try? container.decode([CodableValue].self) {
            self = .array(a)
        } else if let o = try? container.decode([String: CodableValue].self) {
            self = .object(o)
        } else {
            throw DecodingError.dataCorruptedError(in: container, debugDescription: "unrepresentable JSON value")
        }
    }

    func encode(to encoder: Encoder) throws {
        var c = encoder.singleValueContainer()
        switch self {
        case .null: try c.encodeNil()
        case .bool(let b): try c.encode(b)
        case .int(let i): try c.encode(i)
        case .uint(let u): try c.encode(u)
        case .double(let d): try c.encode(d)
        case .string(let s): try c.encode(s)
        case .array(let a): try c.encode(a)
        case .object(let o): try c.encode(o)
        }
    }
}

/// The claim identity (D8): `"<agent_id>:<prompt_hash>"`.
enum Claim {
    static func approvalId(agentId: String, promptHash: String) -> String {
        "\(agentId):\(promptHash)"
    }
}

// MARK: - Full chat transcript (D35, GET /transcript)

/// Bounded client-side window, mirroring the egui board's transcript pane:
/// at most 1000 entries and about 4 MiB of held text. The walk toward the
/// older end keeps going; NEWEST-loaded entries slide out of a small window
/// rather than dead-ending the reader.
enum TranscriptLimits {
    static let maxEntries = 1000
    static let maxTextBytes = 4 * 1024 * 1024
    static let pageLimit = 50
    static let detailMaxBytes = 64 * 1024
}

/// One daemon transcript entry, already redacted by the server (D-083).
struct TranscriptEntry: Codable, Equatable, Sendable {
    var role: String
    var text: String
    /// Epoch millis when the store carried one; absent renders blank.
    var ts: UInt64?
}

/// Exactly the 200 body of `GET /transcript?agent=<id>`.
struct TranscriptPage: Codable, Equatable, Sendable {
    var agent: String
    var store: String
    var session: String
    var bind: String
    var storesUnavailable: [String]
    var entries: [TranscriptEntry]
    var nextCursor: String?
    var skipped: Int

    enum CodingKeys: String, CodingKey {
        case agent, store, session, bind, entries, skipped
        case storesUnavailable = "stores_unavailable"
        case nextCursor = "next_cursor"
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        agent = try c.decode(String.self, forKey: .agent)
        store = try c.decode(String.self, forKey: .store)
        session = try c.decode(String.self, forKey: .session)
        bind = try c.decode(String.self, forKey: .bind)
        storesUnavailable = try c.decodeIfPresent([String].self, forKey: .storesUnavailable) ?? []
        entries = try c.decodeIfPresent([TranscriptEntry].self, forKey: .entries) ?? []
        nextCursor = try c.decodeIfPresent(String.self, forKey: .nextCursor)
        skipped = try c.decodeIfPresent(Int.self, forKey: .skipped) ?? 0
    }
}

/// One typed `{kind, message, candidates?}` failure from `/transcript`.
struct TranscriptFailure: Equatable, Sendable, Error {
    var kind: String
    var message: String
    var candidates: [String]

    private struct WireBody: Decodable {
        var kind: String?
        var message: String?
        var candidates: [Candidate]?
    }

    private struct Candidate: Decodable {
        var label: String
    }

    static func from(status: Int, data: Data) -> TranscriptFailure {
        if let body = try? JSONDecoder().decode(WireBody.self, from: data) {
            return TranscriptFailure(
                kind: body.kind ?? "transport",
                message: body.message ?? "HTTP \(status)",
                candidates: body.candidates?.map(\.label) ?? []
            )
        }
        return TranscriptFailure(kind: "transport", message: "HTTP \(status)", candidates: [])
    }

    var isStaleCursor: Bool { kind == "bad_cursor" }
    var isNotGranted: Bool { kind == "not_granted" }
}

/// Per-agent paged transcript state, pure and testable like the egui pane.
/// Entries are newest-first; `baseOffset` counts newest-loaded entries that
/// slid out of the bounded window.
struct TranscriptPane: Equatable, Sendable {
    var entries: [TranscriptEntry] = []
    var baseOffset = 0
    var heldBytes = 0
    var nextCursor: String?
    var pages = 0
    var session = ""
    var store = ""
    var bind = ""
    var storesUnavailable: [String] = []
    var skipped = 0
    var loading = false
    var error: TranscriptFailure?
    var autoReloaded = false
    var generation: UInt64 = 0

    var canLoadOlder: Bool {
        !loading && error == nil && nextCursor != nil
    }

    var canRetry: Bool {
        !loading && error.map { !$0.isStaleCursor } == true
    }

    mutating func apply(_ page: TranscriptPage) {
        loading = false
        error = nil
        session = page.session
        store = page.store
        if bind.isEmpty || page.bind == "worktree" {
            bind = page.bind
        }
        for store in page.storesUnavailable where !storesUnavailable.contains(store) {
            storesUnavailable.append(store)
        }
        skipped += page.skipped
        nextCursor = page.nextCursor
        for entry in page.entries {
            heldBytes += entry.text.utf8.count
        }
        entries.append(contentsOf: page.entries)
        pages += 1
        slideWindow()
    }

    mutating func apply(_ failure: TranscriptFailure) {
        loading = false
        error = failure
    }

    /// Fresh, empty pane under a new generation. `keepAutoReloaded` preserves
    /// the one-shot bad-cursor guard; false is an explicit user reload.
    mutating func reset(generation: UInt64, keepAutoReloaded: Bool) {
        let auto = keepAutoReloaded ? autoReloaded : false
        self = TranscriptPane(loading: true, autoReloaded: auto,
                              generation: generation)
    }

    /// Mark a retry/older-page fetch in flight while preserving held state.
    mutating func beginFetch() {
        loading = true
        error = nil
    }

    private mutating func slideWindow() {
        var drop = 0
        var bytes = heldBytes
        while entries.count - drop > 1
                && (entries.count - drop > TranscriptLimits.maxEntries
                    || bytes > TranscriptLimits.maxTextBytes) {
            bytes -= entries[drop].text.utf8.count
            drop += 1
        }
        guard drop > 0 else { return }
        entries.removeFirst(drop)
        heldBytes = bytes
        baseOffset += drop
    }
}

enum TranscriptText {
    static func errorText(_ error: TranscriptFailure) -> String {
        switch error.kind {
        case "not_granted":
            return "requires the read_tail grant — ask the host."
        case "ambiguous_session":
            return error.message
        case "bad_cursor":
            return "session changed while paging — reload from the newest page."
        default:
            return "\(error.kind): \(error.message)"
        }
    }

    /// Bounded display slice for one potentially large entry. The daemon
    /// truncates pages, but the client never trusts that cap with layout.
    static func displaySlice(_ text: String) -> (String, Bool) {
        guard text.utf8.count > TranscriptLimits.detailMaxBytes else {
            return (text, false)
        }
        let bytes = Array(text.utf8)
        var end = TranscriptLimits.detailMaxBytes
        while end > 0 && (bytes[end - 1] & 0xC0) == 0x80 {
            end -= 1
        }
        return (String(decoding: bytes[..<end], as: UTF8.self), true)
    }
}
