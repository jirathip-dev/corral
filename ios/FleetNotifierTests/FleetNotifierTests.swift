import CryptoKit
import Combine
import XCTest
@testable import FleetNotifier

// MARK: - Canonical bytes (byte-for-byte serde_json parity, #354 L2 read surface)

final class CanonicalBytesTests: XCTestCase {

    /// The retained read envelope: `rev` rides after the payload; the Rust
    /// side pins the identical literal (src/drive/mod.rs).
    func testReadTailEnvelopeBytesMatchRustShape() {
        let bytes = CanonicalJSON.envelopeBytes(
            requestId: "req-1", capability: "read_tail", target: "herdr:abc",
            payload: CanonicalJSON.readTailPayload(lines: 200), rev: 7)
        XCTAssertEqual(String(data: bytes, encoding: .utf8),
                       #"{"request_id":"req-1","capability":"read_tail","target":"herdr:abc","payload":{"kind":"read_tail","lines":200},"rev":7}"#)
    }

    /// `rev` is omitted when nil (`skip_serializing_if = "Option::is_none"`).
    func testRevOmittedWhenNil() {
        let bytes = CanonicalJSON.envelopeBytes(
            requestId: "r", capability: "read_tail", target: "t",
            payload: CanonicalJSON.readTailPayload(lines: nil), rev: nil)
        XCTAssertEqual(String(data: bytes, encoding: .utf8),
                       #"{"request_id":"r","capability":"read_tail","target":"t","payload":{"kind":"read_tail","lines":null}}"#)
    }

    /// `read_tail.lines` has no skip attr in Rust — None serializes as null.
    func testReadTailLinesNullWhenAbsent() {
        let withLines = CanonicalJSON.readTailPayload(lines: 200)
        XCTAssertEqual(CanonicalJSON.encode(withLines), #"{"kind":"read_tail","lines":200}"#)
    }

    /// Recents v1 is bounded by the daemon's 200-line cap; the client never
    /// requests more.
    func testTailControlIsBoundedTo200Lines() {
        XCTAssertEqual(CanonicalJSON.encode(CanonicalJSON.readTailPayload(lines: 200)),
                       #"{"kind":"read_tail","lines":200}"#)
    }

    /// serde_json string escaping: `"` `\` and control chars; \u00xx for
    /// the rest of the control plane, lowercase hex; non-ASCII raw.
    func testStringEscapingMatchesSerdeJSON() {
        let text = "say \"hi\" \\ path\tnewline\nctl\u{1b}…λ"
        let escaped = CanonicalJSON.escaped(text)
        XCTAssertEqual(escaped, #""say \"hi\" \\ path\tnewline\nctl\u001b…λ""#)
        // Round trip through the decoder-side unescaper (quote-stripped body).
        XCTAssertEqual(CanonicalJSON.unescaped(String(escaped.dropFirst().dropLast())), text)
    }

    /// The signed drive body embeds the envelope byte-identical.
    func testSignedDriveBodyEmbedsEnvelope() {
        let envelope = CanonicalJSON.envelopeBytes(requestId: "req-1", capability: "read_tail",
                                                   target: "herdr:abc",
                                                   payload: CanonicalJSON.readTailPayload(lines: nil), rev: nil)
        let body = CanonicalJSON.signedDriveBody(keyId: "k", signatureB64: "c2ln", envelopeBytes: envelope)
        XCTAssertEqual(String(data: body, encoding: .utf8),
                       #"{"key_id":"k","signature":"c2ln","envelope":{"request_id":"req-1","capability":"read_tail","target":"herdr:abc","payload":{"kind":"read_tail","lines":null}}}"#)
    }

    func testRegisterBodyShape() {
        let body = CanonicalJSON.registerBody(token: "tok", publicKeyB64: "a2V5")
        XCTAssertEqual(String(data: body, encoding: .utf8),
                       #"{"token":"tok","public_key":"a2V5"}"#)
    }

    /// `canonical_device_token_bytes` — fixed order key_id, device_token,
    /// ts (mirror of the Rust DeviceTokenRequest; the Rust test pins the
    /// exact literal).
    func testDeviceTokenCanonicalBytes() {
        let bytes = CanonicalJSON.deviceTokenBytes(keyId: "dev-1", deviceToken: "a1b2c3", ts: 1_700_000_000)
        XCTAssertEqual(String(data: bytes, encoding: .utf8),
                       #"{"key_id":"dev-1","device_token":"a1b2c3","ts":1700000000}"#)
    }

    /// `canonical_grants_read_bytes` — fixed order key_id, request, ts
    /// (mirror of the Rust GrantsReadRequest).
    func testGrantsReadBodyCanonicalShape() {
        let bytes = CanonicalJSON.grantsReadBytes(keyId: "dev_abc", request: "grants-read", ts: 1_700_000_000)
        XCTAssertEqual(String(data: bytes, encoding: .utf8),
                       #"{"key_id":"dev_abc","request":"grants-read","ts":1700000000}"#)
        let body = CanonicalJSON.grantsReadBody(keyId: "dev_abc", signatureB64: "c2ln", requestBytes: bytes)
        XCTAssertEqual(String(data: body, encoding: .utf8),
                       #"{"key_id":"dev_abc","signature":"c2ln","request":{"key_id":"dev_abc","request":"grants-read","ts":1700000000}}"#)
    }

    /// A read_tail drive result decodes into visible lines + #167 blocks.
    func testDriveResponseTailResultDecodes() throws {
        let data = Data(#"{"request_id":"r","ok":true,"rev":4,"result":{"lines":["one","two"],"source_rev":3,"blocks":[{"kind":"agent","text":"one"}]}}"#.utf8)
        let response = try JSONDecoder().decode(DriveResponse.self, from: data)
        XCTAssertEqual(response.result?.tailLines, ["one", "two"])
        XCTAssertEqual(response.result?.tailSourceRev, 3)
        XCTAssertEqual(response.result?.tailBlocks?.first?.kind, .agent)
    }
}

// MARK: - Signature (CryptoKit Ed25519)

final class SigningTests: XCTestCase {
    func testSignVerifyRoundTrip() throws {
        let key = Curve25519.Signing.PrivateKey()
        let signer = DeviceSigner(key: key)
        let bytes = Data("hello".utf8)
        let signature = try signer.sign(bytes)
        XCTAssertEqual(signature.count, 64)
        XCTAssertTrue(key.publicKey.isValidSignature(signature, for: bytes))
        XCTAssertFalse(key.publicKey.isValidSignature(signature, for: Data("hellp".utf8)))
    }

    /// Key material is raw 32 bytes; the public key b64 is the registration
    /// wire form the daemon parses as 32 bytes.
    func testKeySizes() throws {
        let key = Curve25519.Signing.PrivateKey()
        XCTAssertEqual(key.rawRepresentation.count, 32)
        XCTAssertEqual(key.publicKey.rawRepresentation.count, 32)
        let signer = DeviceSigner(key: key)
        XCTAssertEqual(Data(base64Encoded: signer.publicKeyB64)?.count, 32)
    }
}

// MARK: - Read-only default + typed error decoding (#354 L2)

final class ReadOnlyTests: XCTestCase {
    func testRegisterDecodesEmptyGrants() throws {
        let json = """
        {"key_id":"dev-1","grants":[],"expiry_ts":1800000000,"revoked":false,
         "algorithm":"Ed25519","note":"default grants are empty (read-only)"}
        """
        let response = try JSONDecoder().decode(RegisterResponse.self, from: Data(json.utf8))
        XCTAssertEqual(response.keyId, "dev-1")
        XCTAssertTrue(response.grants.isEmpty, "read-only default")
        XCTAssertEqual(response.expiryTs, 1_800_000_000)
    }

    func testNotGrantedRefusalDecodes() throws {
        let json = #"{"kind":"not_granted","message":"capability not granted: read_tail","request_id":"r-1"}"#
        let body = try JSONDecoder().decode(DriveErrorBody.self, from: Data(json.utf8))
        XCTAssertEqual(body.kind, "not_granted")
        XCTAssertEqual(body.requestId, "r-1")
    }

    /// After the cut the capability set is the closed read set; legacy
    /// mutating grant strings never decode into a Capability.
    func testGrantedCapabilitiesAreReadOnly() {
        let agent = Agent(agentId: "a", capabilities: ["read_tail", "read_diff", "approve", "kill"])
        XCTAssertTrue(agent.grantedCapabilities.contains(.readTail))
        XCTAssertTrue(agent.grantedCapabilities.contains(.readDiff))
        XCTAssertEqual(agent.grantedCapabilities.count, 2,
                       "mutating capabilities must not decode after the cut")
    }

    /// Unknown wire keys (e.g. a transitional daemon still emitting
    /// `waiting_on` / `issues`) never break the read model decode.
    func testLegacyWireKeysAreIgnoredByTheReadModel() throws {
        let json = """
        {"agent_id":"herdr:a","source":"herdr","tool":"claude","state":"blocked",
         "reason":"waiting","seq":1,"ts":1,"capabilities":["approve","read_tail"],
         "waiting_on":{"kind":"menu","prompt":"go?","prompt_hash":"sha256:ab","approval_id":"a","choices":["y"]},
         "workspace":{"branch":"main","issues":[{"repo":"corral","number":9,"state":"open","title":"t"}]}}
        """
        let agent = try JSONDecoder().decode(Agent.self, from: Data(json.utf8))
        XCTAssertEqual(agent.state, .blocked)
        XCTAssertEqual(agent.workspace.branch, "main")
    }
}

// MARK: - #354 L2 state-change notification payloads

final class PushPayloadTests: XCTestCase {

    private func agent(_ id: String, state: AgentState, repo: String, branch: String,
                       displayName: String) -> Agent {
        Agent(agentId: id, state: state, seq: 1, ts: 1,
              workspace: Workspace(repo: repo, branch: branch),
              displayName: displayName)
    }

    /// Content contract: title "agent · repo", body "state · branch".
    func testTransitionContentUsesAgentRepoAndStateBranch() {
        let working = PushPayload.transition(type: .started,
                                             agent: agent("herdr:a", state: .working, repo: "demo-garden", branch: "demo-catalog", displayName: "builder"))
        XCTAssertEqual(working.type, .started)
        XCTAssertEqual(working.title, "builder · demo-garden")
        XCTAssertEqual(working.body, "working · demo-catalog")

        let blocked = PushPayload.transition(type: .blocked,
                                             agent: agent("herdr:a", state: .blocked, repo: "demo-garden", branch: "demo-catalog", displayName: "builder"))
        XCTAssertEqual(blocked.body, "blocked · demo-catalog")

        let finished = PushPayload.transition(type: .finished,
                                              agent: agent("herdr:a", state: .idle, repo: "demo-garden", branch: "demo-catalog", displayName: "builder"))
        XCTAssertEqual(finished.body, "idle · demo-catalog")
    }

    /// Missing repo/branch degrade to honest placeholders, never crashes.
    func testTransitionContentHandlesMissingRepoAndBranch() {
        let agent = Agent(agentId: "herdr:a", state: .working, seq: 1, ts: 1)
        let payload = PushPayload.transition(type: .started, agent: agent)
        XCTAssertEqual(payload.title, "herdr:a · no repo")
        XCTAssertEqual(payload.body, "working · no branch")
    }

    func testParsesStartedBlockedFinishedPayloads() {
        for type in [PushPayload.PushType.started, .blocked, .finished] {
            let userInfo: [AnyHashable: Any] = [
                "aps": ["alert": ["title": "builder · demo-garden", "body": "working · demo-catalog"]],
                "type": type.rawValue,
                "agent_id": "herdr:ses-1",
                "ts": 1700000000,
            ]
            let payload = try? XCTUnwrap(PushPayload.parse(userInfo: userInfo))
            XCTAssertEqual(payload?.type, type)
            XCTAssertEqual(payload?.agentId, "herdr:ses-1")
            XCTAssertEqual(payload?.title, "builder · demo-garden")
        }
    }

    /// The DEBUG local bridge embeds asUserInfo; parse must round-trip the
    /// payload (one handler for both paths).
    func testLocalBridgeUserInfoRoundTrips() {
        let payload = PushPayload.transition(
            type: .blocked,
            agent: agent("herdr:ses-2", state: .blocked, repo: "demo-ledger", branch: "demo-migration", displayName: "ledger"))
        let parsed = PushPayload.parse(userInfo: payload.asUserInfo())
        XCTAssertEqual(parsed, payload)
    }

    func testRejectsGarbageAndForeignPayloads() {
        XCTAssertNil(PushPayload.parse(userInfo: ["agent_id": "x"]))
        XCTAssertNil(PushPayload.parse(userInfo: ["type": "alien", "agent_id": "x"]))
        XCTAssertNil(PushPayload.parse(userInfo: [:]))
    }

    // MARK: - #397 composite (host_id, agent_id) target

    /// SAFETY: 32-byte fixtures are valid X25519 public-key byte strings.
    private static let hostA = Data(repeating: 0x11, count: 32).base64EncodedString()
    private static let hostB = Data(repeating: 0x22, count: 32).base64EncodedString()

    func testParsesAndRetainsHostIdFromApnsPayloads() {
        let userInfo: [AnyHashable: Any] = [
            "aps": ["alert": ["title": "builder · demo-garden", "body": "blocked · main"]],
            "type": "blocked",
            "host_id": Self.hostA,
            "agent_id": "herdr:ses-1",
            "ts": 1700000000,
        ]
        let payload = try? XCTUnwrap(PushPayload.parse(userInfo: userInfo))
        XCTAssertEqual(payload?.hostId, Self.hostA,
                       "the payload's host identity must be parsed and retained")
        XCTAssertEqual(payload?.agentId, "herdr:ses-1")
        // Host-less legacy payloads still parse (hostId nil).
        let legacy = try? XCTUnwrap(PushPayload.parse(userInfo: [
            "type": "blocked", "agent_id": "herdr:x",
        ]))
        XCTAssertNil(legacy?.hostId, "legacy payloads carry no host identity")
    }

    func testLocalBridgeRoundTripsHostId() {
        let payload = PushPayload.transition(
            type: .blocked,
            agent: agent("herdr:ses-2", state: .blocked, repo: "demo-ledger",
                         branch: "demo-migration", displayName: "ledger"),
            hostId: Self.hostA)
        let parsed = PushPayload.parse(userInfo: payload.asUserInfo())
        XCTAssertEqual(parsed, payload, "the DEBUG bridge round-trips the host identity")
        XCTAssertEqual(parsed?.hostId, Self.hostA)
    }

    func testEqualRawAgentIdsOnTwoHostsProduceDistinctCompositeIdentifiers() {
        // #397: identifiers + thread ids are namespaced by the composite
        // target — the same raw agent id from two hosts never collides.
        let a = PushPayload(type: .started, agentId: "herdr:dup", hostId: Self.hostA,
                            ts: 1, title: "t", body: "b")
        let b = PushPayload(type: .started, agentId: "herdr:dup", hostId: Self.hostB,
                            ts: 1, title: "t", body: "b")
        XCTAssertNotEqual(a.requestIdentifier, b.requestIdentifier,
                          "two hosts with an equal raw agent id must schedule distinct requests")
        XCTAssertNotEqual(a.threadIdentifier, b.threadIdentifier,
                          "thread identifiers must not merge two hosts' lanes")
        XCTAssertEqual(a.requestIdentifier,
                       "started-\(Self.hostA)-herdr:dup")
        XCTAssertEqual(a.threadIdentifier, "\(Self.hostA)::herdr:dup")
        // Legacy host-less payloads keep the pre-#397 identifier shape.
        let legacy = PushPayload(type: .finished, agentId: "herdr:dup",
                                 ts: 1, title: "t", body: "b")
        XCTAssertEqual(legacy.requestIdentifier, "finished-herdr:dup")
        XCTAssertEqual(legacy.threadIdentifier, "herdr:dup")
    }
}

// MARK: - Episode transition hooks (#354 L2 notifications)

@MainActor
final class EpisodeTransitionTests: XCTestCase {
    private func agent(_ id: String, state: AgentState) -> Agent {
        Agent(agentId: id, state: state, seq: 1, ts: 1)
    }

    private func applySnapshot(_ store: FleetStore, _ state: AgentState, id: String = "a", rev: UInt64 = 1) {
        store.apply(.snapshot(Snapshot(schemaVersion: 3, rev: rev, generatedAt: 1,
                                       agents: [id: agent(id, state: state)])))
    }

    func testStartedBlockedFinishedFireOnDeltaTransitions() {
        let store = FleetStore()
        var started: [String] = []
        var blocked: [String] = []
        var finished: [String] = []
        store.onStarted = { started.append($0) }
        store.onBlocked = { blocked.append($0) }
        store.onFinished = { finished.append($0) }

        // Snapshot seeds the shadows: NO fires (cold-start rule).
        applySnapshot(store, .idle)
        XCTAssertEqual(started, [])
        XCTAssertEqual(blocked, [])
        XCTAssertEqual(finished, [])

        // idle -> working: started.
        store.apply(.delta(Delta(rev: 2, upd: [agent("a", state: .working)], del: [])))
        XCTAssertEqual(started, ["a"])
        // working -> blocked: blocked (mid-episode).
        store.apply(.delta(Delta(rev: 3, upd: [agent("a", state: .blocked)], del: [])))
        XCTAssertEqual(blocked, ["a"])
        // blocked -> working: a resume, NOT a new start.
        store.apply(.delta(Delta(rev: 4, upd: [agent("a", state: .working)], del: [])))
        XCTAssertEqual(started, ["a"], "blocked→working is a resume, not a start")
        // working -> idle: finished fires once.
        store.apply(.delta(Delta(rev: 5, upd: [agent("a", state: .idle)], del: [])))
        XCTAssertEqual(finished, ["a"])
        // Staying idle: no re-fire.
        store.apply(.delta(Delta(rev: 6, upd: [agent("a", state: .idle)], del: [])))
        XCTAssertEqual(finished, ["a"], "episode end fires once until the agent starts again")
        // A new episode: idle -> working -> idle fires finished again.
        store.apply(.delta(Delta(rev: 7, upd: [agent("a", state: .working)], del: [])))
        XCTAssertEqual(started, ["a", "a"])
        store.apply(.delta(Delta(rev: 8, upd: [agent("a", state: .idle)], del: [])))
        XCTAssertEqual(finished, ["a", "a"])
    }

    func testBlockedDedupesWhileStayingBlocked() {
        let store = FleetStore()
        var blocked: [String] = []
        store.onBlocked = { blocked.append($0) }
        applySnapshot(store, .working)
        store.apply(.delta(Delta(rev: 2, upd: [agent("a", state: .blocked)], del: [])))
        store.apply(.delta(Delta(rev: 3, upd: [agent("a", state: .blocked)], del: [])))
        XCTAssertEqual(blocked, ["a"])
    }

    func testFirstSightOfAnActiveAgentFires() {
        let store = FleetStore()
        var started: [String] = []
        var blocked: [String] = []
        store.onStarted = { started.append($0) }
        store.onBlocked = { blocked.append($0) }
        // A fresh delta with NO prior snapshot: a working agent = started,
        // a blocked agent = blocked.
        store.apply(.delta(Delta(rev: 1, upd: [agent("x", state: .working)], del: [])))
        store.apply(.delta(Delta(rev: 2, upd: [agent("y", state: .blocked)], del: [])))
        XCTAssertEqual(started, ["x"])
        XCTAssertEqual(blocked, ["y"])
    }

    func testBlockedToIdleEndsTheEpisode() {
        let store = FleetStore()
        var finished: [String] = []
        store.onFinished = { finished.append($0) }
        applySnapshot(store, .blocked)
        store.apply(.delta(Delta(rev: 2, upd: [agent("a", state: .idle)], del: [])))
        XCTAssertEqual(finished, ["a"])
    }

    func testWireDoneIsTreatedAsEpisodeEnd() {
        let store = FleetStore()
        var finished: [String] = []
        store.onFinished = { finished.append($0) }
        applySnapshot(store, .working)
        store.apply(.delta(Delta(rev: 2, upd: [agent("a", state: .done)], del: [])))
        XCTAssertEqual(finished, ["a"], "transitional daemon done == episode end")
        store.apply(.delta(Delta(rev: 3, upd: [agent("a", state: .done)], del: [])))
        XCTAssertEqual(finished, ["a"], "staying done must not re-fire")
    }

    func testDeletionDropsShadows() {
        let store = FleetStore()
        var started: [String] = []
        store.onStarted = { started.append($0) }
        applySnapshot(store, .idle)
        store.apply(.delta(Delta(rev: 2, upd: [], del: ["a"])))
        // A later re-appearing agent is a NEW episode.
        store.apply(.delta(Delta(rev: 3, upd: [agent("a", state: .working)], del: [])))
        XCTAssertEqual(started, ["a"])
    }
}

// MARK: - Delta application + state-entered tracking

@MainActor
final class DeltaApplyTests: XCTestCase {
    private func agent(_ id: String, state: AgentState, ts: UInt64 = 1) -> Agent {
        Agent(agentId: id, state: state, seq: 1, ts: ts, capabilities: ["read_tail"])
    }

    func testSnapshotReplacesAgents() {
        let store = FleetStore()
        let snapshot = Snapshot(schemaVersion: 3, rev: 10, generatedAt: 1,
                                agents: ["a": agent("a", state: .working)])
        store.apply(.snapshot(snapshot))
        XCTAssertEqual(store.agents.count, 1)
        XCTAssertEqual(store.lastEventId, 10)
    }

    func testDeltaUpsertsAndDeletes() {
        let store = FleetStore()
        store.apply(.snapshot(Snapshot(schemaVersion: 3, rev: 1, generatedAt: 1,
                                       agents: ["a": agent("a", state: .working)])))
        store.apply(.delta(Delta(rev: 2, upd: [agent("a", state: .idle), agent("b", state: .working)], del: [])))
        XCTAssertEqual(store.agents.count, 2)
        XCTAssertEqual(store.agents["a"]?.state, .idle)
        store.apply(.delta(Delta(rev: 3, upd: [], del: ["a"])))
        XCTAssertNil(store.agents["a"])
        XCTAssertEqual(store.lastEventId, 3)
    }

    func testStaleRecoverySnapshotAndDeltaCannotOverwriteNewerSSE() {
        let store = FleetStore()
        var newer = agent("a", state: .working)
        newer.title = "newer SSE"
        store.apply(.snapshot(Snapshot(schemaVersion: 3, rev: 10, generatedAt: 1,
                                       agents: ["a": agent("a", state: .idle)])))
        store.apply(.delta(Delta(rev: 11, upd: [newer], del: [])))

        var stale = agent("a", state: .idle)
        stale.title = "stale fetch"
        store.apply(.snapshot(Snapshot(schemaVersion: 3, rev: 10, generatedAt: 1,
                                       agents: ["a": stale])))
        store.apply(.delta(Delta(rev: 10, upd: [agent("late", state: .working)], del: [])))

        XCTAssertEqual(store.lastEventId, 11)
        XCTAssertEqual(store.agents["a"]?.title, "newer SSE")
        XCTAssertNil(store.agents["late"])
    }

    func testReadTailResultIsStoredBoundedAndRemovedWithAgent() {
        let store = FleetStore()
        store.apply(.snapshot(Snapshot(schemaVersion: 3, rev: 1, generatedAt: 1,
                                       agents: ["a": agent("a", state: .working)])))
        store.rememberTail(Array(repeating: "tail", count: 250), for: "a")
        XCTAssertEqual(store.tail(for: "a")?.count, 200)

        store.apply(.delta(Delta(rev: 2, upd: [], del: ["a"])))
        XCTAssertNil(store.tail(for: "a"))
    }

    // MARK: - #166 review F2: state-entered tracking

    func testStateEnteredAtSeedsFromTsAtFirstSight() {
        let store = FleetStore()
        let a = Agent(agentId: "a", state: .working, ts: 1000)
        store.apply(.snapshot(Snapshot(schemaVersion: 3, rev: 1, generatedAt: 1,
                                       agents: ["a": a])))
        XCTAssertEqual(store.stateEnteredAt["a"], 1000)
    }

    func testStateEnteredAtDoesNotAdvanceOnReasonOrTitleChurn() {
        let store = FleetStore()
        store.apply(.snapshot(Snapshot(schemaVersion: 3, rev: 1, generatedAt: 1,
                                       agents: ["a": Agent(agentId: "a", state: .working, ts: 1000)])))
        var churned = Agent(agentId: "a", state: .working, ts: 5000)
        churned.reason = "running tests"
        churned.title = "same task"
        store.apply(.delta(Delta(rev: 2, upd: [churned], del: [])))
        XCTAssertEqual(store.stateEnteredAt["a"], 1000,
                       "reason/title churn must NOT reset the duration")
    }

    func testStateEnteredAtAdvancesOnStateChange() {
        let store = FleetStore()
        store.apply(.snapshot(Snapshot(schemaVersion: 3, rev: 1, generatedAt: 1,
                                       agents: ["a": Agent(agentId: "a", state: .working, ts: 1000)])))
        store.apply(.delta(Delta(rev: 2, upd: [Agent(agentId: "a", state: .blocked, ts: 3000)], del: [])))
        XCTAssertEqual(store.stateEnteredAt["a"], 3000,
                       "a real state change re-stamps the clock")
    }

    func testStateEnteredAtPrunesDeletedAgents() {
        let store = FleetStore()
        store.apply(.snapshot(Snapshot(schemaVersion: 3, rev: 1, generatedAt: 1,
                                       agents: ["a": Agent(agentId: "a", state: .working, ts: 1000)])))
        store.apply(.delta(Delta(rev: 2, upd: [], del: ["a"])))
        XCTAssertNil(store.stateEnteredAt["a"])
    }
}

// MARK: - Demo seed integrity (#354 L2 read-only fixture)

final class DemoSeedTests: XCTestCase {
    func testSeedCoversRawBoardStatesIncludingDone() {
        let seed = DemoFleet.seed()
        let states = Set(seed.values.map(\.state))
        XCTAssertEqual(states, Set([.working, .idle, .blocked, .done, .unknown]),
                       "the read-only fixture uses herdr's raw vocabulary — a "
                       + "done row proves the Done section renders when present")
        XCTAssertTrue(seed.values.contains { $0.isBlocked })
        XCTAssertTrue(seed.values.contains { $0.state == .idle })
        XCTAssertTrue(seed.values.contains { $0.state == .done })
        XCTAssertTrue(seed.values.contains { $0.state == .unknown })
    }

    func testSeedHasNoActionSurfacesOrClaims() {
        let seed = DemoFleet.seed()
        for agent in seed.values {
            XCTAssertFalse(agent.capabilities.contains("approve"))
            XCTAssertFalse(agent.capabilities.contains("interrupt"))
            XCTAssertFalse(agent.capabilities.contains("kill"))
            XCTAssertFalse(agent.capabilities.contains("attach"))
            XCTAssertTrue(agent.capabilities.contains("read_tail") || agent.capabilities.contains("read_diff"))
        }
    }

    func testSeedUsesOnlyFictionalRepositoriesAndReferences() {
        let forbidden = ["jirathip", "github.com", "/Users/", "~/.herdr", "sendmeter", "plush-meadow", "synergy-costing", "herdr-board", "project-hearthwild"]
        let seed = DemoFleet.seed()
        XCTAssertGreaterThanOrEqual(Set(seed.values.compactMap(\.workspace.repo)).count, 3)
        for agent in seed.values {
            let values = [agent.agentId, agent.displayName ?? "", agent.title ?? "", agent.workspace.repo ?? "", agent.workspace.branch ?? "", agent.workspace.worktreePath ?? "", agent.attachment?.reference ?? ""]
            XCTAssertTrue(values.allSatisfy { value in forbidden.allSatisfy { !value.localizedCaseInsensitiveContains($0) } })
        }
    }

    func testSeedCoversEveryStatusSection() {
        let seed = DemoFleet.seed()
        // The evidence board must be able to show EVERY locked status
        // section with rows — incl. Done (only-when-present is a BoardModel
        // behavior; the demo carries one done row so the section renders).
        let sections = BoardModel.sections(Array(seed.values))
        XCTAssertEqual(sections.statuses.map(\.state),
                       [.blocked, .working, .idle, .done, .unknown],
                       "the fixture must populate every locked status section")
        for status in sections.statuses {
            XCTAssertFalse(status.subgroups.isEmpty,
                           "section \(status.header) must be non-empty for evidence")
        }
        // The orphan (repo = nil) row stays in the fixture and lands in its
        // status bucket's Other subgroup (#371).
        XCTAssertTrue(seed.values.contains { $0.workspace.repo == nil },
                      "the orphan (no-repo) row must be exercised")
    }

    func testSeedPrivacyGateRejectsForbiddenThrowawayValue() {
        let forbidden = ["jirathip", "github.com", "/Users/", "~/.herdr"]
        func isForbidden(_ value: String) -> Bool {
            forbidden.contains { value.localizedCaseInsensitiveContains($0) }
        }
        XCTAssertTrue(isForbidden("https://github.com/jirathip-dev/private"))
        XCTAssertFalse(isForbidden("https://demo.example.invalid/atlas-board/issues/9007"))
    }

    /// Recents #373 fixture: the featured agent's tail exercises the
    /// block-per-run evidence shape — it opens with quiet Status material,
    /// alternates roles (so every canonical block is a distinct display
    /// block), carries one >20-line tool run (cap + Show all) and a diff
    /// run (+ / - / @@ for the ANSI-remap proof), and ends on a call-only
    /// tool run (the muted waiting line). Content stays fully fictional.
    func testFeaturedRecentsFixtureExercisesBlockPerRunTreatments() throws {
        let seed = DemoFleet.seed()
        guard let agent = seed[DemoFleet.featuredAgentID] else {
            return XCTFail("featured demo agent missing from the seed")
        }
        let blocks = DemoFleet.recentBlocks(for: agent)
        XCTAssertFalse(blocks.isEmpty)
        XCTAssertFalse(DemoFleet.recentLines(from: blocks).isEmpty)
        XCTAssertEqual(blocks.first?.kind, .system,
                       "the fixture opens with quiet Status material")
        XCTAssertEqual(blocks.last?.kind, .tool,
                       "the fixture ends on a tool run for the waiting line")
        // Role-run shape: adjacent blocks always change role, so the
        // block-per-run evidence shows one display block per canonical
        // block (nothing silently merges the fixture's runs together).
        let kinds = blocks.map(\.kind)
        let distinctRuns = kinds.reduce(into: [TranscriptBlockKind]()) { partial, kind in
            if partial.last != kind { partial.append(kind) }
        }
        XCTAssertEqual(kinds, distinctRuns,
                       "the fixture must alternate roles so display blocks stay distinct")
        XCTAssertEqual(kinds, [.system, .user, .agent, .tool, .agent,
                               .tool, .agent, .tool, .agent, .tool],
                       "the fixture must cover Status / You / Assistant / Tool runs")
        // One tool run exceeds the 20-line cap (Show-all evidence).
        let long = try XCTUnwrap(blocks.first { $0.kind == .tool &&
            $0.text.components(separatedBy: .newlines).count > RecentOutputModel.lineCap })
        XCTAssertTrue(long.text.hasPrefix("$ pnpm"), "the giant run is the vitest block")
        // A diff run carries the +/-/@@ syntax marks the theme remaps.
        let diff = try XCTUnwrap(blocks.first { $0.kind == .tool && $0.text.contains("git diff") })
        XCTAssertTrue(diff.text.contains("@@"))
        XCTAssertTrue(diff.text.contains("\n-"))
        XCTAssertTrue(diff.text.contains("\n+"))
        // The FINAL tool run has one invocation and no output yet.
        let last = blocks[blocks.count - 1]
        XCTAssertTrue(last.text.hasPrefix("$ rg -n withRetry src/"))
        XCTAssertEqual(last.text.components(separatedBy: .newlines).count, 1)
        XCTAssertLessThanOrEqual(DemoFleet.recentLines(from: blocks).count, 200,
                                 "the demo tail must respect the daemon's 200-line cap")
    }
}

final class SSETests: XCTestCase {
    func testParsesSnapshotFrame() {
        var parser = SSEParser()
        let frames = parser.feed("event: snapshot\nid: 42\ndata: {\"rev\":42}\n\n")
        XCTAssertEqual(frames.count, 1)
        XCTAssertEqual(frames[0].kind, .snapshot)
        XCTAssertEqual(frames[0].id, 42)
        XCTAssertEqual(frames[0].data, #"{"rev":42}"#)
    }

    func testParsesCRLFAndMultiLineData() {
        var parser = SSEParser()
        let frames = parser.feed("event: delta\r\nid: 43\r\ndata: line1\r\ndata: line2\r\n\r\n")
        XCTAssertEqual(frames.count, 1)
        XCTAssertEqual(frames[0].kind, .delta)
        XCTAssertEqual(frames[0].data, "line1\nline2")
    }

    func testIgnoresCommentsAndKeepAlives() {
        var parser = SSEParser()
        let frames = parser.feed(": keep-alive\nevent: delta\nid: 44\ndata: {}\n\n")
        XCTAssertEqual(frames.count, 1)
        XCTAssertEqual(frames[0].id, 44)
    }

    func testChunkBoundariesAcrossFeedCalls() {
        var parser = SSEParser()
        let raw = "event: snapshot\nid: 9\ndata: {\"rev\":9}\n\n"
        let half = String(raw.prefix(raw.count / 2))
        XCTAssertEqual(parser.feed(half).count, 0)
        let rest = String(raw.dropFirst(raw.count / 2))
        let frames = parser.feed(rest)
        XCTAssertEqual(frames.count, 1)
        XCTAssertEqual(frames[0].kind, .snapshot)
        XCTAssertEqual(frames[0].id, 9)
    }

    func testFinishFlushesTrailingFrame() {
        var parser = SSEParser()
        let frames = parser.feed("event: delta\nid: 5\ndata: {}\n")
        XCTAssertEqual(frames.count, 0)
        let flushed = parser.finish()
        XCTAssertEqual(flushed.count, 1)
        XCTAssertEqual(flushed[0].kind, .delta)
        XCTAssertEqual(flushed[0].id, 5)
    }

    func testDecodesSnapshotAndDelta() throws {
        let snapshotJSON = """
        {"schema_version":5,"rev":12,"generated_at":1700000000000,"agents":{
          "herdr:a":{"agent_id":"herdr:a","source":"herdr","tool":"claude","state":"blocked",
          "reason":"waiting","seq":1,"ts":1700000000000,"capabilities":["approve"],
          "waiting_on":{"kind":"menu","prompt":"go?","prompt_hash":"sha256:ab","approval_id":"herdr:a:sha256:ab","choices":["y","n"]},
          "workspace":{"branch":"main"}}}}
        """
        let frame = SSEFrame(kind: .snapshot, id: 12, data: snapshotJSON)
        guard case .event(.snapshot(let snapshot)) = CorraldClient.decode(frame) else {
            return XCTFail("expected snapshot")
        }
        XCTAssertEqual(snapshot.schemaVersion, 5)
        XCTAssertEqual(snapshot.rev, 12)
        let agent = snapshot.agents["herdr:a"]
        XCTAssertEqual(agent?.state, .blocked)
        XCTAssertEqual(agent?.workspace.branch, "main")
        // #354 L2: legacy claim keys (`waiting_on`) are tolerated wire keys —
        // the read model ignores them (asserted in ReadOnlyTests).
    }

    /// #79 defect 2 + review F1: every way a data-bearing frame fails
    /// must be a REPORTED failure — torn payloads name the decoder
    /// error, an UNRECOGNIZED event name is reported AS protocol drift
    /// (keep-alives are comment lines and never frame, so a .message
    /// frame with data is always drift). Only the defensively-
    /// unreachable empty-data case stays silent.
    func testDecodeSurfacesFailuresIncludingUnknownEventNames() {
        let torn = SSEFrame(kind: .snapshot, id: 1, data: "{\"rev\": ")
        guard case .failed(let reason) = CorraldClient.decode(torn) else {
            return XCTFail("torn snapshot must be .failed")
        }
        XCTAssertTrue(reason.contains("snapshot"), reason)

        let badDelta = SSEFrame(kind: .delta, id: 2, data: "{\"nope\":true}")
        guard case .failed = CorraldClient.decode(badDelta) else {
            return XCTFail("undecodable delta must be .failed")
        }

        // F1: the drift case — a future `event: patch` must be VISIBLE.
        let drift = SSEFrame(kind: .message, id: 3, data: "{\"rev\":9}", eventName: "patch")
        guard case .failed(let driftReason) = CorraldClient.decode(drift) else {
            return XCTFail("data under an unknown event name must be .failed")
        }
        XCTAssertTrue(driftReason.contains("patch"), driftReason)

        let empty = SSEFrame(kind: .message, id: 4, data: "")
        guard case .ignored = CorraldClient.decode(empty) else {
            return XCTFail("only the empty-data case is silently ignored")
        }
    }

    /// #79 review F5: the failure path is exercised THROUGH the stream
    /// entry point (ingest), not by calling the note method directly —
    /// deleting the .failed arm now fails this test. A subsequent good
    /// frame recovers to .connected.
    @MainActor
    func testTornFrameThroughIngestSurfacesErrorThenRecovers() async {
        let store = FleetStore()
        // Deterministic (round-3 R-N5): await the returned hop — no
        // sleeps, no timing race.
        await store.ingest(SSEFrame(kind: .delta, id: 1, data: "{\"nope\":")).value
        guard case .error(let message) = store.connectionState else {
            return XCTFail("torn frame via ingest must surface .error, got \(store.connectionState)")
        }
        XCTAssertTrue(message.contains("undecodable"), message)

        let good = "{\"schema_version\":5,\"rev\":9,\"generated_at\":0,\"agents\":{}}"
        await store.ingest(SSEFrame(kind: .snapshot, id: 9, data: good)).value
        XCTAssertEqual(store.connectionState, .connected,
                       "a good frame recovers the connection state")
    }

    /// #79 review F2: a surfaced decode failure reaches the owner's
    /// banner hook (routed to the copyable BannerView), alongside the
    /// .error state.
    @MainActor
    func testDecodeFailureReachesTheBannerHook() {
        let store = FleetStore()
        var routed: String?
        store.onDecodeFailure = { routed = $0 }
        store.noteDecodeFailure("delta frame undecodable: test")
        XCTAssertEqual(routed, "delta frame undecodable: test")
        guard case .error = store.connectionState else {
            return XCTFail("state must also flip to .error")
        }
    }
}

// MARK: - Delta application + block detection

@MainActor







final class KeychainStorageTests: XCTestCase {
    func testLoadOrCreateProducesSigner() throws {
        let (signer, storage) = try DeviceKeyStore.loadOrCreate()
        XCTAssertEqual(signer.publicKeyB64.count > 40, true)
        print("FN-DIAG storage=\(storage) pub=\(signer.publicKeyB64)")
    }

    /// Diagnostic only: unsigned/ad-hoc simulator builds can hit -34018
    /// (errSecMissingEntitlement) from the simulator keychain daemon; the
    /// documented in-app fallback then engages and the UI shows a warning.
    /// On signed device builds (TestFlight) Keychain works. The assertion
    /// is deliberately weak: the contract is "a signer always exists".
    func testKeychainStatus() {
        let query: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: "com.corral.fleetnotifier.keys",
            kSecAttrAccount as String: "device-ed25519",
            kSecReturnData as String: true,
        ]
        var item: CFTypeRef?
        let status = SecItemCopyMatching(query as CFDictionary, &item)
        print("FN-DIAG SecItemCopyMatching status=\(status)")
        let add: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: "com.corral.fleetnotifier.keys",
            kSecAttrAccount as String: "diag-probe",
            kSecValueData as String: Data("probe".utf8),
        ]
        let addStatus = SecItemAdd(add as CFDictionary, nil)
        print("FN-DIAG SecItemAdd probe status=\(addStatus)")
        if addStatus == errSecSuccess {
            SecItemDelete(add as CFDictionary)
        }
    }
}

// MARK: - Branch-name issue inference (D21, ported from egui infer.rs)






final class WorkspaceLineTests: XCTestCase {

    func testBasenameSuppressedWhenItRestatesTheBranch() {
        // herdr flattens branch `/` to `-` for worktree dirs — identical or
        // prefix-related names carry no information and force truncation.
        let flattened = Workspace(branch: "g57/board-d24-d25",
                                  worktreePath: "~/worktrees/corral/g57-board-d24-d25")
        XCTAssertNil(WorkspaceLine.worktreeBasename(flattened))

        let identical = Workspace(branch: "main", worktreePath: "/repo/main")
        XCTAssertNil(WorkspaceLine.worktreeBasename(identical))

        let truncatedDir = Workspace(branch: "fix-migration-late-arrival",
                                     worktreePath: "~/worktrees/synergy-costing/fix-migration")
        XCTAssertNil(WorkspaceLine.worktreeBasename(truncatedDir),
                     "dir that is a prefix of the branch is redundant")
    }

    /// R2-C: a basename that EXTENDS the branch adds tokens and must be
    /// kept — the old rule over-suppressed two worktrees of one branch.
    func testBasenameThatExtendsTheBranchSurvives() {
        let extending = Workspace(branch: "g57/board-d24-d25",
                                  worktreePath: "~/worktrees/corral/g57-board-d24-d25-extra")
        XCTAssertEqual(WorkspaceLine.worktreeBasename(extending), "g57-board-d24-d25-extra",
                       "basename extends the flattened branch → kept (R2-C)")

        let branchWithSuffix = Workspace(branch: "issue-431-x",
                                         worktreePath: "~/worktrees/corral/issue-431-x-shared")
        XCTAssertEqual(WorkspaceLine.worktreeBasename(branchWithSuffix), "issue-431-x-shared",
                       "suffix tokens identify which of several worktrees")
    }

    func testDistinctBasenameSurvives() {
        let distinct = Workspace(branch: "native/611-sparkline",
                                 worktreePath: "~/worktrees/sendmeter/review-611")
        XCTAssertEqual(WorkspaceLine.worktreeBasename(distinct), "review-611")

        let noBranch = Workspace(branch: nil, worktreePath: "/w/dir-name")
        XCTAssertEqual(WorkspaceLine.worktreeBasename(noBranch), "dir-name")

        XCTAssertNil(WorkspaceLine.worktreeBasename(Workspace()))
    }

    /// G100: a long worktree basename (e.g.
    /// `g64-egui-transcript-panel-lazy-paged-virtualized`) must render with
    /// a visible head AND tail — never collapse to a bare `…` stub. The
    /// basename gets the branch's middle-truncation treatment AND the top
    /// priority tier (shared only with the never-truncating badges), so it
    /// is the last segment to compress.
    ///
    /// RED against current main: the basename segment had NO truncation
    /// mode (SwiftUI's default tail → head-only, and at extreme compression
    /// a bare `…`) and priority 1 — below repo/badges' 2 — so it was the
    /// first segment after the branch to collapse. The suite has no
    /// view-rendering harness, so the fix is pinned as policy shape: the
    /// segment middle-truncates and outranks every other segment.
    func testWorkspaceLineLongBasenameKeepsHeadAndTail() {
        let basename = "g64-egui-transcript-panel-lazy-paged-virtualized"
        let long = Workspace(branch: "g64/egui-transcript",
                             worktreePath: "~/worktrees/corral/g64-egui-transcript-panel-lazy-paged-virtualized")
        XCTAssertEqual(WorkspaceLine.worktreeBasename(long), basename,
                       "a basename that EXTENDS the branch renders as a segment at all (D26 R2-C)")
        XCTAssertEqual(WorkspaceLine.SegmentPolicy.truncationMode(for: .basename), .middle,
                       "\(basename) must middle-truncate so head AND tail stay visible")
        XCTAssertEqual(WorkspaceLine.SegmentPolicy.truncationMode(for: .branch), .middle,
                       "the branch keeps its existing middle truncation")
        XCTAssertGreaterThan(WorkspaceLine.SegmentPolicy.priority(for: .basename),
                             WorkspaceLine.SegmentPolicy.priority(for: .branch),
                             "the basename must outlive the branch on compression")
        XCTAssertGreaterThan(WorkspaceLine.SegmentPolicy.priority(for: .basename),
                             WorkspaceLine.SegmentPolicy.priority(for: .repo),
                             "the basename must outlive the repo on compression")
        XCTAssertEqual(WorkspaceLine.SegmentPolicy.priority(for: .basename),
                       WorkspaceLine.SegmentPolicy.priority(for: .badge),
                       "the basename survives as long as the never-truncating badges")
        XCTAssertGreaterThan(WorkspaceLine.SegmentPolicy.priority(for: .badge),
                             WorkspaceLine.SegmentPolicy.priority(for: .repo),
                             "badges keep the never-truncate tier; repo compresses first")
    }

    /// G100 no-regression guard: a short basename renders byte-identical to
    /// today. The basename string still flows to the segment unchanged (the
    /// D26 suppression rule is untouched), middle truncation is applied by
    /// SwiftUI only when the text overflows (a no-op when it fits), and the
    /// priority reorder only changes WHO compresses first — never what fits
    /// when everything fits.
    func testWorkspaceLineShortBasenameUnchanged() {
        let short = Workspace(branch: "g100/workspace-squish",
                              worktreePath: "~/worktrees/corral/review-611")
        XCTAssertEqual(WorkspaceLine.worktreeBasename(short), "review-611",
                       "the short basename string must reach the segment unchanged")
        XCTAssertEqual(WorkspaceLine.SegmentPolicy.truncationMode(for: .basename), .middle,
                       "native middle truncation is a no-op when the text fits")
        XCTAssertEqual(WorkspaceLine.SegmentPolicy.priority(for: .basename),
                       WorkspaceLine.SegmentPolicy.priority(for: .badge),
                       "priorities engage only on compression, so full-width rows render as before")
    }
}

// MARK: - Schema v4 issues decode (G23, daemon wire shape)

final class SSEStreamMockURLProtocol: URLProtocol {
    static var fixture: Data?
    static var served = false
    static var finishAfterServe = false
    /// Lock-guarded request counter: `startLoading()` runs on the
    /// URLProtocol delegate queue while the tests poll from the main
    /// thread — the same NSLock pattern as `StreamFrameBox`/`CursorBox`
    /// (review N1: a bare static var raced under TSan).
    private static let requestLock = NSLock()
    private static var requestCountStorage = 0

    static var requestCount: Int {
        requestLock.lock()
        defer { requestLock.unlock() }
        return requestCountStorage
    }

    static func resetRequestCount() {
        requestLock.lock()
        defer { requestLock.unlock() }
        requestCountStorage = 0
    }

    private static func incrementRequestCount() {
        requestLock.lock()
        defer { requestLock.unlock() }
        requestCountStorage += 1
    }

    override class func canInit(with request: URLRequest) -> Bool { true }
    override class func canonicalRequest(for request: URLRequest) -> URLRequest { request }

    override func startLoading() {
        Self.incrementRequestCount()
        guard let fixture = Self.fixture, let url = request.url else {
            client?.urlProtocol(self, didFailWithError: URLError(.badURL))
            return
        }
        let response = HTTPURLResponse(
            url: url, statusCode: 200, httpVersion: "HTTP/1.1",
            headerFields: ["Content-Type": "text/event-stream"])!
        client?.urlProtocol(self, didReceive: response, cacheStoragePolicy: .notAllowed)
        if !Self.served {
            Self.served = true
            let split = fixture.count / 2
            client?.urlProtocol(self, didLoad: Data(fixture.prefix(split)))
            client?.urlProtocol(self, didLoad: Data(fixture.suffix(fixture.count - split)))
        }
        if Self.finishAfterServe {
            client?.urlProtocolDidFinishLoading(self)
        }
        // Otherwise deliberately never finishes: the daemon's stream stays open.
    }

    override func stopLoading() {}
}

/// Accumulates frames off the URLSession loading path (not the main
/// actor) — lock-guarded, like the app's `CursorBox`.
private final class StreamFrameBox: @unchecked Sendable {
    private let lock = NSLock()
    private var storage: [SSEFrame] = []

    var frames: [SSEFrame] {
        lock.lock()
        defer { lock.unlock() }
        return storage
    }

    func append(_ frame: SSEFrame) {
        lock.lock()
        defer { lock.unlock() }
        storage.append(frame)
    }
}

/// Regression for #90. `AsyncLineSequence` (`bytes.lines`) strips line
/// terminators AND drops the empty line that closes an SSE frame, so the
/// parser never completes a frame and the live board spins forever. This
/// test drives `stream()` — the real `URLSession` byte path, mocked via
/// `URLProtocol` — and asserts frames COMPLETE. A test that feeds
/// hand-built strings straight into `parser.feed()` is vacuous against
/// this defect and does not count.
final class SSEStreamRegressionTests: XCTestCase {

    /// The EXACT shape the daemon emits (issue #90 proof C): `\n`-
    /// separated, frames closed by an empty line, keep-alive comment
    /// lines (`:`) between frames. Swift drops the newline before the
    /// closing delimiter, so the blank line that closes the delta frame
    /// is appended explicitly — the daemon's raw bytes end `...}\n\n`.
    private var fixtureData: Data {
        let payload = """
        event: snapshot
        id: 7
        data: {"schema_version":5,"rev":7,"generated_at":0,"agents":{"herdr:a":{"agent_id":"herdr:a","source":"herdr","tool":"claude","state":"idle","seq":1,"ts":1700000000000}}}

        :
        event: delta
        id: 8
        data: {"rev":8,"upd":[],"del":[]}

        """ + "\n"
        return Data(payload.utf8)
    }

    func testStreamCompletesFramesOverRealURLSessionBytes() async throws {
        SSEStreamMockURLProtocol.fixture = fixtureData
        SSEStreamMockURLProtocol.served = false
        SSEStreamMockURLProtocol.finishAfterServe = false
        SSEStreamMockURLProtocol.resetRequestCount()
        defer { SSEStreamMockURLProtocol.fixture = nil }

        let config = URLSessionConfiguration.ephemeral
        config.protocolClasses = [SSEStreamMockURLProtocol.self]
        let session = URLSession(configuration: config)
        let client = CorraldClient(host: URL(string: "https://sse.test")!, session: session)

        let box = StreamFrameBox()
        let stream = Task {
            await client.stream(lastEventId: { nil }, onEvent: { box.append($0) })
        }

        // Wait for both frames (snapshot + delta) under a hard deadline.
        // Against the bytes.lines regression neither ever completes.
        let deadline = Date().addingTimeInterval(5)
        while box.frames.count < 2, Date() < deadline {
            try? await Task.sleep(nanoseconds: 25_000_000)
        }

        let frames = box.frames
        // Tear down the stream and its URLSession before asserting so the
        // mock's in-flight work cannot outlive the test (review F5).
        stream.cancel()
        await stream.value
        session.invalidateAndCancel()

        XCTAssertEqual(
            frames.count, 2,
            "expected the snapshot + delta frames to COMPLETE over the real byte path — got \(frames.count). "
                + "bytes.lines drops the empty-line terminator, so the parser never closes a frame.")
        guard frames.count == 2 else { return }

        let snapshot = frames[0]
        XCTAssertEqual(snapshot.kind, .snapshot)
        XCTAssertEqual(snapshot.id, 7)
        guard case .event(.snapshot(let decoded)) = CorraldClient.decode(snapshot) else {
            return XCTFail("snapshot frame must decode")
        }
        XCTAssertEqual(decoded.rev, 7)
        XCTAssertEqual(decoded.agents["herdr:a"]?.state, .idle)

        let delta = frames[1]
        XCTAssertEqual(delta.kind, .delta)
        XCTAssertEqual(delta.id, 8)
        guard case .event(.delta(let decodedDelta)) = CorraldClient.decode(delta) else {
            return XCTFail("delta frame must decode")
        }
        XCTAssertEqual(decodedDelta.rev, 8)

        // The keep-alive comment between the frames must neither produce
        // a frame of its own nor merge into either frame.
        XCTAssertEqual(frames.map(\.kind), [.snapshot, .delta],
                       "the ':' keep-alive must not frame and must not break framing")
    }

    /// Review F2: the byte loop's EOF path. A server that closes the
    /// connection mid-frame must NOT emit the truncated frame — WHATWG
    /// EventSource discards pending data at EOF, and `stream()` reconnects
    /// from `Last-Event-ID` so the daemon replays whatever was lost
    /// (emitting it would hand `decode()` half a JSON object and raise a
    /// spurious decode-failure banner). The mock finishes the load, so
    /// this also proves the clean-EOF → reconnect transition: a second
    /// request follows the first.
    func testTruncatedFrameDiscardedAtEOFAndStreamReconnects() async throws {
        // Snapshot frame whose final `data:` line has NO trailing newline —
        // the daemon (or an intermediary such as tailscale serve) dropped
        // the connection mid-line, so `chunk` is non-empty at EOF and the
        // documented discard behaviour is actually exercised (review N2).
        let truncated = Data("event: snapshot\nid: 7\ndata: {\"rev\":7}".utf8)
        SSEStreamMockURLProtocol.fixture = truncated
        SSEStreamMockURLProtocol.served = false
        SSEStreamMockURLProtocol.finishAfterServe = true
        SSEStreamMockURLProtocol.resetRequestCount()
        defer { SSEStreamMockURLProtocol.fixture = nil }

        let config = URLSessionConfiguration.ephemeral
        config.protocolClasses = [SSEStreamMockURLProtocol.self]
        let session = URLSession(configuration: config)
        let client = CorraldClient(host: URL(string: "https://sse.test")!, session: session)

        let box = StreamFrameBox()
        let stream = Task {
            await client.stream(lastEventId: { nil }, onEvent: { box.append($0) })
        }

        // Clean EOF → reconnect: wait for the SECOND request, then cancel.
        let deadline = Date().addingTimeInterval(5)
        while SSEStreamMockURLProtocol.requestCount < 2, Date() < deadline {
            try? await Task.sleep(nanoseconds: 25_000_000)
        }
        stream.cancel()
        await stream.value
        session.invalidateAndCancel()

        XCTAssertGreaterThanOrEqual(
            SSEStreamMockURLProtocol.requestCount, 2,
            "clean EOF must take the reconnect path, not end the stream")
        XCTAssertEqual(
            box.frames.count, 0,
            "the truncated frame must be DISCARDED at EOF — emitting it would hand decode() half a JSON object")
    }
}

// MARK: - Connection failures (#92)

/// Delivery-stage probe for URLProtocol mocks. URLSession constructs each
/// protocol instance on its loader thread, so the test installs a handler
/// before `connect()` and the mock fires it at the top of `startLoading()`.
/// The lock keeps installation from racing the first request.
private final class URLProtocolStartProbe: @unchecked Sendable {
    private let lock = NSLock()
    private var handler: (@Sendable () -> Void)?

    func set(_ handler: @escaping @Sendable () -> Void) {
        lock.lock()
        self.handler = handler
        lock.unlock()
    }

    func clear() {
        lock.lock()
        handler = nil
        lock.unlock()
    }

    func fire() {
        lock.lock()
        let handler = self.handler
        lock.unlock()
        handler?()
    }
}

/// Per-test delivery state plus the expectations the test waits on. XCTest
/// does not expose an expectation's fulfillment state, so this probe records
/// which side of the URLSession seam ran before fulfilling its expectation.
private final class ConnectionDeliveryProbe: @unchecked Sendable {
    let started: XCTestExpectation
    let landed: XCTestExpectation

    private let lock = NSLock()
    private var startRecorded = false
    private var landRecorded = false

    init(startedDescription: String, landedDescription: String) {
        started = XCTestExpectation(description: startedDescription)
        started.assertForOverFulfill = false
        landed = XCTestExpectation(description: landedDescription)
        landed.assertForOverFulfill = false
    }

    struct Status: Sendable {
        let didStart: Bool
        let didLand: Bool
    }

    var status: Status {
        lock.lock()
        defer { lock.unlock() }
        return Status(didStart: startRecorded, didLand: landRecorded)
    }

    func markStarted() {
        lock.lock()
        startRecorded = true
        lock.unlock()
        started.fulfill()
    }

    func markLanded() {
        lock.lock()
        landRecorded = true
        lock.unlock()
        landed.fulfill()
    }
}

/// #92: a URLProtocol mock that FAILS every request (connection refused).
/// `startLoading()` reports `didFailWithError` before any response bytes,
/// so `URLSession.bytes(for:)` throws and the real `stream()` catch-all is
/// what sees the failure.
private final class FailingStreamURLProtocol: URLProtocol {
    private static let startProbe = URLProtocolStartProbe()

    static func setStartHandler(_ handler: @escaping @Sendable () -> Void) {
        startProbe.set(handler)
    }

    static func clearStartHandler() {
        startProbe.clear()
    }

    override class func canInit(with request: URLRequest) -> Bool { true }
    override class func canonicalRequest(for request: URLRequest) -> URLRequest { request }

    override func startLoading() {
        Self.startProbe.fire()
        client?.urlProtocol(self, didFailWithError: URLError(.cannotConnectToHost))
    }

    override func stopLoading() {}
}

/// F1: a URLProtocol mock that serves HTTP 500 — the non-200 arm of
/// `stream()`'s guard, which must surface a status-bearing reason.
private final class Non200StreamURLProtocol: URLProtocol {
    private static let startProbe = URLProtocolStartProbe()

    static func setStartHandler(_ handler: @escaping @Sendable () -> Void) {
        startProbe.set(handler)
    }

    static func clearStartHandler() {
        startProbe.clear()
    }

    override class func canInit(with request: URLRequest) -> Bool { true }
    override class func canonicalRequest(for request: URLRequest) -> URLRequest { request }

    override func startLoading() {
        Self.startProbe.fire()
        guard let url = request.url else {
            client?.urlProtocol(self, didFailWithError: URLError(.badURL))
            return
        }
        let response = HTTPURLResponse(
            url: url, statusCode: 500, httpVersion: "HTTP/1.1", headerFields: nil)!
        client?.urlProtocol(self, didReceive: response, cacheStoragePolicy: .notAllowed)
        client?.urlProtocolDidFinishLoading(self)
    }

    override func stopLoading() {}
}

/// Durable F2 probe mock: FAILS request #1 (connection refused, no response
/// bytes) then serves 200 `text/event-stream` with clean EOF and ZERO body
/// bytes on requests #2+ — an idle fleet serves 0 frames. Lock-guarded
/// request counter, the same NSLock pattern as `SSEStreamMockURLProtocol`
/// (review N1: bare static vars raced under TSan).
private final class ReconnectStreamURLProtocol: URLProtocol {
    private static let startProbe = URLProtocolStartProbe()
    private static let requestLock = NSLock()
    private static var requestCountStorage = 0

    static func setStartHandler(_ handler: @escaping @Sendable () -> Void) {
        startProbe.set(handler)
    }

    static func clearStartHandler() {
        startProbe.clear()
    }

    static var requestCount: Int {
        requestLock.lock()
        defer { requestLock.unlock() }
        return requestCountStorage
    }

    static func resetRequestCount() {
        requestLock.lock()
        defer { requestLock.unlock() }
        requestCountStorage = 0
    }

    private static func incrementRequestCount() {
        requestLock.lock()
        defer { requestLock.unlock() }
        requestCountStorage += 1
    }

    override class func canInit(with request: URLRequest) -> Bool { true }
    override class func canonicalRequest(for request: URLRequest) -> URLRequest { request }

    override func startLoading() {
        Self.startProbe.fire()
        Self.incrementRequestCount()
        guard Self.requestCount > 1, let url = request.url else {
            client?.urlProtocol(self, didFailWithError: URLError(.cannotConnectToHost))
            return
        }
        let response = HTTPURLResponse(
            url: url, statusCode: 200, httpVersion: "HTTP/1.1",
            headerFields: ["Content-Type": "text/event-stream"])!
        client?.urlProtocol(self, didReceive: response, cacheStoragePolicy: .notAllowed)
        client?.urlProtocolDidFinishLoading(self)
    }

    override func stopLoading() {}
}

/// #92 regression: `stream()` used to swallow EVERY connection error in a
/// bare catch, so `connectionState` never left `.connecting` — the UI
/// rendered `ProgressView()` forever with no banner, no `os.Logger` line,
/// no diagnosis (this is why #90 stayed invisible for the app's whole
/// life). This test drives the REAL path — `FleetStore.connect(client:)` →
/// `CorraldClient.stream()` over a real `URLSession` whose `URLProtocol`
/// mock FAILS the request — and asserts the store flips to `.error`, NOT
/// `.connecting`. RED on current main: the bare catch swallows the failure
/// and the state stays `.connecting`, failing the guard below.
@MainActor
final class ConnectionFailureTests: XCTestCase {

    /// One 5s bound covers the complete delivery chain on both sides of the
    /// URLSession seam. This is deliberately not lengthened: #179 observed a
    /// hosted runner miss that chain entirely, so the deadline exists to name
    /// which stage stalled, not to hide slow scheduling behind a longer wait.
    // Keep the waiter nonisolated so XCTest can suspend it independently of
    // the store's actor. The tests still await the real URLProtocol start and
    // FleetStore callback before reading the MainActor state.
    nonisolated private func awaitConnectionDelivery(
        probe: ConnectionDeliveryProbe,
        neverStarted: @Sendable () -> String,
        neverLanded: @Sendable () -> String,
        file: StaticString = #filePath,
        line: UInt = #line
    ) async -> Bool {
        await fulfillment(of: [probe.started, probe.landed], timeout: 5)
        let status = probe.status
        guard status.didStart else {
            XCTFail(neverStarted(), file: file, line: line)
            return false
        }
        guard status.didLand else {
            XCTFail(neverLanded(), file: file, line: line)
            return false
        }
        return true
    }

    nonisolated func testConnectionFailureSurfacesErrorNotConnecting() async {
        let config = URLSessionConfiguration.ephemeral
        config.protocolClasses = [FailingStreamURLProtocol.self]
        let session = URLSession(configuration: config)
        let client = CorraldClient(host: URL(string: "https://sse.test")!, session: session)
        let store = await MainActor.run { FleetStore() }

        let delivery = ConnectionDeliveryProbe(
            startedDescription: "FailingStreamURLProtocol.startLoading ran",
            landedDescription: "stream connection error reached FleetStore")
        FailingStreamURLProtocol.setStartHandler { delivery.markStarted() }
        await MainActor.run {
            store.onConnectionError = { _ in delivery.markLanded() }
            store.connect(client: client)
        }
        addTeardownBlock {
            let streamTask = await MainActor.run { store.disconnect() }
            FailingStreamURLProtocol.clearStartHandler()
            session.invalidateAndCancel()
            if let streamTask { await streamTask.value }
        }

        guard await awaitConnectionDelivery(
            probe: delivery,
            neverStarted: {
                "URLSession never invoked FailingStreamURLProtocol.startLoading() within the 5s delivery bound"
            },
            neverLanded: {
                "FailingStreamURLProtocol.startLoading() ran, but the stream failure never reached FleetStore within the 5s delivery bound"
            }
        ) else { return }

        let state = await MainActor.run { store.connectionState }
        guard case .error(let message) = state else {
            return XCTFail("connection failure must set .error, not \(state)")
        }
        XCTAssertTrue(message.hasPrefix("stream disconnected — "), message)
        // F5: the underlying reason must be non-empty too — the prefix
        // check alone would pass even if the reason were a blank string.
        let reason = String(message.dropFirst("stream disconnected — ".count))
        XCTAssertFalse(reason.isEmpty, "the surfaced reason must not be empty")
    }

    /// F1: an HTTP-level failure must NAME the status and URL — a bare
    /// `localizedDescription` of `DriveError` used to discard the message
    /// entirely, leaving a banner with zero diagnostic content. Drives the
    /// real path with a mock that serves HTTP 500.
    nonisolated func testNon200ResponseNamesStatusInError() async {
        let config = URLSessionConfiguration.ephemeral
        config.protocolClasses = [Non200StreamURLProtocol.self]
        let session = URLSession(configuration: config)
        let client = CorraldClient(host: URL(string: "https://sse.test")!, session: session)
        let store = await MainActor.run { FleetStore() }

        let delivery = ConnectionDeliveryProbe(
            startedDescription: "Non200StreamURLProtocol.startLoading ran",
            landedDescription: "HTTP connection error reached FleetStore")
        Non200StreamURLProtocol.setStartHandler { delivery.markStarted() }
        await MainActor.run {
            store.onConnectionError = { _ in delivery.markLanded() }
            store.connect(client: client)
        }
        addTeardownBlock {
            let streamTask = await MainActor.run { store.disconnect() }
            Non200StreamURLProtocol.clearStartHandler()
            session.invalidateAndCancel()
            if let streamTask { await streamTask.value }
        }

        guard await awaitConnectionDelivery(
            probe: delivery,
            neverStarted: {
                "URLSession never invoked Non200StreamURLProtocol.startLoading() within the 5s delivery bound"
            },
            neverLanded: {
                "Non200StreamURLProtocol.startLoading() ran, but the HTTP failure never reached FleetStore within the 5s delivery bound"
            }
        ) else { return }

        let state = await MainActor.run { store.connectionState }
        guard case .error(let message) = state else {
            return XCTFail("non-200 must surface .error, not \(state)")
        }
        XCTAssertTrue(message.contains("HTTP 500"), message)
        XCTAssertTrue(message.contains("/events"), message)
    }

    /// Review F2 durable probe: a recovered stream clears the `.error`
    /// indicator even when the fleet is idle (no frames arrive for
    /// `apply()` to clear it). Drives the REAL chain — `stream()` →
    /// `onConnected?()` → FleetStore's guarded Task hop → `noteConnected()`
    /// — via a URLProtocol mock that fails request #1 (refused) and serves
    /// 200 + clean EOF with ZERO body bytes on the retry (idle fleet → 0
    /// frames). Goes RED if `onConnected?()` is deleted from
    /// `CorraldClient.stream()`.
    nonisolated func testReconnectOverRealURLSessionClearsErrorState() async {
        ReconnectStreamURLProtocol.resetRequestCount()
        let config = URLSessionConfiguration.ephemeral
        config.protocolClasses = [ReconnectStreamURLProtocol.self]
        let session = URLSession(configuration: config)
        let client = CorraldClient(host: URL(string: "https://sse.test")!, session: session)
        let store = await MainActor.run { FleetStore() }

        let delivery = ConnectionDeliveryProbe(
            startedDescription: "ReconnectStreamURLProtocol.startLoading ran",
            landedDescription: "stream reconnected through FleetStore")
        ReconnectStreamURLProtocol.setStartHandler { delivery.markStarted() }
        await MainActor.run {
            store.onConnected = { delivery.markLanded() }
            store.connect(client: client)
        }
        addTeardownBlock {
            let streamTask = await MainActor.run { store.disconnect() }
            ReconnectStreamURLProtocol.clearStartHandler()
            session.invalidateAndCancel()
            if let streamTask { await streamTask.value }
        }

        // Request #1 must enter the loader; after the 1s backoff the retry's
        // 200 must traverse stream → onConnected → FleetStore hop. The two
        // probes below name whichever side exceeded the unchanged 5s bound.
        guard await awaitConnectionDelivery(
            probe: delivery,
            neverStarted: {
                "URLSession never invoked ReconnectStreamURLProtocol.startLoading() within the 5s delivery bound"
            },
            neverLanded: {
                let requestCount = ReconnectStreamURLProtocol.requestCount
                if requestCount < 2 {
                    return "ReconnectStreamURLProtocol.startLoading() ran, but the retry was not dispatched before the 5s delivery bound (requestCount=\(requestCount))"
                }
                return "ReconnectStreamURLProtocol.startLoading() ran \(requestCount) times, but the retry's 200 never reached FleetStore within the 5s delivery bound"
            }
        ) else { return }

        let (state, agentsAreEmpty) = await MainActor.run {
            (store.connectionState, store.agents.isEmpty)
        }
        XCTAssertEqual(state, .connected,
                       "the 200 on retry must clear the stale error via the real wiring")
        XCTAssertTrue(agentsAreEmpty, "an idle fleet serves 0 frames → 0 agents")
        XCTAssertGreaterThanOrEqual(ReconnectStreamURLProtocol.requestCount, 2,
                                    "the probe must exercise the failure → retry ladder")
    }

    /// F5: the connection reason reaches the owner's banner hook (AppModel
    /// routes it into the dismissible, copyable BannerView) — mirror of
    /// testDecodeFailureReachesTheBannerHook. The RED proof is banked, so
    /// referencing `onConnectionError` here no longer costs anything.
    func testConnectionFailureReachesTheBannerHook() {
        let store = FleetStore()
        var routed: String?
        store.onConnectionError = { routed = $0 }
        store.noteConnectionError("Could not connect to the server.")
        XCTAssertEqual(routed, "Could not connect to the server.")
        guard case .error = store.connectionState else {
            return XCTFail("state must also flip to .error")
        }
    }
}

// MARK: - Issue #91: a stale persisted cursor must not resume into an empty store

/// Captures the `Last-Event-ID` REQUEST header and holds the stream open
/// (one request only): serves a 200 `text/event-stream` response and never
/// finishes — the daemon's open stream — so `FleetStore.connect()` makes a
/// single request whose header the test asserts on. Lock-guarded statics,
/// the same NSLock pattern as `SSEStreamMockURLProtocol` (review N1: bare
/// static vars raced under TSan).
final class LastEventIDCapturingURLProtocol: URLProtocol {
    private static let captureLock = NSLock()
    private static var capturedHeaderStorage: String?
    private static var requestCountStorage = 0

    static var capturedHeader: String? {
        captureLock.lock()
        defer { captureLock.unlock() }
        return capturedHeaderStorage
    }

    static var requestCount: Int {
        captureLock.lock()
        defer { captureLock.unlock() }
        return requestCountStorage
    }

    static func reset() {
        captureLock.lock()
        capturedHeaderStorage = nil
        requestCountStorage = 0
        captureLock.unlock()
    }

    override class func canInit(with request: URLRequest) -> Bool { true }
    override class func canonicalRequest(for request: URLRequest) -> URLRequest { request }

    override func startLoading() {
        Self.captureLock.lock()
        Self.requestCountStorage += 1
        Self.capturedHeaderStorage = request.value(forHTTPHeaderField: "Last-Event-ID")
        Self.captureLock.unlock()
        guard let url = request.url else {
            client?.urlProtocol(self, didFailWithError: URLError(.badURL))
            return
        }
        let response = HTTPURLResponse(
            url: url, statusCode: 200, httpVersion: "HTTP/1.1",
            headerFields: ["Content-Type": "text/event-stream"])!
        client?.urlProtocol(self, didReceive: response, cacheStoragePolicy: .notAllowed)
        client?.urlProtocol(self, didLoad: Data())
        // Deliberately never finishes: the request stays open like the live stream.
    }

    override func stopLoading() {}
}

/// Regression for #91. `AppModel.resetDevice()` wipes the agent map but NOT
/// the persisted `fleetnotifier.lastEventId` key, so `restoreCursor()` can
/// resurrect a cursor into a store holding ZERO agents; `connect()` then
/// sends that cursor as `Last-Event-ID` and the daemon — correctly — replies
/// with deltas only, never the snapshot that would populate the board. The
/// invariant: a cursor is only valid if you hold the state it is a
/// delta-base for. These tests drive the REAL `FleetStore.connect()` path
/// (real `URLSession` byte path, URLProtocol mock) and assert on the
/// CAPTURED request header — the exact wire signal the daemon acts on.
@MainActor
final class StaleCursorTests: XCTestCase {

    private func makeStreamingClient() -> (URLSession, CorraldClient) {
        let config = URLSessionConfiguration.ephemeral
        config.protocolClasses = [LastEventIDCapturingURLProtocol.self]
        let session = URLSession(configuration: config)
        let client = CorraldClient(host: URL(string: "https://sse.test")!, session: session)
        return (session, client)
    }

    /// Waits for the URLProtocol mock to see the stream's first request
    /// (startLoading runs on the URLProtocol delegate queue, not the main
    /// actor) under a hard deadline — the same poll shape as the #90 tests.
    private func waitForFirstRequest() async {
        let deadline = Date().addingTimeInterval(5)
        while LastEventIDCapturingURLProtocol.requestCount < 1, Date() < deadline {
            try? await Task.sleep(nanoseconds: 25_000_000)
        }
    }

    /// Acceptance: an EMPTY store must DROP a persisted cursor. Seeds the
    /// real UserDefaults key and runs `restoreCursor()` — mirroring
    /// AppModel's resetDevice → re-register relaunch path — then connects
    /// with zero agents and asserts the wire carries NO `Last-Event-ID`.
    /// On the unfixed code the stale cursor survives, the header is sent,
    /// and the daemon answers deltas-only: the board never populates.
    func testEmptyStoreDropsPersistedCursor() async throws {
        let suiteName = "corral.cursor.empty.\(UUID().uuidString)"
        let defaults = UserDefaults(suiteName: suiteName)!
        defer { defaults.removePersistentDomain(forName: suiteName) }
        defaults.set("8089", forKey: "fleetnotifier.lastEventId")
        LastEventIDCapturingURLProtocol.reset()

        let store = FleetStore(defaults: defaults)
        store.restoreCursor()
        XCTAssertEqual(store.lastEventId, 8089, "precondition: the stale cursor was restored")

        let (session, client) = makeStreamingClient()
        store.connect(client: client)
        await waitForFirstRequest()
        store.reset()
        session.invalidateAndCancel()

        XCTAssertGreaterThanOrEqual(
            LastEventIDCapturingURLProtocol.requestCount, 1,
            "the stream must reach the wire for this test to mean anything")
        XCTAssertNil(
            LastEventIDCapturingURLProtocol.capturedHeader,
            "an EMPTY store holds no state to resume — the stale cursor must be dropped, "
                + "not sent as Last-Event-ID (the daemon would reply deltas-only and the board stays empty)")
        XCTAssertNil(defaults.string(forKey: "fleetnotifier.lastEventId"),
                     "reset must clear the injected cursor store")
    }

    /// Acceptance: a POPULATED store keeps its cursor — applying a snapshot
    /// (agents non-empty, `lastEventId` = the snapshot rev) then reconnecting
    /// must send that rev, so delta resume survives and a full snapshot is
    /// NOT forced on every reconnect.
    func testPopulatedStoreKeepsCursor() async throws {
        let suiteName = "corral.cursor.populated.\(UUID().uuidString)"
        let defaults = UserDefaults(suiteName: suiteName)!
        defer { defaults.removePersistentDomain(forName: suiteName) }
        LastEventIDCapturingURLProtocol.reset()

        let store = FleetStore(defaults: defaults)
        let snapshotJSON = """
        {"schema_version":5,"rev":8008,"generated_at":0,"agents":{"herdr:a":{"agent_id":"herdr:a","source":"herdr","tool":"claude","state":"idle","seq":1,"ts":1700000000000}}}
        """
        await store.ingest(SSEFrame(kind: .snapshot, id: 8008, data: snapshotJSON)).value
        XCTAssertFalse(store.agents.isEmpty, "precondition: the snapshot populated the store")
        XCTAssertEqual(store.lastEventId, 8008, "precondition: the snapshot rev is the delta-base")

        let (session, client) = makeStreamingClient()
        store.connect(client: client)
        await waitForFirstRequest()
        store.disconnect()
        session.invalidateAndCancel()

        XCTAssertEqual(
            LastEventIDCapturingURLProtocol.capturedHeader, "8008",
            "a populated store holds the delta-base — delta resume must survive reconnect")
    }
}

// MARK: - Nested ObservableObject forwarding (#93)

@MainActor
final class AppModelFleetForwardingTests: XCTestCase {

    /// #93: `fleet` is a NESTED `ObservableObject`. `@Published` on
    /// `AppModel` fires only when the REFERENCE is reassigned, so applying
    /// an SSE frame to the store otherwise re-runs NO `body` — every view
    /// observes `AppModel` but reads `model.fleet.agents` /
    /// `model.fleet.connectionState`. The child's `objectWillChange` must be
    /// forwarded to the parent. This test is RED on the unfixed code
    /// (zero emissions) and GREEN with the forwarding.
    func testFleetFrameApplyEmitsAppModelObjectWillChange() async {
        let model = AppModel()
        var emissions = 0
        let cancellable = model.objectWillChange.sink { emissions += 1 }

        // Real topology + real mutation path: one snapshot frame through
        // FleetStore.ingest — decode off-main, single main-actor apply —
        // exactly what the SSE stream does. Deterministic: await the hop.
        let snapshot = #"{"schema_version":5,"rev":9,"generated_at":0,"agents":{"herdr:a":{"agent_id":"herdr:a","source":"herdr","tool":"claude","state":"working","seq":1,"ts":1,"capabilities":[],"workspace":{}}}}"#
        await model.fleet.ingest(SSEFrame(kind: .snapshot, id: 9, data: snapshot)).value

        XCTAssertEqual(model.fleet.agents.count, 1,
                       "the frame must actually land in the store")
        XCTAssertGreaterThan(emissions, 0,
                             "a fleet mutation must emit AppModel.objectWillChange or the board never re-renders")
        cancellable.cancel()
    }
}

// MARK: - Issue #101: signed self-service grants read (grants refresh)

/// Regression for #101. `AppModel` caches grants from the last `/register`
/// response and there was NO way to refresh them — a host-side promotion
/// stayed invisible until the device re-minted itself read-only. The fix:
/// `POST /grants-read` (signed `{key_id, request, ts}`, verified like
/// `/device-token`) returns the key's CURRENT grants + expiry, and
/// `AppModel.refreshGrants()` re-syncs on cold launch / foreground. These
/// tests drive the REAL `DriveClient` / `AppModel` byte path with a
/// URLProtocol mock and assert on the persisted `DeviceKeyStore` meta.
/// Lock-guarded statics (review N1 discipline, same as
/// `LastEventIDCapturingURLProtocol`).
@MainActor
final class GrantsRefreshTests: XCTestCase {

    final class GrantsReadURLProtocol: URLProtocol {
        private static let lock = NSLock()
        private static var responsesStorage: [URL: (HTTPURLResponse, Data)] = [:]
        private static var requestsStorage: [URLRequest] = []

        static var requests: [URLRequest] {
            lock.lock()
            defer { lock.unlock() }
            return requestsStorage
        }

        static func setResponses(_ responses: [URL: (HTTPURLResponse, Data)]) {
            lock.lock()
            responsesStorage = responses
            requestsStorage = []
            lock.unlock()
        }

        override class func canInit(with request: URLRequest) -> Bool { true }
        override class func canonicalRequest(for request: URLRequest) -> URLRequest { request }

        override func startLoading() {
            Self.lock.lock()
            Self.requestsStorage.append(request)
            let scripted = Self.responsesStorage[request.url!]
            Self.lock.unlock()
            guard let (response, data) = scripted else {
                client?.urlProtocol(self, didFailWithError: URLError(.badURL))
                return
            }
            client?.urlProtocol(self, didReceive: response, cacheStoragePolicy: .notAllowed)
            client?.urlProtocol(self, didLoad: data)
            client?.urlProtocolDidFinishLoading(self)
        }

        override func stopLoading() {}
    }

    private func scriptedSession() -> URLSession {
        let config = URLSessionConfiguration.ephemeral
        config.protocolClasses = [GrantsReadURLProtocol.self]
        return URLSession(configuration: config)
    }

    private func seedMeta(grants: [String]) {
        let meta = DeviceKeyStore.DeviceMeta(keyId: "dev_seeded", host: "http://daemon",
                                             grants: grants, expiryTs: 1, registeredAt: 1)
        UserDefaults.standard.set(try! JSONEncoder().encode(meta), forKey: "fleetnotifier.deviceMeta")
        UserDefaults.standard.set("http://daemon", forKey: "fleetnotifier.host")
    }

    private func clearMeta() {
        UserDefaults.standard.removeObject(forKey: "fleetnotifier.deviceMeta")
        UserDefaults.standard.removeObject(forKey: "fleetnotifier.host")
    }

    /// #101 THE regression test. A cold launch restores `meta.grants` ([]),
    /// then `refreshGrants()` must re-fetch the CURRENT grants from the
    /// daemon and persist them — WITHOUT re-registering or re-minting a new
    /// key. RED on the unfixed code: there is no `refreshGrants()` (or it
    /// is not wired), so cached grants stay `[]`.
    func testRefreshGrantsOnColdLaunchUpdatesCachedGrants() async {
        seedMeta(grants: [])
        defer { clearMeta() }

        let grantsURL = URL(string: "http://daemon/grants-read")!
        GrantsReadURLProtocol.setResponses([
            grantsURL: (HTTPURLResponse(url: grantsURL, statusCode: 200,
                                        httpVersion: nil, headerFields: nil)!,
                        Data(#"{"ok":true,"key_id":"dev_seeded","grants":["read_tail","prompt","interrupt","approve"],"expiry_ts":1800000000,"revoked":false}"#.utf8)),
        ])

        let model = AppModel(session: scriptedSession())
        XCTAssertEqual(model.grants, [], "precondition: cold-launch cache is the read-only []")

        await model.refreshGrants()

        XCTAssertEqual(model.grants, ["read_tail", "prompt", "interrupt", "approve"],
                       "the promoted grants must land on the board")
        let persisted = DeviceKeyStore.loadMeta()
        XCTAssertEqual(persisted?.grants, ["read_tail", "prompt", "interrupt", "approve"],
                       "persisted meta must carry the refreshed grants")
        XCTAssertEqual(persisted?.expiryTs, 1_800_000_000, "expiry must refresh too")
        XCTAssertEqual(persisted?.keyId, "dev_seeded")
        XCTAssertEqual(persisted?.host, "http://daemon")
        XCTAssertEqual(GrantsReadURLProtocol.requests.map(\.url?.path),
                       ["/grants-read"],
                       "the refresh must hit /grants-read exactly once")
    }

    /// Failure path: a network error must NEVER clear the cached grants —
    /// a stale cached set is strictly better than a broken board.
    func testRefreshGrantsFailureKeepsCachedGrants() async {
        seedMeta(grants: ["prompt"])
        defer { clearMeta() }

        // No scripted response → the URLProtocol fails with URLError.
        GrantsReadURLProtocol.setResponses([:])

        let model = AppModel(session: scriptedSession())
        XCTAssertEqual(model.grants, ["prompt"], "precondition: cached grants restored")

        await model.refreshGrants()

        XCTAssertEqual(model.grants, ["prompt"],
                       "a failed refresh must preserve the cached grants, never clear them")
        XCTAssertEqual(DeviceKeyStore.loadMeta()?.grants, ["prompt"],
                       "persisted meta is untouched by a failed refresh")
    }
}

// MARK: - #167 Recent-output single-surface view-model



final class TimeInStateTests: XCTestCase {

    func testRelativeTimeFormatsLikeThePrototype() {
        XCTAssertEqual(RelativeTime.duration(milliseconds: 30_000), "30s")
        XCTAssertEqual(RelativeTime.duration(milliseconds: 42 * 60_000), "42m")
        XCTAssertEqual(RelativeTime.duration(milliseconds: (3 * 60 + 2) * 60_000), "3h 02m")
        XCTAssertEqual(RelativeTime.duration(milliseconds: (1 * 3600 + 10 * 60) * 1000), "1h 10m")
        XCTAssertEqual(RelativeTime.duration(milliseconds: (26 * 3600) * 1000), "1d 2h")
    }

    func testRoundPastMinutesKeepsTwoDigitColumn() {
        XCTAssertEqual(RelativeTime.duration(milliseconds: (2 * 3600 + 5 * 60) * 1000), "2h 05m")
    }

    func testMillisecondsNilWhenTsUnset() {
        let agent = Agent(agentId: "a", state: .working, ts: 0)
        XCTAssertNil(TimeInState.milliseconds(for: agent, now: 10_000))
    }

    func testMillisecondsClampsWhenClockBehindTs() {
        let agent = Agent(agentId: "a", state: .working, ts: 20_000)
        XCTAssertEqual(TimeInState.milliseconds(for: agent, now: 10_000), 0)
    }

    func testMillisecondsIsElapsedSinceRecordTs() {
        let agent = Agent(agentId: "a", state: .working, ts: 1_000_000)
        XCTAssertEqual(TimeInState.milliseconds(for: agent, now: 1_042_000), 42_000)
    }

    /// #166 review F2: the store's client-side `stateEnteredAt` wins over the
    /// record's churn-prone `ts` when present.
    func testMillisecondsPrefersStateEnteredAt() {
        let agent = Agent(agentId: "a", state: .working, ts: 100)
        XCTAssertEqual(TimeInState.milliseconds(for: agent, stateEnteredAt: 500, now: 1500), 1000)
    }
}

// MARK: - Answer availability gate (#166 review F7)

@MainActor
final class FleetRefreshTests: XCTestCase {

    private func agent(_ id: String, state: AgentState, title: String? = nil) -> Agent {
        Agent(agentId: id, state: state, seq: 1, ts: 1,
              capabilities: ["read_tail"], displayName: id, title: title)
    }

    private func snapshot(rev: UInt64, agents: [String: Agent]) -> Snapshot {
        Snapshot(schemaVersion: 5, rev: rev, generatedAt: 1, agents: agents)
    }

    /// Acceptance: snapshot/delta revision ordering — a pull refresh
    /// racing a newer delta must NOT reorder the newer delta behind an
    /// older snapshot (the stream already delivered rev 11; the refresh
    /// that was in flight answers rev 10).
    func testRefreshSnapshotRacingNewerDeltaIsDropped() {
        let store = FleetStore()
        store.applyRefresh(snapshot(rev: 10, agents: ["a": agent("a", state: .idle, title: "base")]))
        store.apply(.delta(Delta(rev: 11, upd: [agent("a", state: .working, title: "newer delta")], del: [])))
        store.applyRefresh(snapshot(rev: 10, agents: ["a": agent("a", state: .idle, title: "stale refresh snapshot")]))

        XCTAssertEqual(store.lastEventId, 11, "cursor must stay on the newer delta")
        XCTAssertEqual(store.agents["a"]?.title, "newer delta",
                       "a stale refresh cannot reorder a newer delta")
    }

    /// A refresh at an equal or newer revision is authoritative: equal
    /// replaces safely, newer replaces the board and advances the cursor
    /// the live stream resumes from.
    func testRefreshAppliesEqualAndNewerSnapshot() {
        let store = FleetStore()
        store.applyRefresh(snapshot(rev: 5, agents: ["a": agent("a", state: .working)]))
        store.applyRefresh(snapshot(rev: 5, agents: ["a": agent("a", state: .idle, title: "authoritative")]))

        XCTAssertEqual(store.lastEventId, 5)
        XCTAssertEqual(store.agents["a"]?.state, .idle,
                       "equal revision is a safe authoritative replacement")

        store.applyRefresh(snapshot(rev: 12, agents: ["b": agent("b", state: .working)]))
        XCTAssertEqual(store.lastEventId, 12,
                       "newer snapshot advances the SSE resume cursor")
        XCTAssertNil(store.agents["a"])
        XCTAssertEqual(store.agents["b"]?.state, .working)
    }

    /// Acceptance: success ends the refresh, shows current agents, and
    /// issues exactly ONE snapshot request.
    func testRefreshFetchesAppliesAndClearsInFlight() async throws {
        let script = DeterministicDriveScript(
            responses: ["/snapshot": try JSONEncoder().encode(
                snapshot(rev: 9, agents: ["a": agent("a", state: .working, title: "fresh")]))])
        let fixture = liveModel(script: script)
        defer { fixture.cleanup() }

        XCTAssertFalse(fixture.model.isRefreshingFleet)
        await fixture.model.refreshFleet()

        XCTAssertFalse(fixture.model.isRefreshingFleet, "success must end the refresh")
        XCTAssertNil(fixture.model.banner)
        XCTAssertEqual(fixture.model.fleet.agents["a"]?.title, "fresh")
        XCTAssertEqual(fixture.model.fleet.lastEventId, 9)
        XCTAssertEqual(script.log.requests.filter { $0.url?.path == "/snapshot" }.count, 1)
    }

    /// Acceptance: failure ends the indicator and surfaces the existing
    /// dismissible/retryable banner — never an endless spinner.
    func testRefreshFailureSurfacesBannerAndClearsInFlight() async {
        // The script has no /snapshot entry, so the protocol answers the
        // default response, which is not a Snapshot — the fetch fails.
        let script = DeterministicDriveScript(response: Data(#"{"ok":true}"#.utf8))
        let fixture = liveModel(script: script)
        defer { fixture.cleanup() }

        await fixture.model.refreshFleet()

        XCTAssertFalse(fixture.model.isRefreshingFleet,
                       "failure must clear the in-flight flag (no endless spinner)")
        XCTAssertEqual(fixture.model.banner?.kind, "fleet_refresh")
        XCTAssertNotNil(fixture.model.banner?.message)
        XCTAssertEqual(fixture.model.fleet.agents.count, 0,
                       "a failed refresh must not mutate the board")
    }

    /// Acceptance: repeated pulls are serialized/coalesced — a second
    /// refresh while one is in flight issues no further request.
    func testRefreshIsCoalescedWhileOneIsInFlight() async throws {
        let gate = DriveRequestGate()
        let script = DeterministicDriveScript(
            responses: ["/snapshot": try JSONEncoder().encode(
                snapshot(rev: 21, agents: ["a": agent("a", state: .working, title: "coalesced")]))],
            gates: ["/snapshot": gate])
        let fixture = liveModel(script: script)
        defer { gate.cancel(); fixture.cleanup() }

        let first = Task { await fixture.model.refreshFleet() }
        let reachedNetwork = await script.log.observed.waitFor(atLeast: 1)
        XCTAssertTrue(reachedNetwork, "the first refresh must reach the network")
        XCTAssertTrue(fixture.model.isRefreshingFleet)

        await fixture.model.refreshFleet() // second pull while in flight
        XCTAssertEqual(script.log.requests.filter { $0.url?.path == "/snapshot" }.count, 1,
                       "a second pull while refreshing must not issue another request")

        gate.release()
        await first.value
        XCTAssertFalse(fixture.model.isRefreshingFleet)
        XCTAssertEqual(fixture.model.fleet.agents["a"]?.title, "coalesced")
    }

    // MARK: fixture

    private struct LiveFixture {
        let model: AppModel
        let session: URLSession
        let defaults: UserDefaults
        let suiteName: String

        @MainActor
        func cleanup() {
            model.stopLive()
            session.invalidateAndCancel()
            defaults.removePersistentDomain(forName: suiteName)
            DeterministicDriveURLProtocol.clearScript()
        }
    }

    private func liveModel(script: DeterministicDriveScript) -> LiveFixture {
        let suiteName = "corral.fleet-refresh.\(UUID().uuidString)"
        // SAFETY: a fresh UUID-based suite name is always a valid suite.
        let defaults = UserDefaults(suiteName: suiteName)!
        DeterministicDriveURLProtocol.setScript(script)
        let config = URLSessionConfiguration.ephemeral
        config.protocolClasses = [DeterministicDriveURLProtocol.self]
        let session = URLSession(configuration: config)
        let model = AppModel(
            session: session,
            defaults: defaults,
            identityLoader: { (DeviceSigner(key: Curve25519.Signing.PrivateKey()),
                               .insecureFallback) },
            loadMeta: { nil },
            saveMeta: { _ in },
            wipeIdentity: {})
        model.mode = .live
        // SAFETY: a fixed valid URL literal.
        model.hostURL = URL(string: "http://daemon")!
        return LiveFixture(model: model, session: session, defaults: defaults,
                           suiteName: suiteName)
    }
}

// MARK: - #256: admin grant toggle must revert on a failed POST /grants

/// Method+path-scoped URLProtocol stub for the host-admin grant surface.
/// GET and POST share the /grants path, so responses key on "GET /grants"
/// and "POST /grants" respectively.

// MARK: - Refresh harness (scripted URLProtocol)

private final class AsyncCount: @unchecked Sendable {
    private let lock = NSLock()
    private var count = 0
    private let updates: AsyncStream<Int>
    private let continuation: AsyncStream<Int>.Continuation

    init() {
        var continuation: AsyncStream<Int>.Continuation?
        let updates = AsyncStream<Int>(bufferingPolicy: .unbounded) {
            continuation = $0
        }
        self.updates = updates
        self.continuation = continuation!
    }

    var value: Int {
        lock.lock()
        defer { lock.unlock() }
        return count
    }

    func increment() {
        lock.lock()
        count += 1
        let next = count
        lock.unlock()
        continuation.yield(next)
    }

    func waitFor(atLeast target: Int,
                 timeoutNanoseconds: UInt64 = 2_000_000_000) async -> Bool {
        if value >= target { return true }
        let updates = self.updates
        return await withTaskGroup(of: Bool.self) { group in
            group.addTask {
                var iterator = updates.makeAsyncIterator()
                while let next = await iterator.next() {
                    if next >= target { return true }
                }
                return false
            }
            group.addTask {
                try? await Task.sleep(nanoseconds: timeoutNanoseconds)
                return false
            }
            let result = await group.next() ?? false
            group.cancelAll()
            return result
        }
    }
}

private final class DriveRequestLog: @unchecked Sendable {
    private let lock = NSLock()
    private var requestStorage: [URLRequest] = []
    let observed = AsyncCount()
    let completed = AsyncCount()
    let cancelled = AsyncCount()

    func record(_ request: URLRequest) {
        lock.lock()
        requestStorage.append(request)
        lock.unlock()
        observed.increment()
    }

    var requests: [URLRequest] {
        lock.lock()
        defer { lock.unlock() }
        return requestStorage
    }
}

private final class DriveRequestGate: @unchecked Sendable {
    private let condition = NSCondition()
    private var released = false
    private var cancelled = false
    private var waiters: [CheckedContinuation<Bool, Never>] = []

    func wait() async -> Bool {
        await withCheckedContinuation { continuation in
            condition.lock()
            if cancelled {
                condition.unlock()
                continuation.resume(returning: false)
            } else if released {
                condition.unlock()
                continuation.resume(returning: true)
            } else {
                waiters.append(continuation)
                condition.unlock()
            }
        }
    }

    func release() {
        condition.lock()
        released = true
        condition.unlock()
        resumeWaiters(returning: true)
    }

    func cancel() {
        condition.lock()
        cancelled = true
        released = true
        condition.unlock()
        resumeWaiters(returning: false)
    }

    private func resumeWaiters(returning result: Bool) {
        condition.lock()
        let waiters = self.waiters
        self.waiters.removeAll()
        condition.unlock()
        waiters.forEach { $0.resume(returning: result) }
    }
}

private final class DeterministicDriveScript: @unchecked Sendable {
    let log = DriveRequestLog()
    let defaultResponse: Data
    let responses: [String: Data]
    let gates: [String: DriveRequestGate]
    let statuses: [String: Int]
    let cancelOnStop: Bool

    init(response: Data, gate: DriveRequestGate? = nil, cancelOnStop: Bool = true) {
        self.defaultResponse = response
        self.responses = [:]
        self.gates = gate.map { ["/drive": $0] } ?? [:]
        self.statuses = [:]
        self.cancelOnStop = cancelOnStop
    }

    init(responses: [String: Data], gates: [String: DriveRequestGate] = [:],
         defaultResponse: Data = Data(#"{"request_id":"r","ok":true,"rev":2}"#.utf8),
         statuses: [String: Int] = [:], cancelOnStop: Bool = true) {
        self.defaultResponse = defaultResponse
        self.responses = responses
        self.gates = gates
        self.statuses = statuses
        self.cancelOnStop = cancelOnStop
    }

    func response(for path: String) -> Data {
        responses[path] ?? defaultResponse
    }

    func status(for path: String) -> Int {
        statuses[path] ?? 200
    }

    func gate(for path: String) -> DriveRequestGate? {
        gates[path]
    }
}

private func requestBodyData(_ request: URLRequest) -> Data? {
    if let body = request.httpBody {
        return body
    }
    guard let stream = request.httpBodyStream else { return nil }
    let opened = stream.streamStatus == .notOpen
    if opened {
        stream.open()
    }
    defer {
        if opened {
            stream.close()
        }
    }
    var data = Data()
    let bufferSize = 8192
    let buffer = UnsafeMutablePointer<UInt8>.allocate(capacity: bufferSize)
    defer { buffer.deallocate() }
    while true {
        let count = stream.read(buffer, maxLength: bufferSize)
        if count < 0 {
            return nil
        }
        if count == 0 {
            break
        }
        data.append(buffer, count: count)
    }
    return data
}


private final class DeterministicDriveURLProtocol: URLProtocol {
    private static let scriptLock = NSLock()
    private static var scriptStorage: DeterministicDriveScript?
    private var activeScript: DeterministicDriveScript?
    private let deliveryQueue = DispatchQueue(
        label: "FleetNotifierTests.DeterministicDriveURLProtocol.\(UUID().uuidString)")
    private var stopWasRecorded = false

    static func setScript(_ script: DeterministicDriveScript) {
        scriptLock.lock()
        scriptStorage = script
        scriptLock.unlock()
    }

    static func clearScript() {
        scriptLock.lock()
        scriptStorage = nil
        scriptLock.unlock()
    }

    private static func currentScript() -> DeterministicDriveScript? {
        scriptLock.lock()
        defer { scriptLock.unlock() }
        return scriptStorage
    }

    override class func canInit(with request: URLRequest) -> Bool { true }
    override class func canonicalRequest(for request: URLRequest) -> URLRequest { request }

    override func startLoading() {
        guard let script = Self.currentScript(), let url = request.url else {
            client?.urlProtocol(self, didFailWithError: URLError(.badURL))
            return
        }
        activeScript = script
        var recordedRequest = request
        if let body = requestBodyData(request) {
            recordedRequest.httpBody = body
        }
        script.log.record(recordedRequest)
        let gate = script.gate(for: url.path)
        Task { [self] in
            let canRespond: Bool
            if let gate {
                canRespond = await gate.wait()
            } else {
                canRespond = true
            }
            deliveryQueue.async {
                if canRespond, !self.stopWasRecorded {
                    let response = HTTPURLResponse(url: url, statusCode: script.status(for: url.path),
                                                   httpVersion: "HTTP/1.1",
                                                   headerFields: nil)!
                    self.client?.urlProtocol(self, didReceive: response,
                                             cacheStoragePolicy: .notAllowed)
                    self.client?.urlProtocol(self, didLoad: script.response(for: url.path))
                    self.client?.urlProtocolDidFinishLoading(self)
                } else {
                    self.client?.urlProtocol(self,
                                             didFailWithError: URLError(.cancelled))
                }
                script.log.completed.increment()
            }
        }
    }

    override func stopLoading() {
        let script = activeScript
        let path = request.url?.path
        deliveryQueue.async {
            guard let script, script.cancelOnStop else { return }
            guard !self.stopWasRecorded else { return }
            self.stopWasRecorded = true
            guard let path,
                  let gate = script.gate(for: path) else { return }
            script.log.cancelled.increment()
            gate.cancel()
        }
    }
}

// MARK: - #362 status-grouped board projections

final class BoardModelBoardV2Tests: XCTestCase {
    private func agent(_ id: String, state: AgentState, repo: String?, branch: String? = nil,
                       ts: UInt64 = 100, displayName: String? = nil) -> Agent {
        Agent(agentId: id, state: state, seq: 1, ts: ts,
              capabilities: ["read_tail"],
              workspace: Workspace(repo: repo, branch: branch),
              displayName: displayName ?? id)
    }

    private func subgroupNames(_ status: BoardModel.StatusSection) -> [String] {
        status.subgroups.map(\.displayName)
    }

    private func subgroupRows(_ status: BoardModel.StatusSection) -> [String] {
        status.subgroups.flatMap { $0.agents.map(\.agentId) }
    }

    func testSectionsBucketByRawStatusInLockedOrder() {
        let sections = BoardModel.sections([
            agent("b1", state: .blocked, repo: "corral"),
            agent("w1", state: .working, repo: "other"),
            agent("i1", state: .idle, repo: "corral"),
            agent("u1", state: .unknown, repo: nil),
        ])
        // Locked order: Blocked → Working → Idle → Unknown; section headers
        // carry the raw status name + the TOTAL across its subgroups.
        XCTAssertEqual(sections.statuses.map(\.state),
                       [.blocked, .working, .idle, .unknown])
        XCTAssertEqual(sections.statuses.map(\.header),
                       ["blocked (1)", "working (1)", "idle (1)", "unknown (1)"])
        XCTAssertEqual(sections.statuses.map(\.total), [1, 1, 1, 1])
        // A one-row section still renders its repo subgroup (uniformity).
        XCTAssertEqual(subgroupNames(sections.statuses[0]), ["corral"])
        XCTAssertEqual(sections.statuses[0].subgroups[0].header, "corral (1)")
    }

    func testEverySectionGroupsRowsIntoRepoSubgroupsAlphabeticalOtherLast() {
        // #371: same status across DIFFERENT repos — including the orphan
        // (repo = nil) — splits the section into repo subgroups: named
        // repos ALPHABETICAL first, the Other subgroup LAST; each subgroup
        // keeps the section's within-bucket order (ts desc, then id).
        let sections = BoardModel.sections([
            agent("z-orphan", state: .working, repo: nil, ts: 50),
            agent("in-other", state: .working, repo: "other", ts: 150),
            agent("in-corral", state: .working, repo: "corral", ts: 200),
            agent("corral-old", state: .working, repo: "corral", ts: 100),
        ])
        XCTAssertEqual(sections.statuses.count, 1)
        let working = sections.statuses[0]
        XCTAssertEqual(working.state, .working)
        XCTAssertEqual(subgroupNames(working), ["corral", "other", "Other"],
                       "named repos alphabetical, Other LAST")
        XCTAssertEqual(working.subgroups[0].header, "corral (2)")
        XCTAssertEqual(working.subgroups[1].header, "other (1)")
        XCTAssertEqual(working.subgroups[2].header, "Other (1)")
        XCTAssertEqual(working.subgroups[2].repo, nil)
        XCTAssertEqual(working.subgroups[0].agents.map(\.agentId),
                       ["in-corral", "corral-old"],
                       "subgroups preserve the bucket's ts-desc order")
        XCTAssertEqual(working.header, "working (4)",
                       "the section total sums every subgroup")
        XCTAssertEqual(subgroupRows(working),
                       ["in-corral", "corral-old", "in-other", "z-orphan"],
                       "subgrouping is a stable partition of the ordered bucket")
    }

    func testBlockedSectionIsGroupedUniformlyLikeEveryOtherSection() {
        // #371: EVERY section incl. Blocked gets repo subgroups — blocked
        // rows across repos render under their repo bands, blocked first
        // overall because the section is first; no agent duplicated.
        let sections = BoardModel.sections([
            agent("idle", state: .idle, repo: "corral", ts: 300),
            agent("blocked-with-repo", state: .blocked, repo: "corral", ts: 200),
            agent("blocked-orphan", state: .blocked, repo: nil, ts: 100),
        ])
        XCTAssertEqual(sections.statuses.map(\.state), [.blocked, .idle])
        XCTAssertEqual(subgroupNames(sections.statuses[0]), ["corral", "Other"])
        XCTAssertEqual(subgroupRows(sections.statuses[0]),
                       ["blocked-with-repo", "blocked-orphan"])
        let allRows = sections.statuses.flatMap(subgroupRows)
        XCTAssertEqual(allRows.count, Set(allRows).count,
                       "every agent must appear in exactly one section/subgroup")
        XCTAssertEqual(Set(allRows), Set(["idle", "blocked-with-repo", "blocked-orphan"]))
    }

    func testDoneGetsItsOwnSectionOnlyWhenHerdrReportsIt() {
        // herdr 0.8.2 finished panes fall back to idle: a done-less fleet
        // (the live-board norm) has NO done section.
        let noDone = BoardModel.sections([
            agent("working", state: .working, repo: "r"),
            agent("idle", state: .idle, repo: "r"),
        ])
        XCTAssertEqual(noDone.statuses.map(\.state), [.working, .idle])

        // When the daemon reports done, a "done (N)" section renders after
        // idle (wire-done ranks WITH idle — state-token rank 2).
        let withDone = BoardModel.sections([
            agent("working", state: .working, repo: "r"),
            agent("idle", state: .idle, repo: "r", ts: 300),
            agent("done", state: .done, repo: "r", ts: 100),
        ])
        XCTAssertEqual(withDone.statuses.map(\.state),
                       [.working, .idle, .done])
        XCTAssertEqual(withDone.statuses[2].header, "done (1)")
        XCTAssertEqual(subgroupRows(withDone.statuses[2]), ["done"])
    }

    func testDoneSectionPositionIsRankTieNotTimestampDriven() {
        // Ordering determinism: done shares idle's rank (2), so its section
        // sits AFTER the idle section even when the done agent is newer —
        // section order must never re-sort by ts across buckets.
        let sections = BoardModel.sections([
            agent("older-idle", state: .idle, repo: "r", ts: 100),
            agent("newer-done", state: .done, repo: "r", ts: 900),
            agent("unknown", state: .unknown, repo: "r"),
        ])
        XCTAssertEqual(sections.statuses.map(\.state),
                       [.idle, .done, .unknown])
    }

    func testWithinStatusOrderingIsTsDescThenAgentId() {
        let sections = BoardModel.sections([
            agent("older", state: .working, repo: "r", ts: 100),
            agent("newer", state: .working, repo: "r", ts: 200),
            agent("tie-a", state: .working, repo: "r", ts: 200),
            agent("tie-b", state: .working, repo: "r", ts: 200),
        ])
        XCTAssertEqual(subgroupRows(sections.statuses[0]),
                       ["newer", "tie-a", "tie-b", "older"],
                       "ts desc, then agent id for determinism")
    }

    func testEmptyAndUnknownRepoStringsFoldIntoTheOtherSubgroup() {
        // A repo key of "" or the literal Other label is not a real repo
        // identity — it folds into the Other bucket so subgroup ids can
        // never collide with the orphan subgroup.
        let sections = BoardModel.sections([
            agent("empty-repo", state: .working, repo: "", ts: 200),
            agent("literal-other", state: .working, repo: "Other", ts: 150),
            agent("orphan", state: .working, repo: nil, ts: 100),
        ])
        XCTAssertEqual(subgroupNames(sections.statuses[0]), ["Other"])
        XCTAssertEqual(subgroupRows(sections.statuses[0]),
                       ["empty-repo", "literal-other", "orphan"],
                       "the Other bucket keeps the section's within-bucket order")
    }

    func testOrphanOnlySectionRendersASingleOtherSubgroup() {
        let sections = BoardModel.sections([
            agent("o1", state: .unknown, repo: nil, ts: 200),
            agent("o2", state: .unknown, repo: nil, ts: 100),
        ])
        XCTAssertEqual(subgroupNames(sections.statuses[0]), ["Other"])
        XCTAssertEqual(sections.statuses[0].header, "unknown (2)")
    }

    func testSectionOrderIsDeterministicRegardlessOfInputOrder() {
        let fleet = [
            agent("idle", state: .idle, repo: "r", ts: 300),
            agent("blocked", state: .blocked, repo: "r", ts: 200),
            agent("unknown", state: .unknown, repo: "r", ts: 100),
        ]
        let forward = BoardModel.sections(fleet)
        let shuffled = BoardModel.sections(fleet.reversed())
        XCTAssertEqual(forward.statuses.map(\.state), [.blocked, .idle, .unknown])
        XCTAssertEqual(forward.statuses, shuffled.statuses,
                       "subgroup bucketing must be input-order independent")
    }

    func testRepoFilterKeepsSectionsAndOnlyTheSelectedReposSubgroup() {
        // #371: selecting a repo chip keeps EVERY status section, showing
        // only that repo's subgroup per status; orphans never match a chip,
        // so no Other subgroup appears under a repo filter.
        let fleet = [
            agent("b-corral", state: .blocked, repo: "corral", ts: 500),
            agent("b-other", state: .blocked, repo: "other", ts: 400),
            agent("b-orphan", state: .blocked, repo: nil, ts: 300),
            agent("w-corral", state: .working, repo: "corral", ts: 200),
            agent("w-orphan", state: .working, repo: nil, ts: 100),
        ]
        let filtered = BoardModel.agents(fleet, in: "corral")
        let sections = BoardModel.sections(filtered)
        XCTAssertEqual(sections.statuses.map(\.state), [.blocked, .working])
        XCTAssertEqual(sections.statuses.map(\.header), ["blocked (1)", "working (1)"],
                       "section totals rescope to the filtered set")
        for status in sections.statuses {
            XCTAssertEqual(subgroupNames(status), ["corral"],
                           "a repo filter shows only that repo's subgroup in every section")
        }
    }

    func testEmptyBoardHasNoSections() {
        let sections = BoardModel.sections([])
        XCTAssertTrue(sections.statuses.isEmpty)
    }
}

// MARK: - #386 status-section collapse (pure per-session state)

/// The status-section collapse contract (#386): a fresh board session has
/// EVERY section EXPANDED, toggling collapses/expands exactly ONE section
/// independently, and no state is shared across sessions — the collapse
/// lives in memory for the board session only and is never persisted
/// (consistent with #373 recents blocks). `StatusSectionCollapse` is the
/// pure state the board view holds per session.
final class BoardStatusSectionCollapseTests: XCTestCase {

    func testFreshSessionDefaultsEverySectionExpanded() {
        let collapse = BoardModel.StatusSectionCollapse.fresh
        for state in AgentState.allCases {
            XCTAssertFalse(collapse.isCollapsed(state),
                           "a fresh board session must start with \(state.displayName) EXPANDED")
        }
        XCTAssertTrue(collapse.collapsed.isEmpty,
                      "a fresh session collapses nothing")
    }

    func testToggleCollapsesThenExpandsTheSameSection() {
        var collapse = BoardModel.StatusSectionCollapse.fresh
        collapse.toggle(.working)
        XCTAssertTrue(collapse.isCollapsed(.working))
        XCTAssertEqual(collapse.collapsed, ["working"])
        collapse.toggle(.working)
        XCTAssertFalse(collapse.isCollapsed(.working),
                       "toggling the same section again must expand it")
        XCTAssertTrue(collapse.collapsed.isEmpty)
    }

    func testCollapseIsIdempotentAndNeverExpands() {
        // The evidence driver collapses through `collapse(_:)` — a task
        // re-fire must never undo an earlier collapse (toggle is only for
        // the interactive bar).
        var collapse = BoardModel.StatusSectionCollapse.fresh
        collapse.collapse(.blocked)
        collapse.collapse(.blocked)
        XCTAssertEqual(collapse.collapsed, ["blocked"],
                       "repeated collapse calls must stay collapsed (idempotent)")
        collapse.collapse(.working)
        XCTAssertTrue(collapse.isCollapsed(.blocked))
        XCTAssertTrue(collapse.isCollapsed(.working))
        collapse.toggle(.blocked)
        XCTAssertFalse(collapse.isCollapsed(.blocked),
                       "only an explicit toggle may expand a collapsed section")
        XCTAssertTrue(collapse.isCollapsed(.working),
                      "collapsing one section never touches another")
    }

    func testCollapseIsPerSectionAndIndependent() {
        var collapse = BoardModel.StatusSectionCollapse.fresh
        collapse.toggle(.blocked)
        collapse.toggle(.idle)
        XCTAssertTrue(collapse.isCollapsed(.blocked))
        XCTAssertTrue(collapse.isCollapsed(.idle))
        XCTAssertFalse(collapse.isCollapsed(.working))
        XCTAssertFalse(collapse.isCollapsed(.done))
        XCTAssertFalse(collapse.isCollapsed(.unknown),
                       "collapsing one section must never touch the others")
        collapse.toggle(.blocked)
        XCTAssertFalse(collapse.isCollapsed(.blocked))
        XCTAssertTrue(collapse.isCollapsed(.idle),
                      "expanding one section must not expand another")
    }

    func testSessionsAreIndependentAndNeverPersisted() {
        var first = BoardModel.StatusSectionCollapse.fresh
        var second = BoardModel.StatusSectionCollapse.fresh
        first.toggle(.blocked)
        XCTAssertTrue(first.isCollapsed(.blocked))
        XCTAssertFalse(second.isCollapsed(.blocked),
                       "a second board session must start fully expanded — no shared/persisted state")
    }

    func testRawStatusIdKeysMatchStatusSectionIds() {
        // The collapse state keys by the SAME id the rendered section
        // exposes (`state.rawValue`), so a collapse survives section
        // re-projection and never drifts from the section it names.
        for state in AgentState.allCases {
            let section = BoardModel.StatusSection(state: state, subgroups: [])
            XCTAssertEqual(section.id, state.rawValue)
        }
    }
}

// MARK: - #371 working-motion math (pure, view-independent)

/// Locks the breathing-heartbeat math (the approved 1.2 s cycle, 160 ms
/// stagger, opacity 0.34 → 1 / scale 0.78 → 1 peaking at 42 %). These bite:
/// a flattened (non-staggered) animation, a wrong cycle, or a broken wrap
/// all go RED here before any view code is involved.
final class WorkingMotionTests: XCTestCase {

    func testStaggeredSquaresShareOneShiftedCurve() {
        // Square n's curve == square 0's curve delayed by n × 160 ms.
        for t in stride(from: 0.0, through: 1.2, by: 0.05) {
            XCTAssertEqual(WorkingMotion.opacity(at: t, square: 0),
                           WorkingMotion.opacity(at: t + WorkingMotion.stagger,
                                                 square: 1),
                           accuracy: 1e-9)
            XCTAssertEqual(WorkingMotion.opacity(at: t, square: 0),
                           WorkingMotion.opacity(at: t + 2 * WorkingMotion.stagger,
                                                 square: 2),
                           accuracy: 1e-9)
            XCTAssertEqual(WorkingMotion.scale(at: t, square: 0),
                           WorkingMotion.scale(at: t + WorkingMotion.stagger,
                                               square: 1),
                           accuracy: 1e-9)
        }
    }

    func testSquaresAreVisiblyOutOfPhaseAtRest() {
        // At t = 0 the three squares are mid-cycle at DIFFERENT opacities —
        // a flattened animation (all three identical) fails this.
        let opacities = (0..<WorkingMotion.squareCount)
            .map { WorkingMotion.opacity(at: 0, square: $0) }
        XCTAssertEqual(Set(opacities).count, WorkingMotion.squareCount,
                       "every square must sit on a different phase of the cycle")
        XCTAssertEqual(opacities[0], WorkingMotion.minOpacity, accuracy: 1e-9,
                       "square 0 starts the cycle at its rest opacity")
        XCTAssertTrue(opacities.allSatisfy { $0 >= WorkingMotion.minOpacity - 1e-9
                                             && $0 <= 1 + 1e-9 })
    }

    func testBreathingBoundsAndPeak() {
        // Opacity ramps 0.34 → 1 and back; scale 0.78 → 1 and back; the
        // peak sits at 42 % of the 1.2 s cycle.
        XCTAssertEqual(WorkingMotion.cycle, 1.2, accuracy: 1e-9)
        XCTAssertEqual(WorkingMotion.opacity(at: 0, square: 0),
                       WorkingMotion.minOpacity, accuracy: 1e-9)
        let peak = WorkingMotion.peakPhase * WorkingMotion.cycle
        XCTAssertEqual(WorkingMotion.opacity(at: peak, square: 0), 1.0,
                       accuracy: 1e-9)
        XCTAssertEqual(WorkingMotion.scale(at: peak, square: 0), 1.0,
                       accuracy: 1e-9)
        XCTAssertEqual(WorkingMotion.scale(at: 0, square: 0),
                       WorkingMotion.minScale, accuracy: 1e-9)
        // The cycle repeats: the value at t + 1.2 s equals the value at t.
        for t in [0.0, 0.3, 0.504, 1.1] {
            XCTAssertEqual(WorkingMotion.opacity(at: t, square: 1),
                           WorkingMotion.opacity(at: t + WorkingMotion.cycle,
                                                 square: 1),
                           accuracy: 1e-9,
                           "the heartbeat must repeat on its 1.2 s cycle")
        }
    }
}

// MARK: - #364 B repo filter chip projections

/// Focused regressions for the #364 repo filter chips. These bite: the
/// chip set must be live per-repo counts (orphans never a chip), the
/// filter must pick WHICH agents the #362 status sections bucket (never
/// regroup them), and a vanished repo must reconcile back to All.
final class RepoFilterChipProjectionTests: XCTestCase {
    private func agent(_ id: String, repo: String?,
                       state: AgentState = .working,
                       ts: UInt64 = 100) -> Agent {
        Agent(agentId: id, state: state, seq: 1, ts: ts,
              capabilities: ["read_tail"],
              workspace: Workspace(repo: repo,
                                   branch: repo.map { "b-" + $0 }),
              displayName: id)
    }

    func testChipsCountAgentsPerRepoAlphabeticallyAndSkipOrphans() {
        let chips = BoardModel.repoFilters([
            agent("a1", repo: "corral"),
            agent("a2", repo: "corral"),
            agent("b1", repo: "fleet-operations"),
            agent("orphan", repo: nil),
        ])
        XCTAssertEqual(chips.map(\.repo), ["corral", "fleet-operations"])
        XCTAssertEqual(chips.map(\.count), [2, 1])
        XCTAssertEqual(chips.map(\.id), ["corral", "fleet-operations"])
    }

    func testDemoSeedChipsMatchTheFixtureBoard() {
        // 8 seeded agents across 4 repos + 1 orphan: the orphan counts
        // under All only, never as its own chip.
        let agents = Array(DemoFleet.seed().values)
        let chips = BoardModel.repoFilters(agents)
        XCTAssertEqual(chips.map(\.repo),
                       ["demo-atlas", "demo-garden", "demo-ledger", "demo-orbit"])
        XCTAssertEqual(chips.map(\.count), [2, 2, 1, 2])
        XCTAssertFalse(chips.contains { $0.repo == "demo-orphan" })
        XCTAssertEqual(BoardModel.agents(agents, in: nil).count, 8,
                       "All must include every agent, orphans included")
    }

    func testEmptyFleetHasNoChips() {
        XCTAssertTrue(BoardModel.repoFilters([] as [Agent]).isEmpty)
    }

    func testFilterKeepsOnlyMatchingRepoAgents() {
        let agents = [
            agent("in-corral", repo: "corral"),
            agent("other", repo: "fleet-operations"),
            agent("orphan", repo: nil),
        ]
        XCTAssertEqual(BoardModel.agents(agents, in: "corral").map(\.agentId),
                       ["in-corral"])
        XCTAssertEqual(BoardModel.agents(agents, in: "corral").count, 1)
        let all = BoardModel.agents(agents, in: nil)
        XCTAssertEqual(Set(all.map(\.agentId)), Set(agents.map(\.agentId)),
                       "nil filter must be the identity — All shows every agent")
    }

    func testFilteredSectionsKeepLockedOrderOverTheFilteredSet() {
        // Demo board filtered to demo-atlas: working (featured) and
        // unknown (atlas-unknown) sections in the locked order — the
        // blocked demo-garden row is filtered out, sections never regroup,
        // and each section shows ONLY the demo-atlas subgroup (#371).
        let agents = Array(DemoFleet.seed().values)
        let filtered = BoardModel.agents(agents, in: "demo-atlas")
        let sections = BoardModel.sections(filtered)
        XCTAssertEqual(sections.statuses.map(\.state), [.working, .unknown])
        XCTAssertEqual(sections.statuses[0].header, "working (1)")
        XCTAssertEqual(sections.statuses[0].subgroups.map(\.displayName),
                       ["demo-atlas"])
        XCTAssertEqual(sections.statuses[0].subgroups[0].agents.map(\.agentId),
                       [DemoFleet.featuredAgentID])
        XCTAssertEqual(sections.statuses[1].subgroups[0].agents.map(\.agentId),
                       ["herdr:demo-atlas-unknown"])

        // Filtering never regroups: three working agents across repos under
        // a repo filter keep their locked within-section order inside the
        // selected repo's subgroup.
        let mixed = [
            agent("in-corral", repo: "corral", ts: 200),
            agent("in-other", repo: "other", ts: 150),
            agent("blocked-corral", repo: "corral", state: .blocked, ts: 900),
        ]
        let corralSections = BoardModel.sections(
            BoardModel.agents(mixed, in: "corral"))
        XCTAssertEqual(corralSections.statuses.map(\.state), [.blocked, .working])
        XCTAssertEqual(corralSections.statuses[0].subgroups.map(\.displayName),
                       ["corral"])
        XCTAssertEqual(corralSections.statuses[1].subgroups[0].agents.map(\.agentId),
                       ["in-corral"])
    }

    func testReconcileKeepsALiveRepoAndDropsAVanishedOne() {
        let chips = [BoardModel.RepoFilterChip(repo: "corral", count: 2)]
        XCTAssertEqual(BoardModel.reconcile("corral", against: chips), "corral")
        XCTAssertNil(BoardModel.reconcile("gone", against: chips),
                     "a repo that left the fleet must render as All")
        XCTAssertNil(BoardModel.reconcile(nil, against: chips),
                     "All stays All")
        XCTAssertNil(BoardModel.reconcile("corral", against: []))
    }
}

// MARK: - #364 C recents sheet request lifecycle

/// Focused discriminating regressions for the reliable recents-sheet
/// reopen. The pre-#364 code latched a sticky `recentsAgentId` behind an
/// equality-guarded onChange: after a dismissal the latch was never
/// cleared, so a re-request of the same agent compared equal and the
/// first tap was swallowed. These tests pin the replacement contract —
/// every request is a fresh monotonic value, and dismissal completion
/// clears (clean close) or re-arms (a tap landed mid-dismissal) the
/// request — and bite if any half of it regresses.
@MainActor
final class RecentsSheetLifecycleTests: XCTestCase {

    private final class TickCounter {
        var count = 0
    }

    /// Fresh demo-mode harness per test: an AppModel with an injected
    /// haptic tick counter and a seeded demo fleet. `exitDemo()` restores
    /// the shared identity lifecycle the way the demo-mode tests expect.
    private func makeHarness() -> (model: AppModel, ticks: TickCounter) {
        let ticks = TickCounter()
        let model = AppModel(haptics: { [weak ticks] in ticks?.count += 1 })
        model.enterDemo()
        return (model, ticks)
    }

    private func agent(_ id: String, repo: String? = "corral") -> Agent {
        Agent(agentId: id, state: .working, seq: 1, ts: 100,
              capabilities: ["read_tail"],
              workspace: Workspace(repo: repo, branch: repo.map { "b-" + $0 }),
              displayName: id)
    }

    private func seedLive(_ model: AppModel, _ ids: [String]) {
        model.mode = .live
        model.fleet.apply(.snapshot(Snapshot(
            schemaVersion: 3, rev: 10, generatedAt: 1,
            agents: Dictionary(uniqueKeysWithValues: ids.map {
                ($0, agent($0))
            }))))
    }

    private func dismissalWrite(_ model: AppModel, _ request: RecentsRequest?) {
        // What SwiftUI's `.sheet(item:)` binding does when a dismissal
        // starts: it writes nil through the binding.
        model.recentsRequest = nil
        model.recentsSheetDismissed()
    }

    func testRowRequestAlwaysProducesAFreshMonotonicValue() throws {
        let (model, _) = makeHarness()
        defer { model.exitDemo() }
        let a = DemoFleet.featuredAgentID
        model.requestRecents(for: a, haptic: false)
        let first = try XCTUnwrap(model.recentsRequest)
        XCTAssertEqual(first.agentId, a)
        model.requestRecents(for: a, haptic: false)
        let second = try XCTUnwrap(model.recentsRequest)
        XCTAssertEqual(second.agentId, a)
        XCTAssertGreaterThan(second.id, first.id,
                             "a same-agent re-request must be a NEW value, not an equal one")
    }

    func testSameAgentReopenAfterDismissalWorksOnTheFirstRequest() throws {
        // #364 C2: the exact reported failure — reopen the SAME agent's
        // sheet after dismissing. Three full cycles must each produce a
        // fresh request (nil → request) with a strictly growing id.
        let (model, _) = makeHarness()
        defer { model.exitDemo() }
        let a = DemoFleet.featuredAgentID
        var previousID: UInt64 = 0
        for _ in 0..<3 {
            model.requestRecents(for: a, haptic: false)
            let request = try XCTUnwrap(model.recentsRequest)
            XCTAssertEqual(request.agentId, a)
            XCTAssertGreaterThan(request.id, previousID,
                                 "every reopen must be a new presentation value")
            previousID = request.id
            dismissalWrite(model, request)
            XCTAssertNil(model.recentsRequest,
                         "a clean dismissal must fully clear the request")
        }
    }

    func testCleanDismissalLeavesNothingPending() throws {
        let (model, _) = makeHarness()
        defer { model.exitDemo() }
        model.requestRecents(for: DemoFleet.featuredAgentID, haptic: false)
        dismissalWrite(model, model.recentsRequest)
        XCTAssertNil(model.recentsRequest)
    }

    func testRequestThatLandsDuringDismissalIsReArmed() throws {
        // #364 C1: a tap that lands while the previous sheet is still
        // dismissing is dropped by SwiftUI's presentation coordinator —
        // the dismissal completion must re-arm it so the FIRST tap works.
        let (model, _) = makeHarness()
        defer { model.exitDemo() }
        model.requestRecents(for: DemoFleet.featuredAgentID, haptic: false)
        model.recentsRequest = nil          // dismissal starts
        model.requestRecents(for: "herdr:demo-garden-blocked", haptic: false)
        let pending = try XCTUnwrap(model.recentsRequest)
        model.recentsSheetDismissed()       // dismissal completes
        let rearmed = try XCTUnwrap(model.recentsRequest)
        XCTAssertEqual(rearmed.agentId, "herdr:demo-garden-blocked",
                       "the mid-dismissal request must survive dismissal completion")
        XCTAssertGreaterThan(rearmed.id, pending.id,
                             "the re-arm must be a fresh presentation value")
    }

    func testUnknownAgentRequestIsIgnoredWithoutHaptic() {
        let (model, ticks) = makeHarness()
        defer { model.exitDemo() }
        model.requestRecents(for: "herdr:ghost", haptic: true)
        XCTAssertNil(model.recentsRequest)
        XCTAssertEqual(ticks.count, 0)
    }

    func testRowTapHapticDeepLinkAndDoneWiring() {
        // #364 A.2: row taps tick once; deep links and auto-demo opens do
        // not; the Done close control ticks once.
        let (model, ticks) = makeHarness()
        defer { model.exitDemo() }
        let a = DemoFleet.featuredAgentID
        model.requestRecents(for: a, haptic: true)
        XCTAssertEqual(ticks.count, 1)
        XCTAssertEqual(model.recentsRequest?.agentId, a)

        model.recentsRequest = nil
        model.recentsSheetDismissed()
        model.requestRecents(for: "herdr:demo-garden-blocked", haptic: false)
        XCTAssertEqual(ticks.count, 1, "programmatic opens must stay silent")

        model.closeRecentsButtonTapped()
        XCTAssertEqual(ticks.count, 2, "the Done close control ticks once")
    }

    func testDeepLinkIsLiveModeOnlyAndHapticFree() {
        // openNotification (notification tap) keeps its live-mode +
        // agent-exists guards and never plays a haptic (it is not a row
        // tap). #397: the no-profile-store runtime routes through the
        // legacy fleet store (host-less payload, pure single-host).
        let (model, ticks) = makeHarness()
        defer { model.exitDemo() }
        model.openNotification(agentId: DemoFleet.featuredAgentID, hostKeyB64: nil)
        XCTAssertNil(model.recentsRequest, "demo mode must ignore deep links")
        XCTAssertEqual(ticks.count, 0)

        seedLive(model, ["a1", "a2"])
        model.openNotification(agentId: "a2", hostKeyB64: nil)
        XCTAssertEqual(model.recentsRequest?.agentId, "a2")
        XCTAssertEqual(ticks.count, 0, "deep links never tick")

        model.openNotification(agentId: "ghost", hostKeyB64: nil)
        XCTAssertEqual(model.recentsRequest?.agentId, "a2",
                       "a missing agent must not displace the open request")
        XCTAssertNotNil(model.banner)
    }

    func testRepoFilterSurvivesAFleetRefresh() {
        // #364 B3: the chip choice lives on the model, so a foreground
        // refresh (which replaces the fleet through the store) never
        // resets it. The pure reconcile handles a vanished repo.
        let (model, _) = makeHarness()
        defer { model.exitDemo() }
        seedLive(model, ["a1", "a2"])
        model.repoFilter = "corral"
        model.fleet.apply(.snapshot(Snapshot(
            schemaVersion: 3, rev: 11, generatedAt: 1,
            agents: ["a3": agent("a3"), "a4": agent("a4")])))
        XCTAssertEqual(model.repoFilter, "corral",
                       "refresh must not clear the filter")
        XCTAssertEqual(BoardModel.reconcile(model.repoFilter,
                                            against: BoardModel.repoFilters(
                                                Array(model.fleet.agents.values))),
                       "corral")
    }
}

// MARK: - Recents v1 tail model (#354 L2 → #373 block-per-run)

/// Focused regressions for the #373 BLOCK-PER-RUN model. These bite: the
/// display model must start a block at every role change, merge consecutive
/// same-role runs (so same-tool calls share ONE block with a compact line
/// per call and a growing live tail appends INTO the open block), split a
/// tool run into invocations + inline output, drop divider-only material,
/// cap giant blocks at 20 lines, and add the muted waiting line only to a
/// TRAILING call-only tool run. A change that re-introduces per-call
/// blocks, divider rows, or loses the append-into-open-block behavior fails
/// here before any view code is involved.
final class RecentBlockModelTests: XCTestCase {
    private func block(_ kind: TranscriptBlockKind, _ text: String) -> TranscriptBlock {
        TranscriptBlock(kind: kind, text: text)
    }

    private func pane(_ blocks: [TranscriptBlock]) -> TailPane {
        var pane = TailPane()
        pane.apply(blocks, lines: [])
        return pane
    }

    func testRoleBoundaryStartsANewBlock() {
        let blocks = RecentOutputModel.displayBlocks(from: pane([
            block(.user, "u1"),
            block(.agent, "a1"),
            block(.user, "u2"),
        ]))
        XCTAssertEqual(blocks.map(\.kind), [.user, .agent, .user],
                       "a role change starts a new block")
        XCTAssertEqual(blocks.map(\.id), ["rb-0", "rb-1", "rb-2"])
    }

    func testConsecutiveSameRoleBlocksMergeIntoOneRun() {
        let blocks = RecentOutputModel.displayBlocks(from: pane([
            block(.agent, "first paragraph"),
            block(.agent, "second paragraph"),
            block(.user, "u1"),
        ]))
        XCTAssertEqual(blocks.map(\.kind), [.agent, .user])
        XCTAssertEqual(blocks[0].rows.map(\.text),
                       ["first paragraph", "second paragraph"],
                       "same-role adjacency stays ONE block with one row per line")
        XCTAssertEqual(blocks[0].rows.map(\.kind),
                       [.prose, .prose])
    }

    func testConsecutiveSameToolCallsShareOneBlockWithCompactLines() {
        // Two adjacent tool runs (same tool family) merge into ONE block
        // with one compact call line per invocation — the #373 grouping.
        // A trailing user block keeps the merged run off the tail so the
        // waiting line cannot attach (that behavior has its own tests).
        let blocks = RecentOutputModel.displayBlocks(from: pane([
            block(.tool, "$ git status --short"),
            block(.tool, "$ git log --oneline -3"),
            block(.user, "thanks"),
        ]))
        XCTAssertEqual(blocks.count, 2)
        XCTAssertEqual(blocks[0].kind, .tool)
        XCTAssertEqual(blocks[0].rows.map(\.kind), [.call, .call],
                       "consecutive same-tool calls share ONE block")
        XCTAssertEqual(blocks[0].rows.map(\.text),
                       ["$ git status --short", "$ git log --oneline -3"],
                       "each invocation stays a compact single line")
        XCTAssertFalse(blocks[0].rows.contains { $0.kind == .waiting })
    }

    func testRoleChangesBetweenToolRunsKeepSeparateBlocks() {
        let blocks = RecentOutputModel.displayBlocks(from: pane([
            block(.tool, "$ git status --short"),
            block(.agent, "prose between runs"),
            block(.tool, "$ cargo test"),
        ]))
        XCTAssertEqual(blocks.map(\.kind), [.tool, .agent, .tool],
                       "a role change between tool runs starts a new block")
    }

    func testToolRowsSplitShellEchoesFromOutput() {
        let blocks = RecentOutputModel.displayBlocks(from: pane([
            block(.tool, "$ git status --short\n M src/retry.ts"),
        ]))
        XCTAssertEqual(blocks[0].rows.map(\.kind), [.call, .output])
        XCTAssertEqual(blocks[0].rows.map(\.text),
                       ["$ git status --short", " M src/retry.ts"])
        XCTAssertEqual(blocks[0].tool, .terminal)
    }

    func testMultipleCallsWithInterleavedOutputKeepOneCompactLinePerCall() {
        let blocks = RecentOutputModel.displayBlocks(from: pane([
            block(.tool, "$ pnpm vitest run\n ✓ 9 passed\n$ pnpm lint"),
        ]))
        XCTAssertEqual(blocks[0].rows.map(\.kind), [.call, .output, .call])
    }

    func testBareFirstLineToolCallIsClassifiedAsCallWithDocIcon() {
        let blocks = RecentOutputModel.displayBlocks(from: pane([
            block(.tool, "read_file src/retry.ts  lines 1-18\n  1  export function withRetry(fn) {"),
        ]))
        XCTAssertEqual(blocks[0].rows.first?.kind, .call,
                       "a bare tool-invocation first line is a call, not output")
        XCTAssertEqual(blocks[0].rows.first?.text,
                       "read_file src/retry.ts  lines 1-18")
        XCTAssertEqual(blocks[0].rows.dropFirst().first?.kind, .output)
        XCTAssertEqual(blocks[0].tool, .doc)
    }

    func testUnrecognizedFirstLineIsOutputNotCall() {
        let blocks = RecentOutputModel.displayBlocks(from: pane([
            block(.tool, "Compiling corrald v0.1.0\nBuild finished"),
        ]))
        XCTAssertEqual(blocks[0].rows.map(\.kind), [.output, .output],
                       "an unclassified first line is never mislabeled as a call")
        XCTAssertEqual(blocks[0].tool, .generic,
                       "a run with no recognizable invocation falls back to the generic icon")
    }

    func testToolIconVocabularyIsLockedPerCommandWord() {
        XCTAssertEqual(RecentOutputModel.toolKind(forCallLine: "$ git status"), .terminal)
        XCTAssertEqual(RecentOutputModel.toolKind(forCallLine: "$ pnpm vitest run"), .terminal)
        XCTAssertEqual(RecentOutputModel.toolKind(forCallLine: "$ rg -n withRetry src/"), .search)
        XCTAssertEqual(RecentOutputModel.toolKind(forCallLine: "$ grep -rn withRetry src/"), .search)
        XCTAssertEqual(RecentOutputModel.toolKind(forCallLine: "read_file src/retry.ts"), .doc)
        XCTAssertEqual(RecentOutputModel.toolKind(forCallLine: "write_file src/retry.ts"), .doc)
        XCTAssertEqual(RecentOutputModel.toolKind(forCallLine: "edit src/retry.ts"), .code)
        XCTAssertEqual(RecentOutputModel.toolKind(forCallLine: "apply_patch src/retry.ts"), .code)
        XCTAssertEqual(RecentOutputModel.toolKind(forCallLine: "$"), .generic)
    }

    func testInteriorBlankOutputLinesAreKeptForSpacing() {
        let blocks = RecentOutputModel.displayBlocks(from: pane([
            block(.tool, "$ pnpm test\n RUN\n\n PASS"),
        ]))
        XCTAssertEqual(blocks[0].rows.map(\.text), ["$ pnpm test", " RUN", "", " PASS"],
                       "blank lines inside output stay as spacing rows")
        let trimmed = RecentOutputModel.displayBlocks(from: pane([
            block(.tool, "\n\n$ pnpm test\n PASS\n\n"),
        ]))
        XCTAssertEqual(trimmed[0].rows.map(\.text), ["$ pnpm test", " PASS"],
                       "leading/trailing blanks never render")
    }

    func testLegacyLinesBecomeOneUnknownBlock() {
        var legacy = TailPane()
        legacy.lines = ["raw line one", "raw line two"]
        let blocks = RecentOutputModel.displayBlocks(from: legacy)
        XCTAssertEqual(blocks.count, 1)
        XCTAssertEqual(blocks[0].kind, .unknown)
        XCTAssertEqual(blocks[0].rows.map(\.kind), [.output, .output])
        XCTAssertEqual(blocks[0].rows.map(\.text), ["raw line one", "raw line two"])
    }

    func testDividerOnlyRowsDropEntirelyAndNeverBreakRuns() {
        let blocks = RecentOutputModel.displayBlocks(from: pane([
            block(.agent, "a"),
            block(.system, "────────────────────────────────"),
            block(.agent, "b"),
            block(.tool, "──"),
        ]))
        XCTAssertEqual(blocks.map(\.kind), [.agent])
        XCTAssertEqual(blocks[0].rows.map(\.text), ["a", "b"],
                       "dropped dividers never split or ride inside a role run")
    }

    func testWaitingLineAppendsToTrailingCallOnlyToolRun() {
        let blocks = RecentOutputModel.displayBlocks(from: pane([
            block(.user, "u"),
            block(.tool, "$ rg -n withRetry src/"),
        ]))
        XCTAssertEqual(blocks.map(\.kind), [.user, .tool])
        XCTAssertEqual(blocks[1].rows.map(\.kind), [.call, .waiting])
        XCTAssertEqual(blocks[1].rows.last?.text, RecentOutputModel.waitingRowText)
    }

    func testWaitingLineNeverAppearsWhenOutputExists() {
        let blocks = RecentOutputModel.displayBlocks(from: pane([
            block(.tool, "$ rg -n withRetry src/\nsrc/retry.ts:12:withRetry(post)"),
        ]))
        XCTAssertEqual(blocks[0].rows.map(\.kind), [.call, .output],
                       "a run with output never shows the waiting line")
    }

    func testWaitingLineOnlyOnTheTrailingRun() {
        let blocks = RecentOutputModel.displayBlocks(from: pane([
            block(.tool, "$ pnpm lint"),
            block(.agent, "prose after the quiet run"),
        ]))
        XCTAssertEqual(blocks.map(\.kind), [.tool, .agent])
        XCTAssertFalse(blocks[0].rows.contains { $0.kind == .waiting },
                       "a mid-stream call-only run is not 'waiting' — it is history")
    }

    func testLineCapHidesRowsBeyondTwentyWithShowAllCount() {
        let text = "$ pnpm vitest run\n" + (1...23).map { "out \($0)" }.joined(separator: "\n")
        let blocks = RecentOutputModel.displayBlocks(from: pane([
            block(.tool, text),
        ]))
        XCTAssertEqual(RecentOutputModel.lineCap, 20)
        XCTAssertEqual(blocks[0].rows.count, 24)
        XCTAssertEqual(blocks[0].cappedLineCount, 4,
                       "the block must report exactly the hidden-by-cap line count")
        let short = RecentOutputModel.displayBlocks(from: pane([
            block(.tool, "$ pnpm vitest run\n" + (1...18).map { "out \($0)" }.joined(separator: "\n")),
        ]))
        XCTAssertEqual(short[0].cappedLineCount, 0)
    }

    func testLiveAppendGrowsTheOpenBlockNotANewOne() {
        // A live tail grows between fetches and the daemon can re-segment
        // the run (a blank line splits canonical tool blocks), so the new
        // content can arrive as a SEPARATE same-kind canonical block. The
        // display model must merge it into the CURRENT semantic block
        // (stable id, appended rows) instead of stacking a duplicate.
        let before = RecentOutputModel.displayBlocks(from: pane([
            block(.user, "u"),
            block(.tool, "$ pnpm vitest run\n RUN v2.1.4"),
        ]))
        XCTAssertEqual(before.count, 2)
        let after = RecentOutputModel.displayBlocks(from: pane([
            block(.user, "u"),
            block(.tool, "$ pnpm vitest run\n RUN v2.1.4"),
            block(.tool, " ✓ 9 passed"),
        ]))
        XCTAssertEqual(after.count, 2, "growth appends INTO the current semantic block")
        XCTAssertEqual(before[1].id, after[1].id,
                       "the open block's identity must stay stable across appends")
        XCTAssertEqual(before[1].rows.count, 2)
        XCTAssertEqual(after[1].rows.count, 3)
        XCTAssertEqual(after[1].rows.last?.text, " ✓ 9 passed")
    }

    func testNewRoleAfterGrowthStartsANewBlockWithoutRenumbering() {
        let grown = RecentOutputModel.displayBlocks(from: pane([
            block(.user, "u"),
            block(.tool, "$ pnpm vitest run\n RUN v2.1.4"),
            block(.tool, " ✓ 9 passed"),
            block(.agent, "done — pushing."),
        ]))
        XCTAssertEqual(grown.map(\.id), ["rb-0", "rb-1", "rb-2"])
        XCTAssertEqual(grown[1].rows.count, 3)
        XCTAssertEqual(grown[2].rows.map(\.text), ["done — pushing."])
    }

    func testPhaseDerivation() {
        XCTAssertEqual(RecentOutputModel.phase(for: nil), .empty)
        let pane = TailPane()
        var loading = pane
        loading.beginFetch()
        XCTAssertEqual(RecentOutputModel.phase(for: loading), .loading)
        var loaded = pane
        loaded.apply([block(.agent, "x")], lines: ["x"])
        XCTAssertEqual(RecentOutputModel.phase(for: loaded), .loaded)
        var failed = pane
        failed.apply(TranscriptFailure(kind: "not_granted", message: "read_tail", candidates: []))
        guard case .error(let failure) = RecentOutputModel.phase(for: failed) else {
            return XCTFail("expected error phase")
        }
        XCTAssertEqual(failure.kind, "not_granted")
    }

    func testDividerClassificationStillClassifies() {
        XCTAssertTrue(RecentOutputRender.isDividerBlock(block(.system, "──────")))
        XCTAssertFalse(RecentOutputRender.isDividerBlock(block(.system, "let sep = \"────\";")))
    }

    func testTranscriptErrorTextNamesTheMissingGrant() {
        let text = TranscriptText.errorText(
            TranscriptFailure(kind: "not_granted", message: "capability not granted: read_tail", candidates: []))
        XCTAssertTrue(text.contains("read_tail grant"), text)
    }

    func testAccessibilityLabelsRemainAttributable() {
        XCTAssertEqual(RecentBlockStyle.roleName(for: .user), "You")
        XCTAssertEqual(RecentBlockStyle.roleName(for: .agent), "Assistant")
        XCTAssertEqual(RecentBlockStyle.roleName(for: .tool), "Tool run")
        XCTAssertEqual(RecentBlockStyle.roleName(for: .system), "Status")
        XCTAssertEqual(RecentBlockStyle.roleName(for: .unknown), "Unknown activity")
    }
}

// MARK: - #373 block-per-run session state (default-expanded + toggle)

/// The sheet-session collapse/reveal contract: fresh sessions are ALL
/// EXPANDED, toggling is per-block, "Show all" reveals once, and reset
/// (dismissal) clears everything — nothing outlives the sheet session.
@MainActor
final class RecentsBlockSessionTests: XCTestCase {
    func testSessionDefaultsToEverythingExpanded() {
        let session = RecentsSheetSession()
        XCTAssertTrue(session.collapsed.isEmpty,
                      "every block DEFAULTS EXPANDED — a fresh session collapses nothing")
        XCTAssertTrue(session.revealed.isEmpty)
    }

    func testToggleCollapsesAndExpandsPerBlock() {
        let session = RecentsSheetSession()
        session.toggleCollapsed("rb-0")
        XCTAssertEqual(session.collapsed, ["rb-0"])
        session.toggleCollapsed("rb-1")
        XCTAssertEqual(session.collapsed, ["rb-0", "rb-1"])
        session.toggleCollapsed("rb-0")
        XCTAssertEqual(session.collapsed, ["rb-1"],
                       "toggling the same block again expands it (per-block, independent)")
    }

    func testRevealIsOneShotPerSession() {
        let session = RecentsSheetSession()
        session.reveal("rb-3")
        XCTAssertEqual(session.revealed, ["rb-3"])
        session.reveal("rb-3")
        XCTAssertEqual(session.revealed, ["rb-3"], "reveal is idempotent")
    }

    func testResetClearsCollapseAndRevealOnDismissal() {
        let session = RecentsSheetSession()
        session.toggleCollapsed("rb-0")
        session.reveal("rb-3")
        session.reset()
        XCTAssertTrue(session.collapsed.isEmpty)
        XCTAssertTrue(session.revealed.isEmpty,
                      "the next sheet session must start fully expanded again")
    }
}
// MARK: - Read-only surface wiring (FleetViews source bundle)

/// Decoy-resistant source-wiring regression: the recents sheet must be fed
/// by the LIVE read_tail drive seam (not a cached/demo-only projection), and
/// the board must project through `BoardModel.sections` (raw status sections
/// — blocked first, repo never a grouping key). Production source rides in
/// the test bundle as `FleetViews.swift.txt` via the test target's
/// preBuildScript.
final class ReadOnlySurfaceWiringTests: XCTestCase {

    private func bundledSource() throws -> String {
        let bundle = Bundle(for: ReadOnlySurfaceWiringTests.self)
        let url = try XCTUnwrap(bundle.url(forResource: "FleetViews", withExtension: "swift.txt"))
        return try String(contentsOf: url, encoding: .utf8)
    }

    func testRecentsSheetCallsTheLiveReadTailDrive() throws {
        let source = try bundledSource()

        // Exactly ONE recents-sheet declaration.
        XCTAssertEqual(source.components(separatedBy: "struct RecentOutputSheet:").count - 1, 1)
        // The sheet body must live-drive the tail. Scope to the sheet so an
        // unrelated helper cannot satisfy the search.
        let sheetMarker = "struct RecentOutputSheet: View {"
        guard let sheetStart = source.range(of: sheetMarker) else {
            return XCTFail("RecentOutputSheet declaration not found")
        }
        let nextDecl = source.range(of: "\n// MARK: - Recents block renderer")
        let sliceEnd = try XCTUnwrap(nextDecl?.lowerBound,
                                     "the recents block renderer MARK must follow the sheet source")
        let slice = String(source[sheetStart.lowerBound..<sliceEnd])
        XCTAssertTrue(slice.contains("model.driveReadTail(agent: agent, hostProfileID: hostProfileID,"),
                      "the recents sheet must auto-refresh through the live read_tail drive, carrying the composite identity")
        XCTAssertTrue(slice.contains("RecentOutputModel.phase(for: tail)"),
                      "the recents sheet must render the tail pane's four-state machine")
        XCTAssertTrue(slice.contains("RecentOutputModel.displayBlocks(from: tail)"),
                      "the recents sheet must render the #373 block-per-run display model")
        XCTAssertTrue(slice.contains("RecentRunBlockView(block: block)"),
                      "the sheet must feed each display block to the block renderer")
        XCTAssertTrue(slice.contains("@StateObject private var session = RecentsSheetSession()"),
                      "the sheet must own its per-session collapse/reveal state")
        // The sheet renders ZERO role/card chrome vocabulary from the
        // rejected rail era (a decoy elsewhere in FleetViews cannot satisfy
        // a slice-scoped assertion).
        for chrome in ["speakerRail", "roleLabel", "showSpeaker",
                       "DisclosureGroup", "RecentBlockRow", "userTint"] {
            XCTAssertFalse(slice.contains(chrome),
                           "role/card chrome \(chrome) must not be wired in the recents sheet")
        }
    }

    /// #373 block renderer wiring pins (decoy-resistant over the bundled
    /// FleetViews source): the block header is the WHOLE-width collapse
    /// toggle, a collapsed block hides its body and rotates the chevron, a
    /// giant block (> lineCap) offers the inline "Show all" reveal, and the
    /// output rows render inside the theme's output-panel token. A
    /// compile-capable bypass of any of those call sites goes RED here.
    func testRecentsBlocksRenderThroughTheBlockModel() throws {
        let source = try bundledSource()
        let start = source.range(of: "\nprivate struct RecentRunBlockView: View {")
        let end = source.range(of: "\nprivate struct RecentOutputLineView")
        let startIndex = try XCTUnwrap(start?.lowerBound,
                                       "the block renderer declaration must exist")
        let endIndex = try XCTUnwrap(end?.lowerBound,
                                     "the output line view declaration must exist")
        let slice = String(source[startIndex..<endIndex])
        // The WHOLE header is the collapse toggle (no separate chevron tap).
        XCTAssertTrue(slice.contains("session.toggleCollapsed(block.id)"),
                      "tapping the block header must toggle that block's collapse")
        XCTAssertTrue(slice.contains("if !isCollapsed {"),
                      "a collapsed block must hide its body")
        XCTAssertTrue(slice.contains("rotationEffect(.degrees(isCollapsed ? -90 : 0))"),
                      "the chevron must rotate when the block collapses")
        XCTAssertTrue(slice.contains("if isCollapsed {\n                    previewLine"),
                      "a collapsed block must reveal its one-line preview")
        XCTAssertTrue(slice.contains(".frame(maxWidth: .infinity, minHeight: 34, alignment: .leading)"),
                      "the header hit target must keep its min-height guard")
        // Per-block 20-line cap + inline Show all.
        XCTAssertTrue(slice.contains("RecentOutputModel.lineCap"),
                      "the block body must respect the locked 20-line cap")
        XCTAssertTrue(slice.contains("block.cappedLineCount > 0"),
                      "the Show all control must exist only for capped blocks")
        XCTAssertTrue(slice.contains("session.reveal(block.id)"),
                      "Show all must reveal the capped tail inline")
        // Output rows ride the theme's output-panel token (the recess rule).
        XCTAssertTrue(slice.contains("theme.tailBackground"),
                      "output rows must render on the themed output panel")
        // Zero role words / rejected rail chrome inside the renderer.
        for chrome in ["roleLabel", "speakerRail", "DisclosureGroup",
                       "userTint", "toolSummary", "RecentRailSpine",
                       "showsTransitionMarker", "RecentDiamond"] {
            XCTAssertFalse(slice.contains(chrome),
                           "chrome \(chrome) must not be in the block renderer")
        }
    }

    func testBoardProjectsThroughRepoSubgroupSections() throws {
        let source = try bundledSource()
        // The FleetView board must be the #371 board-v2 projection: locked
        // status sections whose rows live in always-open repo subgroups.
        // The #364 B chips filter WHICH agents the sections bucket through
        // the pure BoardModel projections; #371 splits each bucket into
        // repo subgroups.
        XCTAssertTrue(source.contains("let chips = BoardModel.repoFilters(agents)"),
                      "board must project the #364 B chip set from the fleet")
        XCTAssertTrue(source.contains("BoardModel.reconcile(model.repoFilter,"),
                      "board must reconcile the chip choice against the live fleet")
        XCTAssertTrue(source.contains("BoardModel.agents(agents, in: activeRepoFilter)"),
                      "board must filter agents through the pure projection")
        XCTAssertTrue(source.contains("BoardModel.sections(\n            BoardModel.agents"),
                      "board must bucket the FILTERED agents into the locked status sections")
        // Scope to the board renderer so an unrelated helper cannot satisfy
        // the search (decoy-resistant: unique declaration marker).
        let boardMarker = "private func boardSections(sections: BoardModel.Sections"
        XCTAssertEqual(source.components(separatedBy: boardMarker).count - 1, 1,
                       "exactly one boardSections renderer must exist")
        guard let boardStart = source.range(of: boardMarker) else {
            return XCTFail("boardSections declaration not found")
        }
        let sliceEnd = try XCTUnwrap(source.range(of: "\n    @ViewBuilder\n    private func agentRow")?.lowerBound,
                                     "agentRow declaration must follow boardSections")
        let slice = String(source[boardStart.lowerBound..<sliceEnd])
        // Renders one section per status bucket, then per-subgroup bands
        // and their rows — no flat per-status row list remains.
        XCTAssertTrue(slice.contains("ForEach(sections.statuses)"),
                      "the board must render the status-section projection")
        XCTAssertTrue(slice.contains("ForEach(status.subgroups)"),
                      "every status section must render its repo subgroups")
        XCTAssertTrue(slice.contains("ForEach(subgroup.agents)"),
                      "every subgroup must render its agent rows")
        XCTAssertFalse(slice.contains("ForEach(status.agents)"),
                       "the flat per-status row loop is gone — #371 groups by repo")
        XCTAssertEqual(slice.components(separatedBy: "repoSubgroupHeader(subgroup, repos: repos)").count - 1, 1,
                       "the subgroup band must be wired into every status section")
        // Status headers carry the raw name + TOTAL count through the
        // shared state-color mapping (mark square, never color-only).
        XCTAssertTrue(slice.contains("Text(status.header)"),
                      "section headers must show the raw status name + total")
        XCTAssertTrue(slice.contains("theme.stateColor(for: status.state)"),
                      "section header marks must consume the shared state mapping")
        // Row taps open recents through the model-owned request (the same
        // funnel deep links and the demo route use), and the sheet is fed
        // straight from that request.
        XCTAssertTrue(source.contains("model.requestRecents(for: agent.agentId, haptic: true)"))
        XCTAssertTrue(source.contains(".sheet(item: $model.recentsRequest,"))
        XCTAssertTrue(source.contains("RecentOutputSheet(agentId: request.agentId,"),
                      "the sheet must receive the request's composite identity")
        XCTAssertTrue(source.contains("hostProfileID: request.hostProfileID,"),
                      "the request's host profile must ride into the sheet")
        XCTAssertTrue(source.contains("onDismiss: { model.recentsSheetDismissed() }"),
                      "dismissal must run the request-lifecycle reconciler")
    }

    /// #364 wiring pin: touch feedback, haptics, chips, and the reopen
    /// lifecycle must all be wired in FleetViews (visual feel stays a
    /// device-side claim; this pins the surface + call sites).
    func test364BoardUXSurfacesAreWired() throws {
        let source = try bundledSource()
        // A.1 pressed-state style exists and rows/chips/banner use it.
        XCTAssertEqual(source.components(separatedBy: "struct BoardPressStyle:").count - 1, 1,
                       "exactly one press style must exist")
        XCTAssertTrue(source.contains("configuration.isPressed"),
                      "the press style must key off touch-down state")
        XCTAssertGreaterThanOrEqual(
            source.components(separatedBy: ".buttonStyle(BoardPressStyle())").count - 1, 3,
            "rows, chips, and the banner close must use the press style")
        // A.2 haptic seam: one selection generator call site, discrete
        // actions only (row tap + Done close).
        XCTAssertEqual(source.components(separatedBy: "UISelectionFeedbackGenerator().selectionChanged()").count - 1, 1,
                       "exactly one haptic call site must exist")
        XCTAssertTrue(source.contains("model.closeRecentsButtonTapped()"),
                      "the sheet Done control must tick on close")
        // A.3: chip hit targets stay >= 44 pt.
        XCTAssertTrue(source.contains(".frame(minHeight: 44)"),
                      "chip hit targets must be >= 44 pt")
        // B: chip row surfaces with the model-owned filter and selected
        // state + VoiceOver selected trait.
        XCTAssertTrue(source.contains("repoChipsRow(chips: chips,"))
        XCTAssertTrue(source.contains("model.repoFilter = chip.repo"))
        XCTAssertTrue(source.contains("accessibilityAddTraits(isSelected ? [.isSelected] : [])"))
        // C: dismissal reconciles the request (reopen lifecycle) — the
        // reconciler itself lives on the model and is pinned by
        // RecentsSheetLifecycleTests; here we pin the view call site.
        XCTAssertTrue(source.contains("onDismiss: { model.recentsSheetDismissed() })"),
                      "the sheet dismissal must run the model reconciler")
    }

    func testRemovedSurfacesAreAbsentFromTheBoardSource() throws {
        let source = try bundledSource()
        for removed in ["AgentDiffSheet", "IssuesBrowserView", "DevicesGrantsView",
                        "AnswerPromptSheet", "TerminalAttachView", "PromptDrafts",
                        "RecentOutputSections", "filterChipRow", "FleetSearchable",
                        "swipeActions", "CannedButtons", "ClaimCard",
                        "RecentBlockRow", "speakerRail", "roleLabel",
                        // #373: the REJECTED #361 continuous-rail vocabulary
                        // is gone from FleetViews entirely.
                        "RecentRailRowView", "RecentRailSpine", "RecentDiamond",
                        "RecentCodeLineView", "railRows", "RailMarker",
                        "showsTransitionMarker", "RecentOutputModel.RailRow"] {
            XCTAssertFalse(source.contains(removed),
                           "removed surface \(removed) must not be wired in FleetViews")
        }
    }
}

// MARK: - #371 board-v2 wiring (subgroup bands, row chips, working motion)

/// Pins the board-v2 SURFACE WIRING in the bundled FleetViews source (the
/// #316 decoy-resistant mechanism): the tinted state chip consuming the
/// shared state mapping, the working heartbeat + Reduce Motion static dot
/// inside that chip (never a spinner), the repo subgroup bands + row repo
/// chips resolving hues from the shared RepoHue function, and the DEMOTED
/// repo subgroup captions (#386) staying visible under the status sections
/// — while the status sections' own THICK collapsible bars (status +
/// TOTAL, chevron, session collapse state) are pinned by
/// StatusSectionCollapseWiringTests (#386). A compile-capable bypass of
/// any of those call sites goes RED here.
final class BoardV2WiringTests: XCTestCase {

    private func bundledSource() throws -> String {
        let bundle = Bundle(for: BoardV2WiringTests.self)
        let url = try XCTUnwrap(bundle.url(forResource: "FleetViews",
                                           withExtension: "swift.txt"))
        return try String(contentsOf: url, encoding: .utf8)
    }

    private func slice(from startMarker: String, to endMarker: String,
                       in source: String) throws -> String {
        let start = try XCTUnwrap(source.range(of: startMarker),
                                  "marker not found: \(startMarker)")
        let end = try XCTUnwrap(source.range(of: endMarker),
                                "end marker not found: \(endMarker)")
        return String(source[start.lowerBound..<end.lowerBound])
    }

    func testWorkingChipBreathesThreeSquaresOrShowsTheStaticDot() throws {
        let source = try bundledSource()
        // The glyph view: Reduce Motion removes the squares and shows a
        // static teal dot; otherwise three squares breathe in stagger on a
        // TimelineView(.animation) schedule.
        let glyph = try slice(from: "struct WorkingMotionGlyph: View {",
                              to: "struct RepoLabelChip: View {",
                              in: source)
        XCTAssertTrue(glyph.contains("let reduceMotion: Bool"),
                      "the working glyph must take the Reduce Motion flag")
        XCTAssertTrue(glyph.contains("Circle()"),
                      "the Reduce Motion fallback must be a STATIC dot")
        XCTAssertTrue(glyph.contains("theme.stateColor(for: .working)"),
                      "the dot/squares must use the shared state color (teal)")
        XCTAssertTrue(glyph.contains("TimelineView(.animation(minimumInterval: 1.0 / 30.0))"),
                      "the squares must animate on a visible timeline")
        XCTAssertTrue(glyph.contains("WorkingMotion.opacity(at: t, square: index)"),
                      "square opacity must ride the shared breathing math")
        XCTAssertTrue(glyph.contains("WorkingMotion.scale(at: t, square: index)"),
                      "square scale must ride the shared breathing math")
        XCTAssertTrue(glyph.contains("ForEach(0..<WorkingMotion.squareCount"),
                      "the working glyph must render exactly the three squares")
        XCTAssertFalse(glyph.contains("ProgressView"),
                       "no spinner may be used for the working state")
        // AgentRow wires the glyph into the working chip branch only.
        let row = try slice(from: "struct AgentRow: View {",
                            to: "// MARK: - Row accessibility",
                            in: source)
        XCTAssertEqual(row.components(separatedBy: "WorkingMotionGlyph(reduceMotion: theme.reduceMotion)").count - 1, 1,
                       "the working chip must consume the motion glyph once")
        XCTAssertTrue(row.contains("case .working:"),
                      "the glyph swap must branch on the working state")
        XCTAssertFalse(row.contains("ProgressView"),
                       "agent rows carry no spinner anywhere")
    }

    func testStateChipIsTintedThroughTheSingleSharedMapping() throws {
        let source = try bundledSource()
        let row = try slice(from: "struct AgentRow: View {",
                            to: "// MARK: - Row accessibility",
                            in: source)
        XCTAssertTrue(row.contains("theme.stateColor(for: agent.state)"),
                      "chip glyph/label ink must consume the shared state mapping")
        XCTAssertTrue(row.contains("theme.stateChipFill(for: agent.state)"),
                      "the chip fill must resolve through the state mapping")
        XCTAssertTrue(row.contains("theme.stateChipBorder(for: agent.state)"),
                      "the chip border must resolve through the state mapping")
        XCTAssertTrue(row.contains("TimeInStateLabel(agent: agent, stateEnteredAt: stateEnteredAt)"),
                      "the row keeps time-in-state inside the state chip")
    }

    func testRepoSubgroupsAndRowChipsConsumeTheSharedRepoHue() throws {
        let source = try bundledSource()
        // #386: status sections are COLLAPSIBLE now — their thick toggle
        // bar (chevron, collapse state, surface1) is pinned by
        // StatusSectionCollapseWiringTests. These pins scope to the
        // SUBGROUP CAPTION ONLY, which stays non-collapsible and DEMOTED:
        // hue resolved from the SAME fleet repo set the chips row uses,
        // rendered on the band tokens with the small secondary caption
        // type (never the status bar's headline tier).
        let renderer = try slice(from: "private func repoSubgroupHeader(",
                                 to: "\n    @ViewBuilder\n    private func agentRow",
                                 in: source)
        XCTAssertEqual(renderer.components(separatedBy: "private func repoSubgroupHeader(").count - 1, 1,
                       "exactly one subgroup caption builder must exist")
        XCTAssertTrue(renderer.contains("theme.repoHue(for: subgroup.repo"),
                      "subgroup captions must consume the shared RepoHue function")
        XCTAssertTrue(renderer.contains("theme.repoBand(for: hue)"),
                      "subgroup captions must use the hue-over-mantle band token")
        XCTAssertTrue(renderer.contains("subgroup.agents.count"),
                      "subgroup captions must carry their count")
        XCTAssertTrue(renderer.contains("subgroup.displayName"),
                      "repo identity is never color-only — the name always renders")
        XCTAssertTrue(renderer.contains(".font(.caption2.weight(.semibold))"),
                      "the repo name must render in the DEMOTED caption2 tier (#386)")
        XCTAssertTrue(renderer.contains("theme.subtext1"),
                      "the repo name must render in secondary ink, not label ink (#386)")
        XCTAssertFalse(renderer.contains("theme.surface1"),
                       "subgroup captions must not paint like the thick status bar (#386)")
        XCTAssertFalse(renderer.contains("DisclosureGroup"),
                       "subgroups are always open — never collapsible")
        XCTAssertFalse(renderer.contains("sectionCollapse"),
                       "subgroup captions must never read the status-bar collapse state")
        XCTAssertFalse(renderer.contains("chevron"),
                       "no chevron affordance may hint the subgroup is collapsible")

        // Row chip: WorkspaceLine renders the repo as a colored label chip
        // (Other for orphans) with the SAME hue + ink helpers.
        let chip = try slice(from: "struct RepoLabelChip: View {",
                             to: "struct AgentRow: View {",
                             in: source)
        XCTAssertTrue(chip.contains("let hue = theme.repoHue(for: repo ?? \"\", among: repos)"),
                      "the row repo chip must consume the shared RepoHue function")
        XCTAssertTrue(chip.contains("theme.repoChipFill(for: hue)"),
                      "the repo chip fill must use the hue-over-base tint")
        XCTAssertTrue(chip.contains("theme.repoChipBorder(for: hue)"),
                      "the repo chip border must use the hue-over-base tint")
        XCTAssertTrue(chip.contains("theme.repoInk(for: hue)"),
                      "the repo name ink must use the locked label ink")
        XCTAssertTrue(chip.contains("BoardModel.otherRepoLabel"),
                      "orphan rows must carry the Other chip, never vanish")
        let line = try slice(from: "struct WorkspaceLine: View {",
                             to: "// MARK: - #364 A touch feedback",
                             in: source)
        XCTAssertEqual(line.components(separatedBy: "RepoLabelChip(repo: w.repo, repos: repos)").count - 1, 1,
                       "the row's workspace line must render exactly one repo chip")
        XCTAssertTrue(line.contains("let w = agent.workspace"),
                      "the workspace line must keep its per-segment layout")
    }
}

// MARK: - #386 status-bar collapse wiring (thick bars, demoted captions)

/// Decoy-resistant source wiring over the bundled FleetViews source (the
/// #316 mechanism): the #386 status sections render as THICK collapsible
/// bars — default EXPANDED via the board-session-only `sectionCollapse`
/// state, the WHOLE bar the toggle (chevron rotates), a collapsed section
/// hides its subgroups/rows but keeps its bar + counts — while the repo
/// subgroup captions below stay visible, DEMOTED (caption2/subtext1), and
/// non-collapsible. A compile-capable bypass of any production call site
/// (a bar that does not read/toggle the state, a subgroup caption that
/// paints like a status bar) goes RED here.
final class StatusSectionCollapseWiringTests: XCTestCase {

    private func bundledSource() throws -> String {
        let bundle = Bundle(for: StatusSectionCollapseWiringTests.self)
        let url = try XCTUnwrap(bundle.url(forResource: "FleetViews",
                                           withExtension: "swift.txt"))
        return try String(contentsOf: url, encoding: .utf8)
    }

    private func slice(from startMarker: String, to endMarker: String,
                       in source: String) throws -> String {
        let start = try XCTUnwrap(source.range(of: startMarker),
                                  "marker not found: \(startMarker)")
        let end = try XCTUnwrap(source.range(of: endMarker),
                                "end marker not found: \(endMarker)")
        return String(source[start.lowerBound..<end.lowerBound])
    }

    func testBoardOwnsOneSessionOnlyCollapseStateDefaultExpanded() throws {
        let source = try bundledSource()
        let boardStart = try XCTUnwrap(source.range(of: "\nstruct FleetView: View {"),
                                       "FleetView declaration must exist")
        let boardEnd = try XCTUnwrap(source.range(of: "\n// MARK: - Banner"),
                                     "the banner section must follow FleetView")
        let slice = String(source[boardStart.lowerBound..<boardEnd.lowerBound])
        // The collapse state is a VIEW-owned @State over the pure
        // BoardModel.StatusSectionCollapse (fresh == all expanded, per
        // BoardStatusSectionCollapseTests) — never a model/persisted
        // property, so it can only live for the board session.
        XCTAssertEqual(slice.components(separatedBy:
            "@State private var sectionCollapse = BoardModel.StatusSectionCollapse()").count - 1, 1,
            "FleetView must own exactly one session collapse state, defaulting to fresh/expanded")
        XCTAssertFalse(slice.contains("UserDefaults"),
                       "collapse state must never be persisted")
    }

    func testExpandedSectionsRenderTheirSubgroupsOnlyThroughTheState() throws {
        let source = try bundledSource()
        let renderer = try slice(from: "private func boardSections(sections: BoardModel.Sections",
                                 to: "private func repoSubgroupHeader(",
                                 in: source)
        // The status bar is the section's header (full-bleed pinned bar).
        XCTAssertTrue(renderer.contains("PinnedHeader(fillsInteractiveWidth: true)"),
                      "the status bar must be the full-width pinned header")
        XCTAssertTrue(renderer.contains("statusSectionBar(status)"),
                      "every status section must render through the #386 bar builder")
        // Subgroups + rows render ONLY while the section is expanded.
        XCTAssertTrue(renderer.contains("if !sectionCollapse.isCollapsed(status.state) {"),
                      "a collapsed section must hide its subgroups and rows")
        XCTAssertTrue(renderer.contains("ForEach(status.subgroups)"),
                      "an expanded section still renders its repo subgroups")
        XCTAssertTrue(renderer.contains("ForEach(subgroup.agents)"),
                      "an expanded section still renders its agent rows")
        XCTAssertFalse(renderer.contains("withAnimation"),
                       "collapse is instant — no animation may wrap the toggle")
    }

    func testStatusBarIsTheThickWholeBarToggleWithCounts() throws {
        let source = try bundledSource()
        let bar = try slice(from: "private func statusSectionBar(",
                            to: "private func repoSubgroupHeader(",
                            in: source)
        XCTAssertTrue(bar.contains("let isCollapsed = sectionCollapse.isCollapsed(status.state)"),
                      "the bar must read the session collapse state")
        XCTAssertTrue(bar.contains("sectionCollapse.toggle(status.state)"),
                      "tapping the bar must toggle exactly that status section")
        XCTAssertTrue(bar.contains("Text(status.header)"),
                      "the bar keeps the raw status name + TOTAL count (counts stay on the bar)")
        XCTAssertTrue(bar.contains(".font(.headline.weight(.bold))"),
                      "the status name must render in the larger bold tier")
        XCTAssertTrue(bar.contains("theme.stateColor(for: status.state)"),
                      "the bar mark must consume the shared state mapping")
        XCTAssertTrue(bar.contains("Image(systemName: \"chevron.down\")"),
                      "the bar must show a disclosure chevron")
        XCTAssertTrue(bar.contains("rotationEffect(.degrees(isCollapsed ? -90 : 0))"),
                      "the chevron must rotate when the section collapses")
        XCTAssertTrue(bar.contains(".background(theme.surface1)"),
                      "the thick bar must use the surface1 tier (mantle chrome contrasts)")
        XCTAssertTrue(bar.contains("minHeight: 44"),
                      "the whole-bar hit target must stay >= 44 pt")
        XCTAssertTrue(bar.contains(".buttonStyle(BoardPressStyle())"),
                      "the bar must give pressed feedback like the other board controls")
        XCTAssertTrue(bar.contains(".accessibilityValue(isCollapsed ? \"Collapsed\" : \"Expanded\")"),
                      "VoiceOver must announce the collapse state")
        XCTAssertFalse(bar.contains("withAnimation"),
                       "the chevron flip is static — Reduce Motion needs no animation gate")
    }

    func testCollapsedSectionRendersOnlyItsBarNotAgentContent() throws {
        let source = try bundledSource()
        let board = try slice(from: "private func boardSections(sections: BoardModel.Sections",
                              to: "private func agentRow",
                              in: source)
        // Exactly ONE #386 collapse gate exists around the subgroup/row
        // loops; a second gate (or an alternate collapsed-content branch)
        // that could leak rows under a collapsed bar goes RED here.
        XCTAssertEqual(board.components(separatedBy:
            "if !sectionCollapse.isCollapsed(status.state) {").count - 1, 1,
            "exactly one collapse gate must wrap the section content")
        XCTAssertEqual(board.components(separatedBy: "ForEach(status.subgroups)").count - 1, 1,
                       "exactly one subgroup loop may exist — no ungated copy beside it")
    }
}

// MARK: - #365 Settings gear (source wiring: always-visible top-bar control)

/// #365: Settings must be an ALWAYS-VISIBLE top-bar gear Button (plain
/// Button, system gear shape, >=44 pt, VoiceOver label) that opens the
/// Settings sheet with the connection pairing surface — NOT a second-class
/// entry hidden inside the DEBUG demo overflow menu. Pins the bundled
/// FleetViews.swift.txt exactly like the #316/#364 wiring tests; a gear
/// moved back into the menu, made DEBUG-only, or losing its label/target
/// size goes RED here.
final class SettingsAccessWiringTests: XCTestCase {

    private func bundledSource() throws -> String {
        let bundle = Bundle(for: SettingsAccessWiringTests.self)
        let url = try XCTUnwrap(bundle.url(forResource: "FleetViews",
                                           withExtension: "swift.txt"))
        return try String(contentsOf: url, encoding: .utf8)
    }

    /// 1-based line numbers of every line whose `#if DEBUG` nesting makes
    /// it DEBUG-active. FleetViews uses flat, non-nested `#if DEBUG` /
    /// `#endif` pairs (no `#else`), so a depth scan is exact.
    private func debugActiveLines(_ source: String) -> Set<Int> {
        var active: Set<Int> = []
        var depth = 0
        for (index, line) in source.split(separator: "\n",
                                          omittingEmptySubsequences: false)
            .enumerated() {
            let trimmed = line.trimmingCharacters(in: .whitespaces)
            if trimmed.hasPrefix("#if DEBUG") {
                depth += 1
            } else if trimmed.hasPrefix("#endif") {
                depth = max(0, depth - 1)
            }
            if depth > 0 { active.insert(index + 1) }
        }
        return active
    }

    private func lineNumbers(of needle: String, in text: String) -> [Int] {
        text.split(separator: "\n", omittingEmptySubsequences: false)
            .enumerated()
            .filter { $0.element.contains(needle) }
            .map { $0.offset + 1 }
    }

    func testSettingsGearIsAReleaseActiveTopBarButton() throws {
        let source = try bundledSource()
        let debug = debugActiveLines(source)

        // The gear is a plain Button whose DIRECT action opens the sheet.
        // (The DEBUG-only settings evidence driver may open the same sheet —
        // release-active occurrences are what the board depends on.)
        let allActionLines = lineNumbers(of: "showSettings = true", in: source)
        let releaseActionLines = allActionLines.filter { !debug.contains($0) }
        XCTAssertEqual(releaseActionLines.count, 1,
                       "exactly one RELEASE-active settings-open action must exist")
        let gearLines = lineNumbers(of: "gearshape", in: source)
        XCTAssertEqual(gearLines.count, 1,
                       "the board must have exactly one system gear shape")
        XCTAssertGreaterThan(gearLines[0], releaseActionLines[0],
                             "the gear must sit in the Button LABEL after its action "
                             + "(a 'Button(\"Settings\", systemImage: \"gearshape\") { showSettings = true }' "
                             + "menu-item spelling is the removed surface)")

        // >=44 pt target + VoiceOver label on the gear.
        XCTAssertEqual(lineNumbers(of: ".accessibilityLabel(\"Settings\")",
                                   in: source).count, 1,
                       "the gear must carry a VoiceOver label")
        XCTAssertEqual(lineNumbers(of: ".frame(minWidth: 44, minHeight: 44)",
                                   in: source).count, 1,
                       "the gear must keep a >=44 pt hit target")

        // RELEASE-active: a DEBUG-gated gear would leave Release builds with
        // NO Settings access at all, so every gear line must sit OUTSIDE the
        // debug-active regions.
        for needle in ["gearshape",
                       ".accessibilityLabel(\"Settings\")",
                       ".frame(minWidth: 44, minHeight: 44)"] {
            let lines = lineNumbers(of: needle, in: source)
            XCTAssertEqual(lines.count, 1, "\(needle) must appear exactly once")
            guard lines.count == 1 else { continue }
            XCTAssertFalse(debug.contains(lines[0]),
                           "\(needle) must be release-active (gear on the board in Release)")
        }
        // The sheet-open action: one release-active (the gear) + the
        // DEBUG-only recorded-evidence drivers (#365 settings, #372 theme,
        // #379 connect, #385 glass and #388 connection-inputs sequences all
        // open the same sheet); all required, none release-gated.
        XCTAssertEqual(releaseActionLines.count, 1,
                       "the gear must be the ONLY release-active settings opener")
        XCTAssertEqual(allActionLines.count - releaseActionLines.count, 9,
                       "the #365, #372, #379, #385, #388, #389, #401-settings, #401-add and #415 add-host-lifecycle DEBUG evidence drivers are the only debug-gated openers")
    }

    func testDemoOverflowMenuIsDebugOnlyAndNoLongerHidesSettings() throws {
        let source = try bundledSource()
        let debug = debugActiveLines(source)

        let toolbarStart = try XCTUnwrap(source.range(of: ".toolbar {"),
                                         "the board toolbar must exist")
        let sheetMarker = try XCTUnwrap(source.range(of: ".sheet(isPresented: $showSettings)"),
                                        "the settings sheet must be bound right after the toolbar")
        let toolbarSlice = String(source[toolbarStart.lowerBound..<sheetMarker.lowerBound])

        // Exactly ONE Menu in the board toolbar: the DEBUG demo overflow.
        XCTAssertEqual(toolbarSlice.components(separatedBy: "Menu {").count - 1, 1,
                       "the board toolbar must keep exactly one Menu (demo overflow only)")
        guard let menuStart = toolbarSlice.range(of: "Menu {"),
              let menuClose = toolbarSlice.range(of: "} label:") else {
            return XCTFail("the demo Menu must have a label")
        }
        let menuSlice = String(toolbarSlice[menuStart.lowerBound..<menuClose.lowerBound])
        XCTAssertFalse(menuSlice.contains("showSettings"),
                       "Settings must NOT hide inside the demo overflow menu (#365)")
        XCTAssertTrue(menuSlice.contains("sparkles"),
                      "the overflow menu's remaining entry is the DEBUG demo toggle")
        XCTAssertTrue(menuSlice.contains("enterDemo") || menuSlice.contains("exitDemo"),
                      "the overflow menu must carry the demo toggle")

        // The overflow chrome + demo strings are DEBUG-only; the gear beside
        // them is release-active (asserted in the sibling test).
        let sliderLines = lineNumbers(of: "slider.horizontal.3", in: source)
        XCTAssertEqual(sliderLines.count, 1,
                       "the slider overflow icon must appear exactly once")
        XCTAssertTrue(debug.contains(sliderLines[0]),
                      "the overflow menu (slider icon) must be DEBUG-gated")
        for demoNeedle in ["Demo mode", "Exit demo"] {
            for line in lineNumbers(of: demoNeedle, in: source) {
                XCTAssertTrue(debug.contains(line),
                              "\(demoNeedle) must stay inside #if DEBUG")
            }
        }
    }

    func testBoardRendersInsideANavigationStackShell() throws {
        let source = try bundledSource()
        // #365: .toolbar only renders inside a navigation shell. The #354
        // cut deleted the board's NavigationStack, orphaning
        // .navigationTitle/.toolbar — the top bar (and with it Settings)
        // never appeared on the board. FleetView must own a stack again.
        // #387: the title inside that shell is EMPTY + INLINE — the board
        // header is chrome-only, so neither the top-of-board state nor the
        // scrolled collapsed bar can render 'Fleet' text.
        let boardMarker = "\nstruct FleetView: View {"
        let boardStart = try XCTUnwrap(source.range(of: boardMarker),
                                       "FleetView declaration must exist")
        let boardEnd = try XCTUnwrap(source.range(of: "\n// MARK: - Banner"),
                                     "the banner section must follow FleetView")
        let slice = String(source[boardStart.lowerBound..<boardEnd.lowerBound])
        XCTAssertEqual(slice.components(separatedBy: "NavigationStack {").count - 1, 1,
                       "FleetView must wrap its board in exactly one NavigationStack")

        let stackLine = try XCTUnwrap(lineNumbers(of: "NavigationStack {", in: slice).first)
        let titleLine = try XCTUnwrap(lineNumbers(of: ".navigationTitle(\"\")", in: slice).first,
                                      "the board must declare the EMPTY title (#387)")
        let inlineLine = try XCTUnwrap(lineNumbers(of: ".navigationBarTitleDisplayMode(.inline)", in: slice).first,
                                       "the board must lock INLINE display mode (#387)")
        let toolbarLine = try XCTUnwrap(lineNumbers(of: ".toolbar {", in: slice).first)
        let sheetLine = try XCTUnwrap(lineNumbers(of: ".sheet(isPresented: $showSettings)", in: slice).first)
        XCTAssertEqual(lineNumbers(of: ".navigationTitle(\"Fleet\")", in: slice).count, 0,
                       "the 'Fleet' navigation title must be gone (#387)")
        XCTAssertLessThan(stackLine, titleLine,
                          "the stack must open before the navigation chrome")
        XCTAssertLessThan(titleLine, inlineLine,
                          "the empty title must precede its inline display-mode lock")
        XCTAssertLessThan(inlineLine, toolbarLine,
                          "the toolbar must be configured inside the stack")
        XCTAssertLessThan(toolbarLine, sheetLine,
                          "the settings sheet binding must follow the toolbar")
    }

    func testSettingsSheetExposesConnectionPairingAndNotifications() throws {
        let source = try bundledSource()
        let start = try XCTUnwrap(source.range(of: "\nstruct SettingsView: View {"),
                                  "SettingsView declaration must exist")
        let nextMark = try XCTUnwrap(source.range(of: "\n// MARK: - How to connect"),
                                     "the How-to-connect section mark must follow SettingsView")
        let slice = String(source[start.lowerBound..<nextMark.lowerBound])

        // Connection pairing surface: host field + registration token +
        // register action with the same enable rules as the connect section
        // (both inputs render through the #388 themed ConnectionField).
        XCTAssertTrue(slice.contains("ConnectionField(title: \"Host (Tailscale host or loopback)\""),
                      "the Settings sheet must expose the connection host field (#365/#388)")
        XCTAssertTrue(slice.contains("ConnectionField(title: \"Registration token\""),
                      "the Settings sheet must expose device pairing (#388)")
        XCTAssertTrue(slice.contains("await model.register(host: host, token: token)"),
                      "the Settings register action must route through the real registration flow")
        XCTAssertTrue(slice.contains("host.isEmpty || token.isEmpty || registering"),
                      "the Settings register action must disable on empty host/token")
        XCTAssertTrue(slice.contains("model.hostURL?.absoluteString"),
                      "the host field must pre-fill from the ACTIVE host on a paired device")
        // Notifications pairing + the Device Remove/revoke action (retained
        // #354 destructive reset, relocated into the Device section by #379).
        XCTAssertTrue(slice.contains("Toggle(\"State-change notifications\""))
        XCTAssertTrue(slice.contains("model.setNotificationsEnabled("))
        XCTAssertTrue(slice.contains("Button(\"Remove device\", role: .destructive)"),
                      "the Device section must carry the Remove/revoke action (#379)")
        XCTAssertTrue(slice.contains("model.resetDevice()"))
        // The sheet is a plain form — no overflow-menu / demo chrome inside.
        XCTAssertEqual(slice.components(separatedBy: "NavigationStack {").count - 1, 1,
                       "the sheet must own exactly one navigation shell")
        for hidden in ["enterDemo", "exitDemo", "Demo mode", "Try demo fleet",
                       "slider.horizontal.3", "Menu {"] {
            XCTAssertFalse(slice.contains(hidden),
                           "\(hidden) must not be wired into the Settings sheet")
        }
    }
}

// MARK: - #387 chrome-only board header (no 'Fleet' title text)

/// Pins the #387 chrome-only header over the bundled FleetViews source
/// (the #316/#365 mechanism): the board declares NO 'Fleet' title text —
/// the navigation title is EMPTY and the bar is locked INLINE, so neither
/// the top-of-board state nor the scrolled collapsed bar can render title
/// text, while the Settings gear toolbar (release-active, >=44 pt, accent
/// tinted) stays exactly as #365 pinned it. Re-adding any titled
/// navigationTitle to the board — or a large-title display mode that
/// could return a title band — goes RED here.
final class NavigationHeaderWiringTests: XCTestCase {

    private func bundledSource() throws -> String {
        let bundle = Bundle(for: NavigationHeaderWiringTests.self)
        let url = try XCTUnwrap(bundle.url(forResource: "FleetViews",
                                           withExtension: "swift.txt"))
        return try String(contentsOf: url, encoding: .utf8)
    }

    private func lineNumbers(of needle: String, in text: String) -> [Int] {
        text.split(separator: "\n", omittingEmptySubsequences: false)
            .enumerated()
            .filter { $0.element.contains(needle) }
            .map { $0.offset + 1 }
    }

    /// The board's own region: FleetView declaration → Banner MARK (the
    /// same boundary the #365/#386 wiring tests slice).
    private func boardSlice(from source: String) throws -> String {
        let boardStart = try XCTUnwrap(source.range(of: "\nstruct FleetView: View {"),
                                       "FleetView declaration must exist")
        let boardEnd = try XCTUnwrap(source.range(of: "\n// MARK: - Banner"),
                                     "the banner section must follow FleetView")
        return String(source[boardStart.lowerBound..<boardEnd.lowerBound])
    }

    func testBoardDeclaresNoFleetTitleTextInAnyNavState() throws {
        let source = try bundledSource()
        let slice = try boardSlice(from: source)
        // The pre-#387 titled chrome is GONE: no .navigationTitle("Fleet")
        // anywhere in the board region — that spelling rendered 'Fleet'
        // in the large-title state AND inline once scrolled.
        XCTAssertEqual(lineNumbers(of: ".navigationTitle(\"Fleet\")", in: slice).count, 0,
                       "the board must not declare the 'Fleet' navigation title (#387)")
        XCTAssertFalse(slice.contains(".navigationBarTitleDisplayMode(.large)"),
                       "no large-title display mode may return a title band to the board")
        // The board region declares EXACTLY ONE navigation title — the
        // empty one — followed by the inline display-mode lock.
        XCTAssertEqual(lineNumbers(of: ".navigationTitle(", in: slice).count, 1,
                       "exactly one navigation title may exist in the board region (#387)")
        XCTAssertTrue(slice.contains(".navigationTitle(\"\")"),
                      "the board title must be the EMPTY title (#387)")
        let inlineLines = lineNumbers(of: ".navigationBarTitleDisplayMode(.inline)", in: slice)
        XCTAssertEqual(inlineLines.count, 1,
                       "the board must force the INLINE display mode exactly once (#387)")
        let titleLine = try XCTUnwrap(lineNumbers(of: ".navigationTitle(\"\")", in: slice).first)
        XCTAssertGreaterThan(inlineLines[0], titleLine,
                             "the inline lock must follow the empty title in the modifier chain")
        // The top-bar chrome still rides the active flavor's accent (the
        // #372 tint that colors the gear in Mocha AND Latte).
        XCTAssertTrue(slice.contains(".tint(theme.accent)"),
                      "the board toolbar chrome must ride the active flavor's accent (#372/#387)")
    }

    func testToolbarGearChromeSurvivesTheTitleFreeHeader() throws {
        let source = try bundledSource()
        let slice = try boardSlice(from: source)
        let titleLine = try XCTUnwrap(lineNumbers(of: ".navigationTitle(\"\")", in: slice).first)
        let toolbarLine = try XCTUnwrap(lineNumbers(of: ".toolbar {", in: slice).first)
        let gearLine = try XCTUnwrap(lineNumbers(of: "gearshape", in: slice).first)
        let labelLine = try XCTUnwrap(lineNumbers(of: ".accessibilityLabel(\"Settings\")", in: slice).first)
        let frameLine = try XCTUnwrap(lineNumbers(of: ".frame(minWidth: 44, minHeight: 44)", in: slice).first)
        let sheetLine = try XCTUnwrap(lineNumbers(of: ".sheet(isPresented: $showSettings)", in: slice).first)
        // Chrome order is unchanged from #365: empty title → inline lock →
        // gear toolbar → settings sheet, with the gear's >=44 pt target +
        // VoiceOver label still release-active (pinned by the sibling #365
        // tests; here we prove it sits AFTER the title-free chrome).
        XCTAssertLessThan(titleLine, toolbarLine,
                          "the gear toolbar must follow the title-free header chrome")
        XCTAssertLessThan(toolbarLine, gearLine,
                          "the gear must live inside the board toolbar")
        XCTAssertLessThan(gearLine, labelLine,
                          "the gear keeps its VoiceOver label right after the shape")
        XCTAssertLessThan(labelLine, frameLine,
                          "the gear keeps its >=44 pt hit target in the same label chain")
        XCTAssertLessThan(toolbarLine, sheetLine,
                          "the settings sheet binding must follow the toolbar")
    }
}

// MARK: - #379 Settings cleanup + How-to-connect sheet

/// #379 wiring pins (bundled FleetViews source — the #316 decoy-resistant
/// mechanism): the Device section is the identity read-out WITHOUT the
/// grants list or any stale capability language; the Settings-header '?'
/// opens the shared How-to-connect sheet; an unpaired first launch
/// auto-presents that sheet once; and the sheet lists the five numbered
/// steps with the copy-host control and the README Setup link. A
/// compile-capable bypass of any of those call sites goes RED here.
final class SettingsConnectWiringTests: XCTestCase {

    private func bundledSource() throws -> String {
        let bundle = Bundle(for: SettingsConnectWiringTests.self)
        let url = try XCTUnwrap(bundle.url(forResource: "FleetViews",
                                           withExtension: "swift.txt"))
        return try String(contentsOf: url, encoding: .utf8)
    }

    /// 1-based line numbers of every `#if DEBUG`-active line (flat
    /// non-nested pairs — same depth scan as the #365/#372 wiring tests).
    private func debugActiveLines(_ source: String) -> Set<Int> {
        var active: Set<Int> = []
        var depth = 0
        for (index, line) in source.split(separator: "\n",
                                          omittingEmptySubsequences: false)
            .enumerated() {
            let trimmed = line.trimmingCharacters(in: .whitespaces)
            if trimmed.hasPrefix("#if DEBUG") {
                depth += 1
            } else if trimmed.hasPrefix("#endif") {
                depth = max(0, depth - 1)
            }
            if depth > 0 { active.insert(index + 1) }
        }
        return active
    }

    private func lineNumbers(of needle: String, in text: String) -> [Int] {
        text.split(separator: "\n", omittingEmptySubsequences: false)
            .enumerated()
            .filter { $0.element.contains(needle) }
            .map { $0.offset + 1 }
    }

    /// The Settings sheet's own region: SettingsView → How-to-connect MARK.
    private func settingsSlice(_ source: String) throws -> String {
        let start = try XCTUnwrap(source.range(of: "\nstruct SettingsView: View {"),
                                  "SettingsView declaration must exist")
        let end = try XCTUnwrap(source.range(of: "\n// MARK: - How to connect"),
                                "the How-to-connect MARK must follow SettingsView")
        return String(source[start.lowerBound..<end.lowerBound])
    }

    /// The shared connect sheet's region: How-to-connect MARK → Recents.
    private func connectSheetSlice(_ source: String) throws -> String {
        let start = try XCTUnwrap(source.range(of: "\n// MARK: - How to connect"),
                                  "the How-to-connect MARK must exist")
        let end = try XCTUnwrap(source.range(of: "\n// MARK: - Recents"),
                                "the Recents MARK must follow the connect sheet")
        return String(source[start.lowerBound..<end.lowerBound])
    }

    /// The board's region: FleetView → Banner MARK (what the #365/#372
    /// board tests slice).
    private func fleetSlice(_ source: String) throws -> String {
        let start = try XCTUnwrap(source.range(of: "\nstruct FleetView: View {"),
                                  "FleetView declaration must exist")
        let end = try XCTUnwrap(source.range(of: "\n// MARK: - Banner"),
                                "the Banner MARK must follow FleetView")
        return String(source[start.lowerBound..<end.lowerBound])
    }

    /// #379 A.1/A.2: the Device section shows Key ID, the Keychain storage
    /// note, the read-only signed device label, the paired/registration
    /// state, and the Remove/revoke action — and NO grants list, NO stale
    /// capability language, and no standalone Reset section.
    func testSettingsDeviceSectionShowsIdentityWithoutGrantsList() throws {
        let slice = try settingsSlice(try bundledSource())
        // The Device section uses the custom header form (same convention
        // as the #372 Appearance section), so its header text marks it.
        XCTAssertTrue(slice.contains("Text(\"Device\")"),
                      "the Device section must exist")
        XCTAssertTrue(slice.contains("LabeledContent(\"Key ID\""),
                      "the Device section must show the Key ID")
        XCTAssertTrue(slice.contains("LabeledContent(\"Key storage\""),
                      "the Device section must show the Keychain storage note")
        XCTAssertTrue(slice.contains("Read-only signed device"),
                      "the Device section must carry the read-only signed device label")
        XCTAssertTrue(slice.contains("\"Paired\" : \"Not paired\""),
                      "the Device section must show the paired/registration state")
        XCTAssertTrue(slice.contains("Button(\"Remove device\", role: .destructive)"),
                      "the Device section must carry the Remove/revoke action")
        // The grants LIST row is gone; the model grant set is never
        // enumerated in Settings; the old Reset section merged into Device.
        XCTAssertFalse(slice.contains("LabeledContent(\"Grants\""),
                       "the grants list row must not exist in Settings")
        XCTAssertFalse(slice.contains("model.grants"),
                       "Settings must not enumerate the grant set")
        XCTAssertFalse(slice.contains("Section(\"Reset\")"),
                       "the standalone Reset section is gone")
        XCTAssertFalse(slice.contains("Button(\"Reset device identity\""),
                       "the old Reset identity button is gone")
        XCTAssertFalse(slice.contains("LabeledContent(\"Name\""),
                       "the cosmetic device-name row is not part of the read-out")
        // Stale capability language: nothing but read_tail + pairing
        // exists in the product now — none of the cut grant names may
        // surface in the Settings sheet.
        for stale in ["prompt", "read_diff", "start_worktree", "kill"] {
            XCTAssertFalse(slice.contains(stale),
                           "stale capability language '\(stale)' must not appear in Settings")
        }
        // Section order stays Connection → Device → Notifications, and the
        // Remove action sits inside the Device region (after its identity
        // rows, before the Notifications section — no Reset section between).
        let connection = try XCTUnwrap(lineNumbers(of: "Section(\"Connection\")",
                                                   in: slice).first)
        let device = try XCTUnwrap(lineNumbers(of: "Text(\"Device\")",
                                               in: slice).first)
        let notifications = try XCTUnwrap(lineNumbers(of: "Section(\"Notifications\")",
                                                      in: slice).first)
        let keyIdRow = try XCTUnwrap(lineNumbers(of: "LabeledContent(\"Key ID\"",
                                                 in: slice).first)
        let remove = try XCTUnwrap(lineNumbers(of: "Button(\"Remove device\"",
                                               in: slice).first)
        XCTAssertLessThan(connection, device, "Connection must precede Device")
        XCTAssertLessThan(device, notifications, "Device must precede Notifications")
        XCTAssertLessThan(keyIdRow, remove, "the Remove/revoke action must follow the Device identity rows")
        XCTAssertLessThan(remove, notifications,
                          "the Remove/revoke action must live before the Notifications section")
    }

    /// #379 B.1: the '?' Help button sits in the Settings header and
    /// presents the shared How-to-connect sheet with the LIVE host field
    /// text (the copy control's source).
    func testSettingsHeaderQuestionButtonPresentsTheConnectSheet() throws {
        let slice = try settingsSlice(try bundledSource())
        XCTAssertEqual(lineNumbers(of: "Image(systemName: \"questionmark.circle\")",
                                   in: slice).count, 1,
                       "the Settings header must carry exactly one '?' Help button")
        XCTAssertEqual(lineNumbers(of: ".accessibilityLabel(\"How to connect\")",
                                   in: slice).count, 1,
                       "the Help button must be VoiceOver-labeled")
        let openers = lineNumbers(of: "showConnectHelp = true", in: slice)
        XCTAssertEqual(openers.count, 1,
                       "the '?' button must be the Settings sheet's only help opener")
        // The debug scan runs over the SAME slice so line numbers agree.
        let debug = debugActiveLines(slice)
        let openerLine = try XCTUnwrap(openers.first,
                                       "the '?' opener must exist (see count pin above)")
        XCTAssertFalse(debug.contains(openerLine),
                       "the '?' help entry must be release-active")
        XCTAssertEqual(lineNumbers(of: ".sheet(isPresented: $showConnectHelp)",
                                   in: slice).count, 1,
                       "SettingsView must present the connect sheet from inside its own stack")
        XCTAssertTrue(slice.contains("HowToConnectSheet(host: host)"),
                      "the '?'-opened sheet must receive the LIVE host field text")
    }

    /// #379 B.1: an unpaired first launch auto-presents the connect sheet
    /// over the board — once per board lifetime, gated on needsSetup.
    func testUnpairedLaunchAutoPresentsTheConnectSheet() throws {
        let source = try bundledSource()
        let slice = try fleetSlice(source)
        XCTAssertEqual(lineNumbers(of: ".sheet(isPresented: $showConnectHelp)",
                                   in: slice).count, 1,
                       "the board must attach exactly one connect-help sheet")
        XCTAssertTrue(slice.contains("HowToConnectSheet(host: model.hostURL?.absoluteString ?? \"\")"),
                      "the board-level sheet must pass the registered host (empty when unpaired)")
        // The AUTO-present: exactly one release-active opener, gated on the
        // unpaired mode, one-shot per board lifetime. The debug scan runs
        // over the SAME slice so line numbers agree.
        let openers = lineNumbers(of: "showConnectHelp = true", in: slice)
        let debug = debugActiveLines(slice)
        let releaseOpeners = openers.filter { !debug.contains($0) }
        XCTAssertEqual(releaseOpeners.count, 1,
                       "exactly one RELEASE-active connect-sheet opener must exist (the unpaired auto-present)")
        let releaseOpener = try XCTUnwrap(releaseOpeners.first,
                                          "the auto-present opener must exist (see count pin above)")
        let firstGuard = try XCTUnwrap(lineNumbers(of: "model.mode == .needsSetup",
                                                   in: slice).first,
                                       "the auto-present must gate on the unpaired mode")
        XCTAssertLessThan(firstGuard, releaseOpener,
                          "the auto-present opener must sit behind a needsSetup gate")
        XCTAssertTrue(slice.contains("autoPresentedConnectHelp"),
                      "the auto-present must be one-shot (never re-pop on a later device removal)")
        let idTasks = lineNumbers(of: ".task(id: model.mode)", in: slice)
        XCTAssertEqual(idTasks.filter { !debug.contains($0) }.count, 1,
                       "a release-active mode-keyed task must drive the auto-present")
    }

    /// #379 B.2/B.3: the connect sheet carries the five numbered steps
    /// with their locked titles, the summarized daemon setup + healthz +
    /// README Setup link (step 1), and the copy-host control (step 2).
    func testConnectSheetListsNumberedStepsCopyHostAndReadmeLink() throws {
        let slice = try connectSheetSlice(try bundledSource())
        XCTAssertEqual(slice.components(separatedBy: "struct HowToConnectSheet: View {").count - 1, 1,
                       "exactly one shared HowToConnectSheet must exist")
        // The five locked step headers, numbered 1...5 in order.
        let steps: [(Int, String)] = [
            (1, "Run the daemon on your Mac"),
            (2, "Reach it from the phone"),
            (3, "Open Settings and paste the Host"),
            (4, "Register with the pairing token"),
            (5, "Enable state-change notifications"),
        ]
        for (number, title) in steps {
            XCTAssertTrue(slice.contains("stepHeader(number: \(number), title: \"\(title)\")"),
                          "step \(number) header must be wired with its locked title")
        }
        // Numbered badges render the digit (structure pin — the runtime
        // digits are what a reviewer sees in the sheet).
        XCTAssertEqual(slice.components(separatedBy: "struct StepNumberBadge: View {").count - 1, 1,
                       "the step-number badge view must exist")
        XCTAssertTrue(slice.contains("Text(\"\\(number)\")"),
                      "the badge must render the step digit")
        // Step 1: summarized daemon setup (launchd/one command), the
        // healthz check, and the README Setup link (#376 target exists).
        XCTAssertTrue(slice.contains("setup-corrald.sh"),
                      "step 1 must summarize the launchd setup script")
        XCTAssertTrue(slice.contains("healthz"),
                      "step 1 must mention the healthz verification")
        XCTAssertTrue(slice.contains("github.com/jirathip-dev/corral#setup"),
                      "step 1 must link the README Setup section (#376 link target)")
        XCTAssertTrue(slice.contains("Open the README Setup section"),
                      "the README link must carry a visible label")
        // Step 2: reach-it-from-the-phone copy-host control.
        XCTAssertTrue(slice.contains("Label(\"Copy host\", systemImage: \"doc.on.doc\")"),
                      "step 2 must offer a Copy host control")
        XCTAssertTrue(slice.contains("UIPasteboard.general.string = host"),
                      "Copy host must write the host string to the pasteboard")
        XCTAssertTrue(slice.contains(".disabled(host.isEmpty)"),
                      "Copy host must disable until a host actually exists")
        // Step 4 copy names the read-only signed pairing outcome.
        XCTAssertTrue(slice.contains("Register device (read-only)"),
                      "step 4 must reference the read-only register action")
        // The sheet never enumerates the grant set either.
        XCTAssertFalse(slice.contains("model.grants"),
                       "the connect sheet must not enumerate grants")
    }
}

// MARK: - #365 Settings host switch (register-while-live stream semantics)

/// URL-keyed protocol for the #365 host-switch regressions. `/events`
/// streams are served and then held OPEN (a live SSE connection); ordinary
/// endpoints (e.g. `/register`) finish after their body. Every request URL
/// is recorded so tests can prove which host the stream connected to.
private final class HostSwitchURLProtocol: URLProtocol {
    private static let lock = NSLock()
    private static var scriptStorage: [URL: (statusCode: Int, body: Data, holdOpen: Bool)] = [:]
    private static var requestsStorage: [URLRequest] = []

    static func setScript(_ script: [URL: (statusCode: Int, body: Data, holdOpen: Bool)]) {
        lock.lock()
        scriptStorage = script
        requestsStorage = []
        lock.unlock()
    }

    static var requests: [URLRequest] {
        lock.lock()
        defer { lock.unlock() }
        return requestsStorage
    }

    static func clearScript() {
        setScript([:])
    }

    override class func canInit(with request: URLRequest) -> Bool { true }
    override class func canonicalRequest(for request: URLRequest) -> URLRequest { request }

    override func startLoading() {
        Self.lock.lock()
        // Materialize the POST body (URLSession may deliver it as a
        // stream — see requestBodyData) so body assertions read it off
        // the recorded copy. The protocol fabricates its response, so the
        // recorded copy is the only consumer of the body bytes.
        var copy = request
        if let body = requestBodyData(request) {
            copy.httpBody = body
        }
        Self.requestsStorage.append(copy)
        // SAFETY: URLProtocol requests always carry a URL.
        let scripted = Self.scriptStorage[request.url!]
        Self.lock.unlock()
        guard let (statusCode, body, holdOpen) = scripted else {
            client?.urlProtocol(self, didFailWithError: URLError(.badURL))
            return
        }
        // SAFETY: fixed valid HTTP response construction from a scripted URL.
        let response = HTTPURLResponse(
            url: request.url!, statusCode: statusCode, httpVersion: "HTTP/1.1",
            headerFields: holdOpen
                ? ["Content-Type": "text/event-stream"]
                : ["Content-Type": "application/json"])!
        client?.urlProtocol(self, didReceive: response, cacheStoragePolicy: .notAllowed)
        if !body.isEmpty {
            client?.urlProtocol(self, didLoad: body)
        }
        if !holdOpen {
            client?.urlProtocolDidFinishLoading(self)
        }
        // holdOpen: keep the connection alive like a real SSE stream — the
        // stream task must be torn down by disconnect(), not by EOF.
    }

    override func stopLoading() {}
}

/// #365 host change: registration is the pairing path for re-pointing an
/// already-paired device at a DIFFERENT host. `register()` must drop the
/// current host's live SSE stream before the new pairing (FleetStore.connect
/// no-ops while a stream runs — otherwise the board keeps streaming the OLD
/// host forever) and must restart the old host's stream when the switch
/// FAILS (otherwise the paired board dies behind the error banner).
@MainActor
final class SettingsHostSwitchTests: XCTestCase {

    private var session: URLSession?
    private var suiteName = ""
    private var model: AppModel?

    // SAFETY: fixed valid URL literals.
    private let hostA = URL(string: "http://host-a")!
    private let hostB = URL(string: "http://host-b")!
    private let eventsA = URL(string: "http://host-a/events")!
    private let eventsB = URL(string: "http://host-b/events")!
    private let registerB = URL(string: "http://host-b/register")!

    private func makeLiveFixture() -> AppModel {
        suiteName = "corral.hostswitch.\(UUID().uuidString)"
        // SAFETY: a fresh UUID-based suite name is always a valid suite.
        let defaults = UserDefaults(suiteName: suiteName)!
        let model = AppModel(session: session!,
                             identityLifecycle: IdentityLifecycle(),
                             defaults: defaults,
                             identityLoader: {
                                 (DeviceSigner(key: Curve25519.Signing.PrivateKey()),
                                  .insecureFallback)
                             },
                             loadMeta: { nil }, saveMeta: { _ in }, wipeIdentity: {})
        model.mode = .live
        model.hostURL = hostA
        return model
    }

    private func scriptedSession(script: [URL: (Int, Data, Bool)]) -> URLSession {
        HostSwitchURLProtocol.setScript(script)
        let config = URLSessionConfiguration.ephemeral
        config.protocolClasses = [HostSwitchURLProtocol.self]
        return URLSession(configuration: config)
    }

    private func waitForRequest(to url: URL, atLeast count: Int = 1,
                                within timeout: TimeInterval = 5) async {
        let deadline = Date().addingTimeInterval(timeout)
        while HostSwitchURLProtocol.requests
            .filter({ $0.url?.absoluteString == url.absoluteString }).count < count,
              Date() < deadline {
            try? await Task.sleep(nanoseconds: 25_000_000)
        }
        XCTAssertGreaterThanOrEqual(
            HostSwitchURLProtocol.requests
                .filter({ $0.url?.absoluteString == url.absoluteString }).count,
            count,
            "expected >= \(count) request(s) to \(url.absoluteString) within \(timeout)s")
    }

    private func requestCount(to url: URL) -> Int {
        HostSwitchURLProtocol.requests
            .filter { $0.url?.absoluteString == url.absoluteString }.count
    }

    override func tearDown() {
        model?.stopLive()
        model = nil
        session?.invalidateAndCancel()
        session = nil
        HostSwitchURLProtocol.clearScript()
        if !suiteName.isEmpty {
            // SAFETY: suiteName was freshly minted per test above.
            UserDefaults(suiteName: suiteName)!
                .removePersistentDomain(forName: suiteName)
            suiteName = ""
        }
        super.tearDown()
    }

    /// #365 AC3 the reconnect half: registering a DIFFERENT host while a
    /// live stream runs must end the OLD host stream and start the NEW
    /// host's stream. RED on the unfixed code: `register()` never drops the
    /// old stream, FleetStore.connect() no-ops on its live streamTask, and
    /// no /events request to the new host ever appears.
    func testRegisteringADifferentHostWhileLiveReconnectsTheStreamToIt() async throws {
        let session = scriptedSession(script: [
            eventsA: (200, Data(), true),
            eventsB: (200, Data(), true),
            registerB: (200, Data(#"{"key_id":"dev_switched","grants":[],"expiry_ts":1800000000,"revoked":false}"#.utf8), false),
        ])
        self.session = session
        let model = makeLiveFixture()
        self.model = model

        // A REAL running SSE task against the CURRENT host (FleetStore
        // connect() is a no-op while one is alive).
        model.fleet.connect(client: CorraldClient(host: hostA, session: session))
        await waitForRequest(to: eventsA)

        await model.register(host: "http://host-b", token: "pair-token")

        XCTAssertEqual(model.hostURL?.absoluteString, "http://host-b",
                       "the model must point at the newly registered host")
        XCTAssertEqual(model.mode, .live)
        await waitForRequest(to: registerB)
        // The discriminator: the NEW host's SSE stream must start (only
        // possible if the old stream was dropped first).
        await waitForRequest(to: eventsB)
        XCTAssertEqual(requestCount(to: eventsA), 1,
                       "the OLD host stream must be disconnected exactly once, never re-requested")
    }

    /// #365 failure half: a host switch that FAILS (bad token / unreachable
    /// host) must keep the paired board on the OLD host — the dropped old
    /// stream is restarted behind the register_failed banner. RED on the
    /// unfixed code: the old stream is never restarted, so the board dies
    /// with no live connection and no way to recover.
    func testFailedHostSwitchRestartsTheOldHostStream() async throws {
        let session = scriptedSession(script: [
            eventsA: (200, Data(), true),
            registerB: (500, Data(#"{"kind":"register_failed","message":"token rejected"}"#.utf8), false),
        ])
        self.session = session
        let model = makeLiveFixture()
        self.model = model

        model.fleet.connect(client: CorraldClient(host: hostA, session: session))
        await waitForRequest(to: eventsA)

        await model.register(host: "http://host-b", token: "wrong-token")

        XCTAssertEqual(model.mode, .live,
                       "a failed switch must not change the mode")
        XCTAssertEqual(model.hostURL?.absoluteString, "http://host-a",
                       "a failed switch must keep the OLD host")
        XCTAssertEqual(model.banner?.kind, "register_failed",
                       "the failure must surface in the board banner")
        // The dropped old-host stream must be RESTARTED (a second /events
        // request to the old host), or the paired board stays dead.
        await waitForRequest(to: eventsA, atLeast: 2)
    }
}

// MARK: - #372 theme wiring (source pins over the bundled FleetViews)

/// #372 wiring: the Appearance picker, the token chrome, and the Reduce
/// Motion gate must all be wired in FleetViews (bundled FleetViews.swift.txt
/// — the same decoy-resistant mechanism as the #316/#364/#365 wiring
/// tests). A picker moved out of Settings, a chip reverting to a system
/// accent, or an un-gated auto-scroll goes RED here.
final class ThemeWiringTests: XCTestCase {

    private func bundledSource() throws -> String {
        let bundle = Bundle(for: ThemeWiringTests.self)
        let url = try XCTUnwrap(bundle.url(forResource: "FleetViews",
                                           withExtension: "swift.txt"))
        return try String(contentsOf: url, encoding: .utf8)
    }

    func testAppearancePickerLivesOnlyInTheSettingsForm() throws {
        let source = try bundledSource()
        let start = try XCTUnwrap(source.range(of: "\nstruct SettingsView: View {"),
                                  "SettingsView declaration must exist")
        let nextMark = try XCTUnwrap(source.range(of: "\n// MARK: - How to connect"),
                                     "the How-to-connect section mark must follow SettingsView")
        let slice = String(source[start.lowerBound..<nextMark.lowerBound])

        // The Appearance section is the FIRST form section and is the only
        // theme control surface (placement lock).
        XCTAssertTrue(slice.contains("appearanceSection"),
                      "SettingsView must own an Appearance section")
        let appearanceLine = try XCTUnwrap(
            slice.lineNumber(of: "appearanceSection"),
            "the appearance section must be invoked in the form")
        let connectionLine = try XCTUnwrap(
            slice.lineNumber(of: "Section(\"Connection\")"),
            "the Connection section must still exist")
        XCTAssertLessThan(appearanceLine, connectionLine,
                          "Appearance must be the FIRST Settings section")
        XCTAssertTrue(slice.contains("CatppuccinFlavor.allCases"),
                      "the picker must offer all four locked flavors")
        XCTAssertTrue(slice.contains("theme.setFlavor(flavor)"),
                      "picking a flavor must route through the ThemeStore")
        XCTAssertTrue(slice.contains("FlavorSwatchStrip(flavor: flavor)"),
                      "each flavor row must preview its palette swatches")
        XCTAssertTrue(slice.contains("Applies to the whole app"),
                      "the Appearance footer must state the app-wide scope")

        // Placement lock: the ONLY theme control is the Settings Appearance
        // section. Every other "theme.setFlavor" call site must sit inside
        // #if DEBUG (the #372 recorded-evidence driver flips flavors for
        // screenshots) — the board chrome, the chips row, and the recents
        // sheet carry NO release-active theme control.
        let debug = debugActiveLines(source)
        let flavorLines = lineNumbers(of: "theme.setFlavor", in: source)
        XCTAssertGreaterThanOrEqual(flavorLines.count, 1,
                                    "the Appearance picker must persist a flavor choice")
        let settingsStartLine = lineNumbers(of: "struct SettingsView: View {",
                                            in: source).first ?? 0
        let settingsEndLine = lineNumbers(of: "// MARK: - How to connect",
                                          in: source).first ?? Int.max
        for line in flavorLines {
            let isRelease = !debug.contains(line)
            if isRelease {
                XCTAssertTrue(line > settingsStartLine && line < settingsEndLine,
                              "release-active flavor control on line \(line) must live "
                              + "in the Settings Appearance section only (placement lock)")
            }
        }
        // The recents sheet carries NO release-active theme control: the
        // #373 evidence driver flips the flavor from inside #if DEBUG, so
        // those lines are debug-active and the placement-lock loop above
        // already exempts them — nothing release-active may live outside
        // the Settings Appearance section.
    }

    /// 1-based line numbers of every `#if DEBUG`-active line (same depth
    /// scan as the #365 settings-access wiring tests).
    private func debugActiveLines(_ source: String) -> Set<Int> {
        var active: Set<Int> = []
        var depth = 0
        for (index, line) in source.split(separator: "\n",
                                          omittingEmptySubsequences: false)
            .enumerated() {
            let trimmed = line.trimmingCharacters(in: .whitespaces)
            if trimmed.hasPrefix("#if DEBUG") {
                depth += 1
            } else if trimmed.hasPrefix("#endif") {
                depth = max(0, depth - 1)
            }
            if depth > 0 { active.insert(index + 1) }
        }
        return active
    }

    private func lineNumbers(of needle: String, in text: String) -> [Int] {
        text.split(separator: "\n", omittingEmptySubsequences: false)
            .enumerated()
            .filter { $0.element.contains(needle) }
            .map { $0.offset + 1 }
    }


    func testBoardChromeConsumesThemeTokens() throws {
        let source = try bundledSource()
        let boardStart = try XCTUnwrap(source.range(of: "\nstruct FleetView: View {"),
                                       "FleetView declaration must exist")
        let boardEnd = try XCTUnwrap(source.range(of: "\n// MARK: - Banner"),
                                     "the banner section must follow FleetView")
        let slice = String(source[boardStart.lowerBound..<boardEnd.lowerBound])
        XCTAssertTrue(slice.contains("theme.repoHueColor(for: repo, among: repos)"),
                      "repo chips must carry the deterministic palette hue dot")
        XCTAssertTrue(slice.contains("isSelected ? theme.accent : theme.base"),
                      "a selected chip fills with the palette accent (mauve, never teal)")
        XCTAssertTrue(slice.contains("isSelected ? theme.crust : theme.subtext1"),
                      "chip ink follows the crust/subtext tokens")
        XCTAssertTrue(slice.contains(".scrollContentBackground(.hidden)"),
                      "the board surface must be token-backed")
        XCTAssertTrue(slice.contains(".background(theme.base)"),
                      "the board background must be the active flavor's base")
        XCTAssertFalse(slice.contains("Color.accentColor"),
                       "no system accent color may remain on the board")
    }

    func testRecentsSheetGatesAutoScrollOnReduceMotion() throws {
        let source = try bundledSource()
        let sheetStart = try XCTUnwrap(source.range(of: "\nstruct RecentOutputSheet: View {"))
        let sheetEnd = try XCTUnwrap(source.range(of: "\n// MARK: - Recents block renderer"))
        let slice = String(source[sheetStart.lowerBound..<sheetEnd.lowerBound])
        XCTAssertTrue(slice.contains("theme.reduceMotion"),
                      "the recents auto-scroll must respect the theme's Reduce Motion state")
        XCTAssertTrue(slice.contains("withAnimation"),
                      "animated scroll must still exist for the default motion path")
    }

    /// #373: the block renderer consumes theme tokens for EVERY surface —
    /// block chrome (surface0, quiet status recess), the output panel
    /// (tailBackground — the accepted recess token: base on Latte so ANSI
    /// hues keep contrast), preview/quiet tiers, and the ANSI-slot segment
    /// colors. No legacy GitHub-dark hexes can satisfy these pins.
    func testRecentsBlocksConsumeThemeTokens() throws {
        let source = try bundledSource()
        let start = try XCTUnwrap(source.range(of: "\n// MARK: - Recents block renderer"),
                                  "the block renderer MARK must exist")
        let slice = String(source[start.lowerBound...])
        XCTAssertTrue(slice.contains("theme.surface0"),
                      "block chrome must be the active flavor's surface0")
        XCTAssertTrue(slice.contains("theme.mixed(.surface0, at: 0.55, over: .base)"),
                      "Status/unknown blocks must recess quietly toward base")
        XCTAssertTrue(slice.contains("theme.tailBackground"),
                      "tool output must sit on the themed output panel (Latte: base)")
        XCTAssertTrue(slice.contains("theme.overlay1"),
                      "the collapsed preview must use the theme's overlay tier")
        XCTAssertTrue(slice.contains("theme.tailQuiet"),
                      "quiet tiers (waiting line, +N lines) must be token-backed")
        XCTAssertTrue(slice.contains("theme.segmentColor(for: kind)"),
                      "syntax marks must resolve through the ANSI-slot segment colors")
        XCTAssertTrue(slice.contains("theme.tailMuted"),
                      "plain output lines must use the muted output tier")
        XCTAssertTrue(slice.contains("theme.color(RecentBlockStyle.accentToken(for: block.kind))"),
                      "role accents must resolve through the palette tokens")
    }

    func testNoLegacyHexesOrPaletteResidueInTheBoardSource() throws {
        let source = try bundledSource()
        XCTAssertFalse(source.contains("RecentOutputPalette"),
                       "the legacy GitHub-dark palette enum must be gone")
        for hex in ["#0d1117", "#e6edf3", "#8b949e", "#2dd4bf", "#58a6ff",
                    "#d29922", "#3fb950", "#f85149", "#a5d6ff", "#ff7b72",
                    "#cf222e", "#0969da", "#6e7781", "#9a6700", "#6e7681",
                    "#8c959f"] {
            XCTAssertFalse(source.contains(hex),
                           "legacy GitHub-dark literal " + hex + " must not exist in FleetViews")
        }
        XCTAssertFalse(source.contains("Color(red:"),
                       "no raw RGB component literals may remain in FleetViews")
    }
}

// MARK: - #372 legacy-hex audit (real files, real greps)

/// The AUDIT GATE as a unit test: greps the actual UI-layer sources on disk
/// (same files the report's shell greps cover) for legacy GitHub-dark hex
/// literals and the old RGB-triplet palette form — zero hits allowed. RED if
/// a future lane reintroduces a legacy literal.
final class LegacyHexAuditTests: XCTestCase {

    private func uiSourceFiles() throws -> [URL] {
        // Test file lives at ios/FleetNotifierTests/ — one level up is ios/.
        let uiDir = URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent()
            .appendingPathComponent("../FleetNotifier/UI", isDirectory: true)
            .standardizedFileURL
        let files = try FileManager.default.contentsOfDirectory(
            at: uiDir, includingPropertiesForKeys: nil)
        return files.filter { $0.pathExtension == "swift" }
    }

    func testUILayerHasZeroLegacyGithubDarkHexLiterals() throws {
        // The exact legacy set the pre-#372 UI carried (the state-token
        // contract light/dark hexes + the GitHub-dark recents palette).
        let legacy = ["#0d1117", "#e6edf3", "#8b949e", "#2dd4bf", "#58a6ff",
                      "#d29922", "#3fb950", "#f85149", "#a5d6ff", "#ff7b72",
                      "#cf222e", "#0969da", "#6e7781", "#9a6700", "#6e7681",
                      "#8c959f", "#10151c", "#161b22", "#1c2128", "#30363d",
                      "#010409", "#24292f", "#21262d", "#c9d1d9", "#f0f6fc",
                      "#afb8c1", "#484f58", "#79c0ff", "#d2a8ff", "#ffdfb6",
                      "#f0883e", "#bc8cff", "#7ee787", "#ffa198", "#ff7b72",
                      "#a5d6ff"]
        let files = try uiSourceFiles()
        XCTAssertGreaterThanOrEqual(files.count, 5,
                                    "the audit must cover the real UI sources")
        for file in files {
            let source = try String(contentsOf: file, encoding: .utf8)
                .lowercased()
            for hex in legacy {
                XCTAssertFalse(source.contains(hex),
                               "\(file.lastPathComponent) carries legacy literal " + hex)
            }
        }
    }

    func testUILayerHasNoRawRGBTripletColorLiterals() throws {
        // The old RecentOutputPalette spelled colors Color(red: x/255, ...);
        // the token system spells them via UIColor(catppuccinHex:).
        for file in try uiSourceFiles() {
            let source = try String(contentsOf: file, encoding: .utf8)
            XCTAssertFalse(source.contains("Color(red:"),
                           "\(file.lastPathComponent) must not build colors from raw RGB triplets")
        }
    }
}

// MARK: - #385 translucent-sheet wiring tests

/// Pins the #385 translucent-sheet WIRING in the bundled FleetViews source:
/// both sheets (RecentOutputSheet + SettingsView) must float over the SHARED
/// translucent backdrop modifier, and that backdrop must carry BOTH the iOS
/// 26+ native Liquid Glass branch (availability-gated) and the <26
/// tinted-material fallback — so a future "simplification" that drops one
/// path (or bypasses the modifier with an opaque fill) fails here. Uses the
/// same bundled-source pattern as the #316/#364 wiring tests.
final class SheetTranslucencyWiringTests: XCTestCase {

    private func bundledSource() throws -> String {
        let bundle = Bundle(for: SheetTranslucencyWiringTests.self)
        let url = try XCTUnwrap(bundle.url(forResource: "FleetViews", withExtension: "swift.txt"))
        return try String(contentsOf: url, encoding: .utf8)
    }

    private func slice(from source: String, startMarker: String,
                       endMarker: String) throws -> String {
        guard let start = source.range(of: startMarker) else {
            throw NSError(domain: "SheetTranslucencyWiringTests", code: 1,
                          userInfo: [NSLocalizedDescriptionKey:
                            "start marker missing from FleetViews source: \(startMarker)"])
        }
        let end = try XCTUnwrap(source.range(of: endMarker)?.lowerBound,
                                "end marker not found: \(endMarker)")
        return String(source[start.lowerBound..<end])
    }

    func testRecentOutputSheetFloatsOverTheSharedTranslucentBackdrop() throws {
        let source = try bundledSource()
        let sheet = try slice(from: source,
                              startMarker: "struct RecentOutputSheet: View {",
                              endMarker: "\n// MARK: - Recents block renderer")
        XCTAssertEqual(sheet.components(separatedBy: "struct RecentOutputSheet:").count - 1, 1,
                       "exactly one RecentOutputSheet declaration")
        XCTAssertTrue(sheet.contains(".translucentSheetBackdrop(theme.base)"),
                      "the recents sheet must present over the #385 translucent backdrop")
    }

    func testSettingsSheetFloatsOverTheSharedTranslucentBackdrop() throws {
        let source = try bundledSource()
        let settings = try slice(from: source,
                                 startMarker: "struct SettingsView: View {",
                                 endMarker: "\nprivate struct FlavorSwatchStrip")
        XCTAssertEqual(settings.components(separatedBy: "struct SettingsView:").count - 1, 1,
                       "exactly one SettingsView declaration")
        XCTAssertTrue(settings.contains(".translucentSheetBackdrop(theme.base)"),
                      "the Settings sheet must present over the #385 translucent backdrop")
        let appliedOpaqueFill = settings
            .split(separator: "\n", omittingEmptySubsequences: false)
            .contains { line in
                let trimmed = line.trimmingCharacters(in: .whitespaces)
                return !trimmed.hasPrefix("//")
                    && trimmed.contains(".background(theme.base)")
            }
        XCTAssertFalse(appliedOpaqueFill,
                       "the Settings form must NOT paint an opaque base fill over "
                       + "the translucent backdrop (the sheet surface shows through)")
    }

    func testBackdropCarriesBothTheGlassAndTheMaterialFallbackPaths() throws {
        let source = try bundledSource()
        let backdrop = try slice(from: source,
                                 startMarker: "// MARK: - #385 Liquid Glass / translucent sheet backdrop",
                                 endMarker: "// MARK: - Settings (Appearance")
        XCTAssertTrue(backdrop.contains("struct TranslucentSheetBackdrop: View"),
                      "the shared backdrop view must exist")
        XCTAssertTrue(backdrop.contains("translucentSheetBackdrop(_ tint: Color)"),
                      "the shared backdrop modifier must exist")
        // iOS 26+ path: NATIVE Liquid Glass, compile-time availability-gated,
        // tinted through the API's theme hook at the locked glass tint.
        XCTAssertTrue(backdrop.contains("#available(iOS 26.0, *)"),
                      "the native-glass path must be availability-gated")
        XCTAssertTrue(backdrop.contains(".glassEffect("),
                      "the iOS 26 path must apply SwiftUI glassEffect")
        XCTAssertTrue(backdrop.contains("SheetBackdrop.glassTintOpacity"),
                      "the glass path must use the locked theme-tint strength")
        // <26 path: the tinted-material fallback the 17–25 runtimes render.
        XCTAssertTrue(backdrop.contains(".ultraThinMaterial"),
                      "the fallback must apply a backdrop blur material")
        XCTAssertTrue(backdrop.contains("SheetBackdrop.fallbackTintAlpha"),
                      "the fallback must tint the material with the locked alpha")
    }
}

// MARK: - #384 per-row repo label visibility (source wiring, bundled source)

/// Decoy-resistant source wiring over the bundled FleetViews source (the
/// #316 mechanism): while ANY repo pill is active (the reconciled filter is
/// not nil/'All') the per-row repo NAME label must disappear from agent
/// rows — the WorkspaceLine then renders only a COLOR-ONLY hue echo that
/// keeps the label chip's exact height (caption2 line-box spacer + chip
/// padding: rows do not jump), and
/// the branch/basename lead the line without a stray separator dot. The
/// flag re-derives SYNCHRONOUSLY from the same pure reconcile that filters
/// the board (no @State, no timer), so tapping 'All' restores the label
/// chip instantly. A compile-capable bypass of any hop — a hardcoded flag,
/// an ungated chip, a decoy echo carrying text — goes RED here.
final class RepoRowLabelWiringTests: XCTestCase {

    private func bundledSource() throws -> String {
        let bundle = Bundle(for: RepoRowLabelWiringTests.self)
        let url = try XCTUnwrap(bundle.url(forResource: "FleetViews",
                                           withExtension: "swift.txt"))
        return try String(contentsOf: url, encoding: .utf8)
    }

    private func slice(from startMarker: String, to endMarker: String,
                       in source: String) throws -> String {
        let start = try XCTUnwrap(source.range(of: startMarker),
                                  "marker not found: \\(startMarker)")
        let end = try XCTUnwrap(source.range(of: endMarker),
                                "end marker not found: \\(endMarker)")
        return String(source[start.lowerBound..<end.lowerBound])
    }

    func testRowLabelHidingDerivesFromTheActiveRepoPillAndReachesEveryRow()
        throws {
        let source = try bundledSource()
        // The hiding flag is a plain body derivation over the SAME
        // reconciled filter that drives the sections ('All'/nil = false).
        XCTAssertEqual(source.components(separatedBy:
            "let rowRepoLabelsHidden = activeRepoFilter != nil").count - 1, 1,
            "the row-label flag must derive once from the reconciled repo filter")
        // Exactly three mentions: the derivation + the two boardSections
        // call sites (demo + live). A hardcoded/extra state decoy REDs.
        XCTAssertEqual(source.components(separatedBy: "rowRepoLabelsHidden").count - 1, 3,
                       "every boardSections call site must consume the derived flag")
        XCTAssertEqual(source.components(separatedBy:
            "hideRepoLabels: rowRepoLabelsHidden").count - 1, 2,
            "demo AND live boards must both thread the flag")
        // boardSections threads the flag into every agent row.
        let renderer = try slice(from: "private func boardSections(sections: BoardModel.Sections",
                                 to: "private func repoSubgroupHeader(",
                                 in: source)
        XCTAssertEqual(renderer.components(separatedBy:
            "agentRow(agent, repos: repos,").count - 1, 1,
            "exactly one agentRow call site must exist in the board renderer")
        XCTAssertTrue(renderer.contains("hideRepoLabel: hideRepoLabels"),
                      "the board must pass the hide flag into every agent row")
        // agentRow passes the flag into AgentRow (one hop inside the row
        // builder) and AgentRow passes it into WorkspaceLine — two call
        // sites file-wide share the same spelling.
        let row = try slice(from: "private func agentRow(_ agent: Agent",
                            to: "/// Board chrome: connection status",
                            in: source)
        XCTAssertEqual(row.components(separatedBy: "hideRepoLabel: hideRepoLabel)").count - 1, 1,
                       "agentRow must pass the flag into AgentRow")
        XCTAssertEqual(source.components(separatedBy:
            "hideRepoLabel: hideRepoLabel)").count - 1, 2,
            "AgentRow must pass the flag into WorkspaceLine (one extra hop)")
    }

    func testUnderAllTheRowKeepsItsRepoNameLabelChip() throws {
        let source = try bundledSource()
        let line = try slice(from: "struct WorkspaceLine: View {",
                             to: "// MARK: - #364 A touch feedback",
                             in: source)
        // The flag defaults to false (no filter = 'All'), and the label
        // chip renders in the else branch of the hide gate.
        XCTAssertTrue(line.contains("var hideRepoLabel: Bool = false"),
                      "WorkspaceLine must default to showing the label ('All')")
        let gate = try XCTUnwrap(line.range(of: "if hideRepoLabel {"),
                                 "the hide gate must exist in WorkspaceLine")
        let chip = try XCTUnwrap(line.range(of: "RepoLabelChip(repo: w.repo, repos: repos)"),
                                 "the repo label chip must still be wired")
        let branchStart = try XCTUnwrap(line.range(of: "if let branch = w.branch {"),
                                        "the branch segment must follow the repo slot")
        XCTAssertLessThan(gate.lowerBound, chip.lowerBound,
                          "the chip must render only after the hide gate opens")
        XCTAssertLessThan(chip.lowerBound, branchStart.lowerBound,
                          "the chip must sit in the else branch, before the branch segment")
        XCTAssertEqual(line.components(separatedBy:
            "RepoLabelChip(repo: w.repo, repos: repos)").count - 1, 1,
            "exactly one repo label chip call may exist (#371 pin retained)")
        // The branch/basename separators are BETWEEN segments only: when
        // the chip is hidden the branch LEADS the line (no stray dot).
        XCTAssertTrue(line.contains("if !hideRepoLabel {\n                    segmentSeparator"),
                      "the branch must not take a leading separator when the label is hidden")
        XCTAssertTrue(line.contains("if !hideRepoLabel || w.branch != nil {"),
                      "a lone basename must not take a leading separator when hidden")
    }

    func testHiddenLabelIsAColorOnlyEchoThatKeepsTheRowHeight() throws {
        let source = try bundledSource()
        let line = try slice(from: "struct WorkspaceLine: View {",
                             to: "// MARK: - #364 A touch feedback",
                             in: source)
        // The hidden branch routes to the color-only echo (never a text
        // label, never the chip chrome).
        XCTAssertTrue(line.contains("repoColorEcho(for: w.repo)"),
                      "the hidden branch must render the color-only echo")
        let echo = try slice(from: "private func repoColorEcho(for repo: String?) -> some View {",
                             to: "/// The worktree basename (D26)",
                             in: source)
        XCTAssertTrue(echo.contains(".fill(theme.repoHueColor(for: repo ?? \"\", among: repos))"),
                      "the echo must use the same deterministic repo hue as the label chip")
        XCTAssertEqual(echo.components(separatedBy: "Text(").count - 1, 1,
                       "the echo may carry ONLY the invisible spacer Text — "
                       + "any visible repo-name Text goes RED")
        XCTAssertFalse(echo.contains("Capsule()"),
                       "the echo must not fake a chip shell")
        XCTAssertTrue(echo.contains(".frame(width: 6, height: 6)"),
                      "the echo must be the small hue dot")
        XCTAssertTrue(echo.contains("Text(\" \").font(.caption2.weight(.bold)).opacity(0)"),
                      "the echo must keep the label chip's caption2 line box "
                      + "(transparent spacer — opacity is purely visual and "
                      + "deterministically keeps layout) so rows never jump "
                      + "on pill toggle")
        XCTAssertTrue(echo.contains(".padding(.vertical, 2)"),
                      "the echo must keep the label chip's vertical padding footprint")
        XCTAssertTrue(echo.contains(".accessibilityHidden(true)"),
                      "the pure-color echo must not add VoiceOver noise")
    }
}

// MARK: - #388 Settings Connection inputs: themed fields + paired-state switching

/// Model-level pins for the #388 registration-state predicate the Settings
/// Connection section reads: the device is registered once it holds an
/// identity key (set by a successful registration / restored live identity,
/// cleared by Remove device) — the token field is pointless exactly when
/// this is true.
@MainActor
final class ConnectionRegistrationModelTests: XCTestCase {

    private func makeModel() -> AppModel {
        // SAFETY: a fresh UUID-based suite name is always a valid suite.
        let defaults = UserDefaults(suiteName: "corral.connregistration.\(UUID().uuidString)")!
        return AppModel(identityLifecycle: IdentityLifecycle(),
                        defaults: defaults,
                        identityLoader: {
                            (DeviceSigner(key: Curve25519.Signing.PrivateKey()),
                             .insecureFallback)
                        },
                        loadMeta: { nil }, saveMeta: { _ in }, wipeIdentity: {})
    }

    func testIsRegisteredTracksTheIdentityKey() {
        let model = makeModel()
        // A fresh device (no restored identity) is NOT registered.
        XCTAssertFalse(model.isRegistered,
                       "a device without an identity key must not be registered")
        // Successful registration stores the daemon-issued key id.
        model.keyId = "dev_388a1b2c3d4e5f60"
        XCTAssertTrue(model.isRegistered,
                      "holding the daemon-issued key id IS the registered state")
        // Remove device (and the unpaired fallback) clears it again.
        model.keyId = nil
        XCTAssertFalse(model.isRegistered,
                       "clearing the key id must return the unpaired state")
    }
}

/// Decoy-resistant #388 source wiring over the bundled FleetViews source
/// (the #316 mechanism): the Settings Connection section hides the
/// Registration-token field while the device is REGISTERED — the host
/// field stays (still editable, still themed), a registration status row +
/// a small Re-register action replace the token/Register rows, and
/// Re-register reveals the token field again. Both inputs render through
/// the shared ConnectionField surface (surface1 fill, text ink, subtext0
/// placeholder, 10 pt radius, hairline surface2 border tinting to the
/// accent while focused — tokens only, no hex literals, no default
/// rounded-border boxes). A compile-capable bypass of any hop goes RED.
final class ConnectionSectionWiringTests: XCTestCase {

    private func bundledSource() throws -> String {
        let bundle = Bundle(for: ConnectionSectionWiringTests.self)
        let url = try XCTUnwrap(bundle.url(forResource: "FleetViews",
                                           withExtension: "swift.txt"))
        return try String(contentsOf: url, encoding: .utf8)
    }

    private func lineNumbers(of needle: String, in text: String) -> [Int] {
        text.split(separator: "\n", omittingEmptySubsequences: false)
            .enumerated()
            .filter { $0.element.contains(needle) }
            .map { $0.offset + 1 }
    }

    /// The Settings sheet's own region: SettingsView → How-to-connect MARK.
    private func settingsSlice(_ source: String) throws -> String {
        let start = try XCTUnwrap(source.range(of: "\nstruct SettingsView: View {"),
                                  "SettingsView declaration must exist")
        let end = try XCTUnwrap(source.range(of: "\n// MARK: - How to connect"),
                                "the How-to-connect MARK must follow SettingsView")
        return String(source[start.lowerBound..<end.lowerBound])
    }

    /// The shared themed input component's own region: ConnectionField →
    /// the SettingsView doc comment that follows it.
    private func fieldSlice(_ source: String) throws -> String {
        let start = try XCTUnwrap(source.range(of: "\nprivate struct ConnectionField: View {"),
                                  "ConnectionField declaration must exist")
        let end = try XCTUnwrap(source.range(
            of: "\n/// #365: the surface behind the board's always-visible gear"),
                                "the SettingsView doc comment must follow ConnectionField")
        return String(source[start.lowerBound..<end.lowerBound])
    }

    func testRegisteredDeviceHidesTheTokenFieldAndShowsTheStatusRow() throws {
        let slice = try settingsSlice(try bundledSource())
        // The paired branch is the positive gate on the model predicate
        // PLUS the local reveal override — exactly once (a decoy gate
        // elsewhere in the sheet goes RED on the count).
        let gate = try XCTUnwrap(slice.range(of: "if model.isRegistered && !revealTokenField {"),
                                 "the Connection section must gate on the registered state")
        XCTAssertEqual(slice.components(separatedBy:
            "if model.isRegistered && !revealTokenField {").count - 1, 1,
            "exactly one registered-state gate may exist in the Settings sheet")

        // The paired branch (gate → its `} else {`) carries the status row
        // + Re-register and NO input fields, NO pairing action.
        let afterGate = slice[gate.lowerBound...]
        let elseRange = try XCTUnwrap(afterGate.range(of: "} else {"),
                                      "the registered branch must have an else")
        let paired = String(afterGate[..<elseRange.lowerBound])
        let statusLines = lineNumbers(of: "Device registered · Key ID ", in: paired)
        XCTAssertEqual(statusLines.count, 1,
                       "the paired section must show exactly one registration status row")
        guard let statusLine = statusLines.first else { return }
        let statusRow = String(paired.split(separator: "\n",
                                            omittingEmptySubsequences: false)[statusLine - 1])
        XCTAssertTrue(statusRow.contains("read-only signed"),
                      "the status copy must live on ONE row with the key id")
        XCTAssertTrue(paired.contains("deviceKeyID"),
                      "the status row must interpolate the device key id")
        XCTAssertTrue(paired.contains("Button(\"Re-register\")"),
                      "the paired section must offer the Re-register action")
        XCTAssertTrue(paired.contains("revealTokenField = true"),
                      "Re-register must reveal the token field")
        XCTAssertFalse(paired.contains("ConnectionField"),
                       "the paired section must render NO input fields")
        XCTAssertFalse(paired.contains("model.register("),
                       "the paired section must not show the Register action")

        // The unpaired/revealed branch keeps the themed token field + the
        // real Register action + the enable rules.
        let unpaired = String(afterGate[elseRange.upperBound...])
        XCTAssertTrue(unpaired.contains("ConnectionField(title: \"Registration token\""),
                      "the token field must return in the unpaired/revealed state")
        XCTAssertTrue(unpaired.contains("model.register(host: host, token: token)"),
                      "the Register action must route through the real registration flow")
        XCTAssertTrue(unpaired.contains("host.isEmpty || token.isEmpty || registering"),
                      "the Register action must disable on empty host/token")
    }

    func testHostFieldStaysEditableAheadOfTheStateGate() throws {
        let slice = try settingsSlice(try bundledSource())
        // The host field renders unconditionally and comes FIRST — a
        // paired device can re-point without retyping the active host.
        let hostLines = lineNumbers(
            of: "ConnectionField(title: \"Host (Tailscale host or loopback)\"", in: slice)
        XCTAssertEqual(hostLines.count, 1,
                       "exactly one host field may exist in the Settings sheet")
        let gateLine = try XCTUnwrap(lineNumbers(
            of: "if model.isRegistered && !revealTokenField {", in: slice).first)
        XCTAssertLessThan(hostLines[0], gateLine,
                          "the host field must precede the paired-state gate")
        // The reveal override is sheet-local state that starts false.
        XCTAssertEqual(lineNumbers(of: "@State private var revealTokenField = false",
                                   in: slice).count, 1,
                       "the reveal override must be sheet-local state")
    }

    func testConnectionFieldsConsumeThemeTokensNotDefaultRoundedBorders() throws {
        let source = try bundledSource()
        let slice = try settingsSlice(source)
        // The Settings Connection inputs no longer use the default
        // rounded-border chrome (the observed square near-black boxes).
        XCTAssertEqual(lineNumbers(of: ".textFieldStyle(.roundedBorder)", in: slice).count, 0,
                       "the Settings Connection inputs must not use .roundedBorder (#388)")

        let field = try fieldSlice(source)
        // Surface + ink + placeholder tokens, 10 pt continuous radius,
        // hairline border, focus accent.
        XCTAssertTrue(field.contains(".background(theme.surface1,"),
                      "the field surface must be the surface1 token")
        XCTAssertEqual(lineNumbers(of: "cornerRadius: 10, style: .continuous)",
                                   in: field).count, 2,
                       "fill AND border must share the 10 pt continuous radius")
        XCTAssertTrue(field.contains(".strokeBorder(focused ? theme.accent : theme.surface2,"),
                      "the hairline border must be surface2, accent while focused")
        XCTAssertTrue(field.contains("lineWidth: 1"),
                      "the border must be a 1 pt hairline")
        // Focus drives the accent border through per-field focus state.
        XCTAssertEqual(lineNumbers(of: "@FocusState private var focused: Bool",
                                   in: field).count, 1,
                       "the field must own per-field focus state")
        XCTAssertEqual(lineNumbers(of: ".focused($focused)", in: field).count, 2,
                       "both the text and the secure variants must observe focus")
        // Ink: theme.text; placeholder: subtext0 drawn only while empty.
        XCTAssertEqual(lineNumbers(of: ".foregroundStyle(theme.text)", in: field).count, 2,
                       "both field variants must render text ink")
        let emptyLine = try XCTUnwrap(lineNumbers(of: "if text.isEmpty {", in: field).first)
        let titleLine = try XCTUnwrap(lineNumbers(of: "Text(title)", in: field).first)
        let placeholderLine = try XCTUnwrap(lineNumbers(
            of: ".foregroundStyle(theme.subtext0)", in: field).first)
        XCTAssertLessThan(emptyLine, titleLine,
                          "the placeholder must render only when the field is empty")
        XCTAssertLessThan(titleLine, placeholderLine,
                          "the placeholder text must take the subtext0 token")
        XCTAssertEqual(lineNumbers(of: ".tint(theme.accent)", in: field).count, 2,
                       "the caret/selection tint must ride the accent token")
        XCTAssertTrue(field.contains("SecureField(\"\""),
                      "the secure variant must exist for the token field")
        XCTAssertTrue(field.contains("TextField(\"\""),
                      "the text variant must exist for the host field")
        // Token-only rule: no hex literals, no raw color literals in the
        // component's CODE lines (doc comments may reference the issue).
        let codeLines = field.split(separator: "\n").filter { line in
            let trimmed = line.trimmingCharacters(in: .whitespaces)
            return !trimmed.hasPrefix("//")
        }
        for line in codeLines where line.contains("#") {
            XCTFail("ConnectionField must not carry literal colors: \(line)")
        }
    }

    func testConnectionInputsEvidenceDriverRecordsBothStatesAcrossPalettes() throws {
        let source = try bundledSource()
        // Six deterministic phases: unpaired + paired on Macchiato, Mocha,
        // and Latte, each behind its own marker.
        for marker in ["phase-1-settings-macchiato-unpaired",
                       "phase-2-settings-mocha-unpaired",
                       "phase-3-settings-latte-unpaired",
                       "phase-4-settings-latte-paired",
                       "phase-5-settings-mocha-paired",
                       "phase-6-settings-macchiato-paired"] {
            XCTAssertEqual(lineNumbers(of: marker, in: source).count, 1,
                           "the \(marker) evidence marker must be written exactly once")
        }
        // The paired phases seed the registration key id the section gates
        // on (no daemon on the sim) — exactly one seed.
        XCTAssertEqual(lineNumbers(of: "model.keyId = \"dev_3f88a1b2c3d4e5f6\"",
                                   in: source).count, 1,
                       "the driver must seed the demo registration key id exactly once")
    }
}

extension StringProtocol {
    /// First 1-based line number containing `needle` (source-wiring helper).
    fileprivate func lineNumber(of needle: String) -> Int? {
        for (index, line) in split(separator: "\n",
                                   omittingEmptySubsequences: false)
            .enumerated() where line.contains(needle) {
            return index + 1
        }
        return nil
    }
}

// MARK: - #389 push entitlement: permission mapping + enable flows

final class NotificationPermissionMappingTests: XCTestCase {
    func testMapsOSStatusesToPosture() {
        XCTAssertEqual(NotificationPermissionState(status: .notDetermined), .notDetermined)
        XCTAssertEqual(NotificationPermissionState(status: .denied), .denied)
        XCTAssertEqual(NotificationPermissionState(status: .authorized), .granted)
        XCTAssertEqual(NotificationPermissionState(status: .provisional), .granted)
        XCTAssertEqual(NotificationPermissionState(status: .ephemeral), .granted)
        // UNAuthorizationStatus has no .restricted member — the enum models
        // it (spec's blocked bucket) and only the provider can produce it.
    }

    func testBlockedGuidanceOnlyForDeniedAndRestricted() {
        XCTAssertTrue(NotificationPermissionState.denied.showsBlockedGuidance)
        XCTAssertTrue(NotificationPermissionState.restricted.showsBlockedGuidance)
        XCTAssertFalse(NotificationPermissionState.notDetermined.showsBlockedGuidance)
        XCTAssertFalse(NotificationPermissionState.granted.showsBlockedGuidance)
    }
}

/// #389: the Settings toggle's enable path is permission-aware — a blocked
/// permission (.denied/.restricted) NEVER silently enables (the state lands
/// in `notificationPermission` for the section's why + 'Open iOS Settings'
/// guidance) and .notDetermined prompts exactly once, enabling only on a
/// grant. The provider is stubbed so the real UNUserNotificationCenter (and
/// its system prompt) is never touched in the test host.
@MainActor
final class NotificationEnableModelTests: XCTestCase {

    private final class StubPermissionProvider: NotificationPermissionProviding,
                                                @unchecked Sendable {
        var status: NotificationPermissionState = .notDetermined
        var promptResult = false
        private(set) var promptCount = 0

        func currentPermission() async -> NotificationPermissionState {
            status
        }

        func requestAuthorization() async -> Bool {
            promptCount += 1
            return promptResult
        }
    }

    private var suiteName = ""

    private func makeModel(provider: StubPermissionProvider,
                           notificationsOn: Bool) -> AppModel {
        suiteName = "corral.notifications389.\(UUID().uuidString)"
        // SAFETY: a fresh UUID-based suite name is always a valid suite.
        let defaults = UserDefaults(suiteName: suiteName)!
        defaults.set(notificationsOn, forKey: AppModel.notificationsKey)
        return AppModel(defaults: defaults,
                        identityLoader: {
                            (DeviceSigner(key: Curve25519.Signing.PrivateKey()),
                             .insecureFallback)
                        },
                        loadMeta: { nil },
                        saveMeta: { _ in },
                        wipeIdentity: {},
                        notificationPermissionProvider: provider)
    }

    private func persisted(_ model: AppModel) -> Bool? {
        guard let suite = UserDefaults(suiteName: suiteName) else { return nil }
        return suite.object(forKey: AppModel.notificationsKey) as? Bool
    }

    private func waitUntil(_ condition: @escaping () -> Bool) async {
        for _ in 0..<150 where !condition() {
            try? await Task.sleep(for: .milliseconds(20))
        }
    }

    override func tearDown() {
        if !suiteName.isEmpty {
            UserDefaults(suiteName: suiteName)?.removePersistentDomain(forName: suiteName)
            suiteName = ""
        }
        super.tearDown()
    }

    func testEnableWhenGrantedPersistsImmediately() async {
        let provider = StubPermissionProvider()
        provider.status = .granted
        let model = makeModel(provider: provider, notificationsOn: false)

        model.setNotificationsEnabled(true)
        await waitUntil { model.notificationsEnabled }

        XCTAssertTrue(model.notificationsEnabled, "a granted permission enables right away")
        XCTAssertEqual(persisted(model), true, "the enable must persist")
        XCTAssertEqual(provider.promptCount, 0, "granted never prompts")
        XCTAssertEqual(model.notificationPermission, .granted)
    }

    func testEnableWhileDeniedStaysOffAndShowsBlockedState() async {
        let provider = StubPermissionProvider()
        provider.status = .denied
        let model = makeModel(provider: provider, notificationsOn: false)

        model.setNotificationsEnabled(true)
        await waitUntil { model.notificationPermission == .denied }

        XCTAssertFalse(model.notificationsEnabled,
                       "a denied permission must never silently enable the toggle")
        XCTAssertEqual(persisted(model), false)
        XCTAssertEqual(provider.promptCount, 0, "denied never prompts")
    }

    func testEnableWhileRestrictedStaysOffAndShowsBlockedState() async {
        let provider = StubPermissionProvider()
        provider.status = .restricted
        let model = makeModel(provider: provider, notificationsOn: false)

        model.setNotificationsEnabled(true)
        await waitUntil { model.notificationPermission == .restricted }

        XCTAssertFalse(model.notificationsEnabled,
                       "a restricted permission must never silently enable the toggle")
        XCTAssertEqual(persisted(model), false)
        XCTAssertEqual(provider.promptCount, 0, "restricted never prompts")
    }

    func testEnableWhenNotDeterminedPromptsOnceAndEnablesOnGrant() async {
        let provider = StubPermissionProvider()
        provider.status = .notDetermined
        provider.promptResult = true
        let model = makeModel(provider: provider, notificationsOn: false)

        model.setNotificationsEnabled(true)
        await waitUntil { model.notificationsEnabled }

        XCTAssertTrue(model.notificationsEnabled, "a grant after the prompt enables")
        XCTAssertEqual(persisted(model), true)
        XCTAssertEqual(provider.promptCount, 1, "exactly one prompt for .notDetermined")
        XCTAssertEqual(model.notificationPermission, .granted)
    }

    func testEnableWhenNotDeterminedAndPromptDeniedStaysOff() async {
        let provider = StubPermissionProvider()
        provider.status = .notDetermined
        provider.promptResult = false
        let model = makeModel(provider: provider, notificationsOn: false)

        model.setNotificationsEnabled(true)
        await waitUntil { model.notificationPermission == .denied }

        XCTAssertFalse(model.notificationsEnabled, "a prompt denial must not enable")
        XCTAssertEqual(persisted(model), false)
        XCTAssertEqual(provider.promptCount, 1)
        XCTAssertEqual(model.notificationPermission, .denied,
                       "the prompt denial lands in the blocked guidance state")
    }

    func testRepeatedEnableDoesNotPromptAgainOnceAlreadyOn() async {
        let provider = StubPermissionProvider()
        provider.status = .notDetermined
        provider.promptResult = true
        let model = makeModel(provider: provider, notificationsOn: false)

        model.setNotificationsEnabled(true)
        await waitUntil { model.notificationsEnabled }
        XCTAssertEqual(provider.promptCount, 1)

        // Second enable while already ON is a no-op — no second prompt.
        model.setNotificationsEnabled(true)
        try? await Task.sleep(for: .milliseconds(100))
        XCTAssertEqual(provider.promptCount, 1, "an already-enabled toggle must not re-prompt")
    }

    func testDisableStaysInstantAndUnconditional() async {
        let provider = StubPermissionProvider()
        provider.status = .denied
        let model = makeModel(provider: provider, notificationsOn: true)

        model.setNotificationsEnabled(false)

        XCTAssertFalse(model.notificationsEnabled, "disabling is immediate")
        XCTAssertEqual(persisted(model), false)
    }

    func testRefreshNotificationPermissionPublishesProviderStatus() async {
        let provider = StubPermissionProvider()
        provider.status = .restricted
        let model = makeModel(provider: provider, notificationsOn: false)

        await model.refreshNotificationPermission()

        XCTAssertEqual(model.notificationPermission, .restricted)
    }
}

// MARK: - #389: Settings Notifications section wiring (source pins)

/// Source-wiring pins over the bundled `FleetViews.swift.txt`: the Settings
/// Notifications section must (a) derive its toggle from the permission
/// posture (a blocked permission displays OFF and routes through the
/// permission-aware setter), (b) show the blocked guidance + 'Open iOS
/// Settings' action on .denied/.restricted instead of a silent caption, and
/// (c) refresh the permission when Settings appears. Decoy rule: the pins
/// are structurally ordered inside the SettingsView slice — a helper
/// anywhere else carrying the strings does not satisfy the ordering.
final class SettingsNotificationWiringTests: XCTestCase {

    private func bundledSource() throws -> String {
        let bundle = Bundle(for: SettingsNotificationWiringTests.self)
        let url = try XCTUnwrap(bundle.url(forResource: "FleetViews",
                                           withExtension: "swift.txt"))
        return try String(contentsOf: url, encoding: .utf8)
    }

    private func lineNumbers(of needle: String, in text: String) -> [Int] {
        text.split(separator: "\n", omittingEmptySubsequences: false)
            .enumerated()
            .filter { $0.element.contains(needle) }
            .map { $0.offset + 1 }
    }

    /// The Settings sheet's own region: SettingsView → How-to-connect MARK.
    private func settingsSlice(_ source: String) throws -> String {
        let start = try XCTUnwrap(source.range(of: "\nstruct SettingsView: View {"),
                                  "SettingsView declaration must exist")
        let end = try XCTUnwrap(source.range(of: "\n// MARK: - How to connect"),
                                "the How-to-connect MARK must follow SettingsView")
        return String(source[start.lowerBound..<end.lowerBound])
    }

    private func firstLine(of needle: String, in text: String) throws -> Int {
        try XCTUnwrap(lineNumbers(of: needle, in: text).first,
                      "\(needle) must exist in the slice")
    }

    func testNotificationsSectionShowsBlockedGuidanceWithOpenSettingsAction() throws {
        let slice = try settingsSlice(try bundledSource())
        let section = try firstLine(of: "Section(\"Notifications\")", in: slice)
        let toggle = try firstLine(of: "Toggle(\"State-change notifications\"", in: slice)
        let blockedGate = try firstLine(
            of: "model.notificationPermission.showsBlockedGuidance", in: slice)
        let why = try firstLine(of: "Corral can't alert you", in: slice)
        let openAction = try firstLine(of: "Button(\"Open iOS Settings\")", in: slice)
        let caption = try firstLine(of: "No badges or catch-up.", in: slice)
        let refresh = try firstLine(
            of: ".task { await model.refreshNotificationPermission() }", in: slice)

        XCTAssertLessThan(section, toggle, "the section must contain the toggle")
        // The toggle's DISPLAYED value derives from the permission posture:
        // the blocked gate sits inside the Binding get, before the setter.
        let setter = try firstLine(of: "model.setNotificationsEnabled(", in: slice)
        let anchor = try firstLine(of: ".id(\"settings.notifications\")", in: slice)
        XCTAssertLessThan(toggle, blockedGate, "the binding get must consult the blocked gate")
        XCTAssertLessThan(blockedGate, setter, "the gate precedes the setter in the binding")
        XCTAssertLessThan(setter, anchor,
                          "the scroll anchor must sit on the toggle row, after the setter")
        // The blocked branch shows WHY then the action; the plain caption
        // only survives on the UNblocked branch (after the action).
        XCTAssertLessThan(blockedGate, why)
        XCTAssertLessThan(anchor, why, "the blocked branch must follow the toggle row")
        XCTAssertLessThan(why, openAction, "the 'why' row must precede the Open iOS Settings action")
        XCTAssertLessThan(openAction, caption,
                          "the plain caption must live on the unblocked branch, after the action")
        XCTAssertLessThan(caption, refresh, "the section must refresh the permission on appear")
        // The action routes through the canonical system-Settings URL.
        XCTAssertEqual(lineNumbers(of: "UIApplication.openSettingsURLString", in: slice).count, 1,
                       "the Open iOS Settings action must use the canonical URL exactly once")
        XCTAssertTrue(slice.contains("openAppSettings()"),
                      "the action must route through the openAppSettings helper")
    }

    func testDeniedNotificationsEvidenceDriverWritesUniqueMarkers() throws {
        let source = try bundledSource()
        for marker in ["phase-1-denied-mocha-board",
                       "phase-2-denied-settings-notifications",
                       "phase-3-denied-done"] {
            XCTAssertEqual(lineNumbers(of: marker, in: source).count, 1,
                           "the \(marker) evidence marker must be written exactly once")
        }
        XCTAssertEqual(lineNumbers(of: "private func runDeniedNotificationsSequence()",
                                   in: source).count, 1)
        XCTAssertEqual(lineNumbers(of: "await runDeniedNotificationsSequence()",
                                   in: source).count, 1,
                       "the driver must be dispatched from runDemoEvidenceIfNeeded exactly once")
        // The Settings scroll stand-in reaches the SAME anchor the section
        // carries, so the captured frame really shows the Notifications row.
        XCTAssertEqual(lineNumbers(of: ".id(\"settings.notifications\")", in: source).count, 1)
        XCTAssertEqual(lineNumbers(of: "proxy.scrollTo(\"settings.notifications\"",
                                   in: source).count, 1)
    }
}

// MARK: - #389: receiveDeviceToken → daemon registry upload (D16 path)

/// #389 AC: the APNs token callback path — `AppDelegate.receiveDeviceToken`
/// must enroll the token on the daemon (signed POST /device-token) once per
/// token, and a duplicate OS callback for the SAME identity + token is
/// suppressed (DeviceTokenState). The delegate's injectable URLSession +
/// IdentityLifecycle + signer let the upload run against a URLProtocol stub
/// — no APNs, no keychain.
@MainActor
final class DeviceTokenUploadTests: XCTestCase {

    private final class TokenUploadURLProtocol: URLProtocol {
        private static let lock = NSLock()
        private static var requestsStorage: [URLRequest] = []

        static var requests: [URLRequest] {
            lock.lock()
            defer { lock.unlock() }
            return requestsStorage
        }

        static func reset() {
            lock.lock()
            defer { lock.unlock() }
            requestsStorage = []
        }

        override class func canInit(with request: URLRequest) -> Bool { true }
        override class func canonicalRequest(for request: URLRequest) -> URLRequest { request }

        override func startLoading() {
            Self.lock.lock()
            // Materialize the body (URLSession may deliver it as a stream —
            // see requestBodyData) so assertions read it off the copy.
            var copy = request
            copy.httpBody = requestBodyData(request)
            Self.requestsStorage.append(copy)
            Self.lock.unlock()
            guard let url = request.url else {
                client?.urlProtocol(self, didFailWithError: URLError(.badURL))
                return
            }
            // SAFETY: fixed literal URL + HTTP status of a test-only response.
            let response = HTTPURLResponse(url: url, statusCode: 200,
                                           httpVersion: nil, headerFields: nil)!
            client?.urlProtocol(self, didReceive: response, cacheStoragePolicy: .notAllowed)
            client?.urlProtocol(self, didLoad: Data(#"{"ok":true}"#.utf8))
            client?.urlProtocolDidFinishLoading(self)
        }

        override func stopLoading() {}
    }

    private func uploadSession() -> URLSession {
        TokenUploadURLProtocol.reset()
        let config = URLSessionConfiguration.ephemeral
        config.protocolClasses = [TokenUploadURLProtocol.self]
        return URLSession(configuration: config)
    }

    private func makeDelegate(signer: DeviceSigner) -> AppDelegate {
        let lifecycle = IdentityLifecycle()
        lifecycle.setCurrent(mode: .live,
                             hostURL: URL(string: "http://daemon"),
                             keyId: "dev_1",
                             signerPublicKeyB64: signer.publicKeyB64)
        return AppDelegate(identityLifecycle: lifecycle,
                           session: uploadSession(),
                           identityProvider: { signer })
    }

    func testReceiveDeviceTokenUploadsSignedTokenToDaemon() async throws {
        let signer = DeviceSigner(key: Curve25519.Signing.PrivateKey())
        let delegate = makeDelegate(signer: signer)
        defer { AppDelegate.apnsRegistered = false }

        // #389 r1: low-entropy deterministic fixture — gitleaks' generic-api-key
        // flagged the original 64-hex literal (high entropy) even though the
        // value is only ever asserted against itself. The token is any
        // 64-hex APNs-token-shaped string the OS could deliver.
        let token = String(repeating: "cafe1234", count: 8)
        let task = delegate.receiveDeviceToken(token)
        await task?.value

        XCTAssertTrue(AppDelegate.apnsRegistered,
                      "a received token marks the device as APNs-registered")
        let requests = TokenUploadURLProtocol.requests
        XCTAssertEqual(requests.map { $0.url?.path }, ["/device-token"],
                       "exactly one signed upload must reach the daemon")
        let body = try XCTUnwrap(requests.first?.httpBody,
                                 "the upload must carry the canonical body")
        let json = try XCTUnwrap(try JSONSerialization.jsonObject(with: body)
                                 as? [String: Any])
        XCTAssertEqual(json["key_id"] as? String, "dev_1")
        XCTAssertFalse((json["signature"] as? String)?.isEmpty ?? true,
                       "the upload must be signed proof of possession")
        let request = try XCTUnwrap(json["request"] as? [String: Any],
                                    "the canonical request bytes ride in the body")
        XCTAssertEqual(request["device_token"] as? String, token,
                       "the canonical request must carry the APNs token")
        XCTAssertEqual(request["key_id"] as? String, "dev_1")
    }

    func testDuplicateCallbackForSameTokenIsSuppressed() async throws {
        let signer = DeviceSigner(key: Curve25519.Signing.PrivateKey())
        let delegate = makeDelegate(signer: signer)
        defer { AppDelegate.apnsRegistered = false }

        let token = "cafe1234cafe1234cafe1234cafe1234cafe1234cafe1234cafe1234cafe1234"
        let first = delegate.receiveDeviceToken(token)
        let second = delegate.receiveDeviceToken(token)
        await first?.value
        await second?.value

        XCTAssertEqual(TokenUploadURLProtocol.requests.map { $0.url?.path },
                       ["/device-token"],
                       "a duplicate OS callback for the same identity + token must not re-upload")
    }
}

// MARK: - #399 host identity + host-profile store/trust

/// X25519 host-key form validation + fingerprint derivation (B3).
final class HostKeyTrustTests: XCTestCase {
    /// SAFETY: 32 zero bytes are a valid X25519 public-key byte string
    /// (any 32-byte value is a valid X25519 public key input); fixture.
    private static let zeroKeyB64 = Data(repeating: 0, count: 32).base64EncodedString()
    /// SAFETY: 32 bytes of 0xAB; fixture only.
    private static let abKeyB64 = Data(repeating: 0xAB, count: 32).base64EncodedString()

    private func response(key: String, algorithm: String = "X25519") -> HostKeyResponse {
        HostKeyResponse(algorithm: algorithm, publicKey: key, note: nil)
    }

    func testWellFormedX25519KeyPasses() {
        XCTAssertTrue(HostKeyTrust.isWellFormed(response(key: Self.zeroKeyB64)))
    }

    func testWrongAlgorithmIsRejected() {
        XCTAssertFalse(HostKeyTrust.isWellFormed(response(key: Self.zeroKeyB64,
                                                          algorithm: "Ed25519")))
    }

    func testMalformedKeysAreRejected() {
        XCTAssertFalse(HostKeyTrust.isWellFormed(response(key: "not-base64!")))
        XCTAssertFalse(HostKeyTrust.isWellFormed(
            response(key: Data(repeating: 0, count: 31).base64EncodedString())))
        XCTAssertFalse(HostKeyTrust.isWellFormed(response(key: "")))
    }

    func testFingerprintIsStableGroupedAndKeySpecific() throws {
        let first = try XCTUnwrap(HostKeyTrust.fingerprint(forBase64: Self.zeroKeyB64))
        let again = try XCTUnwrap(HostKeyTrust.fingerprint(forBase64: Self.zeroKeyB64))
        let other = try XCTUnwrap(HostKeyTrust.fingerprint(forBase64: Self.abKeyB64))
        XCTAssertEqual(first, again, "fingerprint must be deterministic")
        XCTAssertNotEqual(first, other, "different keys must fingerprint differently")
        // 64 uppercase hex chars in 4-char groups.
        XCTAssertEqual(first.filter { $0 != " " }.count, 64)
        XCTAssertEqual(first.split(separator: " ").count, 16)
        XCTAssertEqual(first, first.uppercased())
        XCTAssertNil(HostKeyTrust.fingerprint(forBase64: "bad-key"))
    }

    func testMatchIsExactOnTheFullPinnedKey() {
        let keyA = response(key: Self.zeroKeyB64)
        let keyB = response(key: Self.abKeyB64)
        XCTAssertTrue(HostKeyTrust.matches(keyA, pinnedKeyB64: Self.zeroKeyB64))
        XCTAssertFalse(HostKeyTrust.matches(keyB, pinnedKeyB64: Self.zeroKeyB64))
        XCTAssertFalse(HostKeyTrust.matches(keyB, pinnedKeyB64: "malformed"))
    }
}

/// URL normalization for host profiles (B1): https default, loopback
/// http tolerated for dev, duplicates collapse to the same string.
final class HostURLFormTests: XCTestCase {
    func testSchemeLessInputBecomesHTTPS() {
        XCTAssertEqual(HostURLForm.normalized("host.tail1234.ts.net"),
                       "https://host.tail1234.ts.net")
        XCTAssertEqual(HostURLForm.normalized("macbook-pro"),
                       "https://macbook-pro")
    }

    func testTrailingSlashAndDefaultPortAreNormalized() {
        XCTAssertEqual(HostURLForm.normalized("https://mac.tail1234.ts.net/"),
                       "https://mac.tail1234.ts.net")
        XCTAssertEqual(HostURLForm.normalized("https://mac.tail1234.ts.net:443"),
                       "https://mac.tail1234.ts.net")
        XCTAssertEqual(HostURLForm.normalized("HTTPS://MAC.TAIL1234.TS.NET"),
                       "https://mac.tail1234.ts.net")
    }

    func testLoopbackHTTPAllowedButRemoteHTTPRejected() {
        XCTAssertEqual(HostURLForm.normalized("http://127.0.0.1:8474"),
                       "http://127.0.0.1:8474")
        XCTAssertEqual(HostURLForm.normalized("http://localhost:8474"),
                       "http://localhost:8474")
        XCTAssertNil(HostURLForm.normalized("http://10.0.0.5:8474"),
                     "plain http to a remote host is refused by Add Host")
        XCTAssertNil(HostURLForm.normalized("http://mac.tail1234.ts.net"))
    }

    func testLegacyMigrationPreservesPlainHTTPForExistingDaemons() {
        XCTAssertEqual(HostURLForm.normalizedForLegacyMigration("http://10.0.0.5:8474"),
                       "http://10.0.0.5:8474")
        XCTAssertEqual(HostURLForm.normalizedForLegacyMigration("http://mac.tail1234.ts.net"),
                       "http://mac.tail1234.ts.net")
    }

    func testGarbageIsRejected() {
        XCTAssertNil(HostURLForm.normalized(""))
        XCTAssertNil(HostURLForm.normalized("not a url with spaces"))
        XCTAssertNil(HostURLForm.normalized("ftp://host"))
        XCTAssertNil(HostURLForm.normalized("https://"))
    }

    func testDisplayNameCandidateUsesFirstHostLabel() {
        XCTAssertEqual(HostURLForm.displayNameCandidate(for: "https://mac-pro.tail1234.ts.net"),
                       "mac-pro")
        XCTAssertEqual(HostURLForm.displayNameCandidate(for: "mac-pro"), "mac-pro")
    }
}

/// Host-profile store semantics (B1-B7): add/duplicates/rename/remove,
/// per-profile cursors + key-id scoping, and legacy migration.
final class HostProfileStoreTests: XCTestCase {
    /// SAFETY: fixed valid 32-byte X25519 public-key fixtures.
    static let keyA = Data(repeating: 7, count: 32).base64EncodedString()
    /// SAFETY: fixed valid 32-byte X25519 public-key fixtures.
    static let keyB = Data(repeating: 8, count: 32).base64EncodedString()

    /// In-memory store (nil directory) for store-level tests.
    private func makeStore() -> HostProfileStore {
        HostProfileStore(directory: nil, defaults: .standard)
    }

    func testAddProfileOrdersAndRejectsDuplicates() throws {
        let store = makeStore()
        let a = try store.addProfile(displayName: "Mac",
                                     urlString: "mac.tail1234.ts.net",
                                     registeredAt: 1)
        let b = try store.addProfile(displayName: "Bazzite",
                                     urlString: "https://bazzite.tail1234.ts.net",
                                     hostKeyB64: Self.keyB,
                                     registeredAt: 2)
        XCTAssertEqual(store.orderedProfiles.map(\.displayName), ["Mac", "Bazzite"])
        XCTAssertEqual(a.order, 0)
        XCTAssertEqual(b.order, 1)
        // Empty + duplicate names rejected.
        XCTAssertThrowsError(try store.addProfile(displayName: "   ",
                                                  urlString: "https://x.example",
                                                  registeredAt: 3))
        XCTAssertThrowsError(try store.addProfile(displayName: "mac",
                                                  urlString: "https://y.example",
                                                  registeredAt: 3))
        // Duplicate normalized URL rejected (scheme-less + trailing slash
        // forms collapse onto the existing record).
        XCTAssertThrowsError(try store.addProfile(displayName: "Other",
                                                  urlString: "https://mac.tail1234.ts.net/",
                                                  registeredAt: 3))
        // Duplicate pinned host identity rejected.
        XCTAssertThrowsError(try store.addProfile(displayName: "Other",
                                                  urlString: "https://other.example",
                                                  hostKeyB64: Self.keyB,
                                                  registeredAt: 3))
        // Invalid URL + remote plain http rejected by the Add Host form.
        XCTAssertThrowsError(try store.addProfile(displayName: "Nope",
                                                  urlString: "http://remote.example",
                                                  registeredAt: 3))
    }

    func testRenameIsInPlaceAndURLIsImmutable() throws {
        let store = makeStore()
        let a = try store.addProfile(displayName: "Mac",
                                     urlString: "https://mac.example",
                                     registeredAt: 1)
        let renamed = try store.renameProfile(id: a.id, to: "MacBook Pro")
        XCTAssertEqual(renamed.displayName, "MacBook Pro")
        XCTAssertEqual(renamed.id, a.id, "rename must not mint a new identity")
        XCTAssertEqual(renamed.urlString, "https://mac.example")
        // Empty + duplicate renames rejected.
        XCTAssertThrowsError(try store.renameProfile(id: a.id, to: "  "))
        try store.addProfile(displayName: "Bazzite",
                             urlString: "https://bazzite.example",
                             registeredAt: 2)
        XCTAssertThrowsError(try store.renameProfile(id: a.id, to: "bazzite"))
        // No URL/identity mutation API exists (remove-and-re-pair only).
        let profile = try XCTUnwrap(store.profile(id: a.id))
        XCTAssertNil(profile.hostKeyB64)
    }

    func testFingerprintConfirmationPinsKeyAndLiftsPause() throws {
        let store = makeStore()
        let migrated = try XCTUnwrap(
            store.migrateLegacy(host: "https://mac.example", keyId: "dev_legacy",
                                grants: ["read_tail"], expiryTs: 1_800_000_000,
                                registeredAt: 1))
        XCTAssertEqual(migrated.connectionState,
                       .awaitingFingerprintConfirmation)
        XCTAssertNil(migrated.hostKeyB64)
        let pinned = try store.confirmFingerprint(id: migrated.id,
                                                  hostKeyB64: Self.keyA,
                                                  fingerprint: "F1")
        XCTAssertEqual(pinned.hostKeyB64, Self.keyA)
        XCTAssertEqual(pinned.fingerprint, "F1")
        XCTAssertEqual(pinned.connectionState, .disconnected)
        // A second profile cannot pin the SAME identity: confirming the
        // same key on another profile must throw the duplicate-identity
        // error.
        let other = try store.addProfile(displayName: "Other",
                                         urlString: "https://other.example",
                                         hostKeyB64: Self.keyB,
                                         registeredAt: 2)
        XCTAssertThrowsError(try store.confirmFingerprint(id: other.id,
                                                          hostKeyB64: Self.keyA,
                                                          fingerprint: "F1-again"))
    }

    func testLegacyMigrationIsIdempotentAndPreservesRegistration() {
        let store = makeStore()
        let first = store.migrateLegacy(host: "https://mac.example",
                                        keyId: "dev_legacy",
                                        grants: ["read_tail", "read_diff"],
                                        expiryTs: 1_800_000_000,
                                        registeredAt: 123)
        XCTAssertNotNil(first)
        XCTAssertEqual(first?.keyId, "dev_legacy")
        XCTAssertEqual(first?.grants, ["read_tail", "read_diff"])
        XCTAssertEqual(first?.expiryTs, 1_800_000_000)
        XCTAssertEqual(first?.registeredAt, 123)
        // Running the migration again (relaunch/upgrade) must no-op:
        // exactly ONE profile, never two active legacy/profile records.
        XCTAssertNil(store.migrateLegacy(host: "https://mac.example",
                                         keyId: "dev_legacy",
                                         grants: ["read_tail"],
                                         expiryTs: 1_800_000_000,
                                         registeredAt: 123))
        XCTAssertEqual(store.orderedProfiles.count, 1)
        // A migration without a complete legacy identity is a no-op too.
        let fresh = makeStore()
        XCTAssertNil(fresh.migrateLegacy(host: nil, keyId: nil, grants: [],
                                         expiryTs: nil, registeredAt: 0))
        XCTAssertTrue(fresh.isEmpty)
    }

    func testPerProfileCursorsAndKeyIDsAreScoped() throws {
        let store = makeStore()
        _ = try store.addProfile(displayName: "Mac",
                                 urlString: "https://mac.example",
                                 keyId: "dev_same",
                                 registeredAt: 1)
        _ = try store.addProfile(displayName: "Bazzite",
                                 urlString: "https://bazzite.example",
                                 keyId: "dev_same",
                                 registeredAt: 2)
        // B2: identical deterministic key-id strings are scoped per
        // profile — both records hold theirs independently (the same
        // phone key signed both registrations).
        let ids = store.orderedProfiles.map(\.keyId)
        XCTAssertEqual(ids, ["dev_same", "dev_same"])
        let a = store.orderedProfiles[0]
        let b = store.orderedProfiles[1]
        store.setCursor(41, for: a.id)
        store.setCursor(7, for: b.id)
        XCTAssertEqual(store.cursor(for: a.id), 41)
        XCTAssertEqual(store.cursor(for: b.id), 7)
        store.setCursor(nil, for: a.id)
        XCTAssertNil(store.cursor(for: a.id))
        XCTAssertEqual(store.cursor(for: b.id), 7)
    }

    func testFileBackedStoreRoundTripsAtomically() throws {
        // SAFETY: per-test temp directory under the system temp dir.
        let directory = FileManager.default.temporaryDirectory
            .appendingPathComponent("corral-profiles-\(UUID().uuidString)", isDirectory: true)
        defer { try? FileManager.default.removeItem(at: directory) }
        let store = HostProfileStore(directory: directory)
        _ = try store.addProfile(displayName: "Mac",
                                 urlString: "https://mac.example",
                                 hostKeyB64: Self.keyA,
                                 keyId: "dev_1", registeredAt: 1)
        let reloaded = HostProfileStore(directory: directory)
        XCTAssertEqual(reloaded.orderedProfiles.count, 1)
        XCTAssertEqual(reloaded.orderedProfiles.first?.displayName, "Mac")
        XCTAssertEqual(reloaded.orderedProfiles.first?.hostKeyB64, Self.keyA)
        // Only ONE document exists (atomic replace, no stray temp files).
        let files = try FileManager.default
            .contentsOfDirectory(atPath: directory.path)
        XCTAssertEqual(files, [HostProfileStore.profilesFileName])
    }

    func testPerHostNotificationStateDefaultsOnAndSurvivesReloadAndPurgesOnRemove() throws {
        // #397: notificationsEnabled defaults TRUE (new + legacy docs),
        // persists through the profile document, and dies with removal.
        // SAFETY: per-test temp directory under the system temp dir.
        let directory = FileManager.default.temporaryDirectory
            .appendingPathComponent("corral-notifyflag-\(UUID().uuidString)", isDirectory: true)
        defer { try? FileManager.default.removeItem(at: directory) }
        let store = HostProfileStore(directory: directory)
        let a = try store.addProfile(displayName: "Mac",
                                     urlString: "https://mac.example",
                                     hostKeyB64: Self.keyA,
                                     keyId: "dev_1", registeredAt: 1)
        XCTAssertTrue(a.notificationsEnabled, "new profiles default to ON")
        _ = try store.setNotificationsEnabled(false, id: a.id)
        XCTAssertFalse(store.profile(id: a.id)?.notificationsEnabled ?? true)
        // Reload from the document keeps the persisted OFF state.
        let reloaded = HostProfileStore(directory: directory)
        XCTAssertFalse(reloaded.orderedProfiles.first?.notificationsEnabled ?? true,
                       "the per-host flag must round-trip through the document")
        // Remove Host purges the flag with the record (no orphan state).
        reloaded.removeProfile(id: a.id)
        XCTAssertTrue(reloaded.isEmpty)
    }

    func testLegacyProfileDocumentWithoutNotificationKeyDefaultsEnabled() throws {
        // SAFETY: per-test temp directory under the system temp dir.
        let directory = FileManager.default.temporaryDirectory
            .appendingPathComponent("corral-notifylegacy-\(UUID().uuidString)", isDirectory: true)
        defer { try? FileManager.default.removeItem(at: directory) }
        // Write a profile document WITHOUT the #397 key (pre-#397 bytes).
        let fm = FileManager.default
        try fm.createDirectory(at: directory, withIntermediateDirectories: true)
        let profileJSON = """
        [{"id":"\(UUID().uuidString)","displayName":"Legacy","urlString":"https://old.example",
          "hostKeyB64":null,"fingerprint":null,"keyId":"dev_legacy","grants":["read_tail"],
          "expiryTs":1800000000,"registeredAt":1,"order":0,"connectionState":"disconnected",
          "cursorRev":null,"lastSuccessfulConnectionTs":null}]
        """
        try profileJSON.write(to: directory.appendingPathComponent(HostProfileStore.profilesFileName),
                              atomically: true, encoding: .utf8)
        let store = HostProfileStore(directory: directory)
        XCTAssertEqual(store.orderedProfiles.count, 1)
        XCTAssertEqual(store.orderedProfiles.first?.notificationsEnabled, true,
                       "a pre-#397 document must decode with notifications enabled")
    }

    func testRemoveHostPurgesOnlyThatProfileIncludingCursorAndCache() throws {
        // SAFETY: per-test temp directory under the system temp dir.
        let directory = FileManager.default.temporaryDirectory
            .appendingPathComponent("corral-remove-\(UUID().uuidString)", isDirectory: true)
        defer { try? FileManager.default.removeItem(at: directory) }
        let store = HostProfileStore(directory: directory)
        let a = try store.addProfile(displayName: "Mac",
                                     urlString: "https://mac.example",
                                     hostKeyB64: Self.keyA,
                                     registeredAt: 1)
        let b = try store.addProfile(displayName: "Bazzite",
                                     urlString: "https://bazzite.example",
                                     hostKeyB64: Self.keyB,
                                     registeredAt: 2)
        store.setCursor(11, for: a.id)
        store.setCursor(22, for: b.id)
        let cacheRow = BoardCacheRow(compositeIdentity: BoardCacheDTO.composite(
            hostProfileID: a.id, agentID: "ag1"),
            hostProfileID: a.id,
            agentID: "ag1",
            state: "working",
            ts: 1,
            stateEnteredAt: 1,
            displayName: nil,
            title: nil,
            reason: nil,
            tool: "claude",
            paneReference: "p1",
            repo: "corral",
            branch: "main",
            basename: "corral",
            lastSeen: 1)
        store.boardCache.save([cacheRow], for: a.id)
        store.boardCache.save([], for: b.id)

        store.removeProfile(id: a.id)

        XCTAssertNil(store.profile(id: a.id))
        XCTAssertNotNil(store.profile(id: b.id), "the other profile must survive")
        XCTAssertNil(store.cursor(for: a.id), "A's cursor must be purged")
        XCTAssertEqual(store.cursor(for: b.id), 22, "B's cursor must survive")
        XCTAssertNil(store.boardCache.load(for: a.id), "A's cache file must be purged")
        XCTAssertNotNil(store.boardCache.load(for: b.id), "B's cache must survive")
        let files = try FileManager.default
            .contentsOfDirectory(atPath: directory.path)
            .filter { $0 != HostProfileStore.profilesFileName }
        XCTAssertFalse(files.contains { $0.contains(a.id.uuidString) },
                       "no A-named cache/cursor artifacts may survive")
    }

    func testCommitActivePairingKeepsOtherProfilesAndDedupesURL() throws {
        let store = makeStore()
        _ = try store.addProfile(displayName: "Mac",
                                 urlString: "https://mac.example",
                                 hostKeyB64: Self.keyA,
                                 registeredAt: 1)
        _ = try store.addProfile(displayName: "Bazzite",
                                 urlString: "https://bazzite.example",
                                 hostKeyB64: Self.keyB,
                                 registeredAt: 2)
        // The STORE commit only dedupes the paired URL and appends; the
        // model separately removes the previous ACTIVE record (B5).
        let pairing = try store.commitActivePairing(displayName: "Mac",
                                                    urlString: "https://new.example",
                                                    hostKeyB64: Self.keyA,
                                                    fingerprint: "FP",
                                                    keyId: "dev_2",
                                                    grants: [],
                                                    expiryTs: nil,
                                                    registeredAt: 3)
        XCTAssertEqual(store.orderedProfiles.map(\.urlString),
                       ["https://mac.example", "https://bazzite.example",
                        "https://new.example"],
                       "other profiles must stay intact at the store level")
        XCTAssertEqual(pairing.hostKeyB64, Self.keyA)
        // Re-registering the SAME url refreshes the record instead of
        // duplicating it (one record per URL).
        _ = try store.commitActivePairing(displayName: "Mac",
                                          urlString: "https://new.example",
                                          hostKeyB64: Self.keyA,
                                          fingerprint: "FP",
                                          keyId: "dev_3",
                                          grants: [],
                                          expiryTs: nil,
                                          registeredAt: 4)
        XCTAssertEqual(store.orderedProfiles
            .filter { $0.urlString == "https://new.example" }.count,
            1, "one record per URL")
        XCTAssertEqual(store.orderedProfiles.count, 3)
    }
}

// MARK: - #399 legacy migration (B6) model behavior

/// Reference box so @Sendable store closures share one metadata store
/// with the test body (value capture would freeze the empty dictionary).
private final class LegacyMetaBox: @unchecked Sendable {
    var meta: DeviceKeyStore.DeviceMeta?
}

/// First-upgraded-launch migration pauses once for fingerprint
/// confirmation; no stream/token activity happens before it.
@MainActor
final class HostProfileMigrationModelTests: XCTestCase {
    private var suiteName = ""
    private var model: AppModel?
    private var box: LegacyMetaBox?
    private var session: URLSession?

    private func cleanup() {
        model?.stopLive()
        model = nil
        session?.invalidateAndCancel()
        session = nil
        HostSwitchURLProtocol.clearScript()
        if !suiteName.isEmpty {
            // SAFETY: suiteName was freshly minted per test.
            UserDefaults(suiteName: suiteName)!.removePersistentDomain(forName: suiteName)
            suiteName = ""
        }
        box = nil
    }

    private func makeModel(defaults: UserDefaults,
                           host: String,
                           storeDirectory: URL? = nil,
                           session: URLSession? = nil) -> AppModel {
        let legacyMeta = DeviceKeyStore.DeviceMeta(
            keyId: "dev_legacy", host: host,
            grants: ["read_tail"], expiryTs: 1_800_000_000, registeredAt: 99)
        let metaBox = LegacyMetaBox()
        metaBox.meta = legacyMeta
        box = metaBox
        let signer = DeviceSigner(key: Curve25519.Signing.PrivateKey())
        let store = HostProfileStore(directory: storeDirectory, defaults: defaults)
        return AppModel(session: session ?? URLSession(configuration: .ephemeral),
                        defaults: defaults,
                        identityLoader: { (signer, .insecureFallback) },
                        loadMeta: { [weak metaBox] in metaBox?.meta },
                        saveMeta: { [weak metaBox] meta in metaBox?.meta = meta },
                        removeMeta: { [weak metaBox] in metaBox?.meta = nil },
                        profileStore: store)
    }

    private func scriptedSession(host: String) -> URLSession {
        // SAFETY: fixed fixture URL derived from the host constant.
        let hostKeyURL = URL(string: host)!.appendingPathComponent("/host-key")
        HostSwitchURLProtocol.setScript([
            hostKeyURL:
                (200, Data(#"{"algorithm":"X25519","public_key":"\#(Data(repeating: 0, count: 32).base64EncodedString())"}"#.utf8), false),
        ])
        let config = URLSessionConfiguration.ephemeral
        config.protocolClasses = [HostSwitchURLProtocol.self]
        return URLSession(configuration: config)
    }

    func testMigrationPausesWithOneProfileAndConsumesLegacyKeys() {
        suiteName = "corral.migration.\(UUID().uuidString)"
        // SAFETY: a fresh UUID suite name is always a valid suite.
        let defaults = UserDefaults(suiteName: suiteName)!
        let host = "https://mac.example"
        defaults.set(host, forKey: "fleetnotifier.host")
        defer { cleanup() }
        let session = scriptedSession(host: host)
        self.session = session
        let model = makeModel(defaults: defaults, host: host, session: session)
        self.model = model

        // The FIRST upgraded launch migrates legacy data into ONE profile
        // and pauses for fingerprint confirmation — no stream yet.
        XCTAssertEqual(model.profiles.count, 1)
        let profile = model.profiles[0]
        XCTAssertEqual(profile.keyId, "dev_legacy")
        XCTAssertEqual(profile.grants, ["read_tail"])
        XCTAssertEqual(profile.expiryTs, 1_800_000_000)
        XCTAssertEqual(profile.registeredAt, 99)
        XCTAssertNil(profile.hostKeyB64, "no pin until fingerprint confirmation")
        XCTAssertEqual(profile.connectionState, .awaitingFingerprintConfirmation)
        XCTAssertNotNil(model.fingerprintConfirmation,
                        "migration must pause on the fingerprint confirmation")
        XCTAssertNil(defaults.object(forKey: "fleetnotifier.host"),
                     "the legacy host record must be consumed (never two active records)")
        XCTAssertNil(box?.meta, "legacy DeviceMeta must be consumed")
        model.startLive()
        XCTAssertTrue(HostSwitchURLProtocol.requests.isEmpty,
                      "no stream/fetch may run before fingerprint confirmation")
    }

    func testMigrationIsIdempotentAcrossRelaunches() {
        suiteName = "corral.migration2.\(UUID().uuidString)"
        // SAFETY: a fresh UUID suite name is always a valid suite; temp
        // store dir is per-test under the system temp dir.
        let defaults = UserDefaults(suiteName: suiteName)!
        let storeDirectory = FileManager.default.temporaryDirectory
            .appendingPathComponent("corral-migrate-\(UUID().uuidString)", isDirectory: true)
        defer {
            cleanup()
            try? FileManager.default.removeItem(at: storeDirectory)
        }
        let host = "https://mac.example"
        defaults.set(host, forKey: "fleetnotifier.host")
        let first = makeModel(defaults: defaults, host: host,
                              storeDirectory: storeDirectory)
        XCTAssertEqual(first.profiles.count, 1, "first upgraded launch migrates once")
        first.stopLive()
        // The legacy keys were consumed; a relaunch against the same
        // FILE-backed store must NOT create a second profile.
        let second = makeModel(defaults: defaults, host: host,
                               storeDirectory: storeDirectory)
        self.model = second
        XCTAssertEqual(second.profiles.count, 1,
                       "relaunch after migration must keep exactly one profile")
    }
}

// MARK: - #399 key continuity (B4): fail-closed RED/GREEN target

/// Launch/reconnect key continuity: with a PINNED profile, `/host-key`
/// is re-checked before the live stream opens; a mismatch fails closed
/// (no stream/fetch/push-register/Recent Output), the prior snapshot
/// stays stale, and only Remove Host + fresh pairing recovers.
@MainActor
final class HostKeyContinuityModelTests: XCTestCase {
    private var suiteName = ""
    private var model: AppModel?
    private var session: URLSession?

    /// SAFETY: fixed valid X25519 public-key fixtures (32-byte fills).
    static let pinnedKey = Data(repeating: 1, count: 32).base64EncodedString()
    /// SAFETY: fixed valid X25519 public-key fixtures (32-byte fills).
    static let rotatedKey = Data(repeating: 2, count: 32).base64EncodedString()

    /// SAFETY: fixed valid URL literals used only as scripted-endpoint
    /// keys and request assertions in this test class.
    private let hostURL = URL(string: "https://mac.example")!
    private let eventsURL = URL(string: "https://mac.example/events")!
    private let hostKeyURL = URL(string: "https://mac.example/host-key")!
    private let grantsURL = URL(string: "https://mac.example/grants-read")!
    private let driveURL = URL(string: "https://mac.example/drive")!

    private func cleanup() {
        model?.stopLive()
        model = nil
        session?.invalidateAndCancel()
        session = nil
        HostSwitchURLProtocol.clearScript()
        KeyContinuityGate.reset()
        if !suiteName.isEmpty {
            // SAFETY: suiteName was freshly minted per test.
            UserDefaults(suiteName: suiteName)!.removePersistentDomain(forName: suiteName)
            suiteName = ""
        }
    }

    private func scriptedSession(reportedKey: String) -> URLSession {
        HostSwitchURLProtocol.setScript([
            hostKeyURL: (200, Data(#"{"algorithm":"X25519","public_key":"\#(reportedKey)"}"#.utf8),
                         false),
            eventsURL: (200, Data(), true),
        ])
        let config = URLSessionConfiguration.ephemeral
        config.protocolClasses = [HostSwitchURLProtocol.self]
        return URLSession(configuration: config)
    }

    private func makePinnedModel(reportedKey: String) -> AppModel {
        suiteName = "corral.continuity.\(UUID().uuidString)"
        // SAFETY: a fresh UUID suite name is always a valid suite.
        let defaults = UserDefaults(suiteName: suiteName)!
        let store = HostProfileStore(directory: nil, defaults: defaults)
        // SAFETY: fixed valid fixtures — URL + key + registration.
        let profile = try! store.addProfile(
            displayName: "Mac",
            urlString: hostURL.absoluteString,
            hostKeyB64: Self.pinnedKey,
            fingerprint: "FINGER",
            keyId: "dev_pinned",
            grants: ["read_tail"],
            expiryTs: 1_800_000_000,
            registeredAt: 1)
        defaults.set(profile.id.uuidString, forKey: "fleetnotifier.activeHostProfileID")
        let signer = DeviceSigner(key: Curve25519.Signing.PrivateKey())
        let model = AppModel(session: scriptedSession(reportedKey: reportedKey),
                             defaults: defaults,
                             identityLoader: { (signer, .insecureFallback) },
                             loadMeta: { nil }, saveMeta: { _ in },
                             wipeIdentity: {},
                             profileStore: store)
        return model
    }

    private func requests(to url: URL) -> [URLRequest] {
        HostSwitchURLProtocol.requests.filter { $0.url?.absoluteString == url.absoluteString }
    }

    private func seedSnapshot(model: AppModel, rev: UInt64 = 5) {
        let agent = Agent(agentId: "herdr:a1", source: "herdr", tool: "claude",
                          state: .working, reason: "writing code", seq: 1,
                          ts: 100, capabilities: ["read_tail"],
                          host: Self.pinnedKey)
        let snapshot = Snapshot(schemaVersion: 5, rev: rev,
                                generatedAt: 100, agents: ["herdr:a1": agent])
        model.fleet.apply(.snapshot(snapshot))
    }

    private func waitUntil(_ condition: @autoclosure () -> Bool,
                           timeout: TimeInterval = 5) async {
        let deadline = Date().addingTimeInterval(timeout)
        while !condition(), Date() < deadline {
            try? await Task.sleep(nanoseconds: 25_000_000)
        }
    }

    func testHostKeyMismatchFailsClosedWithNoWorkReachingTheHost() async {
        let model = makePinnedModel(reportedKey: Self.rotatedKey)
        self.model = model
        defer { cleanup() }
        seedSnapshot(model: model)
        model.startLive()
        await waitUntil(model.keyContinuityState == .mismatch)
        XCTAssertEqual(model.keyContinuityState, .mismatch,
                       "a different host key must fail the profile closed")
        XCTAssertGreaterThanOrEqual(requests(to: hostKeyURL).count, 1,
                                    "the /host-key re-check must run first")
        XCTAssertEqual(model.banner?.kind, "host_key_mismatch")
        XCTAssertTrue(requests(to: eventsURL).isEmpty,
                      "no stream may reach the replacement identity")
        XCTAssertEqual(model.fleet.agents.count, 1,
                       "the last safe snapshot must stay stale, not erased")
        // Every other route fails closed too: pull refresh, grants
        // refresh, recents sheet, Recent Output read.
        await model.refreshFleet()
        await model.refreshGrants()
        model.requestRecents(for: "herdr:a1", haptic: false)
        let driveClient = DriveClient(host: hostURL, session: session ?? .shared)
        if let agent = model.fleet.agent("herdr:a1") {
            model.driveReadTail(agent: agent, driveClient: driveClient)
        }
        try? await Task.sleep(nanoseconds: 300_000_000)
        XCTAssertTrue(requests(to: grantsURL).isEmpty,
                      "no signed grants-read may reach the replacement identity")
        XCTAssertTrue(requests(to: driveURL).isEmpty,
                      "no Recent Output read may reach the replacement identity")
        XCTAssertNil(model.recentsRequest, "the recents sheet must stay closed")
        // Push enrollment is gated by the same continuity predicate.
        let allowsPush = await KeyContinuityGate.allowsPushRegistration()
        XCTAssertFalse(allowsPush, "APNs enrollment must be denied on mismatch")
    }

    func testHostKeyMatchOpensTheStreamAfterRecheck() async {
        let model = makePinnedModel(reportedKey: Self.pinnedKey)
        self.model = model
        defer { cleanup() }
        seedSnapshot(model: model)
        model.startLive()
        await waitUntil(!requests(to: eventsURL).isEmpty)
        XCTAssertEqual(model.keyContinuityState, .verified)
        XCTAssertGreaterThanOrEqual(requests(to: hostKeyURL).count, 1,
                                    "the check must run BEFORE the stream opens")
        XCTAssertGreaterThanOrEqual(requests(to: eventsURL).count, 1,
                                    "a matching key opens the live stream")
    }

    func testRemoveHostRecoversFromMismatchAndKeepsTheSharedKey() async {
        let model = makePinnedModel(reportedKey: Self.rotatedKey)
        self.model = model
        defer { cleanup() }
        let signerBefore = model.signer
        model.startLive()
        await waitUntil(model.keyContinuityState == .mismatch)
        XCTAssertEqual(model.keyContinuityState, .mismatch)
        // SAFETY: the pinned fixture guarantees an active profile.
        let profileID = try! XCTUnwrap(model.activeProfile?.id)
        model.removeHost(profileID: profileID)
        XCTAssertEqual(model.profiles.count, 0)
        XCTAssertNotNil(model.signer, "the shared phone key must survive Remove Host")
        XCTAssertNotNil(signerBefore, "fixture must hold a signer")
        XCTAssertEqual(model.mode, .needsSetup)
        XCTAssertEqual(model.keyContinuityState, .notPinned)
        // Fresh pairing is possible again after removal: phase 1 against
        // the same (rotated) key no longer trips the duplicate check.
        let prepared = try! await model.prepareHostPairing(  // SAFETY: scripted session.
            displayName: "Mac", rawURL: "https://mac.example")
        XCTAssertEqual(prepared.hostKey.publicKey, Self.rotatedKey)
        XCTAssertFalse(prepared.fingerprint.isEmpty)
    }
}

/// #415: Add Host flow URLProtocol with per-URL status/body scripts plus
/// an OPTIONAL per-URL gate (in-flight duplicate-submit test). Delivery
/// after a gate mirrors DeterministicDriveURLProtocol's proven queue
/// mechanics. Unscripted URLs fail like an unreachable host.
private final class AddHostFlowURLProtocol: URLProtocol {
    private static let lock = NSLock()
    private static var scriptStorage: [URL: (statusCode: Int, body: Data, holdOpen: Bool)] = [:]
    private static var gatesStorage: [URL: DriveRequestGate] = [:]
    private static var requestsStorage: [URLRequest] = []
    private static let deliveryQueue = DispatchQueue(label: "corral.415.urlprotocol.delivery")

    static func setScript(_ script: [URL: (Int, Data, Bool)],
                          gates: [URL: DriveRequestGate] = [:]) {
        lock.lock()
        scriptStorage = script
        gatesStorage = gates
        requestsStorage = []
        lock.unlock()
    }

    static var requests: [URLRequest] {
        lock.lock()
        defer { lock.unlock() }
        return requestsStorage
    }

    static func clearScript() {
        setScript([:])
    }

    override class func canInit(with request: URLRequest) -> Bool { true }
    override class func canonicalRequest(for request: URLRequest) -> URLRequest { request }

    override func startLoading() {
        Self.lock.lock()
        guard let url = request.url else {
            Self.lock.unlock()
            client?.urlProtocol(self, didFailWithError: URLError(.badURL))
            return
        }
        var copy = request
        if let body = requestBodyData(request) {
            copy.httpBody = body
        }
        Self.requestsStorage.append(copy)
        let scripted = Self.scriptStorage[url]
        let gate = Self.gatesStorage[url]
        Self.lock.unlock()
        guard let (statusCode, body, holdOpen) = scripted else {
            client?.urlProtocol(self, didFailWithError: URLError(.cannotConnectToHost))
            return
        }
        Task { [self] in
            let canRespond: Bool
            if let gate {
                canRespond = await gate.wait()
            } else {
                canRespond = true
            }
            Self.deliveryQueue.async {
                guard canRespond else {
                    self.client?.urlProtocol(self, didFailWithError: URLError(.cancelled))
                    return
                }
                // SAFETY: fixed HTTP response construction from a scripted URL.
                let response = HTTPURLResponse(
                    url: url, statusCode: statusCode, httpVersion: "HTTP/1.1",
                    headerFields: holdOpen
                        ? ["Content-Type": "text/event-stream"]
                        : ["Content-Type": "application/json"])!
                self.client?.urlProtocol(self, didReceive: response,
                                         cacheStoragePolicy: .notAllowed)
                if !body.isEmpty {
                    self.client?.urlProtocol(self, didLoad: body)
                }
                if !holdOpen {
                    self.client?.urlProtocolDidFinishLoading(self)
                }
            }
        }
    }

    override func stopLoading() {}
}

// MARK: - #415 Add Host draft + error lifecycle

/// #415: the Add Host draft lives on the MODEL (scene-scoped, in-memory)
/// and completeAddHost returns an outcome the sheet acts on:
/// - a failed submit keeps every draft value + a phase-identifying,
///   secret-free error, never dismisses and never commits a profile;
/// - a successful submit commits EXACTLY ONE new profile, keeps the
///   previously active Mac profile, clears the draft only after the
///   commit, and lets the caller dismiss exactly once;
/// - repeated submit/retry cannot duplicate profiles and cannot disturb
///   the existing Mac profile.
@MainActor
final class AddHostDraftLifecycleTests: XCTestCase {
    /// SAFETY: fixed synthetic X25519 fixture keys (32-byte fills).
    static let macKey = Data(repeating: 1, count: 32).base64EncodedString()
    static let newKey = Data(repeating: 2, count: 32).base64EncodedString()
    static let registerOK = Data(#"{"key_id":"dev_add","grants":["read_tail"],"expiry_ts":1800000000,"revoked":false,"algorithm":"Ed25519"}"#.utf8)
    static let registerRejected = Data(#"{"kind":"bad_token","message":"registration token is invalid","request_id":"r1"}"#.utf8)

    private var suiteName = ""
    private var model: AppModel?
    private var session: URLSession?

    // SAFETY: fixed valid fixture URL literals (distinct hostnames).
    private var macURL: URL { URL(string: "https://mac.example")! }
    // SAFETY: fixed valid fixture URL literal (distinct hostname).
    private var newURL: URL { URL(string: "https://bazzite.example")! }
    private var token = "tok-415-fixture"

    private func cleanup() {
        model?.stopLive()
        model = nil
        session?.invalidateAndCancel()
        session = nil
        AddHostFlowURLProtocol.clearScript()
        if !suiteName.isEmpty {
            // SAFETY: suiteName was freshly minted per test.
            UserDefaults(suiteName: suiteName)!.removePersistentDomain(forName: suiteName)
            suiteName = ""
        }
    }

    private func waitUntil(_ condition: @autoclosure () -> Bool,
                           timeout: TimeInterval = 6) async {
        let deadline = Date().addingTimeInterval(timeout)
        while !condition(), Date() < deadline {
            try? await Task.sleep(nanoseconds: 25_000_000)
        }
    }

    private func requests(to url: URL) -> [URLRequest] {
        AddHostFlowURLProtocol.requests.filter { $0.url?.absoluteString == url.absoluteString }
    }

    /// One pinned ACTIVE "Mac" profile + scripted endpoints for both
    /// hosts (mac key matches the Mac pin; the new host serves `newKey`).
    private func makeModel(registerResponse: (Int, Data, Bool)? = nil,
                           gates: [URL: DriveRequestGate] = [:]) -> AppModel {
        suiteName = "corral.g415.addhost.\\(UUID().uuidString)"
        // SAFETY: a fresh UUID suite name is always a valid suite.
        let defaults = UserDefaults(suiteName: suiteName)!
        let store = HostProfileStore(directory: nil, defaults: defaults)
        // SAFETY: fixed fixture URL + key literals from the constants above.
        let mac = try! store.addProfile(
            displayName: "Mac",
            urlString: macURL.absoluteString,
            hostKeyB64: Self.macKey,
            fingerprint: "FINGER-MAC",
            keyId: "dev_mac",
            grants: ["read_tail"],
            expiryTs: 1_800_000_000,
            registeredAt: 1)
        defaults.set(mac.id.uuidString, forKey: "fleetnotifier.activeHostProfileID")
        var script: [URL: (Int, Data, Bool)] = [
            macURL.appendingPathComponent("/host-key"): (200,
                Data(#"{"algorithm":"X25519","public_key":"\#(Self.macKey)"}"#.utf8), false),
            newURL.appendingPathComponent("/host-key"): (200,
                Data(#"{"algorithm":"X25519","public_key":"\#(Self.newKey)"}"#.utf8), false),
            // /events hold open (SSE) so post-commit startLive connects.
            macURL.appendingPathComponent("/events"): (200, Data(), true),
            newURL.appendingPathComponent("/events"): (200, Data(), true),
        ]
        if let registerResponse {
            script[newURL.appendingPathComponent("/register")] = registerResponse
        } else {
            script[newURL.appendingPathComponent("/register")] = (200, Self.registerOK, false)
        }
        AddHostFlowURLProtocol.setScript(script, gates: gates)
        let config = URLSessionConfiguration.ephemeral
        config.protocolClasses = [AddHostFlowURLProtocol.self]
        let session = URLSession(configuration: config)
        self.session = session
        let signer = DeviceSigner(key: Curve25519.Signing.PrivateKey())
        let model = AppModel(session: session, defaults: defaults,
                             identityLoader: { (signer, .insecureFallback) },
                             loadMeta: { nil }, saveMeta: { _ in },
                             wipeIdentity: {},
                             profileStore: store)
        self.model = model
        return model
    }

    /// Drive the draft through the SAME model entry points the sheet's
    /// buttons call: values into the scene-scoped draft, then the
    /// phase-1 verification (scripted host key).
    private func primeDraft(model: AppModel,
                            name: String = "Bazzite",
                            rawURL: String? = nil) async throws -> AppModel.PreparedHostPairing {
        model.addHostDraft.name = name
        model.addHostDraft.urlString = rawURL ?? newURL.absoluteString
        model.addHostDraft.token = token
        await model.verifyAddHostDraft()
        let pairing = try XCTUnwrap(model.addHostDraft.prepared,
                                    "the scripted /host-key must reach the confirmation phase")
        XCTAssertEqual(model.addHostDraft.name, name, "the draft keeps the name")
        XCTAssertEqual(model.addHostDraft.token, token, "the draft keeps the token")
        return pairing
    }

    func testFailedRegistrationKeepsDraftWithPhaseErrorAndCommitsNothing() async throws {
        defer { cleanup() }
        let model = makeModel(registerResponse: (401, Self.registerRejected, false))
        let macID = try XCTUnwrap(model.activeProfile?.id)
        let pairing = try await primeDraft(model: model)

        let outcome = await model.completeAddHost(pairing, token: model.addHostDraft.token)

        guard case .failure(let failure) = outcome else {
            return XCTFail("a rejected registration must fail, got \\(outcome)")
        }
        XCTAssertEqual(failure, .registrationFailed(
            "HTTP 401 bad_token: registration token is invalid"))
        XCTAssertFalse(model.addHostDraft.errorMessage?.contains(token) ?? false,
                       "the visible error must never expose the registration token")
        // The sheet stays open: every draft value remains available.
        XCTAssertEqual(model.addHostDraft.name, "Bazzite")
        XCTAssertEqual(model.addHostDraft.urlString, newURL.absoluteString)
        XCTAssertEqual(model.addHostDraft.token, token)
        XCTAssertNotNil(model.addHostDraft.prepared, "the confirmation phase stays current")
        XCTAssertFalse(model.addHostDraft.isWorking)
        // Nothing committed; the original Mac profile is untouched.
        XCTAssertEqual(model.profiles.count, 1)
        XCTAssertEqual(model.profiles.first?.id, macID)
        XCTAssertEqual(model.activeProfileID, macID)
        XCTAssertEqual(requests(to: newURL.appendingPathComponent("/register")).count, 1)
    }

    func testSuccessCommitsExactlyOneProfileKeepsMacAndClearsDraftAfterCommit() async throws {
        defer { cleanup() }
        let model = makeModel()
        let macID = try XCTUnwrap(model.activeProfile?.id)
        let pairing = try await primeDraft(model: model)

        let outcome = await model.completeAddHost(pairing, token: model.addHostDraft.token)

        XCTAssertEqual(outcome, .success)
        // Draft cleared ONLY after the commit succeeded.
        XCTAssertEqual(model.addHostDraft, AppModel.AddHostDraft(),
                       "a successful commit clears the whole scene-scoped draft")
        // EXACTLY ONE new host profile, and the original Mac host is
        // still present with its ORIGINAL record id.
        XCTAssertEqual(model.profiles.count, 2)
        XCTAssertEqual(model.profiles.first { $0.id == macID }?.displayName, "Mac",
                       "the previously active Mac profile must survive the Add Host commit")
        let added = model.profiles.filter { $0.urlString == newURL.absoluteString }
        XCTAssertEqual(added.count, 1, "exactly one profile for the new host")
        XCTAssertEqual(added.first?.keyId, "dev_add")
        XCTAssertEqual(added.first?.hostKeyB64, Self.newKey,
                       "the pinned host key persists with the new profile")
        XCTAssertEqual(model.activeProfileID, added.first?.id,
                       "the runtime binds the freshly paired host as active")
        XCTAssertEqual(requests(to: newURL.appendingPathComponent("/register")).count, 1)
    }

    func testDuplicateSubmitWhileInFlightSendsExactlyOneRegister() async throws {
        defer { cleanup() }
        let gate = DriveRequestGate()
        let model = makeModel(gates: [newURL.appendingPathComponent("/register"): gate])
        let pairing = try await primeDraft(model: model)
        let registerURL = newURL.appendingPathComponent("/register")

        var firstOutcome: AppModel.AddHostOutcome?
        let first = Task { @MainActor in
            firstOutcome = await model.completeAddHost(pairing, token: model.addHostDraft.token)
        }
        await waitUntil(!requests(to: registerURL).isEmpty)
        XCTAssertEqual(requests(to: registerURL).count, 1, "the first submit is in flight")

        // A second submit while the registration is in flight is refused
        // — no second /register, no dismissable success.
        let secondOutcome = await model.completeAddHost(pairing, token: model.addHostDraft.token)
        XCTAssertEqual(secondOutcome, .failure(.inProgress))
        XCTAssertEqual(requests(to: registerURL).count, 1,
                       "an in-flight registration must never duplicate /register")
        XCTAssertEqual(model.profiles.count, 1, "nothing commits while the first runs")

        gate.release()
        await first.value
        XCTAssertEqual(firstOutcome, .success)
        XCTAssertEqual(model.profiles.count, 2)
        XCTAssertEqual(model.profiles.filter { $0.urlString == newURL.absoluteString }.count, 1,
                       "repeated submit cannot create duplicate host profiles")
    }

    func testRetryAfterRejectionCommitsExactlyOneProfile() async throws {
        defer { cleanup() }
        let model = makeModel(registerResponse: (500, Data("boom".utf8), false))
        let pairing = try await primeDraft(model: model)
        let registerURL = newURL.appendingPathComponent("/register")

        let first = await model.completeAddHost(pairing, token: model.addHostDraft.token)
        guard case .failure = first else {
            return XCTFail("the first attempt must fail against a 500")
        }
        XCTAssertEqual(model.profiles.count, 1, "a failed attempt commits nothing")
        XCTAssertEqual(model.activeProfile?.displayName, "Mac")
        // Values still present for the retry (no re-priming needed).
        XCTAssertEqual(model.addHostDraft.name, "Bazzite")
        XCTAssertEqual(model.addHostDraft.token, token)

        // The host recovers: the retry succeeds and still commits ONE
        // profile for the new host (never two).
        AddHostFlowURLProtocol.setScript([
            macURL.appendingPathComponent("/host-key"): (200,
                Data(#"{"algorithm":"X25519","public_key":"\#(Self.macKey)"}"#.utf8), false),
            newURL.appendingPathComponent("/host-key"): (200,
                Data(#"{"algorithm":"X25519","public_key":"\#(Self.newKey)"}"#.utf8), false),
            macURL.appendingPathComponent("/events"): (200, Data(), true),
            newURL.appendingPathComponent("/events"): (200, Data(), true),
            registerURL: (200, Self.registerOK, false),
        ])
        let retry = await model.completeAddHost(pairing, token: model.addHostDraft.token)
        XCTAssertEqual(retry, .success)
        XCTAssertEqual(model.profiles.count, 2)
        XCTAssertEqual(model.profiles.filter { $0.urlString == newURL.absoluteString }.count, 1,
                       "retry after a rejection must not duplicate the profile")
        XCTAssertEqual(model.profiles.first { $0.displayName == "Mac" }?.keyId, "dev_mac")
        XCTAssertEqual(requests(to: registerURL).count, 1)
    }

    func testHostKeyFetchFailureKeepsDraftWithPhaseError() async throws {
        defer { cleanup() }
        let model = makeModel(registerResponse: nil)
        // The new host stops answering /host-key (500).
        AddHostFlowURLProtocol.setScript([
            macURL.appendingPathComponent("/host-key"): (200,
                Data(#"{"algorithm":"X25519","public_key":"\#(Self.macKey)"}"#.utf8), false),
            newURL.appendingPathComponent("/host-key"): (500, Data("boom".utf8), false),
        ])
        let macID = try XCTUnwrap(model.activeProfile?.id)
        model.addHostDraft.name = "Bazzite"
        model.addHostDraft.urlString = newURL.absoluteString
        model.addHostDraft.token = token

        await model.verifyAddHostDraft()

        let message = try XCTUnwrap(model.addHostDraft.errorMessage)
        XCTAssertTrue(message.contains("Could not verify this host's key"),
                      "the failure must name the host-key phase: \\(message)")
        XCTAssertFalse(message.contains(token), "the visible error must never expose the token")
        XCTAssertNil(model.addHostDraft.prepared, "no confirmation phase after a failed fetch")
        XCTAssertEqual(model.addHostDraft.name, "Bazzite")
        XCTAssertEqual(model.addHostDraft.urlString, newURL.absoluteString)
        XCTAssertEqual(model.addHostDraft.token, token)
        XCTAssertFalse(model.addHostDraft.isWorking)
        XCTAssertEqual(model.profiles.count, 1)
        XCTAssertEqual(model.profiles.first?.id, macID)
    }

    func testDuplicateIdentityConflictNeverRegistersOrCommits() async throws {
        defer { cleanup() }
        // The new URL serves the MAC's already-pinned key: the duplicate
        // identity check must stop the flow before any /register.
        let model = makeModel(registerResponse: (200, Self.registerOK, false))
        AddHostFlowURLProtocol.setScript([
            macURL.appendingPathComponent("/host-key"): (200,
                Data(#"{"algorithm":"X25519","public_key":"\#(Self.macKey)"}"#.utf8), false),
            newURL.appendingPathComponent("/host-key"): (200,
                Data(#"{"algorithm":"X25519","public_key":"\#(Self.macKey)"}"#.utf8), false),
        ])
        let macID = try XCTUnwrap(model.activeProfile?.id)
        let registerURL = newURL.appendingPathComponent("/register")

        // Phase 1 already rejects the duplicate identity: the real verify
        // path never reaches the confirmation phase.
        model.addHostDraft.name = "Bazzite"
        model.addHostDraft.urlString = newURL.absoluteString
        model.addHostDraft.token = token
        await model.verifyAddHostDraft()
        let verifyMessage = try XCTUnwrap(model.addHostDraft.errorMessage)
        XCTAssertTrue(verifyMessage.contains("already paired"),
                      "phase 1 must name the already-paired host: \(verifyMessage)")
        XCTAssertNil(model.addHostDraft.prepared)
        XCTAssertTrue(requests(to: registerURL).isEmpty)
        XCTAssertEqual(model.addHostDraft.name, "Bazzite")
        XCTAssertEqual(model.addHostDraft.token, token)

        // Phase 2 re-checks the same identity (race guard): a prepared
        // pairing that somehow passed phase 1 is refused again before any
        // /register — never a commit, never a duplicate.
        let forged = AppModel.PreparedHostPairing(
            displayName: "Bazzite",
            urlString: newURL.absoluteString,
            hostKey: HostKeyResponse(algorithm: "X25519",
                                     publicKey: Self.macKey,
                                     note: nil),
            fingerprint: "FINGER")
        model.clearAddHostDraft()
        model.addHostDraft.token = token
        let outcome = await model.completeAddHost(forged, token: model.addHostDraft.token)

        guard case .failure(let failure) = outcome else {
            return XCTFail("a duplicate host identity must be rejected, got \(outcome)")
        }
        XCTAssertTrue(failure.message.contains("already paired"),
                      "the conflict must name the already-paired host: \(failure.message)")
        XCTAssertTrue(failure.message.contains("Could not add this host"),
                      "conflicts are user-correctable, in-phase failures")
        XCTAssertTrue(requests(to: registerURL).isEmpty,
                      "a duplicate identity must never reach /register")
        XCTAssertEqual(model.profiles.count, 1)
        XCTAssertEqual(model.profiles.first?.id, macID)
        XCTAssertEqual(model.activeProfileID, macID)
        XCTAssertEqual(model.addHostDraft.token, token)
    }

    func testCancelClearsTheDraftAndValuesSurviveUntilThen() async throws {
        defer { cleanup() }
        let model = makeModel(registerResponse: (401, Self.registerRejected, false))
        _ = try await primeDraft(model: model)
        // The draft (incl. the failed submit's values) is what Cancel
        // clears — the same clearAddHostDraft call the sheet's Cancel
        // button invokes. Until then every value is retained.
        model.clearAddHostDraft()
        XCTAssertEqual(model.addHostDraft, AppModel.AddHostDraft())
        XCTAssertNil(model.addHostDraft.prepared)
        XCTAssertNil(model.addHostDraft.errorMessage)
        XCTAssertFalse(model.addHostDraft.isWorking)
    }
}

// MARK: - #399 feed integrity + durable cache (C1/C5)

/// Frame-level pinned-identity acceptance (C1): a frame stamped with a
/// different host is rejected whole; the stale snapshot survives.
@MainActor
final class PinnedFeedIntegrityTests: XCTestCase {
    private func agent(_ id: String, host: String?) -> Agent {
        Agent(agentId: id, host: host)
    }

    func testConformsToPinnedHost() {
        let pinned = "pinned-key"
        let good = FleetEvent.snapshot(Snapshot(schemaVersion: 5, rev: 1,
                                                generatedAt: 0,
                                                agents: ["a": agent("a", host: pinned),
                                                         "b": agent("b", host: nil)]))
        XCTAssertTrue(FleetStore.conformsToPinnedHost(good, pin: pinned),
                      "matching + host-less records pass")
        let bad = FleetEvent.delta(Delta(rev: 2,
                                         upd: [agent("c", host: "rotated-key")],
                                         del: []))
        XCTAssertFalse(FleetStore.conformsToPinnedHost(bad, pin: pinned),
                       "a record stamped with another host fails the frame closed")
    }

    func testMismatchedFrameIsRejectedAndStaleStateSurvives() {
        let store = FleetStore(defaults: .standard)
        defer { store.acceptedHostIdentity = nil; store.reset() }
        let pin = "pinned-key"
        store.acceptedHostIdentity = pin
        store.apply(.snapshot(Snapshot(schemaVersion: 5, rev: 1,
                                       generatedAt: 0,
                                       agents: ["a": agent("a", host: pin)])))
        XCTAssertEqual(store.agents.count, 1)
        var mismatchFired = false
        store.onHostIntegrityMismatch = { mismatchFired = true }
        store.apply(.delta(Delta(rev: 2,
                                 upd: [agent("b", host: "rotated-key")],
                                 del: [])))
        XCTAssertTrue(mismatchFired, "the integrity hook must fire")
        XCTAssertEqual(store.agents.count, 1,
                       "the mismatched frame must be rejected entirely")
        XCTAssertEqual(store.agents["a"]?.host, pin)
        XCTAssertEqual(store.connectionState, .error("host_identity_mismatch"))
    }
}

/// C5: the durable cache stores ONLY the allowlisted DTO fields — no
/// read_tail line/block or transcript text can reach durable storage.
@MainActor
final class BoardMetadataCacheTests: XCTestCase {
    /// SAFETY: per-test temp directory under the system temp dir.
    private func makeDirectory() -> URL {
        FileManager.default.temporaryDirectory
            .appendingPathComponent("corral-cache-\(UUID().uuidString)", isDirectory: true)
    }

    func testSnapshotProjectsOnlyMetadataFields() throws {
        let profileID = UUID()
        let transcriptMarker = "tail-line-\(UUID().uuidString)"
        let blockMarker = "block-text-\(UUID().uuidString)"
        var agent = Agent(agentId: "herdr:a1", source: "herdr", tool: "claude",
                          state: .blocked, reason: "waiting on a review",
                          seq: 2, ts: 42,
                          capabilities: ["read_tail"],
                          host: "host-key")
        agent.displayName = "fix-399"
        agent.title = "host profiles"
        agent.workspace = Workspace(repo: "corral",
                                    branch: "g399-host-profiles",
                                    worktreePath: "/Users/x/.herdr/worktrees/corral/g399-host-profiles",
                                    dirty: false)
        agent.attachment = Attachment(kind: "herdr-pane", reference: "w1:p1")
        // Transcript content exists ONLY in the tail pane the DTO must
        // never see (read_tail lines/blocks are memory-only per C5).
        let store = FleetStore(defaults: .standard)
        store.rememberTail([transcriptMarker, "line 2"], blocks: [
            TranscriptBlock(kind: .agent, text: blockMarker, at: 1),
        ], for: agent.agentId)

        let rows = BoardCacheDTO.snapshot(hostProfileID: profileID,
                                          agents: ["herdr:a1": agent],
                                          stateEnteredAt: ["herdr:a1": 40],
                                          now: 100)
        XCTAssertEqual(rows.count, 1)
        let row = try XCTUnwrap(rows.first)
        XCTAssertEqual(row.agentID, "herdr:a1")
        XCTAssertEqual(row.state, "blocked")
        XCTAssertEqual(row.reason, "waiting on a review")
        XCTAssertEqual(row.tool, "claude")
        XCTAssertEqual(row.paneReference, "w1:p1")
        XCTAssertEqual(row.repo, "corral")
        XCTAssertEqual(row.branch, "g399-host-profiles")
        XCTAssertEqual(row.basename, "g399-host-profiles")
        XCTAssertEqual(row.stateEnteredAt, 40)
        XCTAssertEqual(row.compositeIdentity,
                       BoardCacheDTO.composite(hostProfileID: profileID, agentID: "herdr:a1"))
        // The DTO has no line/block/transcript/token fields by
        // construction — prove it by encoding: the markers cannot appear.
        let data = try JSONEncoder().encode(rows)
        let text = try XCTUnwrap(String(data: data, encoding: .utf8))
        XCTAssertFalse(text.contains(transcriptMarker),
                       "read_tail LINES must never reach the durable cache")
        XCTAssertFalse(text.contains(blockMarker),
                       "transcript BLOCK text must never reach the durable cache")
        XCTAssertFalse(text.contains("device_token"),
                       "no pairing-token field may exist in the DTO")
        store.acceptedHostIdentity = nil
        store.reset()
    }

    func testCacheFileRoundTripsAndSurvivesReloadWithOnlyAllowedKeys() throws {
        let directory = makeDirectory()
        defer { try? FileManager.default.removeItem(at: directory) }
        let profileID = UUID()
        let cache = BoardCacheStore(directory: directory)
        let row = BoardCacheRow(compositeIdentity: "\(profileID)::a1",
                                hostProfileID: profileID,
                                agentID: "a1",
                                state: "working",
                                ts: 5,
                                stateEnteredAt: 5,
                                displayName: "fix",
                                title: "title",
                                reason: "reason",
                                tool: "claude",
                                paneReference: "p1",
                                repo: "corral",
                                branch: "main",
                                basename: "corral",
                                lastSeen: 9)
        cache.save([row], for: profileID)
        // The persisted JSON contains EXACTLY the allowlisted key set.
        let url = try XCTUnwrap(cache.cacheFileURL(for: profileID))
        let raw = try String(contentsOf: url, encoding: .utf8)
        let object = try XCTUnwrap(JSONSerialization.jsonObject(
            with: Data(raw.utf8)) as? [[String: Any]])
        XCTAssertEqual(object.count, 1)
        let keys = Set(try XCTUnwrap(object.first).keys)
        let allowed: Set<String> = ["composite_identity", "host_profile_id", "agent_id",
                                    "state", "ts", "state_entered_at", "display_name",
                                    "title", "reason", "tool", "pane_reference",
                                    "repo", "branch", "basename", "last_seen"]
        XCTAssertEqual(keys, allowed,
                       "the durable cache file must contain ONLY allowlisted DTO fields")
        let reloaded = BoardCacheStore(directory: directory)
        let rows = try XCTUnwrap(reloaded.load(for: profileID))
        XCTAssertEqual(rows.first, row)
        // Remove purges the FILE + any in-memory rows: a fresh store over
        // the same directory must see nothing.
        cache.remove(for: profileID)
        let afterRemoval = BoardCacheStore(directory: directory)
        XCTAssertNil(afterRemoval.load(for: profileID))
        XCTAssertFalse(FileManager.default.fileExists(atPath: url.path),
                       "the cache document must be deleted")
    }
}

// MARK: - #399 host-profile Settings/board wiring (source pins)

/// Source-wiring pins over the bundled FleetViews source: the Settings
/// Hosts section (Add Host entry), the fingerprint confirmation sheet
/// binding, and the Add Host fingerprint flow.
final class HostProfileWiringTests: XCTestCase {
    private func bundledSource() throws -> String {
        let bundle = Bundle(for: HostProfileWiringTests.self)
        let url = try XCTUnwrap(bundle.url(forResource: "FleetViews",
                                           withExtension: "swift.txt"))
        return try String(contentsOf: url, encoding: .utf8)
    }

    /// 1-based line numbers of every line containing the needle.
    private func lineNumbers(of needle: String, in text: String) -> [Int] {
        text.split(separator: "\n", omittingEmptySubsequences: false)
            .enumerated()
            .compactMap { index, line in line.contains(needle) ? index + 1 : nil }
    }

    func testSettingsHostsSectionWiresAddHostAndRemoveHost() throws {
        let source = try bundledSource()
        let start = try XCTUnwrap(source.range(of: "private var hostsSection: some View {"),
                                  "the Hosts section must exist in SettingsView")
        let end = try XCTUnwrap(source.range(of: "// MARK: - #399 Add Host"),
                                "the Add Host views must follow SettingsView")
        let slice = String(source[start.lowerBound..<end.lowerBound])
        XCTAssertTrue(slice.contains("Text(\"Hosts\")"), "Hosts section header")
        XCTAssertTrue(slice.contains("Label(\"Add host\", systemImage: \"plus.circle\")"),
                      "the Add Host entry must exist")
        XCTAssertTrue(slice.contains("showAddHost = true"),
                      "the Add Host row must present the AddHostSheet")
        XCTAssertTrue(slice.contains("Button(\"Remove host\", role: .destructive)"),
                      "Remove Host must be reachable from Settings")
        XCTAssertTrue(slice.contains("model.removeHost(profileID: profile.id)"),
                      "Remove Host must route through the model")
        XCTAssertTrue(slice.contains(".id(\"settings.add-host\")"),
                      "the Add Host row carries its evidence anchor")
    }

    func testAddHostSheetShowsFingerprintBeforeRegistering() throws {
        let source = try bundledSource()
        let start = try XCTUnwrap(source.range(of: "struct AddHostSheet: View {"))
        let end = try XCTUnwrap(source.range(of: "/// #399 B6: the launch-time fingerprint confirmation"))
        let slice = String(source[start.lowerBound..<end.lowerBound])
        XCTAssertTrue(slice.contains("Text(\"Verify host key\")"),
                      "phase 1 fetches the host key before any token")
        // #415: phase 1 runs through the model's verifyAddHostDraft (the
        // scene-scoped draft owns name/URL/token/phase state).
        XCTAssertTrue(slice.contains("model.verifyAddHostDraft()"),
                      "phase 1 must route through the model's draft verify path")
        XCTAssertTrue(slice.contains("Text(\"Confirm fingerprint & register\")"),
                      "phase 2 requires explicit fingerprint confirmation")
        XCTAssertTrue(slice.contains("model.completeAddHost(pairing,"),
                      "registration runs only after confirmation")
        XCTAssertTrue(slice.contains("Registration token"),
                      "the token field appears at the confirmation step")
        XCTAssertTrue(slice.contains("UIPasteboard.general.string = pairing.fingerprint"),
                      "the full fingerprint stays copyable")
        // #415: the sheet dismisses EXACTLY ONCE per outcome — on Cancel
        // and on a successful commit; the old unconditional
        // dismiss-after-submit is gone (a failed submit keeps the sheet
        // open with the draft's error). Scope the pins to the complete()
        // action: the DEBUG evidence drivers above it may dismiss too.
        let completeLine = try XCTUnwrap(
            lineNumbers(of: "private func complete(_ pairing", in: slice).first)
        let callLines = lineNumbers(of: "let outcome = await model.completeAddHost", in: slice)
            .filter { $0 > completeLine }
        XCTAssertEqual(callLines.count, 1, "the submit must await the model outcome once")
        // SAFETY: callLines was asserted non-empty immediately above.
        let successLine = try XCTUnwrap(
            lineNumbers(of: "if case .success = outcome {", in: slice)
                .first { $0 > completeLine },
            "the submit must branch on the model outcome")
        let dismissLines = lineNumbers(of: "dismiss()", in: slice).filter { $0 > completeLine }
        XCTAssertTrue(dismissLines.contains { $0 > successLine && $0 - successLine <= 6 },
                      "a successful commit dismisses the sheet (once)")
        let nearestDismiss = dismissLines
            .map { abs($0 - callLines[0]) }
            .min() ?? 0
        XCTAssertGreaterThan(nearestDismiss, 2,
                             "dismiss() must never directly follow the submit call")
        let cancelLine = try XCTUnwrap(
            lineNumbers(of: "Button(\"Cancel\")", in: slice).first)
        XCTAssertTrue(lineNumbers(of: "dismiss()", in: slice).contains { $0 - cancelLine <= 5 },
                      "Cancel clears the draft and dismisses")
    }

    func testAddHostSheetBindsTheSceneScopedDraft() throws {
        // #415 AC1: every Add Host field/phase lives on the MODEL-owned
        // scene-scoped draft ($model.addHostDraft.*), never sheet @State —
        // a recreated sheet view over the same model (scene lifecycle
        // churn) must render the same name/URL/token/phase.
        let source = try bundledSource()
        let start = try XCTUnwrap(source.range(of: "struct AddHostSheet: View {"))
        let end = try XCTUnwrap(source.range(of: "/// #399 B6: the launch-time fingerprint confirmation"))
        let slice = String(source[start.lowerBound..<end.lowerBound])
        XCTAssertTrue(slice.contains("text: $model.addHostDraft.name"),
                      "the host-name field must bind the scene-scoped draft")
        XCTAssertTrue(slice.contains("text: $model.addHostDraft.urlString"),
                      "the URL field must bind the scene-scoped draft")
        XCTAssertTrue(slice.contains("text: $model.addHostDraft.token"),
                      "the token field must bind the scene-scoped draft")
        XCTAssertTrue(slice.contains("if let prepared = model.addHostDraft.prepared"),
                      "the current verification phase must come from the draft")
        XCTAssertTrue(slice.contains("model.clearAddHostDraft()"),
                      "Cancel clears the scene-scoped draft")
        XCTAssertTrue(slice.contains("Edit host details"),
                      "the confirmation phase keeps a correction affordance")
        XCTAssertFalse(slice.contains("@State private var name"),
                       "the draft must never live in sheet @State")
        XCTAssertFalse(slice.contains("@State private var token"),
                       "the token must never live in sheet @State")
        XCTAssertFalse(slice.contains("var urlString = \"\""),
                       "the URL must never be sheet-local")
    }

    func testBoardPresentsFingerprintConfirmationForPausedProfiles() throws {
        let source = try bundledSource()
        XCTAssertEqual(lineNumbers(of: ".sheet(item: $model.fingerprintConfirmation)",
                                   in: source).count, 1,
                       "the board must present the migration confirmation exactly once")
        XCTAssertTrue(source.contains(
            "FingerprintConfirmationSheet(model: model, request: request)"))
        XCTAssertTrue(source.contains("model.confirmFingerprint(profileID: request.profileID"))
        XCTAssertTrue(source.contains("model.fetchHostKey(profileID: request.profileID)"),
                      "the sheet fetches the key for display")
        XCTAssertTrue(source.contains("Corral never auto-accepts"),
                      "the confirmation must state the no-auto-accept rule")
    }
}

// MARK: - #400 composite client identity (C2)

/// The composite key `(host_profile_id, raw_agent_id)` round-trips and raw
/// agent ids containing ":" (e.g. "herdr:demo") parse unambiguously.
final class CompositeIdentityTests: XCTestCase {
    func testDescriptionUsesCanonicalSeparator() {
        let profileID = UUID()
        let identity = CompositeAgentID(hostProfileID: profileID, agentID: "herdr:a1")
        XCTAssertEqual(identity.description,
                       "\(profileID.uuidString)::herdr:a1")
        XCTAssertEqual(identity.hostProfileID, profileID)
        XCTAssertEqual(identity.agentID, "herdr:a1", "the raw id must stay untouched")
    }

    func testParsesRawAgentIdsContainingColons() {
        let profileID = UUID()
        for raw in ["herdr:a1", "herdr:demo-garden", "plain", "a::b"] {
            let key = CompositeAgentID(hostProfileID: profileID, agentID: raw).description
            // SAFETY: the fixture key was built from the profile id above.
            let parsed = CompositeAgentID(string: key)!
            XCTAssertEqual(parsed.hostProfileID, profileID)
            XCTAssertEqual(parsed.agentID, raw)
        }
    }

    func testRejectsMalformedKeys() {
        XCTAssertNil(CompositeAgentID(string: ""))
        XCTAssertNil(CompositeAgentID(string: "not-a-uuid::herdr:a1"))
        XCTAssertNil(CompositeAgentID(string: "\(UUID().uuidString)"))
        XCTAssertNil(CompositeAgentID(string: "\(UUID().uuidString)::"))
    }
}

// MARK: - #400 stale/offline board projection (C6/C7)

/// Live rows come from the host's store; a disconnected host retains its
/// snapshot rows as STALE (state token preserved verbatim — never recast),
/// and a never-connected host renders its durable allowlisted cache rows.
/// C7 ranks live rows before stale rows inside every (state, repo) bucket.
@MainActor
final class HostBoardProjectionTests: XCTestCase {
    private func agent(_ id: String, state: AgentState, ts: UInt64,
                       repo: String? = nil, blockedReason: String? = nil) -> Agent {
        var workspace = Workspace()
        if let repo { workspace = Workspace(repo: repo, branch: "main") }
        return Agent(agentId: id, state: state, reason: blockedReason,
                     ts: ts, capabilities: [], workspace: workspace)
    }

    private func makeStore(agents: [Agent]) -> FleetStore {
        let store = FleetStore(defaults: .standard)
        store.apply(.snapshot(Snapshot(schemaVersion: 5, rev: 1, generatedAt: 0,
                                       agents: Dictionary(uniqueKeysWithValues:
                                           agents.map { ($0.agentId, $0) }))))
        return store
    }

    func testDisconnectedHostRetainsSnapshotRowsAsStaleWithoutRecastingState() {
        let profileID = UUID()
        let blocked = agent("herdr:b", state: .blocked, ts: 40,
                            blockedReason: "waiting on a review")
        let store = makeStore(agents: [blocked])
        // Host goes offline: rows stay, marked stale, last-seen stamped.
        let rows = HostBoardProjection.boardRows(hostProfileID: profileID,
                                                 store: store, cached: nil,
                                                 connected: false)
        XCTAssertEqual(rows.count, 1)
        let row = rows[0]
        XCTAssertTrue(row.isStale)
        XCTAssertEqual(row.agent.state, .blocked,
                       "a stale Blocked lane must NEVER be recast (urgency/Unknown)")
        XCTAssertEqual(row.agent.reason, "waiting on a review",
                       "retained metadata keeps the last reported reason")
        XCTAssertEqual(row.identity.hostProfileID, profileID)
        XCTAssertEqual(row.lastSeen, 40)
    }

    func testConnectedRowsAreLiveAndAuthoritative() {
        let profileID = UUID()
        let working = agent("herdr:w", state: .working, ts: 90)
        let store = makeStore(agents: [working])
        let rows = HostBoardProjection.boardRows(hostProfileID: profileID,
                                                 store: store, cached: nil,
                                                 connected: true)
        XCTAssertEqual(rows.count, 1)
        XCTAssertFalse(rows[0].isStale)
    }

    func testNeverConnectedHostRendersDurableCacheRowsAsStale() {
        let profileID = UUID()
        let store = FleetStore(defaults: .standard)
        // The allowlisted cache row from a previous session (C5 DTO).
        let cached = BoardCacheRow(compositeIdentity: "\(profileID)::herdr:old",
                                   hostProfileID: profileID, agentID: "herdr:old",
                                   state: "blocked", ts: 50, stateEnteredAt: 50,
                                   displayName: "fix", title: "t", reason: "waiting",
                                   tool: "claude", paneReference: "w1:p1",
                                   repo: "corral", branch: "main",
                                   basename: "corral", lastSeen: 60)
        let rows = HostBoardProjection.boardRows(hostProfileID: profileID,
                                                 store: store, cached: [cached],
                                                 connected: false)
        XCTAssertEqual(rows.count, 1)
        let row = rows[0]
        XCTAssertTrue(row.isStale)
        XCTAssertEqual(row.agent.state, .blocked)
        XCTAssertEqual(row.agent.agentId, "herdr:old")
        XCTAssertEqual(row.lastSeen, 60, "stale last-seen age comes from the cache stamp")
        XCTAssertNil(row.agent.workspace.worktreePath,
                     "the cache holds only the basename — no path is synthesized")
    }

    func testAuthoritativeReconnectReplacesRetainedCacheRows() {
        let profileID = UUID()
        let cached = BoardCacheRow(compositeIdentity: "\(profileID)::herdr:old",
                                   hostProfileID: profileID, agentID: "herdr:old",
                                   state: "blocked", ts: 50, stateEnteredAt: 50,
                                   displayName: nil, title: nil, reason: nil,
                                   tool: nil, paneReference: nil,
                                   repo: "corral", branch: "main",
                                   basename: "corral", lastSeen: 60)
        // The authoritative reconnect snapshot no longer contains the lane.
        let store = makeStore(agents: [agent("herdr:live", state: .idle, ts: 80)])
        let rows = HostBoardProjection.boardRows(hostProfileID: profileID,
                                                 store: store, cached: [cached],
                                                 connected: true)
        XCTAssertEqual(rows.map(\.agent.agentId), ["herdr:live"],
                       "an authoritative reconnect replaces the retained snapshot")
        XCTAssertFalse(rows[0].isStale)
    }

    func testLiveRowsRankBeforeStaleRowsWithinStatusRepoBuckets() {
        // Same (state, repo) bucket: one live + one stale of the same raw
        // lane name on two hosts. Canonical input order (ts desc) has the
        // STALE row first (newer ts); C7 must still rank the LIVE row
        // first inside the bucket, and keep ts/id order on each side.
        let profileA = UUID(), profileB = UUID()
        let live = HostBoardRow(identity: CompositeAgentID(hostProfileID: profileA,
                                                           agentID: "herdr:dup"),
                                agent: agent("herdr:dup", state: .blocked, ts: 10,
                                             repo: "corral"),
                                isStale: false, lastSeen: 10)
        let stale = HostBoardRow(identity: CompositeAgentID(hostProfileID: profileB,
                                                            agentID: "herdr:dup"),
                                 agent: agent("herdr:dup", state: .blocked, ts: 99,
                                              repo: "corral", blockedReason: "old"),
                                 isStale: true, lastSeen: 99)
        let idle = HostBoardRow(identity: CompositeAgentID(hostProfileID: profileB,
                                                           agentID: "herdr:idle"),
                                agent: agent("herdr:idle", state: .idle, ts: 5),
                                isStale: false, lastSeen: 5)
        let ranked = HostBoardProjection.liveFirst([stale, live, idle])
        let ids = ranked.map(\.identity.description)
        XCTAssertEqual(ids.first, live.identity.description,
                       "the LIVE blocked row must lead its (blocked, corral) bucket")
        XCTAssertEqual(ids.dropFirst().first, stale.identity.description,
                       "the stale row follows inside the same bucket")
        XCTAssertEqual(ids.last, idle.identity.description,
                       "the idle bucket is untouched by the blocked bucket")
        XCTAssertEqual(ranked[0].agent.state, .blocked)
        XCTAssertEqual(ranked[1].agent.state, .blocked,
                       "stale blocked stays blocked — never Unknown")
    }
}

// MARK: - #400 per-host stream coordinator (C3/C4/E3)

/// C3/C4 runtime tests: THREE profiles with EQUAL raw agent ids run one
/// independent stream/cursor/generation/task set each; background cancels
/// all; removing one host cancels ONLY that host and purges only its
/// composite state; pull-refresh fans out with per-host outcomes.
@MainActor
final class HostStreamCoordinatorTests: XCTestCase {
    private var suiteName = ""
    private var coordinator: HostStreamCoordinator?

    private func cleanup() {
        coordinator?.stopAll()
        coordinator = nil
        HostSwitchURLProtocol.clearScript()
        if !suiteName.isEmpty {
            // SAFETY: suiteName was freshly minted per test.
            UserDefaults(suiteName: suiteName)!.removePersistentDomain(forName: suiteName)
            suiteName = ""
        }
    }

    private func hostKey(_ value: UInt8) -> String {
        Data(repeating: value, count: 32).base64EncodedString()
    }

    private func waitUntil(_ condition: @autoclosure () -> Bool,
                           timeout: TimeInterval = 5) async {
        let deadline = Date().addingTimeInterval(timeout)
        while !condition(), Date() < deadline {
            try? await Task.sleep(nanoseconds: 25_000_000)
        }
    }

    /// Three pinned profiles with EQUAL raw agent ids under distinct URLs.
    private func makeThreeHostStore() -> (HostProfileStore, [HostProfile], [URL], [String]) {
        let store = HostProfileStore(directory: nil,
                                     // SAFETY: fixed test fixture invariant (see the test's fixture builder); failure here is a harness bug, not a product defect.
                                     defaults: UserDefaults(suiteName: suiteName)!)
        let urls = [
            // SAFETY: fixed valid fixture URLs (distinct hostnames).
            URL(string: "https://h400-a.example")!,
            URL(string: "https://h400-b.example")!,
            URL(string: "https://h400-c.example")!,
        ]
        let keys = [hostKey(1), hostKey(2), hostKey(3)]
        var profiles: [HostProfile] = []
        for index in 0..<3 {
            let profile = try! store.addProfile(displayName: "Host \(index)",
                                                urlString: urls[index].absoluteString,
                                                hostKeyB64: keys[index],
                                                fingerprint: "FINGER",
                                                keyId: "dev_h\(index)",
                                                grants: ["read_tail"],
                                                expiryTs: 1_800_000_000,
                                                registeredAt: 1)
            profiles.append(profile)
        }
        return (store, profiles, urls, keys)
    }

    private func scriptedSession(urls: [URL], keys: [String]) -> URLSession {
        var script: [URL: (Int, Data, Bool)] = [:]
        for (url, key) in zip(urls, keys) {
            // SAFETY: fixed fixture JSON from the fixture key.
            script[url.appendingPathComponent("/host-key")] = (
                200,
                Data(#"{"algorithm":"X25519","public_key":"\#(key)"}"#.utf8),
                false)
            script[url.appendingPathComponent("/events")] = (200, Data(), true)
        }
        HostSwitchURLProtocol.setScript(script)
        let config = URLSessionConfiguration.ephemeral
        config.protocolClasses = [HostSwitchURLProtocol.self]
        return URLSession(configuration: config)
    }

    private func requests(to url: URL) -> [URLRequest] {
        HostSwitchURLProtocol.requests.filter { $0.url?.absoluteString == url.absoluteString }
    }

    private func makeAgent(_ id: String, state: AgentState, host: String?,
                           ts: UInt64, repo: String? = nil) -> Agent {
        Agent(agentId: id, state: state, ts: ts,
              capabilities: ["read_tail"], host: host,
              workspace: Workspace(repo: repo, branch: "main"))
    }

    func testThreeProfilesStartConcurrentStreamsWithIndependentCursors() async {
        suiteName = "corral.h400.coord.\(UUID().uuidString)"
        defer { cleanup() }
        let (store, profiles, urls, keys) = makeThreeHostStore()
        let session = scriptedSession(urls: urls, keys: keys)
        self.session = session
        // SAFETY: fixed test fixture invariant (see the test's fixture builder); failure here is a harness bug, not a product defect.
        let coordinator = HostStreamCoordinator(defaults: UserDefaults(suiteName: suiteName)!,
                                                session: session, profileStore: store,
                                                signerProvider: { nil })
        self.coordinator = coordinator
        coordinator.update(profiles: profiles, startStreams: true)
        // All three hosts open their own streams concurrently.
        for url in urls {
            await waitUntil(!requests(to: url.appendingPathComponent("/events")).isEmpty)
        }
        // Independent cursors: advance ONLY host A's read model.
        let storeA = try! XCTUnwrap(coordinator.store(profileID: profiles[0].id))
        let storeB = try! XCTUnwrap(coordinator.store(profileID: profiles[1].id))
        let storeC = try! XCTUnwrap(coordinator.store(profileID: profiles[2].id))
        storeA.apply(.snapshot(Snapshot(schemaVersion: 5, rev: 42, generatedAt: 0,
                                        agents: ["dup": makeAgent("dup", state: .working,
                                                                  host: keys[0], ts: 1)])))
        XCTAssertEqual(storeA.lastEventId, 42)
        XCTAssertNil(storeB.lastEventId, "host B's cursor must not move with A's data")
        XCTAssertNil(storeC.lastEventId)
        // Reconnect generations are per host: disconnect + reconnect A only.
        let generationA = storeA.connectionGeneration
        let generationB = storeB.connectionGeneration
        storeA.disconnect()
        coordinator.startSessionIfNeeded(profiles[0])
        await waitUntil(storeA.connectionGeneration > generationA)
        XCTAssertEqual(storeB.connectionGeneration, generationB,
                       "host B's connection generation must not bump when A reconnects")
    }

    func testBackgroundStopAllCancelsEveryHostStream() async {
        suiteName = "corral.h400.bg.\(UUID().uuidString)"
        defer { cleanup() }
        let (store, profiles, urls, keys) = makeThreeHostStore()
        let session = scriptedSession(urls: urls, keys: keys)
        self.session = session
        // SAFETY: fixed test fixture invariant (see the test's fixture builder); failure here is a harness bug, not a product defect.
        let coordinator = HostStreamCoordinator(defaults: UserDefaults(suiteName: suiteName)!,
                                                session: session, profileStore: store,
                                                signerProvider: { nil })
        self.coordinator = coordinator
        coordinator.update(profiles: profiles, startStreams: true)
        for url in urls {
            await waitUntil(!requests(to: url.appendingPathComponent("/events")).isEmpty)
        }
        coordinator.stopAll()
        for profile in profiles {
            let store = try! XCTUnwrap(coordinator.store(profileID: profile.id))
            XCTAssertFalse(store.isStreaming, "background must cancel host \(profile.displayName)'s stream")
            XCTAssertEqual(store.connectionState, .disconnected)
        }
    }

    func testRemoveOneHostCancelsOnlyItsStreamAndPurgesOnlyItsState() async {
        suiteName = "corral.h400.remove.\(UUID().uuidString)"
        defer { cleanup() }
        let (store, profiles, urls, keys) = makeThreeHostStore()
        let session = scriptedSession(urls: urls, keys: keys)
        self.session = session
        // SAFETY: fixed test fixture invariant (see the test's fixture builder); failure here is a harness bug, not a product defect.
        let coordinator = HostStreamCoordinator(defaults: UserDefaults(suiteName: suiteName)!,
                                                session: session, profileStore: store,
                                                signerProvider: { nil })
        self.coordinator = coordinator
        coordinator.update(profiles: profiles, startStreams: true)
        for url in urls {
            await waitUntil(!requests(to: url.appendingPathComponent("/events")).isEmpty)
        }
        // Seed every host with the SAME raw id + a per-host tail.
        for (index, profile) in profiles.enumerated() {
            let hostStore = try! XCTUnwrap(coordinator.store(profileID: profile.id))
            hostStore.apply(.snapshot(Snapshot(schemaVersion: 5, rev: 1, generatedAt: 0,
                                               agents: ["dup": makeAgent("dup", state: .blocked,
                                                                         host: keys[index], ts: 10)])))
            hostStore.rememberTail(["line-h\(index)"], for: "dup")
        }
        let doomed = profiles[1]
        let doomedStore = try! XCTUnwrap(coordinator.store(profileID: doomed.id))
        coordinator.remove(profileID: doomed.id)
        // E3: the removed host's stream is canceled and its rows/tails are
        // purged; the OTHER hosts keep streaming with their state intact.
        XCTAssertFalse(doomedStore.isStreaming,
                       "host removal must cancel that host's stream task")
        XCTAssertEqual(doomedStore.connectionState, .disconnected)
        XCTAssertTrue(doomedStore.agents.isEmpty, "the removed host's rows must be purged")
        XCTAssertNil(doomedStore.tail(for: "dup"), "the removed host's tails must be purged")
        for index in [0, 2] {
            let survivor = try! XCTUnwrap(coordinator.store(profileID: profiles[index].id))
            XCTAssertTrue(survivor.isStreaming,
                          "removing one host must never cancel another host's stream")
            XCTAssertEqual(survivor.agent("dup")?.agentId, "dup",
                           "the survivor's equal raw id must be untouched")
            XCTAssertEqual(survivor.tail(for: "dup"), ["line-h\(index)"])
        }
        XCTAssertNil(coordinator.store(profileID: doomed.id),
                     "the session must be gone after removal")
    }

    func testRefreshFansOutAndAppliesSuccessfulResultsWhenAnotherHostFails() async {
        suiteName = "corral.h400.refresh.\(UUID().uuidString)"
        defer { cleanup() }
        let (store, profiles, urls, keys) = makeThreeHostStore()
        // A pull-refresh fan-out: hosts 0 and 2 answer a snapshot, host 1
        // fails (500). Session stores exist without streams.
        var script: [URL: (Int, Data, Bool)] = [:]
        let snapshotA = Snapshot(schemaVersion: 5, rev: 9, generatedAt: 0,
                                 agents: ["dup": makeAgent("dup", state: .working,
                                                           host: keys[0], ts: 9)])
        let snapshotC = Snapshot(schemaVersion: 5, rev: 9, generatedAt: 0,
                                 agents: ["dup": makeAgent("dup", state: .idle,
                                                           host: keys[2], ts: 9)])
        // SAFETY: fixture snapshots encode through the same Codable the
        // wire uses.
        script[urls[0].appendingPathComponent("/snapshot")] = (
            200, try! JSONEncoder().encode(snapshotA), false)
        script[urls[1].appendingPathComponent("/snapshot")] = (
            500, Data("boom".utf8), false)
        script[urls[2].appendingPathComponent("/snapshot")] = (
            200, try! JSONEncoder().encode(snapshotC), false)
        HostSwitchURLProtocol.setScript(script)
        let config = URLSessionConfiguration.ephemeral
        config.protocolClasses = [HostSwitchURLProtocol.self]
        let session = URLSession(configuration: config)
        self.session = session
        let coordinator = HostStreamCoordinator(defaults: UserDefaults(suiteName: suiteName)!,
                                                session: session, profileStore: store,
                                                signerProvider: { nil })
        self.coordinator = coordinator
        coordinator.update(profiles: profiles, startStreams: false)
        let outcomes = await coordinator.refreshAll(profiles: profiles)
        XCTAssertNil(outcomes[profiles[0].id] ?? nil, "host A's successful refresh applies")
        XCTAssertNotNil(outcomes[profiles[1].id] ?? nil, "host B's failure is isolated")
        XCTAssertNil(outcomes[profiles[2].id] ?? nil, "host C's successful refresh applies")
        let storeA = try! XCTUnwrap(coordinator.store(profileID: profiles[0].id))
        let storeB = try! XCTUnwrap(coordinator.store(profileID: profiles[1].id))
        let storeC = try! XCTUnwrap(coordinator.store(profileID: profiles[2].id))
        XCTAssertEqual(storeA.lastEventId, 9, "successful results apply even when another host fails")
        XCTAssertEqual(storeC.lastEventId, 9)
        XCTAssertNil(storeB.lastEventId, "the failed host keeps no partial state")
        XCTAssertEqual(storeB.connectionState, .disconnected,
                       "a failed host must not freeze or erase the others")
    }

    private var session: URLSession?
}

// MARK: - #400 equal-raw-id isolation (C2)

/// Equal raw agent ids on three hosts coexist: snapshot upserts/deletes,
/// state-duration tracking, and tails from one host NEVER touch the equal
/// raw id on another host — every surface keys by the composite identity.
@MainActor
final class MultiHostIsolationTests: XCTestCase {
    private var suiteName = ""

    private func cleanup() {
        HostSwitchURLProtocol.clearScript()
        if !suiteName.isEmpty {
            // SAFETY: suiteName was freshly minted per test.
            UserDefaults(suiteName: suiteName)!.removePersistentDomain(forName: suiteName)
            suiteName = ""
        }
    }

    private func hostKey(_ value: UInt8) -> String {
        Data(repeating: value, count: 32).base64EncodedString()
    }

    /// Three pinned profiles, all holding the SAME raw agent id "herdr:dup".
    private func makeThreeHostStore() -> (HostProfileStore, [HostProfile]) {
        let store = HostProfileStore(directory: nil,
                                     // SAFETY: fixed test fixture invariant (see the test's fixture builder); failure here is a harness bug, not a product defect.
                                     defaults: UserDefaults(suiteName: suiteName)!)
        var profiles: [HostProfile] = []
        for index in 0..<3 {
            // SAFETY: fixed valid fixture URLs (distinct hostnames).
            let url = URL(string: "https://iso-h\(index).example")!
            let profile = try! store.addProfile(displayName: "Host \(index)",
                                                urlString: url.absoluteString,
                                                hostKeyB64: hostKey(UInt8(index + 1)),
                                                fingerprint: "FINGER",
                                                keyId: "dev_iso\(index)",
                                                grants: ["read_tail"],
                                                expiryTs: 1_800_000_000,
                                                registeredAt: 1)
            profiles.append(profile)
        }
        return (store, profiles)
    }

    private func agent(_ id: String, state: AgentState, host: String, ts: UInt64) -> Agent {
        Agent(agentId: id, state: state, ts: ts,
              capabilities: ["read_tail"], host: host,
              workspace: Workspace(repo: "corral", branch: "main"))
    }

    func testEqualRawIdsCoexistAcrossHosts() {
        suiteName = "corral.h400.iso.\(UUID().uuidString)"
        defer { cleanup() }
        let (store, profiles) = makeThreeHostStore()
        let coordinator = HostStreamCoordinator(
            // SAFETY: fixed test fixture invariant (see the test's fixture builder); failure here is a harness bug, not a product defect.
            defaults: UserDefaults(suiteName: suiteName)!,
            session: URLSession(configuration: .ephemeral),
            profileStore: store, signerProvider: { nil })
        defer { coordinator.stopAll() }
        coordinator.update(profiles: profiles, startStreams: false)
        let keys = [hostKey(1), hostKey(2), hostKey(3)]
        for (index, profile) in profiles.enumerated() {
            let hostStore = try! XCTUnwrap(coordinator.store(profileID: profile.id))
            hostStore.apply(.snapshot(Snapshot(schemaVersion: 5, rev: 1, generatedAt: 0,
                                               agents: ["herdr:dup": agent("herdr:dup",
                                                                           state: .working,
                                                                           host: keys[index],
                                                                           ts: 10)])))
        }
        // The aggregate board carries THREE distinct composite rows for the
        // one raw id — never one row silently clobbering the others.
        let rows = coordinator.aggregateRows(profiles: profiles, activeStoreProvider: { nil })
        let dups = rows.filter { $0.identity.agentID == "herdr:dup" }
        XCTAssertEqual(dups.count, 3,
                       "an equal raw id on three hosts must produce three composite rows")
        XCTAssertEqual(Set(dups.map(\.identity.hostProfileID)).count, 3)
        XCTAssertEqual(dups.filter { $0.isStale }.count, 0, "seeded rows are live")
    }

    func testUpdateDeleteAndStateDurationFromOneHostNeverTouchEqualRawIds() {
        suiteName = "corral.h400.iso2.\(UUID().uuidString)"
        defer { cleanup() }
        let (store, profiles) = makeThreeHostStore()
        let coordinator = HostStreamCoordinator(
            // SAFETY: fixed test fixture invariant (see the test's fixture builder); failure here is a harness bug, not a product defect.
            defaults: UserDefaults(suiteName: suiteName)!,
            session: URLSession(configuration: .ephemeral),
            profileStore: store, signerProvider: { nil })
        defer { coordinator.stopAll() }
        coordinator.update(profiles: profiles, startStreams: false)
        let keys = [hostKey(1), hostKey(2), hostKey(3)]
        for (index, profile) in profiles.enumerated() {
            let hostStore = try! XCTUnwrap(coordinator.store(profileID: profile.id))
            hostStore.apply(.snapshot(Snapshot(schemaVersion: 5, rev: 1, generatedAt: 0,
                                               agents: ["herdr:dup": agent("herdr:dup",
                                                                           state: .working,
                                                                           host: keys[index],
                                                                           ts: 10)])))
        }
        // Host A upserts dup → blocked at rev 2. B and C must not move.
        let storeA = try! XCTUnwrap(coordinator.store(profileID: profiles[0].id))
        let storeB = try! XCTUnwrap(coordinator.store(profileID: profiles[1].id))
        let storeC = try! XCTUnwrap(coordinator.store(profileID: profiles[2].id))
        storeA.apply(.delta(Delta(rev: 2,
                                  upd: [agent("herdr:dup", state: .blocked,
                                              host: keys[0], ts: 20)],
                                  del: [])))
        XCTAssertEqual(storeA.agent("herdr:dup")?.state, .blocked)
        XCTAssertEqual(storeB.agent("herdr:dup")?.state, .working,
                       "host A's update must never touch B's equal raw id")
        XCTAssertEqual(storeC.agent("herdr:dup")?.state, .working)
        // State-duration tracking is per host too.
        XCTAssertNotNil(storeA.stateEnteredAt["herdr:dup"])
        XCTAssertEqual(storeB.stateEnteredAt["herdr:dup"], 10,
                       "host B's state clock must not move with host A's update")
        // Host B DELETES dup at rev 3: only B's row disappears.
        storeB.apply(.delta(Delta(rev: 3, upd: [], del: ["herdr:dup"])))
        XCTAssertNil(storeB.agent("herdr:dup"))
        XCTAssertNotNil(storeA.agent("herdr:dup"), "A's equal raw id survives B's deletion")
        XCTAssertNotNil(storeC.agent("herdr:dup"))
        // Tails: same raw id, distinct per-host content.
        storeA.rememberTail(["from-host-a"], for: "herdr:dup")
        storeC.rememberTail(["from-host-c"], for: "herdr:dup")
        XCTAssertEqual(storeA.tail(for: "herdr:dup"), ["from-host-a"])
        XCTAssertEqual(storeC.tail(for: "herdr:dup"), ["from-host-c"])
        XCTAssertNil(storeB.tail(for: "herdr:dup"), "the deleted target's tail is purged with it")
        // The composite aggregate reflects exactly the survivors.
        let rows = coordinator.aggregateRows(profiles: profiles, activeStoreProvider: { nil })
        let dupRows = rows.filter { $0.identity.agentID == "herdr:dup" }
        XCTAssertEqual(Set(dupRows.map(\.identity.hostProfileID)),
                       Set([profiles[0].id, profiles[2].id]))
        XCTAssertEqual(dupRows.first { $0.identity.hostProfileID == profiles[0].id }?.agent.state,
                       .blocked)
    }
}

// MARK: - #400 Recent Output composite routing (E1/E2/E3)

/// E1: opening a row resolves EXACTLY one profile and signs read_tail with
/// THAT profile's key id against THAT profile URL — never another host.
/// E2: offline keeps loaded output (memory-only) and disables reload.
/// E3: host removal purges only the composite target's sheet/tails.
@MainActor
final class RecentsCompositeRouteTests: XCTestCase {
    private var suiteName = ""
    private var model: AppModel?
    private var session: URLSession?

    static let hostAKey = Data(repeating: 7, count: 32).base64EncodedString()
    static let hostBKey = Data(repeating: 8, count: 32).base64EncodedString()

    // SAFETY: fixed valid fixture URLs under distinct hostnames.
    private let urlA = URL(string: "https://route-a.example")!
    private let urlB = URL(string: "https://route-b.example")!

    private func cleanup() {
        model?.stopLive()
        model = nil
        session?.invalidateAndCancel()
        session = nil
        AppDelegate.apnsRegistered = false
        UserDefaults.standard.removeObject(forKey: AppDelegate.deviceTokenUploadedKey)
        AppDelegate.shared?.clearRetainedDeviceToken()
        KeyContinuityGate.reset()
        HostSwitchURLProtocol.clearScript()
        if !suiteName.isEmpty {
            // SAFETY: suiteName was freshly minted per test.
            UserDefaults(suiteName: suiteName)!.removePersistentDomain(forName: suiteName)
            suiteName = ""
        }
    }

    private func waitUntil(_ condition: @autoclosure () -> Bool,
                           timeout: TimeInterval = 5) async {
        let deadline = Date().addingTimeInterval(timeout)
        while !condition(), Date() < deadline {
            try? await Task.sleep(nanoseconds: 25_000_000)
        }
    }

    /// Two pinned profiles: A = the ACTIVE host, B = a coordinator host.
    private func makeModel(seedAgentInB: Bool,
                           scriptDrive: Bool) -> (AppModel, HostProfile, HostProfile) {
        suiteName = "corral.h400.route.\(UUID().uuidString)"
        // SAFETY: a fresh UUID suite name is always a valid suite.
        let defaults = UserDefaults(suiteName: suiteName)!
        let store = HostProfileStore(directory: nil, defaults: defaults)
        let profileA = try! store.addProfile(displayName: "Host A",
                                             urlString: urlA.absoluteString,
                                             hostKeyB64: Self.hostAKey,
                                             fingerprint: "FINGER",
                                             keyId: "dev_route_a",
                                             grants: ["read_tail"],
                                             expiryTs: 1_800_000_000,
                                             registeredAt: 1)
        let profileB = try! store.addProfile(displayName: "Host B",
                                             urlString: urlB.absoluteString,
                                             hostKeyB64: Self.hostBKey,
                                             fingerprint: "FINGER",
                                             keyId: "dev_route_b",
                                             grants: ["read_tail"],
                                             expiryTs: 1_800_000_000,
                                             registeredAt: 1)
        defaults.set(profileA.id.uuidString, forKey: "fleetnotifier.activeHostProfileID")
        let signer = DeviceSigner(key: Curve25519.Signing.PrivateKey())
        var script: [URL: (Int, Data, Bool)] = [:]
        for (url, key) in [(urlA, Self.hostAKey), (urlB, Self.hostBKey)] {
            // SAFETY: fixed fixture JSON from the fixture keys.
            script[url.appendingPathComponent("/host-key")] = (
                200, Data(#"{"algorithm":"X25519","public_key":"\#(key)"}"#.utf8), false)
            script[url.appendingPathComponent("/events")] = (200, Data(), true)
        }
        if scriptDrive {
            // SAFETY: a fixed minimal drive response fixture.
            let driveOK = Data(#"{"request_id":"r1","ok":true,"rev":5,"result":{"lines":["l1"],"blocks":[]}}"#.utf8)
            script[urlA.appendingPathComponent("/drive")] = (200, driveOK, false)
            script[urlB.appendingPathComponent("/drive")] = (200, driveOK, false)
        }
        HostSwitchURLProtocol.setScript(script)
        let config = URLSessionConfiguration.ephemeral
        config.protocolClasses = [HostSwitchURLProtocol.self]
        let session = URLSession(configuration: config)
        self.session = session
        let model = AppModel(session: session, defaults: defaults,
                             identityLoader: { (signer, .insecureFallback) },
                             loadMeta: { nil }, saveMeta: { _ in },
                             wipeIdentity: {}, profileStore: store)
        self.model = model
        // Both hosts verify + stream (active A via its own gate; B via the
        // coordinator), so composite reads are authorized.
        model.startLive()
        return (model, profileA, profileB)
    }

    private func requests(to url: URL) -> [URLRequest] {
        HostSwitchURLProtocol.requests.filter { $0.url?.absoluteString == url.absoluteString }
    }

    private func agent(_ id: String, state: AgentState, host: String, ts: UInt64) -> Agent {
        Agent(agentId: id, state: state, ts: ts,
              capabilities: ["read_tail"], host: host,
              workspace: Workspace(repo: "corral", branch: "main"))
    }

    private func seed(model: AppModel, profileA: HostProfile, profileB: HostProfile,
                      seedAgentInB: Bool) async {
        // Active host A holds the equal raw id too.
        model.fleet.apply(.snapshot(Snapshot(schemaVersion: 5, rev: 5, generatedAt: 0,
                                             agents: ["herdr:dup": agent("herdr:dup",
                                                                         state: .working,
                                                                         host: Self.hostAKey,
                                                                         ts: 5)])))
        // Coordinator host B (verify posture first via its own stream).
        // SAFETY: fixed test fixture invariant (see the test's fixture builder); failure here is a harness bug, not a product defect.
        let coordinator = try! XCTUnwrap(model.coordinator)
        if seedAgentInB {
            let storeB = try! XCTUnwrap(coordinator.store(profileID: profileB.id))
            storeB.apply(.snapshot(Snapshot(schemaVersion: 5, rev: 5, generatedAt: 0,
                                            agents: ["herdr:dup": agent("herdr:dup",
                                                                        state: .blocked,
                                                                        host: Self.hostBKey,
                                                                        ts: 5)])))
        }
        await waitUntil(coordinator.allowsLiveWork(profileID: profileB.id))
        await waitUntil(model.keyContinuityState == .verified)
    }

    func testRecentsOpensAndRoutesReadTailToTheOwningHostOnly() async {
        defer { cleanup() }
        let (model, _, profileB) = makeModel(seedAgentInB: true, scriptDrive: true)
        // SAFETY: fixed test fixture invariant (see the test's fixture builder); failure here is a harness bug, not a product defect.
        await seed(model: model, profileA: try! XCTUnwrap(model.activeProfile),
                   profileB: profileB, seedAgentInB: true)
        // E1: request for the COMPOSITE target (host B + raw dup).
        model.requestRecents(for: "herdr:dup", hostProfileID: profileB.id, haptic: false)
        XCTAssertEqual(model.recentsRequest?.hostProfileID, profileB.id)
        XCTAssertEqual(model.recentsRequest?.agentId, "herdr:dup",
                       "the raw agent id stays untouched")
        let bAgent = try! XCTUnwrap(model.fleetAgent(hostProfileID: profileB.id,
                                                     agentID: "herdr:dup"))
        XCTAssertEqual(bAgent.state, .blocked, "the sheet resolves B's row, not A's working row")
        // Drive with a client bound to host A on purpose: the composite
        // route must STILL sign against B's URL with B's key id.
        let clientForA = DriveClient(host: urlA, session: session ?? .shared)
        model.driveReadTail(agent: bAgent, hostProfileID: profileB.id,
                            driveClient: clientForA)
        await waitUntil(!requests(to: urlB.appendingPathComponent("/drive")).isEmpty)
        XCTAssertTrue(requests(to: urlA.appendingPathComponent("/drive")).isEmpty,
                      "a read_tail for host B must NEVER reach host A")
        let driveBody = try! XCTUnwrap(requests(to: urlB.appendingPathComponent("/drive")).first?.httpBody)
        let json = try! XCTUnwrap(try JSONSerialization.jsonObject(with: driveBody)
                                  as? [String: Any])
        XCTAssertEqual(json["key_id"] as? String, "dev_route_b",
                       "the read must be signed with the OWNING profile's key id")
        // The canonical signed-drive envelope rides inline under
        // "envelope" and carries the untouched raw target id.
        let envelope = try! XCTUnwrap(json["envelope"] as? [String: Any])
        XCTAssertEqual(envelope["target"] as? String, "herdr:dup",
                       "the raw agent id is sent untouched")
        // The loaded tail lands in B's store — never A's.
        let coordinator = try! XCTUnwrap(model.coordinator)
        await waitUntil(coordinator.tailPane(profileID: profileB.id, agentID: "herdr:dup") != nil)
        XCTAssertEqual(coordinator.tailPane(profileID: profileB.id, agentID: "herdr:dup")?.lines,
                       ["l1"])
        XCTAssertNil(model.fleet.tailPane(for: "herdr:dup"),
                     "host B's tail must never land in host A's read model")
    }

    func testMissingTargetOnOwningHostNeverSearchesAnotherHost() async {
        defer { cleanup() }
        // Host A HAS "herdr:dup"; host B does NOT.
        let (model, _, profileB) = makeModel(seedAgentInB: false, scriptDrive: true)
        // SAFETY: fixed test fixture invariant (see the test's fixture builder); failure here is a harness bug, not a product defect.
        await seed(model: model, profileA: try! XCTUnwrap(model.activeProfile),
                   profileB: profileB, seedAgentInB: false)
        // E1: opening (B, dup) must NOT resolve A's equal raw id.
        model.requestRecents(for: "herdr:dup", hostProfileID: profileB.id, haptic: false)
        XCTAssertNil(model.recentsRequest,
                     "no other host may satisfy a composite open request")
        let aDup = try! XCTUnwrap(model.fleetAgent(hostProfileID: nil, agentID: "herdr:dup"))
        let clientForA = DriveClient(host: urlA, session: session ?? .shared)
        model.driveReadTail(agent: aDup, hostProfileID: profileB.id,
                            driveClient: clientForA)
        try? await Task.sleep(nanoseconds: 300_000_000)
        XCTAssertTrue(requests(to: urlA.appendingPathComponent("/drive")).isEmpty,
                      "no drive may fall back to another host's row")
        XCTAssertTrue(requests(to: urlB.appendingPathComponent("/drive")).isEmpty,
                      "no drive may run against a host that does not own the row")
    }

    func testOfflineSheetKeepsLoadedOutputAndDisablesReload() async {
        defer { cleanup() }
        let (model, _, profileB) = makeModel(seedAgentInB: true, scriptDrive: true)
        // SAFETY: fixed test fixture invariant (see the test's fixture builder); failure here is a harness bug, not a product defect.
        await seed(model: model, profileA: try! XCTUnwrap(model.activeProfile),
                   profileB: profileB, seedAgentInB: true)
        let coordinator = try! XCTUnwrap(model.coordinator)
        let storeB = try! XCTUnwrap(coordinator.store(profileID: profileB.id))
        storeB.rememberTail(["already-loaded"], for: "herdr:dup")
        XCTAssertEqual(model.recentsRouteState(hostProfileID: profileB.id, agentID: "herdr:dup"),
                       .live)
        // Host B goes offline mid-sheet.
        storeB.noteConnectionError("host offline")
        XCTAssertEqual(model.recentsRouteState(hostProfileID: profileB.id, agentID: "herdr:dup"),
                       .offline, "loaded output stays visible with an offline marker")
        XCTAssertEqual(storeB.tail(for: "herdr:dup"), ["already-loaded"],
                       "the loaded output is retained (memory-only)")
        // Reload is disabled while disconnected: no new drive request.
        let bAgent = try! XCTUnwrap(storeB.agent("herdr:dup"))
        let clientForB = DriveClient(host: urlB, session: session ?? .shared)
        model.driveReadTail(agent: bAgent, hostProfileID: profileB.id,
                            driveClient: clientForB)
        try? await Task.sleep(nanoseconds: 200_000_000)
        XCTAssertTrue(requests(to: urlB.appendingPathComponent("/drive")).isEmpty,
                      "reload must be disabled until the host reconnects")
        // Reconnection restores the live route.
        storeB.noteConnected()
        XCTAssertEqual(model.recentsRouteState(hostProfileID: profileB.id, agentID: "herdr:dup"),
                       .live)
    }

    func testOfflineWithNothingLoadedIsUnavailableAndNeverSynthesizes() async {
        defer { cleanup() }
        let (model, _, profileB) = makeModel(seedAgentInB: true, scriptDrive: true)
        // SAFETY: fixed test fixture invariant (see the test's fixture builder); failure here is a harness bug, not a product defect.
        await seed(model: model, profileA: try! XCTUnwrap(model.activeProfile),
                   profileB: profileB, seedAgentInB: true)
        let coordinator = try! XCTUnwrap(model.coordinator)
        let storeB = try! XCTUnwrap(coordinator.store(profileID: profileB.id))
        storeB.noteConnectionError("host offline")
        XCTAssertEqual(model.recentsRouteState(hostProfileID: profileB.id, agentID: "herdr:dup"),
                       .unavailable, "nothing loaded + disconnected = unavailable")
        XCTAssertNil(storeB.tail(for: "herdr:dup"),
                     "no synthesized/persisted content may appear")
    }

    func testRemoveHostPurgesOnlyTheCompositeTargetsSheetAndTails() async {
        defer { cleanup() }
        let (model, profileA, profileB) = makeModel(seedAgentInB: true, scriptDrive: true)
        await seed(model: model, profileA: profileA, profileB: profileB, seedAgentInB: true)
        // Open B's sheet, then remove host B while it is open (E3).
        model.requestRecents(for: "herdr:dup", hostProfileID: profileB.id, haptic: false)
        XCTAssertEqual(model.recentsRequest?.hostProfileID, profileB.id)
        model.removeHost(profileID: profileB.id)
        XCTAssertNil(model.recentsRequest,
                     "removing a host purges its open sheet state")
        // SAFETY: fixed test fixture invariant (see the test's fixture builder); failure here is a harness bug, not a product defect.
        let coordinator = try! XCTUnwrap(model.coordinator)
        XCTAssertNil(coordinator.store(profileID: profileB.id),
                     "the removed host's session is gone")
        // The ACTIVE host keeps streaming with its equal raw id intact.
        XCTAssertNotNil(model.fleet.agent("herdr:dup"),
                        "host A's equal raw id must survive B's removal")
        XCTAssertEqual(model.activeProfile?.id, profileA.id)
        XCTAssertEqual(model.profiles.count, 1)
    }
}

// MARK: - #397 host-aware push: per-host enrollment, clears, composite routing

/// #397 replaces the #400 F2 "2+ hosts disables push" posture: every
/// paired host enrolls the SAME phone APNs token independently under its
/// OWN signed registration record (key id), per-host notification state
/// lives in Settings, Remove Host / per-host disable clears that host's
/// token (pending + retried when unreachable), and notification taps
/// route by the payload's composite `host_id` — never a guess.
@MainActor
final class HostAwarePushModelTests: XCTestCase {
    private var suiteName = ""
    private var model: AppModel?
    private var session: URLSession?
    /// Retains the fixture AppDelegate — `AppDelegate.shared` is weak, so
    /// the delegate that receives/retains the token must be owned here.
    private var delegate: AppDelegate?

    static let hostKey = Data(repeating: 9, count: 32).base64EncodedString()
    static let otherHostKey = Data(repeating: 10, count: 32).base64EncodedString()
    /// SAFETY: deterministic fixture token — any 64-hex APNs-shaped string.
    static let tokenHex = "cafe1234cafe1234cafe1234cafe1234cafe1234cafe1234cafe1234cafe1234"
    static let registerOK = Data(#"{"key_id":"dev_push_b","grants":["read_tail"],"expiry_ts":1800000000,"revoked":false,"algorithm":"Ed25519"}"#.utf8)
    static let tokenOK = Data(#"{"ok":true,"key_id":"k","push_registered":false}"#.utf8)

    private func cleanup() {
        model?.stopLive()
        model = nil
        delegate?.clearRetainedDeviceToken()
        delegate?.onDeviceTokenReceived = nil
        delegate = nil
        session?.invalidateAndCancel()
        session = nil
        AppDelegate.apnsRegistered = false
        UserDefaults.standard.removeObject(forKey: AppDelegate.deviceTokenUploadedKey)
        KeyContinuityGate.reset()
        HostSwitchURLProtocol.clearScript()
        if !suiteName.isEmpty {
            // SAFETY: suiteName was freshly minted per test.
            UserDefaults(suiteName: suiteName)!.removePersistentDomain(forName: suiteName)
            suiteName = ""
        }
    }

    private func waitUntil(_ condition: @autoclosure () -> Bool,
                           timeout: TimeInterval = 6) async {
        let deadline = Date().addingTimeInterval(timeout)
        while !condition(), Date() < deadline {
            try? await Task.sleep(nanoseconds: 25_000_000)
        }
    }

    /// Two pinned profiles (A active, B coordinator-owned). Both hosts
    /// answer /host-key with their pinned key and /events; /device-token
    /// answers with `hostBTokenStatus` (and 200 for host A).
    private func makeModel(profileCount: Int = 2,
                           hostBTokenStatus: Int = 200) -> AppModel {
        suiteName = "corral.h397.push.\(UUID().uuidString)"
        // SAFETY: a fresh UUID suite name is always a valid suite.
        let defaults = UserDefaults(suiteName: suiteName)!
        let store = HostProfileStore(directory: nil, defaults: defaults)
        // SAFETY: fixed valid fixture URLs (distinct hostnames).
        let urlA = URL(string: "https://push-a.example")!
        let urlB = URL(string: "https://push-b.example")!
        let profileA = try! store.addProfile(displayName: "Host A",
                                             urlString: urlA.absoluteString,
                                             hostKeyB64: Self.hostKey,
                                             fingerprint: "FINGER",
                                             keyId: "dev_push_a",
                                             grants: ["read_tail"],
                                             expiryTs: 1_800_000_000,
                                             registeredAt: 1)
        if profileCount > 1 {
            try! store.addProfile(displayName: "Host B",
                                  urlString: urlB.absoluteString,
                                  hostKeyB64: Self.otherHostKey,
                                  fingerprint: "FINGER",
                                  keyId: "dev_push_b",
                                  grants: ["read_tail"],
                                  expiryTs: 1_800_000_000,
                                  registeredAt: 1)
        }
        defaults.set(profileA.id.uuidString, forKey: "fleetnotifier.activeHostProfileID")
        let signer = DeviceSigner(key: Curve25519.Signing.PrivateKey())
        var script: [URL: (Int, Data, Bool)] = [:]
        for (url, key) in [(urlA, Self.hostKey), (urlB, Self.otherHostKey)] {
            // SAFETY: fixed fixture JSON from the fixture keys.
            script[url.appendingPathComponent("/host-key")] = (
                200, Data(#"{"algorithm":"X25519","public_key":"\#(key)"}"#.utf8), false)
            script[url.appendingPathComponent("/events")] = (200, Data(), true)
            script[url.appendingPathComponent("/device-token")] = (
                url == urlB ? hostBTokenStatus : 200, Self.tokenOK, false)
        }
        script[URL(string: "https://push-b.example/register")!] = (200, Self.registerOK, false)
        HostSwitchURLProtocol.setScript(script)
        let config = URLSessionConfiguration.ephemeral
        config.protocolClasses = [HostSwitchURLProtocol.self]
        let session = URLSession(configuration: config)
        self.session = session
        // The shared AppDelegate (SAME defaults suite) owns the retained
        // token + the ACTIVE-host lifecycle upload. Retained here —
        // AppDelegate.shared is weak and the upload tasks read it.
        let fixtureDelegate = AppDelegate(identityLifecycle: .shared,
                                           session: session,
                                           defaults: defaults,
                                           identityProvider: { signer })
        self.delegate = fixtureDelegate
        let model = AppModel(session: session, defaults: defaults,
                             identityLoader: { (signer, .insecureFallback) },
                             loadMeta: { nil }, saveMeta: { _ in },
                             wipeIdentity: {}, profileStore: store)
        self.model = model
        return model
    }

    private func requests(to url: URL) -> [URLRequest] {
        HostSwitchURLProtocol.requests.filter { $0.url?.absoluteString == url.absoluteString }
    }

    private func deviceTokenBodies(to url: URL) -> [[String: Any]] {
        requests(to: url).compactMap { request in
            guard let body = request.httpBody,
                  let json = try? JSONSerialization.jsonObject(with: body)
                    as? [String: Any] else { return nil }
            return json
        }
    }

    private func deviceTokenValue(_ body: [String: Any]) -> String? {
        (body["request"] as? [String: Any])?["device_token"] as? String
    }

    private func keyID(_ body: [String: Any]) -> String? {
        body["key_id"] as? String
    }

private func startAndVerifyBothHosts(_ model: AppModel) async -> (UUID, UUID) {
        let profileA = model.profiles[0]
        let profileB = model.profiles[1]
        model.startLive()
        await waitUntil(model.keyContinuityState == .verified)
        await waitUntil(model.coordinator?.posture(profileID: profileB.id) == .verified)
        // The per-host fan-out only runs on startLive — re-enter it now
        // that BOTH hosts are verified so enrollments are guaranteed.
        model.startLive()
        return (profileA.id, profileB.id)
    }

    private func seedAgent(_ id: String, host: String,
                           in model: AppModel, profileID: UUID?) {
        let agent = Agent(agentId: id, state: .idle, ts: 1, host: host)
        if let profileID, profileID != model.activeProfileID,
           let store = model.coordinator?.store(profileID: profileID) {
            store.apply(.snapshot(Snapshot(schemaVersion: 5, rev: 1, generatedAt: 0,
                                           agents: [id: agent])))
        } else {
            model.fleet.apply(.snapshot(Snapshot(schemaVersion: 5, rev: 1, generatedAt: 0,
                                                 agents: [id: agent])))
        }
    }

    func testTwoHostsEnrollTheSameTokenIndependentlyUnderTheirOwnKeyIDs() async {
        defer { cleanup() }
        let model = makeModel()
        // The OS token arrives BEFORE the stream is live: retained only.
        _ = self.delegate?.receiveDeviceToken(Self.tokenHex)
// SAFETY: fixed valid fixture URL (distinct hostname).
        let urlA = URL(string: "https://push-a.example/device-token")!
// SAFETY: fixed valid fixture URL (distinct hostname).
        let urlB = URL(string: "https://push-b.example/device-token")!
        let (profileA, profileB) = await startAndVerifyBothHosts(model)

        // ACTIVE host A is enrolled by the delegate's lifecycle path; host
        // B by the model's per-host fan-out — BOTH with the same token,
        // each signed under ITS OWN key id.
        await waitUntil(!requests(to: urlA).isEmpty && !requests(to: urlB).isEmpty)
        let bodiesA = deviceTokenBodies(to: urlA)
        let bodiesB = deviceTokenBodies(to: urlB)
        let enrollA = bodiesA.first { deviceTokenValue($0) == Self.tokenHex }
        let enrollB = bodiesB.first { deviceTokenValue($0) == Self.tokenHex }
        XCTAssertNotNil(enrollA, "host A must enroll the retained token")
        XCTAssertNotNil(enrollB, "host B must enroll the SAME retained token")
// SAFETY: enrollA was asserted non-nil immediately above.
        XCTAssertEqual(keyID(enrollA!), "dev_push_a",
                       "A's enrollment is signed with A's key id")
// SAFETY: enrollB was asserted non-nil immediately above.
        XCTAssertEqual(keyID(enrollB!), "dev_push_b",
                       "B's enrollment is signed with B's key id (independent record)")
// SAFETY: enrollB was asserted non-nil immediately above.
        XCTAssertEqual(deviceTokenValue(enrollB!), Self.tokenHex)

        // #397: the 2+-host blanket gate is gone — the ACTIVE host's gate
        // now reflects only ITS continuity posture.
        let allows = await KeyContinuityGate.allowsPushRegistration()
        XCTAssertTrue(allows, "a verified host may enroll with 2+ profiles")
        XCTAssertTrue(model.pendingPushTokenClears.isEmpty,
                      "normal operation schedules no clears")
        XCTAssertEqual(model.profiles.first { $0.id == profileA }?.notificationsEnabled, true)
        XCTAssertEqual(model.profiles.first { $0.id == profileB }?.notificationsEnabled, true)
    }

    func testUnverifiedPinnedHostNeverEnrollsTheToken() async {
        defer { cleanup() }
        // Host B's /host-key never answers (no script entry): its posture
        // stays .verifying — an UNVERIFIED key must not receive a token.
        suiteName = "corral.h397.unverified.\(UUID().uuidString)"
        // SAFETY: fresh UUID suite; script omits B's host-key endpoint.
        let defaults = UserDefaults(suiteName: suiteName)!
        let store = HostProfileStore(directory: nil, defaults: defaults)
        // SAFETY: fixed valid fixture URLs.
        let urlA = URL(string: "https://push-a.example")!
        let urlB = URL(string: "https://push-b.example")!
        let profileA = try! store.addProfile(displayName: "Host A",
                                             urlString: urlA.absoluteString,
                                             hostKeyB64: Self.hostKey,
                                             fingerprint: "FINGER",
                                             keyId: "dev_push_a",
                                             grants: ["read_tail"],
                                             expiryTs: 1_800_000_000,
                                             registeredAt: 1)
        let profileB = try! store.addProfile(displayName: "Host B",
                                             urlString: urlB.absoluteString,
                                             hostKeyB64: Self.otherHostKey,
                                             fingerprint: "FINGER",
                                             keyId: "dev_push_b",
                                             grants: ["read_tail"],
                                             expiryTs: 1_800_000_000,
                                             registeredAt: 1)
        defaults.set(profileA.id.uuidString, forKey: "fleetnotifier.activeHostProfileID")
        let signer = DeviceSigner(key: Curve25519.Signing.PrivateKey())
        var script: [URL: (Int, Data, Bool)] = [:]
        script[urlA.appendingPathComponent("/host-key")] = (
            200, Data(#"{"algorithm":"X25519","public_key":"\#(Self.hostKey)"}"#.utf8), false)
        script[urlA.appendingPathComponent("/events")] = (200, Data(), true)
        script[urlA.appendingPathComponent("/device-token")] = (200, Self.tokenOK, false)
        // Host B answers ONLY /device-token (never /host-key): its stream
        // can never verify, so no enrollment may reach it.
        script[urlB.appendingPathComponent("/device-token")] = (200, Self.tokenOK, false)
        HostSwitchURLProtocol.setScript(script)
        let config = URLSessionConfiguration.ephemeral
        config.protocolClasses = [HostSwitchURLProtocol.self]
        let session = URLSession(configuration: config)
        self.session = session
        let fixtureDelegate = AppDelegate(identityLifecycle: .shared, session: session,
                                           defaults: defaults,
                                           identityProvider: { signer })
        self.delegate = fixtureDelegate
        let model = AppModel(session: session, defaults: defaults,
                             identityLoader: { (signer, .insecureFallback) },
                             loadMeta: { nil }, saveMeta: { _ in },
                             wipeIdentity: {}, profileStore: store)
        self.model = model
        _ = fixtureDelegate.receiveDeviceToken(Self.tokenHex)
        model.startLive()
        await waitUntil(model.keyContinuityState == .verified)
        // Give B's verification + any fan-out time to (not) happen.
        try? await Task.sleep(nanoseconds: 600_000_000)
        let bodiesB = deviceTokenBodies(to: urlB.appendingPathComponent("/device-token"))
        XCTAssertTrue(bodiesB.isEmpty,
                      "an UNVERIFIED pinned host must never receive the token (AC7)")
        XCTAssertEqual(model.coordinator?.posture(profileID: profileB.id), .verifying)
    }

    func testPerHostDisableClearsOnlyThatHostsToken() async {
        defer { cleanup() }
        let model = makeModel()
// SAFETY: fixed valid fixture URL (distinct hostname).
        let urlA = URL(string: "https://push-a.example/device-token")!
// SAFETY: fixed valid fixture URL (distinct hostname).
        let urlB = URL(string: "https://push-b.example/device-token")!
        _ = self.delegate?.receiveDeviceToken(Self.tokenHex)
        await startAndVerifyBothHosts(model)
        await waitUntil(!deviceTokenBodies(to: urlA).isEmpty
                        && !deviceTokenBodies(to: urlB).isEmpty)

        // Disable host B: ONLY B's enrollment is cleared (empty token).
// SAFETY: makeModel always seeds Host B; the disable below needs it.
        let profileB = model.profiles.first { $0.displayName == "Host B" }!
        model.setHostNotificationsEnabled(profileID: profileB.id, enabled: false)
        await waitUntil(deviceTokenBodies(to: urlB).contains {
            deviceTokenValue($0) == ""
        })
        await waitUntil(model.pendingPushTokenClears.isEmpty)
        let clearsB = deviceTokenBodies(to: urlB).filter { deviceTokenValue($0) == "" }
        XCTAssertEqual(clearsB.count, 1, "one empty-token clear for host B")
        XCTAssertEqual(keyID(clearsB[0]), "dev_push_b")
        XCTAssertTrue(deviceTokenBodies(to: urlA).allSatisfy { deviceTokenValue($0) != "" },
                      "host A's enrollment must be untouched by B's disable")
        XCTAssertEqual(model.profiles.first { $0.id == profileB.id }?.notificationsEnabled,
                       false, "the per-host flag must persist on the profile")
        XCTAssertTrue(model.pendingPushTokenClears.isEmpty,
                      "a reachable disable clears immediately")

        // Re-enable re-enrolls the retained token under B's key id.
        let enrollmentsBefore = deviceTokenBodies(to: urlB).filter {
            deviceTokenValue($0) == Self.tokenHex
        }.count
        model.setHostNotificationsEnabled(profileID: profileB.id, enabled: true)
        await waitUntil(deviceTokenBodies(to: urlB).filter {
            deviceTokenValue($0) == Self.tokenHex
        }.count > enrollmentsBefore)
        XCTAssertEqual(model.profiles.first { $0.id == profileB.id }?.notificationsEnabled,
                       true)
    }

    func testOfflinePerHostDisableKeepsClearPendingAndRetriesOnReconnect() async {
        defer { cleanup() }
        let model = makeModel(hostBTokenStatus: 500)
// SAFETY: fixed valid fixture URL (distinct hostname).
        let urlB = URL(string: "https://push-b.example/device-token")!
        _ = self.delegate?.receiveDeviceToken(Self.tokenHex)
        await startAndVerifyBothHosts(model)

// SAFETY: makeModel always seeds Host B; the disable below needs it.
        let profileB = model.profiles.first { $0.displayName == "Host B" }!
        model.setHostNotificationsEnabled(profileID: profileB.id, enabled: false)
        await waitUntil(!model.pendingPushTokenClears.isEmpty)
        XCTAssertTrue(model.pendingPushTokenClears.contains(profileB.id),
                      "an unreachable host surfaces its pending clear")
        XCTAssertEqual(model.pendingPushClearNames(), ["Host B"],
                       "Settings shows the pending cleanup name")

        // The host reconnects → the clear retries and lands.
        HostSwitchURLProtocol.setScript([
            urlB: (200, Self.tokenOK, false),
        ])
        model.retryPendingPushTokenClear(profileID: profileB.id)
        await waitUntil(!deviceTokenBodies(to: urlB).contains { deviceTokenValue($0) == "" })
        await waitUntil(model.pendingPushTokenClears.isEmpty)
        XCTAssertFalse(model.pendingPushTokenClears.contains(profileB.id))
    }

    func testRemoveHostClearsReachableHostsToken() async {
        defer { cleanup() }
        let model = makeModel()
// SAFETY: fixed valid fixture URL (distinct hostname).
        let urlA = URL(string: "https://push-a.example/device-token")!
// SAFETY: fixed valid fixture URL (distinct hostname).
        let urlB = URL(string: "https://push-b.example/device-token")!
        _ = self.delegate?.receiveDeviceToken(Self.tokenHex)
        await startAndVerifyBothHosts(model)
        await waitUntil(!deviceTokenBodies(to: urlA).isEmpty
                        && !deviceTokenBodies(to: urlB).isEmpty)

// SAFETY: makeModel always seeds Host B; the removal below needs it.
        let profileB = model.profiles.first { $0.displayName == "Host B" }!
        model.removeHost(profileID: profileB.id)
        // The removal's empty-token clear lands for B; A is untouched.
        await waitUntil(deviceTokenBodies(to: urlB).contains { deviceTokenValue($0) == "" })
        await waitUntil(model.pendingPushTokenClears.isEmpty)
// SAFETY: the empty-token clear was awaited just above.
        XCTAssertEqual(keyID(deviceTokenBodies(to: urlB).filter { deviceTokenValue($0) == "" }.first!),
                       "dev_push_b")
        XCTAssertTrue(deviceTokenBodies(to: urlA).allSatisfy { deviceTokenValue($0) != "" },
                      "removing B never clears A's enrollment")
        XCTAssertEqual(model.profiles.map(\.displayName), ["Host A"])
        XCTAssertTrue(model.pendingPushTokenClears.isEmpty)
        XCTAssertNil(model.coordinator?.store(profileID: profileB.id),
                     "the removed host's session is gone")
    }

    func testRemoveOfflineHostKeepsPendingClearUntilSameURLRepairs() async {
        defer { cleanup() }
        let model = makeModel(hostBTokenStatus: 500)
// SAFETY: fixed valid fixture URL (distinct hostname).
        let urlB = URL(string: "https://push-b.example")!
        _ = self.delegate?.receiveDeviceToken(Self.tokenHex)
        await startAndVerifyBothHosts(model)

// SAFETY: makeModel always seeds Host B; the removal below needs it.
        let profileB = model.profiles.first { $0.displayName == "Host B" }!
        model.removeHost(profileID: profileB.id)
        await waitUntil(!model.pendingPushTokenClears.isEmpty)
        XCTAssertEqual(model.profiles.map(\.displayName), ["Host A"])
        XCTAssertEqual(model.pendingPushClearNames(), ["Host B"],
                       "a removed-but-unreachable host shows host-side cleanup guidance")
        // A retry while the profile is gone keeps the pending entry (the
        // URL is not paired, so nothing to clear against yet).
        model.retryPendingPushTokenClear(profileID: profileB.id)
        XCTAssertTrue(model.pendingPushTokenClears.contains(profileB.id))

        // Re-pairing the SAME URL supersedes the stale removal-clear (the
        // fresh enrollment writes the token the shared key record holds).
// SAFETY: prepareHostPairing only throws on invalid fixture input.
        let prepared = try! await model.prepareHostPairing(
            displayName: "Host B", rawURL: urlB.absoluteString)
        await model.completeAddHost(prepared, token: "tok")
        XCTAssertTrue(model.pendingPushTokenClears.isEmpty,
                      "a same-URL re-pair supersedes the pending removal-clear")
        XCTAssertEqual(model.profiles.first { $0.displayName == "Host B" }?.keyId,
                       "dev_push_b", "re-pairing restores the same deterministic key record")
    }

    func testNotificationTapsRouteByCompositeHostIdentityNeverGuessing() async {
        defer { cleanup() }
        let model = makeModel()
        _ = self.delegate?.receiveDeviceToken(Self.tokenHex)
        let (profileA, profileB) = await startAndVerifyBothHosts(model)
        // Equal RAW agent ids on both hosts, stamped with each host's key.
        seedAgent("herdr:dup", host: Self.hostKey, in: model, profileID: profileA)
        seedAgent("herdr:dup", host: Self.otherHostKey, in: model, profileID: profileB)

        // A's alert opens A's recents; B's alert opens B's — the raw id is
        // preserved for the /drive request on BOTH routes.
        model.openNotification(agentId: "herdr:dup", hostKeyB64: Self.hostKey)
        XCTAssertEqual(model.recentsRequest?.hostProfileID, profileA)
        XCTAssertEqual(model.recentsRequest?.agentId, "herdr:dup")
        model.recentsRequest = nil

        model.openNotification(agentId: "herdr:dup", hostKeyB64: Self.otherHostKey)
        XCTAssertEqual(model.recentsRequest?.hostProfileID, profileB,
                       "the SAME raw agent id on host B opens B's sheet, never A's")
        XCTAssertEqual(model.recentsRequest?.agentId, "herdr:dup")
        model.recentsRequest = nil

        // A legacy host-less payload with 2+ hosts is NON-ACTIONABLE.
        model.openNotification(agentId: "herdr:dup", hostKeyB64: nil)
        XCTAssertNil(model.recentsRequest, "no host_id + 2 hosts = no guessing")
        XCTAssertEqual(model.banner?.kind, "notification_host_unknown",
                       "bounded diagnostic, never a cross-host open")
        model.banner = nil

        // An unknown / removed host's payload is NON-ACTIONABLE.
        model.openNotification(agentId: "herdr:dup", hostKeyB64: "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=")
        XCTAssertNil(model.recentsRequest, "an unknown host identity never opens a lane")
        XCTAssertEqual(model.banner?.kind, "notification_host_unknown")

        // A notification for a REMOVED host cannot recreate its profile or
        // cached lane — and never falls through to the other host.
        model.recentsRequest = nil
        model.banner = nil
        model.removeHost(profileID: profileB)
        XCTAssertNil(model.coordinator?.store(profileID: profileB))
        model.openNotification(agentId: "herdr:dup", hostKeyB64: Self.otherHostKey)
        XCTAssertNil(model.recentsRequest,
                     "a removed host's alert must not open the remaining host's lane")
        XCTAssertEqual(model.banner?.kind, "notification_host_unknown")
    }

    func testSingleHostLegacyRoutingStillWorksWithAndWithoutHostID() async {
        defer { cleanup() }
        let model = makeModel(profileCount: 1)
// SAFETY: fixed valid fixture URL (distinct hostname).
        let urlA = URL(string: "https://push-a.example/device-token")!
        _ = self.delegate?.receiveDeviceToken(Self.tokenHex)
        model.startLive()
        await waitUntil(model.keyContinuityState == .verified)
        await waitUntil(!deviceTokenBodies(to: urlA).isEmpty)
        seedAgent("herdr:a1", host: Self.hostKey, in: model, profileID: model.profiles[0].id)

        // The sole host still routes a host_id-bearing payload…
        model.openNotification(agentId: "herdr:a1", hostKeyB64: Self.hostKey)
        XCTAssertEqual(model.recentsRequest?.agentId, "herdr:a1")
        model.recentsRequest = nil
        // …AND a legacy host-less payload (F1 back-compat).
        model.openNotification(agentId: "herdr:a1", hostKeyB64: nil)
        XCTAssertEqual(model.recentsRequest?.agentId, "herdr:a1")
        model.recentsRequest = nil
        // A FOREIGN host identity with one PINNED host stays non-actionable
        // (never guess a rotated/replacement identity).
        model.openNotification(agentId: "herdr:a1", hostKeyB64: Self.otherHostKey)
        XCTAssertNil(model.recentsRequest)
        XCTAssertEqual(model.banner?.kind, "notification_host_unknown")
    }
}

// MARK: - #401 multi-host host filter + session-only default (D1/D2/D6)

/// D1 defaults (every fresh launch starts All Hosts + All Repos; both
/// filters session-only), the D2 host selection lifecycle, removed-host
/// reconciliation, Settings reorder/rename routing, and the #400 rev N2
/// recents-route .unavailable guarantee for removed hosts.
@MainActor
final class MultiHostHostFilterModelTests: XCTestCase {
    private var suiteName = ""
    private var model: AppModel?
    private var session: URLSession?

    static let keyA = Data(repeating: 21, count: 32).base64EncodedString()
    static let keyB = Data(repeating: 22, count: 32).base64EncodedString()

    private func cleanup() {
        model?.stopLive()
        model = nil
        session?.invalidateAndCancel()
        session = nil
        KeyContinuityGate.reset()
        HostSwitchURLProtocol.clearScript()
        if !suiteName.isEmpty {
            // SAFETY: suiteName was freshly minted per test.
            UserDefaults(suiteName: suiteName)!.removePersistentDomain(forName: suiteName)
            suiteName = ""
        }
    }

    /// Two profiles: A unpinned (paused on fingerprint confirmation — no
    /// continuity network on remove-host re-arm paths), B pinned. No
    /// streams are started anywhere in these fixtures.
    private func makeStore(defaults: UserDefaults) -> (HostProfileStore, HostProfile, HostProfile) {
        let store = HostProfileStore(directory: nil, defaults: defaults)
        // SAFETY: fixed fixture URLs (distinct hostnames).
        let profileA = try! store.addProfile(displayName: "Host A",
                                             urlString: "https://filter-a.example",
                                             hostKeyB64: nil,
                                             keyId: "dev_filter_a",
                                             grants: ["read_tail"],
                                             expiryTs: 1_800_000_000,
                                             registeredAt: 1)
        let profileB = try! store.addProfile(displayName: "Host B",
                                             urlString: "https://filter-b.example",
                                             hostKeyB64: Self.keyB,
                                             fingerprint: "FINGER",
                                             keyId: "dev_filter_b",
                                             grants: ["read_tail"],
                                             expiryTs: 1_800_000_000,
                                             registeredAt: 1)
        return (store, profileA, profileB)
    }

    private func makeModel() -> (AppModel, HostProfileStore, HostProfile, HostProfile) {
        suiteName = "corral.h401.filter.\\(UUID().uuidString)"
        // SAFETY: a fresh UUID suite name is always a valid suite.
        let defaults = UserDefaults(suiteName: suiteName)!
        let (store, profileA, profileB) = makeStore(defaults: defaults)
        defaults.set(profileA.id.uuidString, forKey: "fleetnotifier.activeHostProfileID")
        let signer = DeviceSigner(key: Curve25519.Signing.PrivateKey())
        let model = AppModel(defaults: defaults,
                             identityLoader: { (signer, .insecureFallback) },
                             loadMeta: { nil }, saveMeta: { _ in },
                             wipeIdentity: {}, profileStore: store)
        self.model = model
        return (model, store, profileA, profileB)
    }

    func testFreshLaunchDefaultsToAllHostsAndAllReposAndStaysSessionOnly() {
        defer { cleanup() }
        let (model, _, _, profileB) = makeModel()
        XCTAssertNil(model.hostFilter,
                     "D1: every fresh launch starts at All Hosts")
        XCTAssertNil(model.repoFilter,
                     "D1: every fresh launch starts at All Repos")
        // The choices survive in-session refreshes but NEVER persist: a
        // fresh model over the SAME store/defaults starts All again.
        model.hostFilter = profileB.id
        model.repoFilter = "corral"
        // SAFETY: the suite name was freshly minted in makeModel().
        let defaults = UserDefaults(suiteName: suiteName)!
        let store = HostProfileStore(directory: nil, defaults: defaults)
        let signer = DeviceSigner(key: Curve25519.Signing.PrivateKey())
        let relaunched = AppModel(defaults: defaults,
                                  identityLoader: { (signer, .insecureFallback) },
                                  loadMeta: { nil }, saveMeta: { _ in },
                                  wipeIdentity: {}, profileStore: store)
        self.model = relaunched
        XCTAssertNil(relaunched.hostFilter,
                     "D1: the host filter is session-only (never persisted)")
        XCTAssertNil(relaunched.repoFilter,
                     "D1: the repo filter is session-only (never persisted)")
    }

    func testSelectHostFilterKeepsOnlyConfiguredProfilesAndReconcilesRemoval() {
        defer { cleanup() }
        let (model, _, profileA, profileB) = makeModel()
        XCTAssertTrue(model.multiHostConfigured, "2+ profiles arm the host surface")
        model.selectHostFilter(profileB.id)
        XCTAssertEqual(model.hostFilter, profileB.id)
        model.selectHostFilter(profileA.id)
        XCTAssertEqual(model.hostFilter, profileA.id)
        // All Hosts.
        model.selectHostFilter(nil)
        XCTAssertNil(model.hostFilter)
        // Unknown/removed ids reconcile to All (never a dangling filter).
        model.selectHostFilter(UUID())
        XCTAssertNil(model.hostFilter, "an unknown host selection renders All")
        model.selectHostFilter(profileB.id)
        model.removeHost(profileID: profileB.id)
        XCTAssertNil(model.hostFilter,
                     "removing the selected host reconciles the filter to All Hosts")
        XCTAssertEqual(model.profiles.map(\.displayName), ["Host A"])
        XCTAssertFalse(model.multiHostConfigured, "one profile left = single host")
    }

    func testSingleHostKeepsTheLegacyBoardSurfaceInert() {
        defer { cleanup() }
        let (model, _, profileB, _) = makeModel()
        XCTAssertNotNil(model.aggregateBoardRows,
                        "2+ profiles arm the composite board rows")
        // One host (remove B): the aggregate path is INERT — the board
        // keeps the legacy fleet-store rendering (F1 parity: no host chip
        // row, no row badges — the guard the wiring tests pin too).
        model.removeHost(profileID: profileB.id)
        XCTAssertEqual(model.profiles.count, 1)
        XCTAssertNil(model.aggregateBoardRows,
                     "single-host F1: no composite board rows, no host row/badges")
    }

    func testReorderDrivesTheStoreOrderThatChipsFollow() {
        defer { cleanup() }
        let (model, store, _, _) = makeModel()
        // [A, B] → move row 0 down to destination 2 ⇒ [B, A].
        model.moveHosts(from: IndexSet(integer: 0), to: 2)
        XCTAssertEqual(model.profiles.map(\.displayName), ["Host B", "Host A"])
        XCTAssertEqual(store.orderedProfiles.map(\.displayName), ["Host B", "Host A"],
                       "the store order (the chip order) follows the drag")
        XCTAssertEqual(model.profiles.map(\.order), [0, 1],
                       "store re-normalizes orders to consecutive integers")
        // Move row 1 (A) up to destination 0 ⇒ [A, B].
        model.moveHosts(from: IndexSet(integer: 1), to: 0)
        XCTAssertEqual(model.profiles.map(\.displayName), ["Host A", "Host B"])
    }

    func testRenameHostInPlaceSurfacesDuplicateErrorWithoutChangingName() {
        defer { cleanup() }
        let (model, _, _, profileB) = makeModel()
        XCTAssertNil(model.renameHost(id: profileB.id, to: "Renamed Host"))
        XCTAssertEqual(model.profiles.first { $0.id == profileB.id }?.displayName,
                       "Renamed Host")
        // Duplicate display names are rejected with the error text.
        let error = model.renameHost(id: profileB.id, to: "Host A")
        XCTAssertNotNil(error)
        XCTAssertEqual(model.profiles.first { $0.id == profileB.id }?.displayName,
                       "Renamed Host", "a rejected rename never changes the name")
    }

    // MARK: - #400 rev N2: removed-host targets render .unavailable

    func testRemovedHostRecentsTargetRendersUnavailableNeverFallingThroughToActiveStore() {
        defer { cleanup() }
        let (model, _, _, profileB) = makeModel()
        // The ACTIVE store is connected and holds an EQUAL raw agent id —
        // exactly the fall-through the removed-host target must never take.
        let signer = DeviceSigner(key: Curve25519.Signing.PrivateKey())
        model.fleet.apply(.snapshot(Snapshot(schemaVersion: 5, rev: 1,
                                             generatedAt: 0,
                                             agents: ["herdr:dup": Agent(
                                                agentId: "herdr:dup", state: .working,
                                                ts: 5, capabilities: ["read_tail"])])))
        model.fleet.noteConnected()
        // Host B's OWN session had loaded output before it went offline
        // (E2 offline posture), then the host was REMOVED.
        if let storeB = model.coordinator?.store(profileID: profileB.id) {
            storeB.rememberTail(["already-loaded"], for: "herdr:dup")
            storeB.noteConnectionError("host unreachable")
        }
        XCTAssertEqual(model.recentsRouteState(hostProfileID: profileB.id,
                                               agentID: "herdr:dup"),
                       .offline, "loaded output + disconnected = offline while paired")
        model.removeHost(profileID: profileB.id)
        XCTAssertNil(model.coordinator?.store(profileID: profileB.id),
                     "the removed host's session is gone")
        XCTAssertEqual(model.recentsRouteState(hostProfileID: profileB.id,
                                               agentID: "herdr:dup"),
                       .unavailable,
                       "N2: a REMOVED-host target renders .unavailable — it must NEVER fall through to the active fleet store and resolve an equal raw id")
    }

    func testPausedHostRecentsTargetRendersUnavailable() {
        defer { cleanup() }
        let (model, _, profileA, _) = makeModel()
        XCTAssertEqual(model.recentsRouteState(hostProfileID: profileA.id,
                                               agentID: "herdr:dup"),
                       .unavailable,
                       "an awaiting-fingerprint (paused) host has no live route")
    }
}

// MARK: - #401 multi-host board projections (D2-D7 pure view model)

/// Pure projection coverage for the multi-host board: host chips (counts +
/// health + unified All chip), repo filters rescoped per host (D4), merged
/// repo subgroups (D5), outage summaries (D7), health classification, and
/// the last-seen/stale text helpers (C6).
final class MultiHostBoardProjectionTests: XCTestCase {
    private func agent(_ id: String, state: AgentState, ts: UInt64,
                       repo: String? = nil) -> Agent {
        var workspace = Workspace()
        if let repo { workspace = Workspace(repo: repo, branch: "main") }
        return Agent(agentId: id, state: state, reason: nil,
                     ts: ts, capabilities: [], workspace: workspace)
    }

    private func row(_ hostID: UUID, _ id: String, state: AgentState,
                     ts: UInt64, repo: String? = nil,
                     stale: Bool = false, lastSeen: UInt64 = 0) -> HostBoardRow {
        HostBoardRow(identity: CompositeAgentID(hostProfileID: hostID, agentID: id),
                     agent: agent(id, state: state, ts: ts, repo: repo),
                     isStale: stale,
                     lastSeen: lastSeen == 0 ? ts : lastSeen)
    }

    func testLaneCountsAndHostFilterRows() {
        let a = UUID(), b = UUID()
        let rows = [row(a, "herdr:a1", state: .working, ts: 10, repo: "corral"),
                    row(a, "herdr:a2", state: .idle, ts: 20, repo: "demo"),
                    row(b, "herdr:b1", state: .blocked, ts: 30, repo: "corral")]
        XCTAssertEqual(BoardModel.laneCounts(rows), [a: 2, b: 1],
                       "D3: per-host total lane counts are repo-independent")
        XCTAssertEqual(BoardModel.rows(rows, forHost: a).count, 2)
        XCTAssertEqual(BoardModel.rows(rows, forHost: b).count, 1)
        XCTAssertEqual(BoardModel.rows(rows, forHost: nil).count, 3,
                       "nil = All Hosts")
    }

    func testRepoFiltersRescopeToTheSelectedHost() {
        let a = UUID(), b = UUID()
        let rows = [row(a, "herdr:a1", state: .working, ts: 10, repo: "corral"),
                    row(a, "herdr:a2", state: .working, ts: 20, repo: "demo-atlas"),
                    row(b, "herdr:b1", state: .blocked, ts: 30, repo: "demo-atlas"),
                    row(b, "herdr:b2", state: .blocked, ts: 40, repo: "demo-garden")]
        // All Hosts: unified chip set + counts.
        let all = BoardModel.repoFilters(rows)
        XCTAssertEqual(all.map(\.repo), ["corral", "demo-atlas", "demo-garden"])
        XCTAssertEqual(all.map(\.count), [1, 2, 1])
        // Host A: choices + counts recalculate (D4).
        let aChips = BoardModel.repoFilters(BoardModel.rows(rows, forHost: a))
        XCTAssertEqual(aChips.map(\.repo), ["corral", "demo-atlas"])
        XCTAssertEqual(aChips.map(\.count), [1, 1])
        // Host B.
        let bChips = BoardModel.repoFilters(BoardModel.rows(rows, forHost: b))
        XCTAssertEqual(bChips.map(\.repo), ["demo-atlas", "demo-garden"])
        // Host + repo both apply (D4).
        let filtered = BoardModel.rows(BoardModel.rows(rows, forHost: b), in: "demo-atlas")
        XCTAssertEqual(filtered.count, 1)
        XCTAssertEqual(filtered.first?.identity.hostProfileID, b)
    }

    func testHostChipsAllFirstWithUnifiedCountAndPartialHealth() {
        let a = UUID(), b = UUID()
        let liveFacts = BoardModel.HostRuntimeFacts(isConnected: true)
        let offlineFacts = BoardModel.HostRuntimeFacts(isConnected: false)
        let chips = BoardModel.hostChips(hosts: [
            BoardModel.HostFilterChip(profileID: a, displayName: "Host A",
                                      laneCount: 3,
                                      health: BoardModel.hostChipHealth(for: liveFacts)),
            BoardModel.HostFilterChip(profileID: b, displayName: "Host B",
                                      laneCount: 1,
                                      health: BoardModel.hostChipHealth(for: offlineFacts)),
        ])
        XCTAssertEqual(chips.count, 3)
        XCTAssertTrue(chips[0].isAll, "the All chip leads the row")
        XCTAssertEqual(chips[0].laneCount, 4, "All = the UNIFIED lane count")
        XCTAssertEqual(chips[0].health, .offline,
                       "partial health: All is never live while a host is offline")
        XCTAssertEqual(chips.map(\.displayName), ["All", "Host A", "Host B"],
                       "hosts follow the user-controlled order")
        // Zero-lane hosts remain visible (D3).
        let zero = BoardModel.hostChips(hosts: [
            BoardModel.HostFilterChip(profileID: a, displayName: "Host A",
                                      laneCount: 0, health: .connecting),
        ])
        XCTAssertEqual(zero.map(\.displayName), ["All", "Host A"])
        XCTAssertEqual(zero[1].laneCount, 0)
    }

    func testAllChipHealthIsLiveOnlyWhenEveryHostIsLive() {
        let a = UUID(), b = UUID()
        let chips = BoardModel.hostChips(hosts: [
            BoardModel.HostFilterChip(profileID: a, displayName: "Host A",
                                      laneCount: 1, health: .live),
            BoardModel.HostFilterChip(profileID: b, displayName: "Host B",
                                      laneCount: 1, health: .live),
        ])
        XCTAssertEqual(chips[0].health, .live)
    }

    func testHealthClassificationFailsClosedOnMismatchAndUnconfirmed() {
        XCTAssertEqual(BoardModel.hostChipHealth(for:
            BoardModel.HostRuntimeFacts(isConnected: true)), .live)
        XCTAssertEqual(BoardModel.hostChipHealth(for:
            BoardModel.HostRuntimeFacts(isConnected: true, keyMismatch: true)),
            .keyMismatch, "a mismatched host is never live")
        XCTAssertEqual(BoardModel.hostChipHealth(for:
            BoardModel.HostRuntimeFacts(isConnected: true, awaitingFingerprint: true)),
            .awaitingFingerprint)
        XCTAssertEqual(BoardModel.hostChipHealth(for:
            BoardModel.HostRuntimeFacts(isConnecting: true)), .connecting)
        XCTAssertEqual(BoardModel.hostChipHealth(for:
            BoardModel.HostRuntimeFacts()), .offline)
    }

    func testOutageSummaryTexts() {
        XCTAssertNil(BoardModel.hostOutageSummary(hosts: []))
        XCTAssertNil(BoardModel.hostOutageSummary(hosts: [
            BoardModel.HostFilterChip(profileID: UUID(), displayName: "A",
                                      laneCount: 1, health: .live)]))
        XCTAssertEqual(BoardModel.hostOutageSummary(hosts: [
            BoardModel.HostFilterChip(profileID: UUID(), displayName: "A",
                                      laneCount: 1, health: .live),
            BoardModel.HostFilterChip(profileID: UUID(), displayName: "B",
                                      laneCount: 1, health: .offline)]),
            "1 host offline")
        XCTAssertEqual(BoardModel.hostOutageSummary(hosts: [
            BoardModel.HostFilterChip(profileID: UUID(), displayName: "A",
                                      laneCount: 1, health: .offline),
            BoardModel.HostFilterChip(profileID: UUID(), displayName: "B",
                                      laneCount: 1, health: .offline)]),
            "2 hosts offline")
        XCTAssertEqual(BoardModel.hostOutageSummary(hosts: [
            BoardModel.HostFilterChip(profileID: UUID(), displayName: "A",
                                      laneCount: 1, health: .offline),
            BoardModel.HostFilterChip(profileID: UUID(), displayName: "B",
                                      laneCount: 1, health: .keyMismatch)]),
            "1 host offline · 1 host key mismatch")
        XCTAssertEqual(BoardModel.hostOutageSummary(hosts: [
            BoardModel.HostFilterChip(profileID: UUID(), displayName: "A",
                                      laneCount: 0,
                                      health: .awaitingFingerprint)]),
            "1 host awaiting fingerprint")
    }

    func testHostSectionsMergeSameRepoAcrossHostsAndPreserveLiveFirstRanking() {
        let a = UUID(), b = UUID()
        // Input arrives in #400's canonical + live-first order (the stale
        // blocked row of the SAME repo has the NEWER ts — live must still
        // lead inside the (blocked, corral) bucket).
        let live = row(a, "herdr:dup", state: .blocked, ts: 10, repo: "corral")
        let stale = row(b, "herdr:dup", state: .blocked, ts: 99, repo: "corral",
                        stale: true, lastSeen: 99)
        let idle = row(b, "herdr:idle", state: .idle, ts: 5)
        let orphan = row(a, "herdr:orphan", state: .blocked, ts: 1, repo: nil)
        let sections = BoardModel.hostSections([live, stale, idle, orphan])
        XCTAssertEqual(sections.statuses.map(\.state), [.blocked, .idle])
        let blocked = sections.statuses[0]
        XCTAssertEqual(blocked.subgroups.map(\.repo), ["corral", nil],
                       "named repos alphabetical, Other LAST — equal repo names from several hosts share one subgroup (D5)")
        let corral = blocked.subgroups[0]
        XCTAssertEqual(corral.rows.map(\.identity.hostProfileID), [a, b],
                       "the same repo from two hosts shares one subgroup")
        XCTAssertFalse(corral.rows[0].isStale)
        XCTAssertTrue(corral.rows[1].isStale)
        XCTAssertEqual(corral.rows[0].agent.state, .blocked)
        XCTAssertEqual(corral.rows[1].agent.state, .blocked,
                       "a stale Blocked lane stays Blocked — never recast (C7)")
        XCTAssertEqual(blocked.header, "blocked (3)")
        XCTAssertEqual(sections.statuses[1].total, 1)
    }

    func testHostSubgroupOtherBucketContainsNoRepoRows() {
        let a = UUID()
        let rows = [row(a, "herdr:o", state: .working, ts: 5, repo: nil)]
        let subgroups = BoardModel.hostSubgroups(of: rows)
        XCTAssertEqual(subgroups.map(\.repo), [nil])
        XCTAssertEqual(subgroups[0].displayName, BoardModel.otherRepoLabel)
    }

    func testLastSeenLabelAndExpiryTextAreDeterministic() {
        let now: UInt64 = 1_800_000_000_000
        XCTAssertEqual(RelativeTime.lastSeenLabel(lastSeenMs: now - 360_000,
                                                  nowMs: now),
                       "last seen 6m ago")
        XCTAssertEqual(RelativeTime.lastSeenLabel(lastSeenMs: now - 90_000,
                                                  nowMs: now),
                       "last seen 1m ago")
        XCTAssertEqual(RelativeTime.lastSeenLabel(lastSeenMs: now + 5_000,
                                                  nowMs: now),
                       "last seen 0s ago",
                       "a clock-skewed future stamp never renders a negative age")
        XCTAssertNil(BoardModel.expiryText(epochSeconds: nil))
        XCTAssertEqual(BoardModel.expiryText(epochSeconds: 1_800_000_000),
                       "2027-01-15")
    }
}

// MARK: - #401 multi-host surface wiring (FleetViews source bundle)

/// Source-wiring pins over the bundled FleetViews source: the host-chip row
/// renders ONLY with 2+ profiles (above the repo row), the All-Hosts row
/// badges + stale/last-seen markers ride the composite renderer, Settings
/// exposes the per-host D7 surface (reorder/rename/retry/remove + F2 copy),
/// and the Add Host sheet prefills the name from the URL (B3).
final class MultiHostSurfaceWiringTests: XCTestCase {
    private func bundledSource() throws -> String {
        let bundle = Bundle(for: MultiHostSurfaceWiringTests.self)
        let url = try XCTUnwrap(bundle.url(forResource: "FleetViews",
                                           withExtension: "swift.txt"))
        return try String(contentsOf: url, encoding: .utf8)
    }

    func testHostChipRowSitsAboveRepoRowUnderTheMultiHostGuard() throws {
        let source = try bundledSource()
        // The host chips row + outage summary live INSIDE the 2+ profile
        // guard (probe (a): removing the guard or hoisting the row out of
        // it makes this RED — the single-host layout stays byte-comparable).
        let start = try XCTUnwrap(source.range(of: "// #401 D2: with 2+ profiles the HOST-chip row"))
        let end = try XCTUnwrap(source.range(of: "if let banner = model.banner"))
        let slice = String(source[start.lowerBound..<end.lowerBound])
        let guardRange = try XCTUnwrap(slice.range(of: "if model.multiHostConfigured {"),
                                       "the host chip row must sit inside the 2+ profile guard")
        let chipsRange = try XCTUnwrap(slice.range(of: "hostChipsRow(chips: hostChipRow,"),
                                       "the host chip row must be wired above the repo row")
        XCTAssertLessThan(guardRange.lowerBound, chipsRange.lowerBound)
        XCTAssertEqual(slice.components(separatedBy: "hostChipsRow(chips: hostChipRow,").count - 1, 1,
                       "exactly one host-chip row call site")
        XCTAssertTrue(slice.contains("hostOutageSummaryRow(hostOutageSummary)"),
                      "the compact D7 summary must ride under the host chips")
        XCTAssertTrue(slice.contains("} else {\n                            repoChipsRow(chips: chips,"),
                      "the single-host repo row must keep its own (unchanged) branch")
    }

    func testHostSelectionFlowsThroughTheModelAndProjections() throws {
        let source = try bundledSource()
        XCTAssertTrue(source.contains("model.selectHostFilter(chip.profileID)"),
                      "chip taps must route through the model filter")
        XCTAssertTrue(source.contains("BoardModel.rows(aggregateRows, forHost: hostFilter)"),
                      "the board rows must filter by the selected host")
        XCTAssertTrue(source.contains("BoardModel.repoFilters(hostRows)"),
                      "repo chips/counts must rescope to the selected host (D4)")
        XCTAssertTrue(source.contains("BoardModel.hostSections("),
                      "the multi-host board must bucket rows through the pure projection")
        XCTAssertTrue(source.contains("model.aggregateBoardRows"),
                      "the board consumes #400's aggregate rows")
    }

    func testRowBadgesAndStaleMarkersAreWiredInTheCompositeRenderer() throws {
        let source = try bundledSource()
        let start = try XCTUnwrap(source.range(of: "private func hostAgentRow("))
        let end = try XCTUnwrap(source.range(of: "/// The display name of a row's owning host"))
        let slice = String(source[start.lowerBound..<end.lowerBound])
        XCTAssertTrue(slice.contains("hostProfileID: row.identity.hostProfileID"),
                      "the row tap must carry the composite host identity (E1)")
        XCTAssertTrue(slice.contains("HostBadgeChip(name: hostName)"),
                      "D6: the compact textual host badge must be wired into composite rows")
        XCTAssertTrue(source.contains("showHostBadge: showHostBadges"),
                      "the badge flag must flow from the All-Hosts rule")
        // probe (b): dropping the stale branch removes this marker → RED.
        XCTAssertTrue(slice.contains("if row.isStale {"),
                      "C6: the stale branch must render on retained rows")
        XCTAssertTrue(slice.contains("StaleRowLabel(lastSeenMs: row.lastSeen)"),
                      "C6: the last-seen age label must consume the row's stamp")
        XCTAssertTrue(source.contains("stale · \\(RelativeTime.lastSeenLabel(lastSeenMs: lastSeenMs, nowMs: now))"),
                      "C6: the stale label must show the relative last-seen age")
        XCTAssertTrue(source.contains("hostRowSummary(row, hostName: hostName)"),
                      "row VoiceOver must carry the badge/staleness facts (D8)")
    }

    func testSettingsExposesReorderRenameRetryRemoveAndPerHostNotifyState() throws {
        let source = try bundledSource()
        let start = try XCTUnwrap(source.range(of: "private var hostsSection: some View {"))
        let end = try XCTUnwrap(source.range(of: "// MARK: - #399 Add Host"))
        let slice = String(source[start.lowerBound..<end.lowerBound])
        XCTAssertTrue(slice.contains(".onMove(perform: moveHosts)"),
                      "D2: Settings must support drag-to-reorder")
        XCTAssertTrue(source.contains("EditButton()"),
                      "D2: the reorder affordance must exist with 2+ hosts")
        XCTAssertTrue(slice.contains("model.retryHostConnection(profile)"),
                      "D7: per-host Retry must route through the model")
        XCTAssertTrue(slice.contains("model.renameHost(id: profile.id, to: name)"),
                      "D7/B5: Rename must route through the model (display name only)")
        XCTAssertTrue(slice.contains("Button(\"Remove host\", role: .destructive)"),
                      "D7: Remove Host stays reachable per row")
        XCTAssertTrue(slice.contains("model.removeHost(profileID: profile.id)"),
                      "D7: Remove Host routes through the model")
        XCTAssertTrue(slice.contains("LabeledContent(\"Grants expiry\","),
                      "D7: grants expiry must be readable per host")
        XCTAssertTrue(slice.contains("Last seen"),
                      "D7: last seen must be readable per host")
        // #397: the per-host notification state rides the EXISTING host
        // row (no redesigned chrome) and routes through the model.
        XCTAssertTrue(slice.contains("Toggle(\"Notify about this host\""),
                      "#397: each host row must carry its per-host notification toggle")
        XCTAssertTrue(slice.contains("model.setHostNotificationsEnabled(profileID: profile.id,"),
                      "#397: the per-host toggle must route through the model")
        XCTAssertTrue(source.contains("settings.hosts.notify"),
                      "#397: each per-host toggle carries a row anchor")
        XCTAssertTrue(source.contains("settings.notifications.pending-clear"),
                      "#397: pending per-host token cleanup must surface in Settings")
    }

    func testAddHostSheetPrefillsNameFromURL() throws {
        let source = try bundledSource()
        let start = try XCTUnwrap(source.range(of: "struct AddHostSheet: View {"))
        let end = try XCTUnwrap(source.range(of: "/// #399 B6: the launch-time fingerprint confirmation"))
        let slice = String(source[start.lowerBound..<end.lowerBound])
        // #399 rev B3: the sheet must prefill the NAME from the entered URL
        // through the existing candidate helper — never leave `name` dead.
        // #415: the prefill writes into the model-owned scene-scoped draft.
        XCTAssertTrue(slice.contains("model.addHostDraft.name = HostURLForm.displayNameCandidate(for: newValue)"),
                      "B3: the sheet must prefill the host name from the URL")
        XCTAssertTrue(slice.contains("guard model.addHostDraft.name.isEmpty else { return }"),
                      "B3: a hand-entered name must never be overwritten")
        XCTAssertTrue(slice.contains("onChange(of: model.addHostDraft.urlString)"),
                      "B3: the prefill must track URL entry")
    }

    func testHostChipsAre44PtAccessibleAndTokenThemed() throws {
        let source = try bundledSource()
        let start = try XCTUnwrap(source.range(of: "private func hostChipButton("))
        let end = try XCTUnwrap(source.range(of: "/// #401 D7: the ONE compact board-level outage summary"))
        let slice = String(source[start.lowerBound..<end.lowerBound])
        XCTAssertTrue(slice.contains(".frame(minHeight: 44)"),
                      "D8: interactive host chips keep the ≥44 pt target")
        XCTAssertTrue(slice.contains(".accessibilityAddTraits(isSelected ? [.isSelected] : [])"),
                      "D8: selected host chips carry the VoiceOver selected trait")
        XCTAssertTrue(source.contains(".id(\"board.host-chips\")"),
                      "the host chip row carries its scroll anchor")
        XCTAssertTrue(source.contains("BoardModel.hostHealthToken(health)"),
                      "D8: health colors resolve through the shared token mapping")
    }
}
