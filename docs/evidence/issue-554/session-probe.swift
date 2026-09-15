// #554 transport probe — the four measured facts the contract's mechanism
// decisions rest on. Run with `swift docs/evidence/issue-554/session-probe.swift`
// (macOS host toolchain; no simulator, no app build, no network egress beyond a
// black-holed RFC1918 address that never answers).
// Raw output is recorded in docs/evidence/issue-554/timeout-precedence.md.
import Foundation

final class Mock: URLProtocol {
    override class func canInit(with request: URLRequest) -> Bool { true }
    override class func canonicalRequest(for request: URLRequest) -> URLRequest { request }
    override func startLoading() {
        // SAFETY: fixture response for a fixed fixture URL.
        let resp = HTTPURLResponse(url: request.url!, statusCode: 200,
                                   httpVersion: "HTTP/1.1", headerFields: ["Content-Type": "application/json"])!
        client?.urlProtocol(self, didReceive: resp, cacheStoragePolicy: .notAllowed)
        client?.urlProtocol(self, didLoad: Data("ok".utf8))
        client?.urlProtocolDidFinishLoading(self)
    }
    override func stopLoading() {}
}

final class HoldOpen: URLProtocol {
    static let lock = NSLock()
    nonisolated(unsafe) static var started = 0
    nonisolated(unsafe) static var stopped = 0

    override class func canInit(with request: URLRequest) -> Bool { true }
    override class func canonicalRequest(for request: URLRequest) -> URLRequest { request }
    override func startLoading() {
        Self.lock.lock(); Self.started += 1; Self.lock.unlock()
        // Never finishes: the task is torn down by cancellation only.
    }
    override func stopLoading() {
        Self.lock.lock(); Self.stopped += 1; Self.lock.unlock()
    }
}

// 1/2: a session derived through `session.configuration` keeps the injected
// URLProtocol stack and the base session's request timeout.
let config = URLSessionConfiguration.ephemeral
config.protocolClasses = [Mock.self]
let base = URLSession(configuration: config)
let derivedConfig = base.configuration.copy() as! URLSessionConfiguration
derivedConfig.waitsForConnectivity = false
let derived = URLSession(configuration: derivedConfig)
print("MOCK preserved:", derived.configuration.protocolClasses?.contains { $0 == Mock.self } ?? false)
print("timeout base/derived:", base.configuration.timeoutIntervalForRequest,
      derived.configuration.timeoutIntervalForRequest)
print("shared timeout/wfc:", URLSession.shared.configuration.timeoutIntervalForRequest,
      URLSession.shared.configuration.waitsForConnectivity)

let sem = DispatchSemaphore(value: 0)
derived.dataTask(with: URL(string: "https://probe.example/host-key")!) { data, resp, err in
    print("derived fetch:", (resp as? HTTPURLResponse)?.statusCode ?? -1,
          String(data: data ?? Data(), encoding: .utf8) ?? "-", err?.localizedDescription ?? "-")
    sem.signal()
}.resume()
_ = sem.wait(timeout: .now() + 10)

// 3: `invalidateAndCancel()` semantics — a task STARTED before invalidation is
// torn down (stopLoading) and completes as NSURLErrorCancelled.
let holdConfig = URLSessionConfiguration.ephemeral
holdConfig.protocolClasses = [HoldOpen.self]
let holding = URLSession(configuration: holdConfig)
let holdSem = DispatchSemaphore(value: 0)
var reported = "still pending"
holding.dataTask(with: URL(string: "https://probe.example/host-key")!) { _, _, err in
    reported = (err as NSError?).map { "domain=\($0.domain) code=\($0.code)" } ?? "NO ERROR"
    holdSem.signal()
}.resume()
Thread.sleep(forTimeInterval: 0.5)
print("hold-open started:", HoldOpen.started, "stopped:", HoldOpen.stopped)
holding.invalidateAndCancel()
_ = holdSem.wait(timeout: .now() + 10)
print("after invalidateAndCancel stopped:", HoldOpen.stopped, "completion:", reported)

// 4: request-level vs session-level timeout precedence. The preflight request in
// Network/CorraldClient.swift sets `timeoutInterval = 15`, so this decides
// whether a session-level short timeout could bound it (it cannot).
let shortConfig = URLSessionConfiguration.ephemeral
shortConfig.timeoutIntervalForRequest = 2
let blackholed = URLSession(configuration: shortConfig)
var request = URLRequest(url: URL(string: "https://10.255.255.1/")!)
request.timeoutInterval = 15
let started = Date()
let blackholeSem = DispatchSemaphore(value: 0)
blackholed.dataTask(with: request) { _, _, err in
    print("blackhole elapsed=" + String(format: "%.1f", Date().timeIntervalSince(started)) + "s",
          "code=\((err as NSError?)?.code ?? 0)")
    blackholeSem.signal()
}.resume()
if blackholeSem.wait(timeout: .now() + 25) == .timedOut {
    print("blackhole still pending after 25s: the request-level 15s wins over the session-level 2s")
}
blackholed.invalidateAndCancel()
