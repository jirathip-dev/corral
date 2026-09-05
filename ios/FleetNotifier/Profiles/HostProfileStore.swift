import Foundation

// MARK: - Host profile store (#399 B1-B7)

/// File-backed (or in-memory) ordered host-profile store.
///
/// Persistence contract:
/// - One JSON document per app install (`host-profiles-v1.json`) written
///   ATOMICALLY with iOS file protection; the phone key itself stays in
///   the Keychain (DeviceKeyStore) — this file never holds key material,
///   only the pinned PUBLIC host keys + registration metadata.
/// - Per-profile SSE cursors live under `fleetnotifier.hostProfile.<id>`
///   in the injected defaults (same store the single-host cursor uses).
/// - Remove Host purges the profile, its cursor, AND its durable
///   board-cache file in one call (B7), leaving every other profile and
///   the shared phone key intact.
///
/// Rollback safety (B6): writes replace the whole document atomically, so
/// an interrupted save leaves the previous document intact; migration only
/// commits when a profile document write succeeds and never runs twice
/// (idempotent by construction — it no-ops once any profile exists).
final class HostProfileStore {
    static let profilesFileName = "host-profiles-v1.json"
    static let legacyHostKey = "fleetnotifier.host"
    static let legacyMetaKey = "fleetnotifier.deviceMeta"

    /// Production default directory for the profile document + per-host
    /// cache files (Application Support/FleetNotifier).
    static func defaultDirectory() -> URL? {
        let fm = FileManager.default
        guard let base = fm.urls(for: .applicationSupportDirectory,
                                 in: .userDomainMask).first else { return nil }
        return base.appendingPathComponent("FleetNotifier", isDirectory: true)
    }

    /// Directory for the profiles document + per-profile cache files; nil
    /// = memory-only store (unit tests, model fixtures).
    private let directory: URL?
    private let defaults: UserDefaults
    /// Cache store sharing the same directory (remove-host purges both).
    let boardCache: BoardCacheStore
    private(set) var profiles: [HostProfile] = []

    init(directory: URL?, defaults: UserDefaults = .standard) {
        self.directory = directory
        self.defaults = defaults
        self.boardCache = BoardCacheStore(directory: directory)
        load()
    }

    var isEmpty: Bool { profiles.isEmpty }

    var orderedProfiles: [HostProfile] {
        profiles.sorted { $0.order < $1.order }
    }

    func profile(id: UUID) -> HostProfile? {
        profiles.first { $0.id == id }
    }

    func index(of id: UUID) -> Int? {
        profiles.firstIndex { $0.id == id }
    }

    // MARK: - Mutations

    /// Add a host profile. Order appends at the end. Duplicate normalized
    /// URL and duplicate pinned host identity are rejected, as are empty
    /// and duplicate display names (B3).
    @discardableResult
    func addProfile(displayName: String,
                    urlString: String,
                    hostKeyB64: String? = nil,
                    fingerprint: String? = nil,
                    keyId: String? = nil,
                    grants: [String] = [],
                    expiryTs: UInt64? = nil,
                    registeredAt: UInt64 = nowEpochSeconds()) throws -> HostProfile {
        let name = displayName.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !name.isEmpty else { throw HostProfileError.emptyDisplayName }
        guard let normalized = HostURLForm.normalized(urlString) else {
            throw HostProfileError.invalidURL
        }
        try validateUniqueness(name: name, normalizedURL: normalized,
                               hostKeyB64: hostKeyB64, excluding: nil)
        let profile = HostProfile(displayName: name,
                                  urlString: normalized,
                                  hostKeyB64: hostKeyB64,
                                  fingerprint: fingerprint,
                                  keyId: keyId,
                                  grants: grants,
                                  expiryTs: expiryTs,
                                  registeredAt: registeredAt,
                                  order: (profiles.map(\.order).max() ?? -1) + 1,
                                  connectionState: hostKeyB64 == nil
                                      ? .awaitingFingerprintConfirmation
                                      : .disconnected)
        profiles.append(profile)
        save()
        return profile
    }

    /// Rename in place only (B5). URL/identity changes are remove-and-
    /// re-pair — there is no mutation API for them.
    @discardableResult
    func renameProfile(id: UUID, to newDisplayName: String) throws -> HostProfile {
        let name = newDisplayName.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !name.isEmpty else { throw HostProfileError.emptyDisplayName }
        guard let idx = index(of: id) else { throw HostProfileError.profileNotFound }
        try validateUniqueness(name: name,
                               normalizedURL: profiles[idx].urlString,
                               hostKeyB64: profiles[idx].hostKeyB64,
                               excluding: id)
        profiles[idx].displayName = name
        save()
        return profiles[idx]
    }

    /// Persist the confirmed pinned host key + fingerprint (B3/B6). Only
    /// this write transitions a profile out of
    /// `.awaitingFingerprintConfirmation` — connecting before it is
    /// impossible (see `HostProfile.mayConnect`).
    @discardableResult
    func confirmFingerprint(id: UUID, hostKeyB64: String, fingerprint: String) throws -> HostProfile {
        guard let idx = index(of: id) else { throw HostProfileError.profileNotFound }
        try validateUniqueness(name: profiles[idx].displayName,
                               normalizedURL: profiles[idx].urlString,
                               hostKeyB64: hostKeyB64,
                               excluding: id)
        profiles[idx].hostKeyB64 = hostKeyB64
        profiles[idx].fingerprint = fingerprint
        profiles[idx].connectionState = .disconnected
        save()
        return profiles[idx]
    }

    /// Fold a successful `/register` response into the profile (B3/B6):
    /// key_id/grants/expiry are per-host metadata scoped by profile.
    @discardableResult
    func applyRegistration(id: UUID, keyId: String, grants: [String], expiryTs: UInt64?) throws -> HostProfile {
        guard let idx = index(of: id) else { throw HostProfileError.profileNotFound }
        profiles[idx].keyId = keyId
        profiles[idx].grants = grants
        profiles[idx].expiryTs = expiryTs
        save()
        return profiles[idx]
    }

    /// Order swap used by drag-to-reorder (#401 consumes it later).
    func moveProfile(id: UUID, toOrder newOrder: Int) throws {
        guard let idx = index(of: id) else { throw HostProfileError.profileNotFound }
        var moved = profiles.remove(at: idx)
        let clamped = min(max(newOrder, 0), profiles.count)
        moved.order = clamped
        profiles.insert(moved, at: clamped)
        // Re-normalize the order fields to consecutive integers.
        for (index, var profile) in profiles.enumerated() {
            profile.order = index
            profiles[index] = profile
        }
        save()
    }

    /// #397: persist one host's per-host notification enrollment flag
    /// (the profile document owns it; removal purges it with the record).
    @discardableResult
    func setNotificationsEnabled(_ enabled: Bool, id: UUID) throws -> HostProfile {
        guard let idx = index(of: id) else { throw HostProfileError.profileNotFound }
        profiles[idx].notificationsEnabled = enabled
        save()
        return profiles[idx]
    }

    func noteConnectionState(id: UUID, _ state: ProfileConnectionState) {
        guard let idx = index(of: id) else { return }
        profiles[idx].connectionState = state
        save()
    }

    /// C6: stamp the last-successful-connection time (epoch millis) — a
    /// healthy idle host emits no data frames, so this is updated on
    /// connection success, not only on events.
    func noteLastSuccessfulConnection(id: UUID, at ts: UInt64 = nowEpochMillis()) {
        guard let idx = index(of: id) else { return }
        profiles[idx].lastSuccessfulConnectionTs = ts
        save()
    }

    // MARK: - Pairing commit (legacy mirror + Add Host)

    /// Pre-pairing validation for the Add Host form (B3): the display
    /// name and normalized URL must be non-empty/non-duplicate. The
    /// duplicate-identity check happens once the host key is fetched.
    func validateCandidate(displayName: String, urlString: String) throws {
        let name = displayName.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !name.isEmpty else { throw HostProfileError.emptyDisplayName }
        guard let normalized = HostURLForm.normalized(urlString) else {
            throw HostProfileError.invalidURL
        }
        try validateUniqueness(name: name, normalizedURL: normalized,
                               hostKeyB64: nil, excluding: nil)
    }

    /// B3: a fetched host key must not already be pinned by another
    /// profile (duplicate pinned host identity rejection).
    func validateCandidateIdentity(hostKeyB64: String) throws {
        guard let existing = profiles.first(where: { $0.hostKeyB64 == hostKeyB64 }) else {
            return
        }
        throw HostProfileValidationError.duplicateHostIdentity(existing.displayName)
    }

    /// Commit a completed pairing into the store (B5). Any stale record
    /// for the URL is purged first (re-registration refreshes; one record
    /// per URL), then the fresh record lands at the END of the order.
    /// The caller (AppModel) removes the previous ACTIVE profile record
    /// separately — remove-and-re-pair is a MODEL decision because the
    /// active identity lives there. `hostKeyB64` nil = legacy single-host
    /// flow (no pin, parity); non-nil = fingerprint-confirmed Add Host
    /// pairing (B3). `notificationsEnabled` carries the replaced record's
    /// per-host notification state when the URL is unchanged (#397 — a
    /// re-pair must not silently re-enable a host the user muted).
    @discardableResult
    func commitActivePairing(displayName: String,
                             urlString: String,
                             hostKeyB64: String?,
                             fingerprint: String?,
                             keyId: String,
                             grants: [String],
                             expiryTs: UInt64?,
                             registeredAt: UInt64,
                             notificationsEnabled: Bool = true) throws -> HostProfile {
        guard let normalized = HostURLForm.normalizedForLegacyMigration(urlString) else {
            throw HostProfileError.invalidURL
        }
        let name = uniqueDisplayName(
            displayName.trimmingCharacters(in: .whitespacesAndNewlines))
        guard !name.isEmpty else { throw HostProfileError.emptyDisplayName }
        for profile in profiles where profile.urlString == normalized {
            removeProfile(id: profile.id)
        }
        let profile = HostProfile(displayName: name,
                                  urlString: normalized,
                                  hostKeyB64: hostKeyB64,
                                  fingerprint: fingerprint,
                                  keyId: keyId,
                                  grants: grants,
                                  expiryTs: expiryTs,
                                  registeredAt: registeredAt,
                                  order: (profiles.map(\.order).max() ?? -1) + 1,
                                  connectionState: .disconnected,
                                  notificationsEnabled: notificationsEnabled)
        profiles.append(profile)
        save()
        return profile
    }

    private func uniqueDisplayName(_ name: String) -> String {
        let lower = name.lowercased()
        guard profiles.contains(where: { $0.displayName.lowercased() == lower }) else {
            return name
        }
        var suffix = 2
        while profiles.contains(where: {
            $0.displayName.lowercased() == "\(name) (\(suffix))".lowercased()
        }) {
            suffix += 1
        }
        return "\(name) (\(suffix))"
    }

    // MARK: - Cursors (per host)

    private static func cursorKey(_ id: UUID) -> String {
        "fleetnotifier.hostProfile.cursor.\(id.uuidString)"
    }

    func cursor(for id: UUID) -> UInt64? {
        guard profile(id: id) != nil,
              let raw = defaults.string(forKey: Self.cursorKey(id)) else { return nil }
        return UInt64(raw)
    }

    func setCursor(_ rev: UInt64?, for id: UUID) {
        guard profile(id: id) != nil else { return }
        if let rev {
            defaults.set(String(rev), forKey: Self.cursorKey(id))
        } else {
            defaults.removeObject(forKey: Self.cursorKey(id))
        }
        if let idx = index(of: id) {
            profiles[idx].cursorRev = rev
            save()
        }
    }

    // MARK: - Remove Host (B7, local unlink)

    /// Remove one host's profile + cursor + durable cache + in-memory
    /// cache row. Every OTHER profile and the shared phone signing key
    /// stay intact. This is a LOCAL unlink: the daemon's registry entry
    /// remains until the host removes it host-side.
    func removeProfile(id: UUID) {
        guard let idx = index(of: id) else { return }
        profiles.remove(at: idx)
        defaults.removeObject(forKey: Self.cursorKey(id))
        boardCache.remove(for: id)
        save()
        // #400 consumes removal for stream-task/tail cancellation; this
        // child exposes the purge (profile/cursor/cache/tails cleared)
        // without owning the runtime task coordination.
    }

    /// Remove every profile (Remove device, shared-phone-key reset). The
    /// keychain identity itself is wiped by the caller (DeviceKeyStore).
    func removeAll() {
        let ids = profiles.map(\.id)
        for id in ids {
            defaults.removeObject(forKey: Self.cursorKey(id))
            boardCache.remove(for: id)
        }
        profiles = []
        save()
    }

    // MARK: - Legacy migration (B6)

    /// First upgraded launch: atomically migrate the legacy single-host
    /// `fleetnotifier.host` + `DeviceMeta` into the FIRST ordered profile.
    /// No new token, no `/register`; key_id/grants/expiry are preserved.
    /// The profile starts WITHOUT a pinned host key in
    /// `.awaitingFingerprintConfirmation` — the app pauses once, fetches
    /// the host key, and only connects after fingerprint confirmation.
    ///
    /// Idempotent by construction: once ANY profile exists (migrated or
    /// newly paired) this no-ops, so a relaunch can never produce two
    /// active legacy/profile records. Rollback-safe: the caller only
    /// clears the legacy keys after this returns a profile (document
    /// write already succeeded).
    @discardableResult
    func migrateLegacy(host: String?,
                       keyId: String?,
                       grants: [String],
                       expiryTs: UInt64?,
                       registeredAt: UInt64) -> HostProfile? {
        guard profiles.isEmpty,
              let host,
              let normalized = HostURLForm.normalizedForLegacyMigration(host),
              let keyId, !keyId.isEmpty else {
            return nil
        }
        let name = HostURLForm.displayNameCandidate(for: host)
        let profile = HostProfile(displayName: name,
                                  urlString: normalized,
                                  hostKeyB64: nil,
                                  fingerprint: nil,
                                  keyId: keyId,
                                  grants: grants,
                                  expiryTs: expiryTs,
                                  registeredAt: registeredAt,
                                  order: 0,
                                  connectionState: .awaitingFingerprintConfirmation)
        profiles.append(profile)
        save()
        return profile
    }

    // MARK: - Persistence

    private func profilesURL() -> URL? {
        directory?.appendingPathComponent(Self.profilesFileName)
    }

    private func load() {
        guard let url = profilesURL(),
              let data = try? Data(contentsOf: url) else { return }
        // A corrupt document is pairing METADATA only (the signing key
        // lives in the Keychain): start empty and let the next save
        // replace the document. Failing the whole app over display
        // metadata would lock a healthy keychain identity out.
        if let decoded = try? JSONDecoder().decode([HostProfile].self, from: data) {
            profiles = decoded
        }
    }

    private func save() {
        guard let url = profilesURL() else { return }
        let fm = FileManager.default
        if let directory {
            try? fm.createDirectory(at: directory, withIntermediateDirectories: true)
        }
        guard let data = try? JSONEncoder().encode(profiles) else { return }
        // Atomic replace + iOS file protection (C5-style durability for
        // the profile document too).
        try? data.write(to: url, options: [.atomic, .completeFileProtection])
    }

    static func nowEpochSeconds() -> UInt64 {
        UInt64(Date().timeIntervalSince1970)
    }

    static func nowEpochMillis() -> UInt64 {
        UInt64(Date().timeIntervalSince1970 * 1000)
    }

    // MARK: - Uniqueness

    private func validateUniqueness(name: String,
                                    normalizedURL: String,
                                    hostKeyB64: String?,
                                    excluding id: UUID?) throws {
        for other in profiles where other.id != id {
            if other.displayName.lowercased() == name.lowercased() {
                throw HostProfileValidationError.duplicateDisplayName(other.displayName)
            }
            if other.urlString == normalizedURL {
                throw HostProfileValidationError.duplicateURL(other.displayName)
            }
            if let hostKeyB64, let otherKey = other.hostKeyB64,
               otherKey == hostKeyB64 {
                throw HostProfileValidationError.duplicateHostIdentity(other.displayName)
            }
        }
    }
}

// MARK: - Display-name candidate

extension HostURLForm {
    /// Prefill display name from a URL/host (B3: name prefilled from
    /// URL/Tailscale hostname). Falls back to the raw string when the URL
    /// cannot be parsed.
    static func displayNameCandidate(for raw: String) -> String {
        let candidate = raw.hasPrefix("http") ? raw : "https://" + raw
        if let url = URL(string: candidate), let host = url.host, !host.isEmpty {
            // Keep the first label for readability ("macbook-pro" from
            // "macbook-pro.tail1234.ts.net").
            return host.split(separator: ".").first.map(String.init) ?? host
        }
        return raw
    }
}
