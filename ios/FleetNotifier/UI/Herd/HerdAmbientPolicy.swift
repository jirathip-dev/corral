import Foundation

// #459: ambient scene-motion policy. Reduce Motion keeps the intentional
// static scene (that gate stays in the view that already owns it); Low
// Power Mode and a serious or critical thermal state prefer the same static
// fallback over spending frames on ambience. Pure and deterministic so
// every rung of the ladder is testable without device state.
enum HerdAmbientPolicy {
    static func ambientMotionAllowed(lowPower: Bool, thermal: ProcessInfo.ThermalState) -> Bool {
        if lowPower { return false }
        switch thermal {
        case .nominal, .fair: return true
        case .serious, .critical: return false
        @unknown default: return false
        }
    }
}
