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

/// The seven canonical drive capabilities (D7, #267 adds `read_issues`).
/// UI buttons are rendered from grants + agent capabilities — never
/// hardcoded per tool.
enum Capability: String, Codable, CaseIterable, Sendable {
    case prompt, interrupt, approve, readTail = "read_tail",
         readDiff = "read_diff", readIssues = "read_issues", kill, attach

    var displayName: String {
        switch self {
        case .prompt: return "Prompt"
        case .interrupt: return "Interrupt"
        case .approve: return "Approve"
        case .readTail: return "Tail"
        case .readDiff: return "Diff"
        case .readIssues: return "Issues"
        case .kill: return "Kill"
        case .attach: return "Attach"
        }
    }

    /// Plain-language description shown beside each grant toggle on the
    /// Devices & Grants surface (#209) — mirrors the approved mockup.
    var grantDescription: String {
        switch self {
        case .prompt: return "Send prompts / steer the agent"
        case .interrupt: return "Interrupt a running task"
        case .approve: return "Approve tool calls & awaiting decisions"
        case .readTail: return "Read live agent output"
        case .readDiff: return "Read the agent's worktree diff"
        case .readIssues: return "Read repo issues (list + detail)"
        case .kill: return "Terminate a task"
        case .attach: return "Attach to a session & stream events"
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

/// One GitHub issue label (name + GitHub color, no `#`).
struct IssueLabel: Codable, Equatable, Hashable, Sendable {
    var name: String
    var color: String
}

/// One GitHub issue comment (#267): display author, body, ISO-8601
/// `createdAt` verbatim from GitHub.
struct IssueComment: Codable, Equatable, Hashable, Sendable {
    var author: String?
    var body: String
    var createdAt: String?

    enum CodingKeys: String, CodingKey {
        case author, body
        case createdAt = "created_at"
    }
}

/// Issue reference joined into the agent model (G23): mirrors corrald's
/// `GhIssueRef` — the bound PR's authoritative `closingIssuesReferences`.
/// Authoritative linkage only; branch-name inference lives in BoardModel
/// and is display-only (D21).
/// #267: the same wire type is reused for the read-only issue BROWSER
/// payload (the `repos` map of the `read_issues` drive result), so the
/// agent-chip path and the browser rows decode one shape. Every field
/// beyond the original four is `decodeIfPresent`-defaulted: older daemons
/// (no body/comments) and closing-refs (no body/comments by design) decode
/// cleanly.
struct GhIssueRef: Codable, Equatable, Sendable {
    var repo: String
    var number: UInt64
    var state: String
    var title: String
    var labels: [IssueLabel]
    var url: String
    var body: String?
    var commentTotal: UInt64?
    var comments: [IssueComment]

    init(repo: String, number: UInt64, state: String, title: String,
         labels: [IssueLabel] = [], url: String = "", body: String? = nil,
         commentTotal: UInt64? = nil, comments: [IssueComment] = []) {
        self.repo = repo
        self.number = number
        self.state = state
        self.title = title
        self.labels = labels
        self.url = url
        self.body = body
        self.commentTotal = commentTotal
        self.comments = comments
    }

    enum CodingKeys: String, CodingKey {
        case repo, number, state, title, labels, url, body, comments
        case commentTotal = "comment_total"
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        repo = try c.decode(String.self, forKey: .repo)
        number = try c.decode(UInt64.self, forKey: .number)
        state = try c.decode(String.self, forKey: .state)
        title = try c.decode(String.self, forKey: .title)
        labels = try c.decodeIfPresent([IssueLabel].self, forKey: .labels) ?? []
        url = try c.decodeIfPresent(String.self, forKey: .url) ?? ""
        body = try c.decodeIfPresent(String.self, forKey: .body)
        commentTotal = try c.decodeIfPresent(UInt64.self, forKey: .commentTotal)
        comments = try c.decodeIfPresent([IssueComment].self, forKey: .comments) ?? []
    }
}

/// #267: the read-only issue browser payload (`{"repos": {<fleet>: [issue]}}`,
/// served by the grant-gated `/drive read_issues` arm and the board's
/// `GET /issues`). Repo keys are the fleet/issue-group names the daemon
/// groups under; empty arrays are informational placeholders.
struct IssuesBrowserWire: Codable, Equatable, Sendable {
    var repos: [String: [GhIssueRef]]

    /// All issues across repos, newest-number first (the daemon sorts each
    /// repo by number; the browser renders one flat list per approved V3).
    var all: [GhIssueRef] {
        repos.values.flatMap { $0 }.sorted { l, r in
            l.number > r.number
        }
    }
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
    /// #210: per-fleet health strip (orch alive, live worker count,
    /// presence-heartbeat anchor, warnings). Absent from older daemons —
    /// decodes to empty. NEVER carries spend/balance state.
    var fleetHealth: [FleetHealthEntry]

    enum CodingKeys: String, CodingKey {
        case schemaVersion = "schema_version"
        case rev
        case generatedAt = "generated_at"
        case agents
        case fleetHealth = "fleet_health"
    }

    init(
        schemaVersion: UInt32,
        rev: UInt64,
        generatedAt: UInt64,
        agents: [String: Agent],
        fleetHealth: [FleetHealthEntry] = []
    ) {
        self.schemaVersion = schemaVersion
        self.rev = rev
        self.generatedAt = generatedAt
        self.agents = agents
        self.fleetHealth = fleetHealth
    }

    init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        schemaVersion = try container.decode(UInt32.self, forKey: .schemaVersion)
        rev = try container.decode(UInt64.self, forKey: .rev)
        generatedAt = try container.decode(UInt64.self, forKey: .generatedAt)
        agents = try container.decode([String: Agent].self, forKey: .agents)
        fleetHealth = try container.decodeIfPresent([FleetHealthEntry].self, forKey: .fleetHealth) ?? []
    }
}

/// #210: one fleet's health row (HEALTH ONLY — no spend/balance).
struct FleetHealthEntry: Codable, Equatable, Sendable {
    var name: String
    var ghRepo: String
    var paused: Bool
    var orch: String
    var orchAlive: Bool
    var orchState: String?
    var workers: Int
    /// Epoch-millis anchor of the orch's presence heartbeat; clients render
    /// `now - lastHeartbeat` so the age ticks between snapshots.
    var lastHeartbeat: UInt64?
    var degraded: Bool
    var warnings: [String]

    enum CodingKeys: String, CodingKey {
        case name
        case ghRepo = "gh_repo"
        case paused
        case orch
        case orchAlive = "orch_alive"
        case orchState = "orch_state"
        case workers
        case lastHeartbeat = "last_heartbeat"
        case degraded
        case warnings
    }
}

extension FleetHealthEntry {
    /// Tolerant decode: the daemon omits empty `warnings` (serde
    /// skip_serializing_if) — decode to `[]` instead of failing the whole
    /// snapshot. Living in an extension keeps the memberwise initializer
    /// for the demo seed + tests.
    init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        name = try container.decode(String.self, forKey: .name)
        ghRepo = try container.decode(String.self, forKey: .ghRepo)
        paused = try container.decode(Bool.self, forKey: .paused)
        orch = try container.decode(String.self, forKey: .orch)
        orchAlive = try container.decode(Bool.self, forKey: .orchAlive)
        orchState = try container.decodeIfPresent(String.self, forKey: .orchState)
        workers = try container.decode(Int.self, forKey: .workers)
        lastHeartbeat = try container.decodeIfPresent(UInt64.self, forKey: .lastHeartbeat)
        degraded = try container.decode(Bool.self, forKey: .degraded)
        warnings = try container.decodeIfPresent([String].self, forKey: .warnings) ?? []
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

/// One registered device projected by the host-admin `GET /grants` read
/// surface (#209). Public keys and push tokens stay host-side; `name` is
/// the optional cosmetic label the device supplied at registration.
struct AdminGrantDevice: Codable, Equatable, Identifiable, Sendable {
    var id: String { keyId }
    var keyId: String
    var name: String?
    var grants: [String]
    var revoked: Bool
    var expiryTs: UInt64
    var createdTs: UInt64

    enum CodingKeys: String, CodingKey {
        case keyId = "key_id"
        case name, grants, revoked
        case expiryTs = "expiry_ts"
        case createdTs = "created_ts"
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        keyId = try c.decode(String.self, forKey: .keyId)
        name = try c.decodeIfPresent(String.self, forKey: .name)
        grants = try c.decodeIfPresent([String].self, forKey: .grants) ?? []
        revoked = try c.decodeIfPresent(Bool.self, forKey: .revoked) ?? false
        expiryTs = try c.decodeIfPresent(UInt64.self, forKey: .expiryTs) ?? 0
        createdTs = try c.decodeIfPresent(UInt64.self, forKey: .createdTs) ?? 0
    }
}

/// The host-admin `GET /grants` envelope (#209).
struct AdminGrantsView: Codable, Equatable, Sendable {
    var ok: Bool
    var devices: [AdminGrantDevice]

    enum CodingKeys: String, CodingKey {
        case ok, devices
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        ok = try c.decodeIfPresent(Bool.self, forKey: .ok) ?? true
        devices = try c.decodeIfPresent([AdminGrantDevice].self, forKey: .devices) ?? []
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

    var tailSourceRev: UInt64? {
        guard case .object(let object) = self,
              case .uint(let rev) = object["source_rev"] else { return nil }
        return rev
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

    /// #232: the daemon's bounded ReadDiffResult — one paged page
    /// (diffstat + changed-files list + unified diff lines).
    var diffPage: DiffPageWire? {
        guard case .object = self else { return nil }
        guard let data = try? JSONEncoder().encode(self) else { return nil }
        return try? JSONDecoder().decode(DiffPageWire.self, from: data)
    }

    /// #267: the daemon's read-only issue browser payload (`{"repos": …}`).
    var issuesBrowser: IssuesBrowserWire? {
        guard case .object = self else { return nil }
        guard let data = try? JSONEncoder().encode(self) else { return nil }
        return try? JSONDecoder().decode(IssuesBrowserWire.self, from: data)
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

/// One daemon block (D7). `kind` is the client's only vocabulary;
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

/// The four block kinds (D7).
enum TranscriptBlockKind: String, Codable, Equatable, Sendable {
    case user, agent, tool, system
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

/// Per-agent worktree-diff state (#232): one bounded page at a time,
/// accumulated lazily so a large diff never materializes fully in memory.
/// The sheet presents exactly what the daemon served (paged, redacted,
/// clamped) — this client never re-bounds.
struct DiffPane: Equatable, Sendable {
    var repo: String?
    var branch: String?
    var head: String?
    var stats: DiffStatsWire = DiffStatsWire(files: 0, adds: 0, dels: 0)
    var files: [DiffFileWire] = []
    var filesTruncated = false
    var lines: [String] = []
    var total = 0
    var hasMore = false
    var nextOffset: Int?
    var isLoading = false
    var error: String?

    var isEmpty: Bool { lines.isEmpty && files.isEmpty && !isLoading && error == nil }

    mutating func beginFetch() {
        isLoading = true
        error = nil
    }

    /// Fold one daemon page. Offset 0 seeds the pane; later pages append at
    /// the page's aggregate offset (a worktree change renumbers offsets, so
    /// a gap reseeds from the new page instead of interleaving stale lines).
    mutating func apply(_ page: DiffPageWire) {
        isLoading = false
        error = nil
        repo = page.repo ?? repo
        branch = page.branch ?? branch
        head = page.head ?? head
        stats = page.stats
        files = page.files
        filesTruncated = page.filesTruncated
        total = page.total
        hasMore = page.hasMore
        nextOffset = page.nextOffset
        if page.offset == 0 && lines.isEmpty {
            lines = page.lines
        } else if page.offset <= lines.count {
            // Page window may overlap already-known lines (a re-fetch of the
            // same window is idempotent) — append only the not-yet-known
            // suffix beyond the accumulated count.
            let overlap = lines.count - page.offset
            if overlap < page.lines.count {
                lines.append(contentsOf: page.lines[overlap...])
            }
        } else {
            lines = page.lines
        }
    }

    mutating func apply(_ failure: String) {
        isLoading = false
        error = failure
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

/// #267: the fleet-level read-only issue browser pane — the last fetched
/// `IssuesBrowserWire` with the four-state machine (idle/loading/loaded/
/// error), mirroring `DiffPane`. The daemon serves the bounded gh poller
/// window; the VIEW owns the lazy comment reveal (client-side within the
/// fetched newest-first window).
struct IssuesBrowserPane: Equatable, Sendable {
    var repos: [String: [GhIssueRef]] = [:]
    var isLoading = false
    var error: String?
    var updatedAt: Date?

    var isEmpty: Bool {
        repos.values.allSatisfy(\.isEmpty) && !isLoading && error == nil
    }

    mutating func beginFetch() {
        isLoading = true
        error = nil
    }

    mutating func apply(_ wire: IssuesBrowserWire) {
        isLoading = false
        error = nil
        repos = wire.repos
        updatedAt = Date()
    }

    mutating func apply(_ failure: String) {
        isLoading = false
        error = failure
        updatedAt = nil
    }

    mutating func reset() {
        self = IssuesBrowserPane()
    }
}

// MARK: - #232 read_diff wire shapes (mirror corrald::drive::ReadDiffResult)

/// Whole-diff diffstat (all tracked changes vs HEAD).
struct DiffStatsWire: Codable, Equatable, Sendable {
    var files: Int
    var adds: Int
    var dels: Int
}

struct DiffFileWire: Codable, Equatable, Sendable {
    var path: String
    var adds: Int
    var dels: Int
}

/// One bounded read_diff page (daemon-clamped; the client never re-bounds).
struct DiffPageWire: Codable, Equatable, Sendable {
    var repo: String?
    var branch: String?
    var head: String?
    var stats: DiffStatsWire
    var files: [DiffFileWire]
    var filesTruncated: Bool
    var offset: Int
    var lines: [String]
    var total: Int
    var hasMore: Bool
    var nextOffset: Int?

    enum CodingKeys: String, CodingKey {
        case repo, branch, head, stats, files, offset, lines, total
        case filesTruncated = "files_truncated"
        case hasMore = "has_more"
        case nextOffset = "next_offset"
    }
}
