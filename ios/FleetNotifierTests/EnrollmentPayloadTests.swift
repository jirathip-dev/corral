import XCTest
@testable import FleetNotifier

/// #486 payload-parser conformance (#485 frozen §5.1). All fixture material
/// is deliberately synthetic (32-byte base64 fills); nothing here is a real
/// host key, code, endpoint, or credential.
final class EnrollmentPayloadTests: XCTestCase {
    // SAFETY: synthetic fixture material only — fixed 32-byte fills encoded
    // to base64, never a real key or redemption code.
    static let syntheticHostKey = Data(repeating: 21, count: 32).base64EncodedString()
    // SAFETY: synthetic fixture material only (fixed 32-byte fill).
    static let syntheticCode = Data(repeating: 11, count: 32).base64EncodedString()
    static let syntheticEndpoint = "https://synthetic-host.tail0000.ts.net"
    static let fixtureExpiresTs: UInt64 = 1_900_000_000
    static let fixtureNow: UInt64 = 1_800_000_000

    /// The exact canonical producer text for this fixture.
    private func canonicalText(
        v: String = "1",
        hostKey: String? = nil,
        endpoint: String? = nil,
        code: String? = nil,
        expiresTs: String? = nil,
        scope: String = EnrollmentQRPayload.readTailScope
    ) -> String {
        let key = hostKey ?? Self.syntheticHostKey
        let url = endpoint ?? Self.syntheticEndpoint
        let redemptionCode = code ?? Self.syntheticCode
        let deadline = expiresTs ?? String(Self.fixtureExpiresTs)
        return #"{"v":\#(v),"host_key":"\#(key)","endpoint":"\#(url)","code":"\#(redemptionCode)","expires_ts":\#(deadline),"scope":"\#(scope)"}"#
    }

    private func assertParseFails(_ text: String,
                                  _ expected: EnrollmentPayloadError,
                                  now: UInt64 = EnrollmentPayloadTests.fixtureNow,
                                  line: UInt = #line) {
        XCTAssertThrowsError(try EnrollmentQRPayload.parse(text, now: now), line: line) { error in
            XCTAssertEqual(error as? EnrollmentPayloadError, expected, line: line)
        }
    }

    func testParsesExactCanonicalProducerPayload() throws {
        let payload = try EnrollmentQRPayload.parse(canonicalText(), now: Self.fixtureNow)

        XCTAssertEqual(payload.v, 1)
        XCTAssertEqual(payload.hostKeyB64, Self.syntheticHostKey)
        XCTAssertEqual(payload.endpoint, Self.syntheticEndpoint)
        XCTAssertEqual(payload.code, Self.syntheticCode)
        XCTAssertEqual(payload.expiresTs, Self.fixtureExpiresTs)
        XCTAssertEqual(payload.scope, EnrollmentQRPayload.readTailScope)
    }

    func testRejectsUnsupportedVersion() {
        assertParseFails(canonicalText(v: "2"), .unsupportedVersion(2))
        assertParseFails(canonicalText(v: "0"), .unsupportedVersion(0))
        // A quoted version is not the canonical integer form.
        assertParseFails(canonicalText(v: "\"1\""), .notCanonical)
    }

    func testRejectsMissingFieldsInCanonicalOrder() {
        let withoutScope = #"{"v":1,"host_key":"\#(Self.syntheticHostKey)","endpoint":"\#(Self.syntheticEndpoint)","code":"\#(Self.syntheticCode)","expires_ts":\#(Self.fixtureExpiresTs)}"#
        assertParseFails(withoutScope, .missingField("scope"))

        let withoutHostKey = #"{"v":1,"endpoint":"\#(Self.syntheticEndpoint)","code":"\#(Self.syntheticCode)","expires_ts":\#(Self.fixtureExpiresTs),"scope":"read_tail"}"#
        assertParseFails(withoutHostKey, .missingField("host_key"))
    }

    func testRejectsExtraFields() {
        // The owner channel refuses a `grants` parameter; so does the device
        // parser — extra fields are never tolerated.
        let text = canonicalText()
            .replacingOccurrences(of: "\"scope\":\"read_tail\"}",
                                  with: "\"scope\":\"read_tail\",\"grants\":[\"read_diff\"]}")
        assertParseFails(text, .extraField("grants"))
    }

    func testRejectsReorderedFields() {
        let text = #"{"v":1,"endpoint":"\#(Self.syntheticEndpoint)","host_key":"\#(Self.syntheticHostKey)","code":"\#(Self.syntheticCode)","expires_ts":\#(Self.fixtureExpiresTs),"scope":"read_tail"}"#
        assertParseFails(text, .notCanonical)
    }

    func testRejectsInsignificantWhitespaceAndTrailingNewline() {
        let spaced = canonicalText().replacingOccurrences(of: "\"endpoint\"", with: " \"endpoint\"")
        assertParseFails(spaced, .notCanonical)
        assertParseFails(canonicalText() + "\n", .notCanonical)
        assertParseFails(" " + canonicalText(), .notCanonical)
    }

    /// Operational correction pin: whitespace INSIDE a JSON string value is
    /// string content. It must be decided by the field's own rule (here: the
    /// endpoint may not contain whitespace), never mislabelled as
    /// malformed/non-canonical text.
    func testStringContentWhitespaceIsRejectedPerFieldNotAsMalformedText() {
        let text = canonicalText(endpoint: "https://synthetic host.tail0000.ts.net")
        assertParseFails(text, .invalidEndpoint)
    }

    func testRejectsNonHTTPSendpoint() {
        assertParseFails(canonicalText(endpoint: "http://synthetic-host.tail0000.ts.net"),
                         .invalidEndpoint)
        // Loopback http is acceptable for `HostURLForm`, but the frozen QR
        // contract requires https — the payload gate is stricter.
        assertParseFails(canonicalText(endpoint: "http://127.0.0.1:8474"), .invalidEndpoint)
    }

    func testRejectsScopeOtherThanReadTail() {
        assertParseFails(canonicalText(scope: "read_diff"), .unsupportedScope("read_diff"))
        assertParseFails(canonicalText(scope: ""), .unsupportedScope(""))
    }

    func testRejectsPastDeadlineAndTheExactExpiryEdge() throws {
        let expired = canonicalText(expiresTs: String(Self.fixtureNow - 1))
        assertParseFails(expired, .expired, now: Self.fixtureNow)
        // Frozen edge: now >= expires_ts is already expired.
        assertParseFails(canonicalText(expiresTs: String(Self.fixtureNow)),
                         .expired, now: Self.fixtureNow)
        // One second of headroom parses.
        let live = try EnrollmentQRPayload.parse(canonicalText(expiresTs: String(Self.fixtureNow + 1)),
                                                 now: Self.fixtureNow)
        XCTAssertEqual(live.expiresTs, Self.fixtureNow + 1)
    }

    func testRejectsMalformedHostKeyMaterial() {
        // SAFETY: synthetic fixtures; wrong-length and non-base64 strings.
        let shortKey = Data(repeating: 1, count: 16).base64EncodedString()
        assertParseFails(canonicalText(hostKey: shortKey), .invalidHostKey)
        assertParseFails(canonicalText(hostKey: "!!!!not-base64!!!!"), .invalidHostKey)
        assertParseFails(canonicalText(hostKey: Self.syntheticHostKey + "AAAA"), .invalidHostKey)
    }

    func testRejectsMalformedCodeMaterial() {
        let shortCode = Data(repeating: 7, count: 16).base64EncodedString()
        assertParseFails(canonicalText(code: shortCode), .invalidCode)
        assertParseFails(canonicalText(code: "not base64"), .invalidCode)
    }

    func testRejectsNonJSONText() {
        assertParseFails("not json at all", .malformedText)
        // A JSON array is not the payload object.
        assertParseFails(#"["v",1]"#, .malformedText)
        assertParseFails("", .malformedText)
    }

    func testRejectsDuplicateFields() {
        let text = #"{"v":1,"v":1,"host_key":"\#(Self.syntheticHostKey)","endpoint":"\#(Self.syntheticEndpoint)","code":"\#(Self.syntheticCode)","expires_ts":\#(Self.fixtureExpiresTs),"scope":"read_tail"}"#
        assertParseFails(text, .notCanonical)
    }

    func testExpiryHelperMirrorsTheFrozenEdge() throws {
        let payload = try EnrollmentQRPayload.parse(canonicalText(), now: Self.fixtureNow)
        XCTAssertFalse(payload.isExpired(now: payload.expiresTs - 1))
        XCTAssertTrue(payload.isExpired(now: payload.expiresTs))
        XCTAssertTrue(payload.isExpired(now: payload.expiresTs + 1))
    }

    func testParseFailuresNeverEmbedTheRedemptionCode() {
        let cases: [String] = [
            canonicalText(v: "2"),
            canonicalText(scope: "read_diff"),
            canonicalText(expiresTs: String(Self.fixtureNow - 1)),
            canonicalText(code: "not base64"),
        ]
        for text in cases {
            do {
                _ = try EnrollmentQRPayload.parse(text, now: Self.fixtureNow)
                XCTFail("expected a parse failure")
            } catch {
                let description = String(describing: error) + " " + ((error as? EnrollmentPayloadError)?.errorDescription ?? "")
                XCTAssertFalse(description.contains(Self.syntheticCode),
                               "parse failures must never echo the code: \(description)")
            }
        }
    }
}
