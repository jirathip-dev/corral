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
        XCTAssertTrue(BoardModel.repoFilters([]).isEmpty)
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
        // openRecents (notification tap) keeps its live-mode + agent-exists
        // guards and never plays a haptic (it is not a row tap).
        let (model, ticks) = makeHarness()
        defer { model.exitDemo() }
        model.openRecents(for: DemoFleet.featuredAgentID)
        XCTAssertNil(model.recentsRequest, "demo mode must ignore deep links")
        XCTAssertEqual(ticks.count, 0)

        seedLive(model, ["a1", "a2"])
        model.openRecents(for: "a2")
        XCTAssertEqual(model.recentsRequest?.agentId, "a2")
        XCTAssertEqual(ticks.count, 0, "deep links never tick")

        model.openRecents(for: "ghost")
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
        XCTAssertTrue(slice.contains("model.driveReadTail(agent: agent, driveClient: driveClient, silent: true)"),
                      "the recents sheet must auto-refresh through the live read_tail drive")
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
        XCTAssertTrue(source.contains("RecentOutputSheet(agentId: request.agentId, model: model)"))
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
/// chips resolving hues from the shared RepoHue function, and the section
/// headers carrying status + TOTAL. A compile-capable bypass of any of
/// those call sites goes RED here.
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
        // Subgroup band: hue resolved from the SAME fleet repo set the
        // chips row uses, rendered with the band/ink/rail tokens.
        let renderer = try slice(from: "private func statusSectionHeader(",
                                 to: "\n    @ViewBuilder\n    private func agentRow",
                                 in: source)
        XCTAssertEqual(renderer.components(separatedBy: "private func repoSubgroupHeader(").count - 1, 1,
                       "exactly one subgroup band builder must exist")
        XCTAssertTrue(renderer.contains("theme.repoHue(for: subgroup.repo"),
                      "subgroup bands must consume the shared RepoHue function")
        XCTAssertTrue(renderer.contains("theme.repoBand(for: hue)"),
                      "subgroup bands must use the hue-over-mantle band token")
        XCTAssertTrue(renderer.contains("theme.repoInk(for: hue)"),
                      "subgroup names must use the locked label ink")
        XCTAssertTrue(renderer.contains("subgroup.agents.count"),
                      "subgroup headers must carry their count")
        XCTAssertTrue(renderer.contains("subgroup.displayName"),
                      "repo identity is never color-only — the name always renders")
        XCTAssertFalse(renderer.contains("DisclosureGroup"),
                       "subgroups are always open — never collapsible")
        XCTAssertFalse(renderer.contains("isExpanded"),
                       "no expand/collapse state may exist on the board")
        XCTAssertFalse(renderer.contains("chevron"),
                       "no chevron affordance may hint at collapsibility")

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
        // DEBUG-only recorded-evidence drivers (#365 settings, #372 theme
        // and #379 connect sequences all open the same sheet); all
        // required, none release-gated.
        XCTAssertEqual(releaseActionLines.count, 1,
                       "the gear must be the ONLY release-active settings opener")
        XCTAssertEqual(allActionLines.count - releaseActionLines.count, 3,
                       "the #365, #372 and #379 DEBUG evidence drivers are the only debug-gated openers")
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
        let boardMarker = "\nstruct FleetView: View {"
        let boardStart = try XCTUnwrap(source.range(of: boardMarker),
                                       "FleetView declaration must exist")
        let boardEnd = try XCTUnwrap(source.range(of: "\n// MARK: - Banner"),
                                     "the banner section must follow FleetView")
        let slice = String(source[boardStart.lowerBound..<boardEnd.lowerBound])
        XCTAssertEqual(slice.components(separatedBy: "NavigationStack {").count - 1, 1,
                       "FleetView must wrap its board in exactly one NavigationStack")

        let stackLine = try XCTUnwrap(lineNumbers(of: "NavigationStack {", in: slice).first)
        let titleLine = try XCTUnwrap(lineNumbers(of: ".navigationTitle(\"Fleet\")", in: slice).first)
        let toolbarLine = try XCTUnwrap(lineNumbers(of: ".toolbar {", in: slice).first)
        let sheetLine = try XCTUnwrap(lineNumbers(of: ".sheet(isPresented: $showSettings)", in: slice).first)
        XCTAssertLessThan(stackLine, titleLine,
                          "the stack must open before the navigation chrome")
        XCTAssertLessThan(titleLine, toolbarLine,
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
        // register action with the same enable rules as the connect section.
        XCTAssertTrue(slice.contains("TextField(\"Host (Tailscale host or loopback)\""),
                      "the Settings sheet must expose the connection host field (#365)")
        XCTAssertTrue(slice.contains("SecureField(\"Registration token\""),
                      "the Settings sheet must expose device pairing")
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

    /// #379 A.1/A.2: the Device section shows Key id, the Keychain storage
    /// note, the read-only signed device label, the paired/registration
    /// state, and the Remove/revoke action — and NO grants list, NO stale
    /// capability language, and no standalone Reset section.
    func testSettingsDeviceSectionShowsIdentityWithoutGrantsList() throws {
        let slice = try settingsSlice(try bundledSource())
        // The Device section uses the custom header form (same convention
        // as the #372 Appearance section), so its header text marks it.
        XCTAssertTrue(slice.contains("Text(\"Device\")"),
                      "the Device section must exist")
        XCTAssertTrue(slice.contains("LabeledContent(\"Key id\""),
                      "the Device section must show the Key id")
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
        let keyIdRow = try XCTUnwrap(lineNumbers(of: "LabeledContent(\"Key id\"",
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
        Self.requestsStorage.append(request)
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
