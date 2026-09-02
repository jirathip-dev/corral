import Foundation

// MARK: - Read model (mirror of src/core/model.rs, schema v5)

/// Coarse agent lifecycle state. Deliberately small: per-tool nuance lives in
/// `reason`, not here.
///
/// #354 L2: the board shows herdr's RAW state tokens verbatim
/// (working / idle / blocked / unknown — herdr 0.8.2 has NO done; finished
/// Hermes panes fall back to idle). `done` is retained only as a wire-decode
/// case (the daemon's enum still carries it); a live board never shows it.
enum AgentState: String, Codable, CaseIterable, Sendable {
    case idle, working, blocked, done, unknown

    /// Raw herdr state token, verbatim (spec amendment 09-02: no
    /// Corral-invented wording — no Needs-you / Supervising / Finished).
    var displayName: String {
        rawValue
    }

    var accessibilityLabel: String { "State: \(displayName)" }
}

enum CiStatus: String, Codable, CaseIterable, Sendable {
    case success, failure, pending, unknown
}

/// The retained read capabilities (D7, closed set after the #354 cut). The
/// daemon's register response may grant only `read_tail` / `read_diff`;
/// unknown grant strings simply decode to nothing (rawValue init returns
/// nil and `compactMap` drops them), so an older daemon ledger stays safe.
enum Capability: String, Codable, CaseIterable, Sendable {
    case readTail = "read_tail"
    case readDiff = "read_diff"
}

/// Git topology + task-centric read-model fields (P2). Every field defaults
/// so P1-shaped payloads still decode. The `issues` join (G23) was removed
/// with the Issues browser cut (#354 L2) — unknown wire keys are ignored by
/// Codable, so snapshots from a transitional daemon still decode.
struct Workspace: Codable, Equatable, Sendable {
    var repo: String?
    var branch: String?
    var worktreePath: String?
    var prNumber: UInt64?
    var ciStatus: CiStatus?
    var dirty: Bool
    var ahead: UInt64
    var behind: UInt64

    enum CodingKeys: String, CodingKey {
        case repo, branch
        case worktreePath = "worktree_path"
        case prNumber = "pr_number"
        case ciStatus = "ci_status"
        case dirty, ahead, behind
    }

    init(repo: String? = nil, branch: String? = nil, worktreePath: String? = nil,
         prNumber: UInt64? = nil, ciStatus: CiStatus? = nil, dirty: Bool = false,
         ahead: UInt64 = 0, behind: UInt64 = 0) {
        self.repo = repo
        self.branch = branch
        self.worktreePath = worktreePath
        self.prNumber = prNumber
        self.ciStatus = ciStatus
        self.dirty = dirty
        self.ahead = ahead
        self.behind = behind
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
    }
}

/// Link back to the source's own identity for this agent (e.g. herdr pane).
/// The small pane reference is the board row's debug aid.
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
        case parentId = "parent_id"
        case host, workspace, attachment
        case displayName = "display_name"
        case title
    }

    init(agentId: String, source: String = "herdr", tool: String = "claude", state: AgentState = .unknown,
         reason: String? = nil, seq: UInt64 = 0, ts: UInt64 = 0, capabilities: [String] = [],
         parentId: String? = nil,
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

    init(
        schemaVersion: UInt32,
        rev: UInt64,
        generatedAt: UInt64,
        agents: [String: Agent],
    ) {
        self.schemaVersion = schemaVersion
        self.rev = rev
        self.generatedAt = generatedAt
        self.agents = agents
    }

    init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        schemaVersion = try container.decode(UInt32.self, forKey: .schemaVersion)
        rev = try container.decode(UInt64.self, forKey: .rev)
        generatedAt = try container.decode(UInt64.self, forKey: .generatedAt)
        agents = try container.decode([String: Agent].self, forKey: .agents)
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

/// Response to a drive read: `{request_id, ok, error?, error_kind?, rev, result?}`.
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
    /// Cosmetic device label the daemon stored for this key (#209).
    var name: String?

    enum CodingKeys: String, CodingKey {
        case keyId = "key_id"
        case grants
        case expiryTs = "expiry_ts"
        case revoked, algorithm, note, name
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        keyId = try c.decode(String.self, forKey: .keyId)
        grants = try c.decodeIfPresent([String].self, forKey: .grants) ?? []
        expiryTs = try c.decodeIfPresent(UInt64.self, forKey: .expiryTs) ?? 0
        revoked = try c.decodeIfPresent(Bool.self, forKey: .revoked)
        algorithm = try c.decodeIfPresent(String.self, forKey: .algorithm)
        note = try c.decodeIfPresent(String.self, forKey: .note)
        name = try c.decodeIfPresent(String.self, forKey: .name)
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
/// intentionally small and shaped as `{"lines": [String]}` plus the #167
/// `blocks` array; other result fields remain opaque to this client.
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

    var tailSourceRev: UInt64? {
        guard case .object(let object) = self else { return nil }
        switch object["source_rev"] {
        case .uint(let rev): return rev
        // Small integers decode as Int64 (the single-value decoder tries
        // Int64 before UInt64); the daemon's revisions are well below 2^63.
        case .int(let rev) where rev >= 0: return UInt64(rev)
        default: return nil
        }
    }

    /// #167: the `blocks` array the daemon now serves ADDITIVELY alongside
    /// `lines` on a read_tail result.
    var tailBlocks: [TranscriptBlock]? {
        guard case .object(let object) = self,
              case .array(let values) = object["blocks"] else {
            return nil
        }
        guard let data = try? JSONEncoder().encode(CodableValue.array(values)) else {
            return nil
        }
        return try? JSONDecoder().decode([TranscriptBlock].self, from: data)
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

/// One daemon block (D7 + #315). `kind` is the client's only vocabulary;
/// `truncatedBefore` is the count lifted from a `... +N lines` marker that
/// preceded this block (absent = no marker).
struct TranscriptBlock: Codable, Equatable, Sendable {
    var kind: TranscriptBlockKind
    var text: String
    /// Epoch millis at block boundary (absent = not labelled).
    var at: UInt64?
    var truncatedBefore: UInt32?

    enum CodingKeys: String, CodingKey {
        case kind, text, at
        case truncatedBefore = "truncated_before"
    }

    init(kind: TranscriptBlockKind, text: String, at: UInt64? = nil,
         truncatedBefore: UInt32? = nil) {
        self.kind = kind
        self.text = text
        self.at = at
        self.truncatedBefore = truncatedBefore
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        kind = try c.decode(TranscriptBlockKind.self, forKey: .kind)
        text = try c.decode(String.self, forKey: .text)
        at = try c.decodeIfPresent(UInt64.self, forKey: .at)
        truncatedBefore = try c.decodeIfPresent(UInt32.self, forKey: .truncatedBefore)
    }
}

/// The block kinds (D7 + #315). `unknown` is terminal content the daemon
/// could not provenance — preserved, never falsely attributed.
enum TranscriptBlockKind: String, Codable, Equatable, Sendable {
    case user, agent, tool, system, unknown
}

/// One typed drive-refusal result used by the Recent-output tail state.
struct TranscriptFailure: Equatable, Sendable, Error {
    var kind: String
    var message: String
    var candidates: [String]
}

/// Per-agent live tail state (#167). The daemon serves `read_tail` ADDITIVELY
/// as `{lines, blocks}`; this pane keeps the blocks (the block renderer) and
/// the bounded lines (the legacy text surface), plus the four-state machine
/// (loading / empty / error / loaded) and a hard-timeout marker.
struct TailPane: Equatable, Sendable {
    var sourceRev: UInt64? = nil
    var blocks: [TranscriptBlock] = []
    var lines: [String] = []
    var loading = false
    var error: TranscriptFailure?
    var updatedAt: Date?
    var generation: UInt64 = 0

    var isEmpty: Bool { blocks.isEmpty && lines.isEmpty }

    mutating func beginFetch() {
        loading = true
        error = nil
    }

    mutating func apply(_ pageBlocks: [TranscriptBlock], lines: [String]) {
        loading = false
        error = nil
        updatedAt = Date()
        blocks = pageBlocks
        self.lines = lines
    }

    mutating func apply(_ failure: TranscriptFailure) {
        loading = false
        error = failure
    }

    mutating func reset() {
        self = TailPane()
    }
}

enum TranscriptText {
    static func errorText(_ error: TranscriptFailure) -> String {
        switch error.kind {
        case "not_granted":
            return "requires the read_tail grant — ask the host."
        default:
            return "\(error.kind): \(error.message)"
        }
    }
}
