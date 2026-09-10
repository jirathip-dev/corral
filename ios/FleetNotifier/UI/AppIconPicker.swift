import SwiftUI
import UIKit

// MARK: - #464 Settings → Appearance → App Icon (native alternate-icon picker)
//
// Exactly four shipping choices, consumed from the #463 packaging contract:
// Bay is the PRIMARY (a nil `alternateIconName` restores it) and the stable
// alternates are exactly Palomino, Black and Grey. The live `UIApplication`
// state is the ONLY source of truth — there is no saved preference, no
// inference from theme/mode, and no automatic switching.
//
// Previews are the SHIPPED art: the four generated loadable imagesets
// (byte-identical to the approved masters) named by the literal image-loader
// call sites in `preview` below — the only non-symbol loaders the app is
// allowed to have, and the fail-closed #444/#463 source guard allowlists
// exactly those four names.

/// The four approved shipping choices, in picker order.
enum FleetAppIcon: String, CaseIterable, Identifiable, Sendable {
    case bay
    case palomino
    case black
    case grey

    /// Bay primary first, then the three alternates.
    static let shipping: [FleetAppIcon] = [.bay, .palomino, .black, .grey]

    var id: String { rawValue }

    var displayName: String {
        switch self {
        case .bay: return "Bay"
        case .palomino: return "Palomino"
        case .black: return "Black"
        case .grey: return "Grey"
        }
    }

    /// The compiled catalog rendition name (#463 appiconset names).
    var assetName: String {
        switch self {
        case .bay: return "AppIcon"
        case .palomino, .black, .grey: return displayName
        }
    }

    /// The `UIApplication.alternateIconName` value; nil restores Bay.
    var alternateIconName: String? {
        self == .bay ? nil : assetName
    }

    /// The shipped catalog art for the picker preview. #464: the appiconset
    /// renditions are not vendable to the app on iOS 18+, so the previews are
    /// the four generated loadable imagesets (byte-identical to the approved
    /// masters). Literal call sites so the source guard can allowlist exactly
    /// these four names.
    var preview: Image {
        switch self {
        case .bay: return Image("BayPreview")
        case .palomino: return Image("PalominoPreview")
        case .black: return Image("BlackPreview")
        case .grey: return Image("GreyPreview")
        }
    }

    /// Maps live system state; an unrecognised alternate name has no tile.
    init?(systemAlternateIconName: String?) {
        guard let name = systemAlternateIconName else {
            self = .bay
            return
        }
        guard let match = Self.shipping.first(where: { $0.alternateIconName == name }) else {
            return nil
        }
        self = match
    }
}

/// The narrow seam over `UIApplication`'s alternate-icon API, so unit tests
/// can simulate success, failure, unsupported devices, current-selection
/// no-ops, nil restores and rapid taps without a device.
@MainActor
protocol AppIconSystem: AnyObject {
    var supportsAlternateIcons: Bool { get }
    var alternateIconName: String? { get }
    /// Requests the change. iOS presents its own confirmation dialog; the
    /// completion fires afterwards with the user's decision.
    func setAlternateIconName(_ name: String?,
                              completion: @escaping @MainActor (Error?) -> Void)
}

/// The production seam over the real `UIApplication`.
@MainActor
final class UIApplicationAppIconSystem: AppIconSystem {
    var supportsAlternateIcons: Bool { UIApplication.shared.supportsAlternateIcons }
    var alternateIconName: String? { UIApplication.shared.alternateIconName }

    func setAlternateIconName(_ name: String?,
                              completion: @escaping @MainActor (Error?) -> Void) {
        // The standard iOS confirmation dialog is part of the flow: this call
        // never suppresses it, and the completion reports the outcome.
        UIApplication.shared.setAlternateIconName(name) { error in
            Task { @MainActor in completion(error) }
        }
    }
}

/// The picker's state: live system state plus ephemeral in-flight status.
/// It caches NOTHING else — no UserDefaults, no parallel icon preference.
@MainActor
final class AppIconPickerModel: ObservableObject {
    @Published private(set) var supportsAlternateIcons: Bool
    /// The live system selection (nil while the system reports a name that is
    /// not one of the four shipping choices).
    @Published private(set) var selection: FleetAppIcon?
    /// In-flight guard: at most one request may be pending.
    @Published private(set) var isApplying = false
    /// Retryable failure copy; cleared on the next accepted request.
    @Published private(set) var errorMessage: String?

    private let system: AppIconSystem

    /// `system` defaults to the real `UIApplication` seam; tests inject the
    /// mock through the same protocol.
    init(system: AppIconSystem? = nil) {
        let system = system ?? UIApplicationAppIconSystem()
        self.system = system
        self.supportsAlternateIcons = system.supportsAlternateIcons
        self.selection = FleetAppIcon(systemAlternateIconName: system.alternateIconName)
    }

    /// Re-reads the live system state — on appear, on scene-active return, and
    /// after every completion. Never a parallel saved preference.
    func refresh() {
        supportsAlternateIcons = system.supportsAlternateIcons
        selection = FleetAppIcon(systemAlternateIconName: system.alternateIconName)
    }

    /// Handles a tile tap. The current selection and any in-flight request are
    /// no-ops; otherwise the request is sent and iOS confirms with the user.
    func select(_ icon: FleetAppIcon) {
        guard supportsAlternateIcons, !isApplying else { return }
        refresh()
        guard icon != selection else { return }
        isApplying = true
        errorMessage = nil
        system.setAlternateIconName(icon.alternateIconName) { [weak self] error in
            self?.completeChange(to: icon, error: error)
        }
    }

    private func completeChange(to icon: FleetAppIcon, error: Error?) {
        isApplying = false
        // The system is authoritative: re-read instead of trusting the
        // completion callback, so a stale or lying result cannot show success.
        refresh()
        if error != nil {
            errorMessage = "Couldn't change the Home Screen icon. "
                + "Tap \(icon.displayName) to try again."
            return
        }
        guard selection == icon else {
            errorMessage = "iOS didn't apply the \(icon.displayName) icon. "
                + "Tap \(icon.displayName) to try again."
            return
        }
        errorMessage = nil
    }
}
