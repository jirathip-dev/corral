import AppKit
import ApplicationServices
import CoreGraphics
import Foundation

struct WindowRecord: Encodable {
    let placement: Int
    let window_number: Int?
    let layer: Int?
    let onscreen: Bool?
    let bounds: [String: Double?]?
    let owner: String?
    let owner_pid: Int?
    let owner_pid_exact_match: Bool
    let name: String?
}

struct ProbeOutput: Encodable {
    let target_pid: Int
    let accessibility_probe_ok: Bool
    let accessibility_error: String?
    let process_pid: Int?
    let process_visible: Bool?
    let frontmost: Bool?
    let key_window: Bool?
    let main_window: Bool?
    let window_count: Int
    let frontmost_application_pid: Int?
    let frontmost_matches_target: Bool?
    let cg_owner_pid_match: Bool
    let window_visible: Bool
    let on_active_space: Bool?
    let current_space_index: Int?
    let target_space_index: Int?
    let space_index_reason: String
    let windows: [WindowRecord]
}

guard CommandLine.arguments.count == 2, let targetPID = Int32(CommandLine.arguments[1]) else {
    fputs("usage: native-window-probe PID\n", stderr)
    exit(2)
}

let frontmostPID = NSWorkspace.shared.frontmostApplication.map { Int($0.processIdentifier) }
let rawWindows = CGWindowListCopyWindowInfo([.optionOnScreenOnly], kCGNullWindowID) as? [[String: Any]] ?? []

func number(_ value: Any?) -> Int? {
    (value as? NSNumber)?.intValue
}

func boolean(_ value: Any?) -> Bool? {
    (value as? NSNumber).map { $0.boolValue }
}

func bounds(_ value: Any?) -> [String: Double?]? {
    guard let dictionary = value as? [String: Any] else { return nil }
    func coordinate(_ key: String) -> Double? {
        (dictionary[key] as? NSNumber)?.doubleValue
    }
    return [
        "x": coordinate("X"),
        "y": coordinate("Y"),
        "width": coordinate("Width"),
        "height": coordinate("Height"),
    ]
}

func boolAttribute(_ element: AXUIElement, _ name: CFString) -> Bool? {
    var value: CFTypeRef?
    guard AXUIElementCopyAttributeValue(element, name, &value) == .success,
          let number = value as? NSNumber else {
        return nil
    }
    return number.boolValue
}

let application = AXUIElementCreateApplication(targetPID)
var accessibilityErrors: [String] = []
var applicationPID: pid_t = 0
let pidError = AXUIElementGetPid(application, &applicationPID)
if pidError != .success {
    accessibilityErrors.append("pid=\(pidError.rawValue)")
}
let processVisible = boolAttribute(application, kAXHiddenAttribute as CFString).map { !$0 }
if processVisible == nil {
    accessibilityErrors.append("hidden=unavailable")
}
let frontmost = boolAttribute(application, kAXFrontmostAttribute as CFString)
if frontmost == nil {
    accessibilityErrors.append("frontmost=unavailable")
}
var windowsValue: CFTypeRef?
let windowsError = AXUIElementCopyAttributeValue(
    application,
    kAXWindowsAttribute as CFString,
    &windowsValue
)
if windowsError != .success {
    accessibilityErrors.append("windows=\(windowsError.rawValue)")
}
let accessibilityWindows = (windowsValue as? [AXUIElement]) ?? []
let keyWindow = accessibilityWindows.contains {
    boolAttribute($0, kAXFocusedAttribute as CFString) == true
}
let mainWindow = accessibilityWindows.contains {
    boolAttribute($0, kAXMainAttribute as CFString) == true
}

let windows = rawWindows.enumerated().map { placement, dictionary in
    let ownerPID = number(dictionary[kCGWindowOwnerPID as String])
    return WindowRecord(
        placement: placement,
        window_number: number(dictionary[kCGWindowNumber as String]),
        layer: number(dictionary[kCGWindowLayer as String]),
        onscreen: boolean(dictionary[kCGWindowIsOnscreen as String]),
        bounds: bounds(dictionary[kCGWindowBounds as String]),
        owner: dictionary[kCGWindowOwnerName as String] as? String,
        owner_pid: ownerPID,
        owner_pid_exact_match: ownerPID == Int(targetPID),
        name: dictionary[kCGWindowName as String] as? String
    )
}

let ownerMatch = windows.contains { $0.owner_pid_exact_match }
let targetWindowVisible = windows.contains { window in
    guard window.owner_pid_exact_match,
          window.onscreen == true,
          window.layer == 0,
          let bounds = window.bounds,
          let width = bounds["width"],
          let height = bounds["height"] else {
        return false
    }
    return (width ?? 0) > 0 && (height ?? 0) > 0
}
let output = ProbeOutput(
    target_pid: Int(targetPID),
    accessibility_probe_ok: pidError == .success && processVisible != nil
        && frontmost != nil && windowsError == .success,
    accessibility_error: accessibilityErrors.isEmpty
        ? nil
        : accessibilityErrors.joined(separator: ","),
    process_pid: pidError == .success ? Int(applicationPID) : nil,
    process_visible: processVisible,
    frontmost: frontmost,
    key_window: accessibilityWindows.isEmpty ? nil : keyWindow,
    main_window: accessibilityWindows.isEmpty ? nil : mainWindow,
    window_count: accessibilityWindows.count,
    frontmost_application_pid: frontmostPID,
    frontmost_matches_target: frontmostPID.map { $0 == Int(targetPID) },
    cg_owner_pid_match: ownerMatch,
    window_visible: targetWindowVisible,
    on_active_space: targetWindowVisible ? true : false,
    current_space_index: nil,
    target_space_index: nil,
    space_index_reason: "unavailable_without_private_CGS_space_api",
    windows: windows
)

let encoder = JSONEncoder()
encoder.outputFormatting = [.sortedKeys]
let data = try encoder.encode(output)
FileHandle.standardOutput.write(data)
FileHandle.standardOutput.write(Data([0x0a]))
