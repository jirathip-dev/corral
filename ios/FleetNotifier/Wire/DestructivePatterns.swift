import Foundation

/// Client-side mirror of the daemon's step-up gate (src/auth/step_up.rs) so
/// Face ID can be prompted BEFORE a destructive payload is sent. The daemon
/// remains the boundary — a mirror mismatch only costs an extra 403 → mint →
/// retry round trip, which DriveClient handles reactively too.
enum DestructivePatterns {

    /// Ported verbatim from `PATTERNS` in src/auth/step_up.rs.
    private static let patterns: [(name: String, needle: String)] = [
        ("rm -rf", "rm -rf"),
        ("rm -fr", "rm -fr"),
        ("rm -r -f", "rm -r -f"),
        ("rm --recursive --force", "rm --recursive --force"),
        ("rm --force --recursive", "rm --force --recursive"),
        ("push --force", "push --force"),
        ("push --force-with-lease", "push --force-with-lease"),
        ("push -f", "push -f"),
        ("pipe to sh", "| sh"),
        ("pipe to sh", "|sh"),
        ("pipe to bash", "| bash"),
        ("pipe to bash", "|bash"),
        ("pipe to zsh", "| zsh"),
        ("pipe to zsh", "|zsh"),
        ("remote eval", "-c \"$(curl"),
        ("remote eval", "-c \"$(wget"),
        ("remote eval", "-c \"$(fetch"),
        ("remote eval", "eval \"$(curl"),
        ("remote eval", "eval \"$(wget"),
        ("remote eval", "eval \"$(fetch"),
        ("process substitution", "<(curl"),
        ("process substitution", "<(wget"),
        ("process substitution", "<(fetch"),
        ("~/.aws", "~/.aws"),
        (".aws/credentials", ".aws/credentials"),
        ("~/.ssh", "~/.ssh"),
        (".env", ".env"),
        ("dd of=", "dd of="),
    ]

    /// Ported verbatim from `PATTERN_PAIRS` (both needles must be present).
    private static let patternPairs: [(name: String, a: String, b: String)] = [
        ("dd if=…of=", "dd if=", "of="),
        ("download and run", "curl", "&& sh"),
        ("download and run", "curl", "&& bash"),
        ("download and run", "wget", "&& sh"),
        ("download and run", "wget", "&& bash"),
        ("download and run", "fetch", "&& sh"),
        ("download and run", "fetch", "&& bash"),
    ]

    /// F1 canonicalizer: lowercase, whitespace runs collapsed to one space,
    /// `$HOME` → `~` (post-lowercase), `'` → `"`.
    static func canonicalize(_ text: String) -> String {
        let lowered = text.lowercased()
        let tilded = lowered.replacingOccurrences(of: "$home", with: "~")
        let quoted = tilded.replacingOccurrences(of: "'", with: "\"")
        return quoted.split(whereSeparator: { $0.isWhitespace }).joined(separator: " ")
    }

    /// Which destructive pattern (if any) a payload string matches.
    static func detect(in text: String) -> String? {
        let canon = canonicalize(text)
        for pattern in patterns where canon.contains(pattern.needle) {
            return pattern.name
        }
        for pair in patternPairs where canon.contains(pair.a) && canon.contains(pair.b) {
            return pair.name
        }
        return nil
    }

    /// Mirror of `StepUpGate::destructive_pattern` over a whole payload:
    /// every string VALUE is scanned (recursively) through the canonicalizer.
    static func detect(in value: CanonicalJSON.Value) -> String? {
        switch value {
        case .string(let s):
            return detect(in: s)
        case .object(let pairs):
            for pair in pairs {
                if let hit = detect(in: pair.value) { return hit }
            }
            return nil
        case .array(let items):
            for item in items {
                if let hit = detect(in: item) { return hit }
            }
            return nil
        case .int, .uint, .bool, .null:
            return nil
        }
    }

    /// True when a drive envelope's payload would require step-up.
    static func required(_ payload: CanonicalJSON.Value) -> Bool {
        detect(in: payload) != nil
    }
}
