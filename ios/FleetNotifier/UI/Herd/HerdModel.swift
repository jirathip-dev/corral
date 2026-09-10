import CryptoKit
import SwiftUI

// Presentation only. Neither a second fleet store nor an operational clock.
enum FleetPresentation: String, CaseIterable { case board = "Board", herd = "Herd" }
enum HorsePose: String { case stand, working, blocked, done, unknown, graze, alertStatic }

struct HorseIdentity: Equatable {
    let coat: Int
    let breed: Int
    let mane: Int
    let tack: Int
    let accessory: Int
    let phase: Double

    init(name: String) {
        let bytes = Array(SHA256.hash(data: Data(name.utf8)))
        coat = Int(bytes[0]) % 8
        mane = Int(bytes[1] >> 2) % 3
        breed = Int(bytes[2] >> 4) % 3
        tack = Int(bytes[3]) % 3
        accessory = Int(bytes[4]) % 3
        phase = Double(bytes[0] % 30)
    }
    var blaze: Bool { [1, 4, 5, 7].contains(coat) }
}

struct HerdHorse: Identifiable, Equatable {
    let agent: Agent
    let hostProfileID: UUID?
    let hostName: String?
    let disconnected: Bool
    var id: String { (hostProfileID?.uuidString ?? "") + "::" + agent.agentId }
    var name: String { agent.displayName ?? agent.agentId }
    var identity: HorseIdentity { HorseIdentity(name: name) }
    var state: AgentState { disconnected ? .unknown : agent.state }
    var atRail: Bool { agent.state == .blocked }
    var statusText: String {
        disconnected ? "unknown · last known \(agent.state.rawValue)" : state.rawValue
    }
    func pose(elapsed: Double, reduceMotion: Bool) -> HorsePose {
        switch state {
        case .blocked: return reduceMotion ? .alertStatic : .blocked
        case .working: return reduceMotion ? .stand : .working
        case .done: return .done
        case .unknown: return .unknown
        case .idle:
            let phase = (elapsed + identity.phase).truncatingRemainder(dividingBy: 30)
            return !reduceMotion && phase >= 24 && phase < 28 ? .graze : .stand
        }
    }
    func roam(elapsed: Double, enabled: Bool) -> CGFloat {
        guard enabled, !disconnected, state == .idle || state == .working else { return 0 }
        // 132 pt art in a 156 pt slot leaves 12 pt inset; ±4 retains ≥8.
        return 4 * sin(elapsed * .pi / 12 + identity.phase)
    }
}

struct HerdPaddock: Identifiable {
    let repo: String?
    let horses: [HerdHorse]
    var id: String { repo.map { "repo:" + $0 } ?? "other:" }
    var title: String { repo ?? BoardModel.otherRepoLabel }
    var field: [HerdHorse] { horses.filter { !$0.atRail } }
    var blockedCount: Int { horses.filter(\.atRail).count }
}

enum HerdProjection {
    static func paddocks(_ horses: [HerdHorse]) -> [HerdPaddock] {
        let grouped = Dictionary(grouping: horses) { BoardModel.repoKey(of: $0.agent) }
        return grouped.keys.sorted { left, right in
            let leftWorking = grouped[left]?.contains { $0.state == .working } == true
            let rightWorking = grouped[right]?.contains { $0.state == .working } == true
            if leftWorking != rightWorking { return leftWorking }
            return (left ?? "\u{10ffff}") < (right ?? "\u{10ffff}")
        }.map { HerdPaddock(repo: $0, horses: grouped[$0] ?? []) }
    }
    static func reconciledPaddockID(_ selected: String?, in ids: [String]) -> String? {
        ids.contains(selected ?? "") ? selected : ids.first
    }
    static func single(_ sections: BoardModel.Sections, host: UUID?, disconnected: Bool) -> [HerdHorse] {
        sections.statuses.flatMap(\.subgroups).flatMap(\.agents).map {
            HerdHorse(agent: $0, hostProfileID: host, hostName: nil, disconnected: disconnected)
        }
    }
    static func multiple(_ sections: BoardModel.HostSections, names: [UUID: String]) -> [HerdHorse] {
        sections.statuses.flatMap(\.subgroups).flatMap(\.rows).map {
            HerdHorse(agent: $0.agent, hostProfileID: $0.identity.hostProfileID,
                      hostName: names[$0.identity.hostProfileID], disconnected: $0.isStale)
        }
    }
}

@MainActor
final class HerdClock: ObservableObject {
    @Published private(set) var elapsed = 0.0
    private(set) var ticks = 0
    private(set) var running = false
    private var task: Task<Void, Never>?
    func start() {
        guard task == nil else { return }
        running = true
        elapsed = 0
        let start = ContinuousClock.now
        task = Task { [weak self] in
            while !Task.isCancelled {
                do { try await Task.sleep(for: .milliseconds(100)) }
                catch { return }
                guard let self, !Task.isCancelled else { return }
                let duration = start.duration(to: .now).components
                self.elapsed = Double(duration.seconds) + Double(duration.attoseconds) / 1e18
                self.ticks += 1
            }
        }
    }
    func stop() {
        task?.cancel()
        task = nil
        running = false
        elapsed = 0
    }
    deinit { task?.cancel() }
}

enum RanchPlane: String, CaseIterable, Identifiable {
    case sky, hills, barnTrees, ground, rearFences, foreground, horses, frontRail
    var id: String { rawValue }
    var ratio: CGFloat {
        switch self {
        case .sky: return 0.05
        case .hills: return 0.12
        case .barnTrees: return 0.22
        case .ground, .rearFences: return 0.40
        case .foreground: return 0.65
        case .horses: return 1
        case .frontRail: return 0
        }
    }
    func offset(scroll: CGFloat, coverage: CGFloat, reduceMotion: Bool) -> CGFloat {
        guard !reduceMotion, scroll.isFinite, coverage.isFinite else { return 0 }
        return max(-max(0, coverage), min(max(0, coverage), -scroll * ratio))
    }
}
