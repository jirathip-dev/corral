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
    func testSeedCoversRawBoardStatesWithoutDone() {
        let seed = DemoFleet.seed()
        let states = Set(seed.values.map(\.state))
        XCTAssertEqual(states, Set([.working, .idle, .blocked, .unknown]),
                       "the read-only fixture uses herdr's raw vocabulary; done is gone")
        XCTAssertTrue(seed.values.contains { $0.isBlocked })
        XCTAssertTrue(seed.values.contains { $0.state == .idle })
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
        // section with rows, projected through the real model — no repo
        // grouping anywhere.
        let sections = BoardModel.sections(Array(seed.values))
        XCTAssertEqual(sections.statuses.map(\.state),
                       [.blocked, .working, .idle, .unknown],
                       "the fixture must populate every locked status section")
        for status in sections.statuses {
            XCTAssertFalse(status.agents.isEmpty,
                           "section \(status.header) must be non-empty for evidence")
        }
        // The orphan (repo = nil) row stays in the fixture and lands in its
        // status bucket — repo is row metadata, not a grouping key.
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

    /// Recents v1 fixture: the featured agent's tail is a live-tail-only
    /// block stream (no partition scaffolding) that derives non-empty lines.
    func testFeaturedRecentsFixtureIsALiveTailStream() throws {
        let seed = DemoFleet.seed()
        guard let agent = seed[DemoFleet.featuredAgentID] else {
            return XCTFail("featured demo agent missing from the seed")
        }
        let blocks = DemoFleet.recentBlocks(for: agent)
        XCTAssertFalse(blocks.isEmpty)
        XCTAssertFalse(DemoFleet.recentLines(from: blocks).isEmpty)
        XCTAssertTrue(blocks.contains { $0.kind == .user && $0.text.contains("verify the diff") })
        // The fixture deliberately keeps a divider-only row so rail
        // evidence proves #361 divider rows are dropped, not hidden.
        XCTAssertTrue(blocks.contains { block in
            RecentOutputRender.isDividerBlock(
                TranscriptBlock(kind: block.kind, text: block.text))
        }, "the recents fixture must keep its divider-only row for rail-drop evidence")
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

final class BoardModelReadOnlyTests: XCTestCase {
    private func agent(_ id: String, state: AgentState, repo: String?, branch: String? = nil,
                       ts: UInt64 = 100, displayName: String? = nil) -> Agent {
        Agent(agentId: id, state: state, seq: 1, ts: ts,
              capabilities: ["read_tail"],
              workspace: Workspace(repo: repo, branch: branch),
              displayName: displayName ?? id)
    }

    func testSectionsBucketByRawStatusInLockedOrder() {
        let sections = BoardModel.sections([
            agent("b1", state: .blocked, repo: "corral"),
            agent("w1", state: .working, repo: "other"),
            agent("i1", state: .idle, repo: "corral"),
            agent("u1", state: .unknown, repo: nil),
        ])
        // Locked order: Blocked → Working → Idle → Unknown. Repo is never
        // a grouping key, so four agents across three repos still yield
        // exactly four one-row status buckets.
        XCTAssertEqual(sections.statuses.map(\.state),
                       [.blocked, .working, .idle, .unknown])
        XCTAssertEqual(sections.statuses.map(\.header),
                       ["blocked (1)", "working (1)", "idle (1)", "unknown (1)"])
        XCTAssertEqual(sections.statuses.map { $0.agents.map(\.agentId) },
                       [["b1"], ["w1"], ["i1"], ["u1"]])
    }

    func testRepoIsRowMetadataNeverTheGroupingKey() {
        // Same status across DIFFERENT repos — including the orphan
        // (repo = nil) — collapses into ONE working bucket; there is no
        // repo section, no "no repo" orphan bucket.
        let sections = BoardModel.sections([
            agent("in-corral", state: .working, repo: "corral", ts: 200),
            agent("in-other", state: .working, repo: "other", ts: 150),
            agent("orphan", state: .working, repo: nil, ts: 100),
        ])
        XCTAssertEqual(sections.statuses.count, 1)
        XCTAssertEqual(sections.statuses[0].state, .working)
        XCTAssertEqual(sections.statuses[0].agents.map(\.agentId),
                       ["in-corral", "in-other", "orphan"],
                       "repo must not partition a status bucket")
        XCTAssertEqual(sections.statuses[0].header, "working (3)")
    }

    func testBlockedAgentsLeadTheBoardExactlyOnce() {
        // Blocked is the FIRST section (attention-first); the old
        // cross-repo promotion is gone — no agent is duplicated into a
        // second bucket, blocked orphan or not.
        let sections = BoardModel.sections([
            agent("idle", state: .idle, repo: "corral", ts: 300),
            agent("blocked-with-repo", state: .blocked, repo: "corral", ts: 200),
            agent("blocked-orphan", state: .blocked, repo: nil, ts: 100),
        ])
        XCTAssertEqual(sections.statuses.map(\.state), [.blocked, .idle])
        XCTAssertEqual(sections.statuses[0].agents.map(\.agentId),
                       ["blocked-with-repo", "blocked-orphan"])
        let allRows = sections.statuses.flatMap { $0.agents.map(\.agentId) }
        XCTAssertEqual(allRows.count, Set(allRows).count,
                       "every agent must appear in exactly one section")
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
        XCTAssertEqual(withDone.statuses[2].agents.map(\.agentId), ["done"])
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
        XCTAssertEqual(sections.statuses[0].agents.map(\.agentId),
                       ["newer", "tie-a", "tie-b", "older"],
                       "ts desc, then agent id for determinism")
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
        XCTAssertEqual(forward.statuses, shuffled.statuses)
    }

    func testEmptyBoardHasNoSections() {
        let sections = BoardModel.sections([])
        XCTAssertTrue(sections.statuses.isEmpty)
    }
}

// MARK: - Recents v1 tail model (#354 L2)

final class RecentOutputTailModelTests: XCTestCase {
    private func block(_ kind: TranscriptBlockKind, _ text: String) -> TranscriptBlock {
        TranscriptBlock(kind: kind, text: text)
    }

    func testTailRowsUseCanonicalBlocksWhenPresent() {
        let pane = TailPane()
        var populated = pane
        populated.apply([block(.user, "hi"), block(.agent, "hello")],
                        lines: ["hi", "hello"])
        let rows = RecentOutputModel.tailRows(from: populated)
        XCTAssertEqual(rows.map(\.kind), [.user, .agent])
    }

    func testLegacyLinesFallBackToHonestUnknownBlocks() {
        let pane = TailPane()
        var legacy = pane
        legacy.lines = ["raw line one", "raw line two"]
        let rows = RecentOutputModel.tailRows(from: legacy)
        XCTAssertEqual(rows.map(\.kind), [.unknown, .unknown])
        XCTAssertEqual(rows.map(\.text), ["raw line one", "raw line two"])
    }

    func testAdjacentToolAndSystemBlocksMerge() {
        let pane = TailPane()
        var populated = pane
        populated.apply([block(.tool, "one"), block(.tool, "two"),
                         block(.agent, "three"), block(.system, "four"), block(.system, "five")],
                        lines: [])
        let rows = RecentOutputModel.tailRows(from: populated)
        XCTAssertEqual(rows.map(\.kind), [.tool, .agent, .system])
        XCTAssertEqual(rows[0].text, "one\ntwo")
        XCTAssertEqual(rows[2].text, "four\nfive")
    }

    func testDividerOnlyBlockIsClassifiedAsDivider() {
        XCTAssertTrue(RecentOutputRender.isDividerBlock(block(.system, "──────")))
        XCTAssertFalse(RecentOutputRender.isDividerBlock(block(.system, "let sep = \"────\";")))
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

    func testIdentifiedBlocksNeverCollide() {
        let rows = RecentOutputModel.identifiedBlocks([block(.agent, "same"), block(.agent, "same")])
        XCTAssertEqual(rows.count, 2)
        XCTAssertNotEqual(rows[0].id, rows[1].id)
    }

    func testTranscriptErrorTextNamesTheMissingGrant() {
        let text = TranscriptText.errorText(
            TranscriptFailure(kind: "not_granted", message: "capability not granted: read_tail", candidates: []))
        XCTAssertTrue(text.contains("read_tail grant"), text)
    }
}

// MARK: - #361 continuous rail model

/// Focused regressions for the #361 continuous rail. These bite: the row
/// model must render ZERO divider-only rows, mark role ONLY as a shape at
/// semantic transitions (never per row, never as role text), and keep the
/// full stream in daemon chronological order. A change that re-introduces
/// divider rows, per-row markers, or reordering fails here before any view
/// code is involved.
final class RecentRailModelTests: XCTestCase {
    private func block(_ kind: TranscriptBlockKind, _ text: String) -> TranscriptBlock {
        TranscriptBlock(kind: kind, text: text)
    }

    private func pane(_ blocks: [TranscriptBlock]) -> TailPane {
        var pane = TailPane()
        pane.apply(blocks, lines: [])
        return pane
    }

    func testRailRowsDropDividerOnlyRowsEntirely() {
        let rows = RecentOutputModel.railRows(from: pane([
            block(.user, "hi"),
            block(.system, "────────────────────────────────"),
            block(.agent, "hello"),
            block(.tool, "──"),
            block(.agent, "again")
        ]))
        XCTAssertEqual(rows.map(\.block.kind), [.user, .agent, .agent])
        XCTAssertEqual(rows.map(\.block.text), ["hi", "hello", "again"],
                       "dropping a divider must never drop or reorder content")
        XCTAssertTrue(rows.allSatisfy { !RecentOutputRender.isDividerBlock($0.block) },
                      "the rail must render ZERO divider-only rows")
        let plain = RecentOutputModel.tailRows(from: pane([
            block(.system, "────────────────────────────────"),
            block(.agent, "hello")
        ]))
        XCTAssertTrue(plain.allSatisfy { !RecentOutputRender.isDividerBlock($0) },
                      "the tail row model itself must contain ZERO divider-only rows")
    }

    func testRailRowsDropLegacyDividerLines() {
        var legacy = TailPane()
        legacy.lines = ["raw one", "────────────────────────────────"]
        let rows = RecentOutputModel.railRows(from: legacy)
        XCTAssertEqual(rows.map(\.block.kind), [.unknown])
        XCTAssertEqual(rows.map(\.block.text), ["raw one"],
                       "a legacy divider line is raw furniture and must not render")
    }

    func testRailPreservesFullChronologicalOrder() {
        let fixture = [
            block(.agent, "first agent"),
            block(.system, "────────────────────────────────"),
            block(.user, "user input"),
            block(.tool, "tool output"),
            block(.agent, "second agent"),
            block(.system, "diagnostic"),
            block(.unknown, "raw pane line")
        ]
        let rows = RecentOutputModel.railRows(from: pane(fixture))
        XCTAssertEqual(rows.map(\.block.text),
                       ["first agent", "user input", "tool output",
                        "second agent", "diagnostic", "raw pane line"],
                       "the rail is ONE continuous stream in full chronological order")
    }

    func testTransitionMarkerOnlyAtRoleChanges() {
        let rows = RecentOutputModel.railRows(from: pane([
            block(.agent, "a1"),
            block(.agent, "a2"),
            block(.user, "u1"),
            block(.user, "u2"),
            block(.tool, "t1"),
            block(.agent, "a3")
        ]))
        XCTAssertEqual(rows.map(\.showsTransitionMarker),
                       [true, false, true, false, true, true],
                       "markers appear ONLY at semantic role transitions, never per row")
    }

    func testContinuationAfterDroppedDividerCarriesNoMarker() {
        let rows = RecentOutputModel.railRows(from: pane([
            block(.agent, "a"),
            block(.system, "──────"),
            block(.agent, "b")
        ]))
        XCTAssertEqual(rows.map(\.block.kind), [.agent, .agent])
        XCTAssertEqual(rows.map(\.showsTransitionMarker), [true, false],
                       "the divider drops out of the sequence, so b continues the same role run")
    }

    func testDividerNeverRidesInsideMergedContentRow() {
        let rows = RecentOutputModel.railRows(from: pane([
            block(.system, "read_tail page truncated to the newest 200 lines."),
            block(.system, "────────────────────────────────"),
            block(.agent, "hello")
        ]))
        XCTAssertEqual(rows.map(\.block.kind), [.system, .agent])
        XCTAssertEqual(rows[0].block.text,
                       "read_tail page truncated to the newest 200 lines.",
                       "a divider-only row must drop BEFORE merging so it never rides inside a content row")
        XCTAssertFalse(rows[0].block.text.contains("─"),
                       "no divider furniture may survive inside a merged content row")
    }

    func testSystemAndUnknownRowsNeverCarryMarkers() {
        let rows = RecentOutputModel.railRows(from: pane([
            block(.agent, "a"),
            block(.system, "diagnostic"),
            block(.agent, "b"),
            block(.unknown, "raw pane line"),
            block(.agent, "c")
        ]))
        XCTAssertEqual(rows.map(\.showsTransitionMarker), [true, false, true, false, true],
                       "system/unknown rows are raw output, never role markers")
    }

    func testRoleMarkersAreLockedPerRole() {
        XCTAssertEqual(RecentOutputModel.marker(for: .user), .diamond)
        XCTAssertEqual(RecentOutputModel.marker(for: .agent), .circle)
        XCTAssertEqual(RecentOutputModel.marker(for: .tool), .square)
        XCTAssertNil(RecentOutputModel.marker(for: .system))
        XCTAssertNil(RecentOutputModel.marker(for: .unknown))
    }

    func testRailRowIdentitiesNeverCollide() {
        let rows = RecentOutputModel.railRows(from: pane([
            block(.agent, "same"),
            block(.agent, "same")
        ]))
        XCTAssertEqual(rows.count, 2)
        XCTAssertNotEqual(rows[0].id, rows[1].id)
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
        let nextDecl = source.range(of: "\n// MARK: - Rail row renderer")
            ?? source.range(of: "\nprivate struct RecentRailRowView")
        let sliceEnd = try XCTUnwrap(nextDecl?.lowerBound,
                                     "the recents rail renderer declaration must exist in the sheet source")
        let slice = String(source[sheetStart.lowerBound..<sliceEnd])
        XCTAssertTrue(slice.contains("model.driveReadTail(agent: agent, driveClient: driveClient, silent: true)"),
                      "the recents sheet must auto-refresh through the live read_tail drive")
        XCTAssertTrue(slice.contains("RecentOutputModel.phase(for: tail)"),
                      "the recents sheet must render the tail pane's four-state machine")
        XCTAssertTrue(slice.contains("RecentOutputModel.railRows(from: tail)"),
                      "the recents sheet must render the continuous rail row model")
        XCTAssertTrue(slice.contains("RecentRailSpine()"),
                      "the recents sheet must ride ONE continuous spine behind the rail rows (#361 R1)")
        // The rail renders ZERO divider rules, cards, and role labels: the
        // V3-era chrome vocabulary must not exist inside the sheet (a decoy
        // elsewhere in FleetViews cannot satisfy a slice-scoped assertion).
        for chrome in ["speakerRail", "roleLabel", "showSpeaker",
                       "DisclosureGroup", "RecentBlockRow", "userTint"] {
            XCTAssertFalse(slice.contains(chrome),
                           "role/card chrome \(chrome) must not be wired in the recents sheet")
        }
    }

    func testRailSpineIsOneContinuousSpan() throws {
        let source = try bundledSource()
        let start = source.range(of: "\nprivate struct RecentRailSpine")
        let end = source.range(of: "\nprivate struct RecentCodeLineView")
        let startIndex = try XCTUnwrap(start?.lowerBound,
                                       "the continuous spine primitive must exist")
        let endIndex = try XCTUnwrap(end?.lowerBound,
                                     "the code line view declaration must exist")
        let slice = String(source[startIndex..<endIndex])
        XCTAssertTrue(slice.contains("Rectangle()"),
                      "the spine must be a single drawn line, not per-row segments")
        XCTAssertTrue(slice.contains(".frame(width: 1.5)"),
                      "the spine must be one thin vertical line")
        XCTAssertTrue(slice.contains("maxHeight: .infinity"),
                      "the spine must span the whole rail stack continuously")
        XCTAssertTrue(slice.contains("RecentOutputPalette.railLine"),
                      "the spine must use the locked rail-line token")
    }

    func testRecentsRailRendererUsesTransitionMarkersOnly() throws {
        let source = try bundledSource()
        let start = source.range(of: "\nprivate struct RecentRailRowView: View {")
        let end = source.range(of: "\nprivate struct RecentCodeLineView")
        let startIndex = try XCTUnwrap(start?.lowerBound,
                                       "the rail renderer declaration must exist")
        let endIndex = try XCTUnwrap(end?.lowerBound,
                                     "the code line view declaration must exist")
        let slice = String(source[startIndex..<endIndex])
        XCTAssertTrue(slice.contains("row.showsTransitionMarker"),
                      "the rail renderer must gate its gutter marker on the model transition flag")
        XCTAssertTrue(slice.contains("RecentOutputModel.marker(for: block.kind)"),
                      "the rail renderer must draw the locked role marker")
        // Zero role text / cards / per-row chrome inside the renderer.
        for chrome in ["roleLabel", "speakerRail", "DisclosureGroup",
                       "userTint", "toolSummary", "role text"] {
            XCTAssertFalse(slice.contains(chrome),
                           "chrome \(chrome) must not be in the rail renderer")
        }
    }

    func testBoardProjectsThroughStatusSections() throws {
        let source = try bundledSource()
        // The FleetView board must be the status-section projection
        // (locked order, repo never a grouping key), not repo groups.
        XCTAssertTrue(source.contains("BoardModel.sections(agents)"), "board must project through BoardModel.sections")
        // Scope to the board renderer so an unrelated helper cannot satisfy
        // the search (decoy-resistant: unique declaration marker).
        let boardMarker = "private func boardSections(sections: BoardModel.Sections)"
        XCTAssertEqual(source.components(separatedBy: boardMarker).count - 1, 1,
                       "exactly one boardSections renderer must exist")
        guard let boardStart = source.range(of: boardMarker) else {
            return XCTFail("boardSections declaration not found")
        }
        let sliceEnd = try XCTUnwrap(source.range(of: "\n    @ViewBuilder\n    private func agentRow")?.lowerBound,
                                     "agentRow declaration must follow boardSections")
        let slice = String(source[boardStart.lowerBound..<sliceEnd])
        // Renders one section per status bucket, header = status + count.
        XCTAssertTrue(slice.contains("ForEach(sections.statuses)"),
                      "the board must render the status-section projection")
        XCTAssertTrue(slice.contains("ForEach(status.agents)"),
                      "every agent of a status bucket must render")
        XCTAssertTrue(slice.contains("status.header"),
                      "section headers must show the raw status name + count")
        // No repo grouping, no blocked promotion in the renderer.
        for repoChrome in ["sections.repos", "sections.blocked", "repo.header", "ForEach(repo.agents)"] {
            XCTAssertFalse(slice.contains(repoChrome),
                           "repo chrome \(repoChrome) must not be wired into the status board")
        }
        // Row taps open recents through the model-owned deep-link target.
        XCTAssertTrue(source.contains("model.recentsAgentId = agent.agentId"))
        XCTAssertTrue(source.contains("RecentOutputSheet(agentId: target.agentId, model: model)"))
    }

    func testRemovedSurfacesAreAbsentFromTheBoardSource() throws {
        let source = try bundledSource()
        for removed in ["AgentDiffSheet", "IssuesBrowserView", "DevicesGrantsView",
                        "AnswerPromptSheet", "TerminalAttachView", "PromptDrafts",
                        "RecentOutputSections", "filterChipRow", "FleetSearchable",
                        "swipeActions", "CannedButtons", "ClaimCard",
                        "RecentBlockRow", "speakerRail", "roleLabel"] {
            XCTAssertFalse(source.contains(removed),
                           "removed surface \(removed) must not be wired in FleetViews")
        }
    }
}
