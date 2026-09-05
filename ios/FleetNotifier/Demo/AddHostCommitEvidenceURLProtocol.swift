import Foundation

#if DEBUG
/// #415 evidence transport (DEBUG only, launch-arg gated): answers the
/// Add Host flow's endpoints deterministically on the evidence simulator
/// where no corrald daemon exists. The app is launched with
/// `-corralDemoAddHostCommitEvidence`; FleetNotifierApp then wires a
/// session whose protocolClasses include this class, so the REAL
/// prepare/register/commit model flow runs end to end while only the
/// transport is synthetic. Responses are keyed by the request's host so
/// the pre-existing "Mac" profile's pinned key matches what its
/// `/host-key` re-checks return (key continuity stays coherent) while the
/// NEW host presents its own identity.
///
/// Never logs or persists request bodies — the fixture registration
/// token travels only inside the URLProtocol's in-memory request, exactly
/// like a real transport.
final class AddHostCommitEvidenceURLProtocol: URLProtocol {
    private static let lock = NSLock()
    /// Hostname suffix → X25519 key served for /host-key on that host.
    private static let macHostMarker = "mac-evidence"
    /// SAFETY: fixed synthetic fixture keys (32-byte fills; never real).
    private static let macKey = Data(repeating: 21, count: 32).base64EncodedString()
    private static let newHostKey = Data(repeating: 22, count: 32).base64EncodedString()

    override class func canInit(with request: URLRequest) -> Bool { true }
    override class func canonicalRequest(for request: URLRequest) -> URLRequest { request }

    override func startLoading() {
        guard let requestURL = request.url,
              let url = request.url else {
            client?.urlProtocol(self, didFailWithError: URLError(.badURL))
            return
        }
        let host = url.host?.lowercased() ?? ""
        let path = url.path
        Self.lock.lock()
        let status: Int
        let body: Data
        let headers: [String: String]
        if path.hasSuffix("/host-key") {
            // SAFETY: fixture JSON built from the fixed fixture key strings.
            let key = host.contains(Self.macHostMarker) ? Self.macKey : Self.newHostKey
            status = 200
            body = Data(#"{"algorithm":"X25519","public_key":"\#(key)"}"#.utf8)
            headers = ["Content-Type": "application/json"]
        } else if path.hasSuffix("/register") {
            // The deterministic register response the real daemon shape
            // uses: key_id/grants/expiry/algorithm.
            status = 200
            body = Data(#"{"key_id":"dev_evidence_add","grants":["read_tail"],"expiry_ts":1800000000,"revoked":false,"algorithm":"Ed25519"}"#.utf8)
            headers = ["Content-Type": "application/json"]
        } else if path.hasSuffix("/events") {
            // SSE hold-open: an empty, never-finished event stream so the
            // success-path startLive connects quietly.
            status = 200
            body = Data()
            headers = ["Content-Type": "text/event-stream"]
        } else {
            status = 404
            body = Data()
            headers = ["Content-Type": "application/json"]
        }
        Self.lock.unlock()
        // SAFETY: fixed HTTPURLResponse built from the scripted values above.
        guard let response = HTTPURLResponse(url: requestURL, statusCode: status,
                                             httpVersion: "HTTP/1.1",
                                             headerFields: headers) else {
            client?.urlProtocol(self, didFailWithError: URLError(.badServerResponse))
            return
        }
        client?.urlProtocol(self, didReceive: response, cacheStoragePolicy: .notAllowed)
        if !body.isEmpty {
            client?.urlProtocol(self, didLoad: body)
        }
        if path.hasSuffix("/events") {
            // Hold the stream open (mirrors the test harness' SSE
            // hold-open); stopLoading tears it down.
        } else {
            client?.urlProtocolDidFinishLoading(self)
        }
    }

    override func stopLoading() {}
}
#endif
