import Foundation
import os

/// DEV-ONLY live verification harness (launch argument `-liveVerify`).
///
/// Runs the conformance core against a REAL corrald from inside the app, on
/// the simulator's network stack (the simulator shares the Mac's loopback,
/// so 127.0.0.1 reaches corrald directly):
///
///   register (read-only default) → snapshot render → host /grants
///   promotion → signed read_tail drive on a real agent → tampered-envelope
///   refusal. Every step is logged; the /audit entry is the evidence.
///
/// Tokens come from launch arguments (`-registrationToken`, `-adminToken`),
/// never from the codebase. Not reachable from the UI.
@MainActor
final class LiveVerifyRunner {
    private let model: AppModel
    private let log = Logger(subsystem: "com.corral.fleetnotifier", category: "liveverify")

    init(model: AppModel) {
        self.model = model
    }

    private func arg(_ name: String) -> String? {
        let args = CommandLine.arguments
        guard let index = args.firstIndex(of: name), args.indices.contains(index + 1) else { return nil }
        return args[index + 1]
    }

    func run() {
        Task {
            await execute()
        }
    }

    private func execute() async {
        log.info("⚙️ liveVerify start")
        let hostRaw = arg("-host") ?? "127.0.0.1:8474"
        guard let registrationToken = arg("-registrationToken"),
              let adminToken = arg("-adminToken") else {
            log.error("liveVerify: missing -registrationToken/-adminToken/-host")
            return
        }
        let host = URL(string: hostRaw.hasPrefix("http") ? hostRaw : "http://\(hostRaw)")!
        let driveClient = DriveClient(host: host)
        let readClient = CorraldClient(host: host)

        do {
            // 1. Device identity + registration (R1): read-only default.
            let (signer, storage) = try DeviceKeyStore.loadOrCreate()
            log.info("⚙️ key storage: \(String(describing: storage)) public key \(signer.publicKeyB64)")
            let registered = try await driveClient.register(token: registrationToken, signer: signer)
            log.info("⚙️ registered key_id=\(registered.keyId) grants=\(registered.grants) expiry_ts=\(registered.expiryTs)")
            precondition(registered.grants.isEmpty, "registration MUST return empty grants (read-only default)")

            // 2. Read path (R2): snapshot render.
            let snapshot = try await readClient.fetchSnapshot()
            log.info("⚙️ snapshot schema_version=\(snapshot.schemaVersion) rev=\(snapshot.rev) agents=\(snapshot.agents.count)")
            precondition(snapshot.schemaVersion >= 3,
                         "schema must be v3+ (v4 added workspace.issues — G23)")
            let realAgent = snapshot.agents.values.first { $0.agentId.hasPrefix("herdr:ses_") || $0.agentId.hasPrefix("herdr:") }
            guard let targetAgent = realAgent else {
                log.error("⚙️ no herdr agents in snapshot")
                return
            }
            log.info("⚙️ target agent: \(targetAgent.agentId) state=\(targetAgent.state.rawValue) tool=\(targetAgent.tool)")

            // 3. Host promotion (admin /grants) so read_tail is granted.
            let grantsBody = try JSONEncoder().encode(AdminGrants(action: "set_grants", keyId: registered.keyId, grants: ["read_tail"]))
            let (grantStatus, grantData) = try await post(host: host, adminToken: adminToken, body: grantsBody)
            log.info("⚙️ /grants set_grants → HTTP \(grantStatus) \(String(data: grantData, encoding: .utf8) ?? "")")

            // 4. Signed drive: read_tail on a real agent (R3).
            let requestId = DriveClient.newRequestId()
            log.info("⚙️ drive read_tail request_id=\(requestId) target=\(targetAgent.agentId)")
            let payload = CanonicalJSON.readTailPayload(lines: 200)
            let result = await driveClient.drive(capability: .readTail, target: targetAgent.agentId,
                                                 payload: payload, rev: snapshot.rev, requestId: requestId,
                                                 keyId: registered.keyId, signer: signer,
                                                 biometrics: Biometrics(evaluate: { true }),
                                                 stepUp: false)
            switch result {
            case .dispatched(let response):
                log.info("⚙️ DRIVE OK request_id=\(response.requestId) ok=\(response.ok) rev=\(response.rev) error=\(response.error ?? "nil")")
            case .refused(let error):
                log.error("⚙️ DRIVE REFUSED \(String(describing: error))")
            }

            // 5. Tampered envelope must be refused with 401 bad_signature (R4).
            let tamperedRid = DriveClient.newRequestId()
            let honestBytes = CanonicalJSON.envelopeBytes(requestId: tamperedRid, capability: "read_tail",
                                                          target: targetAgent.agentId,
                                                          payload: CanonicalJSON.readTailPayload(lines: 200),
                                                          rev: snapshot.rev)
            let signature = try signer.sign(honestBytes).base64EncodedString()
            let mutated = CanonicalJSON.readTailPayload(lines: 1)
            let tamperedEnvelope = CanonicalJSON.envelopeBytes(requestId: tamperedRid, capability: "read_tail",
                                                               target: targetAgent.agentId, payload: mutated,
                                                               rev: snapshot.rev)
            let tamperedBody = CanonicalJSON.signedDriveBody(keyId: registered.keyId, signatureB64: signature, envelopeBytes: tamperedEnvelope)
            let (tamperedStatus, tamperedData) = try await post(host: host, body: tamperedBody)
            log.info("⚙️ tampered envelope → HTTP \(tamperedStatus) \(String(data: tamperedData, encoding: .utf8) ?? "")")

            // 6. Step-up mint (R9 mint half; no dispatch).
            do {
                let minted = try await driveClient.mintStepUpToken(keyId: registered.keyId, signer: signer)
                log.info("⚙️ step-up mint ok token_prefix=\(minted.token.prefix(8)) ttl_secs=\(minted.ttlSecs) expires_ts=\(minted.expiresTs)")
            } catch {
                log.error("⚙️ step-up mint failed \(String(describing: error))")
            }

            log.info("⚙️ liveVerify DONE — check the daemon /audit log for request_id \(requestId)")
        } catch {
            log.error("⚙️ liveVerify failed: \(String(describing: error))")
        }
    }

    private func post(host: URL, adminToken: String? = nil, body: Data) async throws -> (Int, Data) {
        var request = URLRequest(url: host.appendingPathComponent("/drive"))
        request.httpMethod = "POST"
        request.setValue("application/json", forHTTPHeaderField: "Content-Type")
        if let adminToken {
            request = URLRequest(url: host.appendingPathComponent("/grants"))
            request.httpMethod = "POST"
            request.setValue("application/json", forHTTPHeaderField: "Content-Type")
            request.setValue("Bearer \(adminToken)", forHTTPHeaderField: "Authorization")
        }
        request.httpBody = body
        let (data, response) = try await URLSession.shared.data(for: request)
        let status = (response as? HTTPURLResponse)?.statusCode ?? -1
        return (status, data)
    }
}

private struct AdminGrants: Encodable {
    let action: String
    let keyId: String
    let grants: [String]

    enum CodingKeys: String, CodingKey {
        case action, grants
        case keyId = "key_id"
    }
}
