import Foundation

// MARK: - Host profile model + store (#399 B1-B7)

/// Connection posture of one host profile (B1). Single-host behavior is
/// unchanged until a second profile exists (#394 F1); the multi-host
/// coordinator (#400) consumes these states per profile.
enum ProfileConnectionState: String, Codable, Equatable, Sendable {
    /// Paired but the pinned host key is not confirmed yet (legacy
    /// migration pause, B6): the profile must not open a live stream.
    case awaitingFingerprintConfirmation
    case disconnected
    case connected
    /// The URL's host key no longer matches the pinned key (B4): the
    /// profile fails closed and stays stale until Remove Host + fresh
    /// pairing. Never auto-accepted.
    case keyMismatch
}

/// One host profile (B1). The profile id is the ONLY stable internal
/// routing identity; display name and URL are user-facing labels and are
/// never routing identities. The pinned key + fingerprint are written once
/// at fingerprint confirmation and immutable afterwards — changing URL or
/// identity is remove-and-re-pair (B5), never an edit.
struct HostProfile: Codable, Equatable, Identifiable, Sendable {
    /// Stable internal profile id (routing identity).
    var id: UUID
    /// User-controlled display name: non-empty and unique across profiles.
    var displayName: String
    /// Immutable normalized URL (https; loopback http tolerated for dev).
    var urlString: String
    /// Pinned full X25519 host public key, base64 (B3/B4). Nil until the
    /// user confirms the fingerprint — a host without a pin has no
    /// continuity contract and no live stream opens on launch.
    var hostKeyB64: String?
    /// Derived fingerprint (display/confirmation only; never an id).
    var fingerprint: String?
    /// Per-host registration metadata (B2): key_id/grants/expiry are
    /// scoped by profile even when deterministic key-id strings collide.
    var keyId: String?
    var grants: [String]
    var expiryTs: UInt64?
    /// Wall-clock epoch seconds when this device registered with the host.
    var registeredAt: UInt64
    /// User-controlled order (chips/filters follow it, #401). New hosts
    /// append at the end; drag-to-reorder mutates only this field.
    var order: Int
    var connectionState: ProfileConnectionState
    /// Per-host SSE cursor (B1). The single-host FleetStore cursor mirrors
    /// the first profile's value for parity; #400 owns N live cursors.
    var cursorRev: UInt64?
    /// Epoch millis of the last successful HTTP/SSE connection (C6) —
    /// updated on a successful connection, not only on data events.
    var lastSuccessfulConnectionTs: UInt64?
    /// #397: per-host state-change notification enrollment. The global
    /// Notifications control is retained; this flag decides whether THIS
    /// host may enroll the APNs token (and fire the DEBUG bridge). True by
    /// default — absent on pre-#397 profile documents, decoded as true.
    var notificationsEnabled: Bool

    /// Whether this profile is allowed to open a live stream right now.
    var mayConnect: Bool {
        hostKeyB64 != nil && connectionState != .awaitingFingerprintConfirmation
            && connectionState != .keyMismatch
    }

    init(id: UUID = UUID(),
         displayName: String,
         urlString: String,
         hostKeyB64: String? = nil,
         fingerprint: String? = nil,
         keyId: String? = nil,
         grants: [String] = [],
         expiryTs: UInt64? = nil,
         registeredAt: UInt64,
         order: Int,
         connectionState: ProfileConnectionState = .disconnected,
         cursorRev: UInt64? = nil,
         lastSuccessfulConnectionTs: UInt64? = nil,
         notificationsEnabled: Bool = true) {
        self.id = id
        self.displayName = displayName
        self.urlString = urlString
        self.hostKeyB64 = hostKeyB64
        self.fingerprint = fingerprint
        self.keyId = keyId
        self.grants = grants
        self.expiryTs = expiryTs
        self.registeredAt = registeredAt
        self.order = order
        self.connectionState = connectionState
        self.cursorRev = cursorRev
        self.lastSuccessfulConnectionTs = lastSuccessfulConnectionTs
        self.notificationsEnabled = notificationsEnabled
    }
}

// MARK: - Codable (additive #397 field, backward-compatible decode)

extension HostProfile {
    /// Custom CodingKeys keep the stored document's JSON key names
    /// identical to the pre-#397 form for every EXISTING field; the
    /// additive `notificationsEnabled` key decodes as true when absent so
    /// an old `host-profiles-v1.json` loads unchanged (per-host state was
    /// introduced after that document format shipped).
    private enum CodingKeys: String, CodingKey {
        case id
        case displayName
        case urlString
        case hostKeyB64
        case fingerprint
        case keyId
        case grants
        case expiryTs
        case registeredAt
        case order
        case connectionState
        case cursorRev
        case lastSuccessfulConnectionTs
        case notificationsEnabled
    }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        id = try c.decode(UUID.self, forKey: .id)
        displayName = try c.decode(String.self, forKey: .displayName)
        urlString = try c.decode(String.self, forKey: .urlString)
        hostKeyB64 = try c.decodeIfPresent(String.self, forKey: .hostKeyB64)
        fingerprint = try c.decodeIfPresent(String.self, forKey: .fingerprint)
        keyId = try c.decodeIfPresent(String.self, forKey: .keyId)
        grants = try c.decodeIfPresent([String].self, forKey: .grants) ?? []
        expiryTs = try c.decodeIfPresent(UInt64.self, forKey: .expiryTs)
        registeredAt = try c.decode(UInt64.self, forKey: .registeredAt)
        order = try c.decode(Int.self, forKey: .order)
        connectionState = try c.decodeIfPresent(ProfileConnectionState.self,
                                                 forKey: .connectionState) ?? .disconnected
        cursorRev = try c.decodeIfPresent(UInt64.self, forKey: .cursorRev)
        lastSuccessfulConnectionTs = try c.decodeIfPresent(UInt64.self,
                                                           forKey: .lastSuccessfulConnectionTs)
        // #397 additive: pre-#397 documents have no key — default ON.
        notificationsEnabled = try c.decodeIfPresent(Bool.self,
                                                     forKey: .notificationsEnabled) ?? true
    }

    func encode(to encoder: Encoder) throws {
        var c = encoder.container(keyedBy: CodingKeys.self)
        try c.encode(id, forKey: .id)
        try c.encode(displayName, forKey: .displayName)
        try c.encode(urlString, forKey: .urlString)
        try c.encodeIfPresent(hostKeyB64, forKey: .hostKeyB64)
        try c.encodeIfPresent(fingerprint, forKey: .fingerprint)
        try c.encodeIfPresent(keyId, forKey: .keyId)
        try c.encode(grants, forKey: .grants)
        try c.encodeIfPresent(expiryTs, forKey: .expiryTs)
        try c.encode(registeredAt, forKey: .registeredAt)
        try c.encode(order, forKey: .order)
        try c.encode(connectionState, forKey: .connectionState)
        try c.encodeIfPresent(cursorRev, forKey: .cursorRev)
        try c.encodeIfPresent(lastSuccessfulConnectionTs, forKey: .lastSuccessfulConnectionTs)
        try c.encode(notificationsEnabled, forKey: .notificationsEnabled)
    }
}

/// Typed store failures surfaced to the pairing UI (B3/B5/B7).
enum HostProfileError: Error, Equatable, LocalizedError {
    case emptyDisplayName
    case duplicateDisplayName
    case invalidURL
    case duplicateURL
    case duplicateHostIdentity
    case invalidHostKeyForm
    case profileNotFound

    var errorDescription: String? {
        switch self {
        case .emptyDisplayName:
            return "Host name must not be empty."
        case .duplicateDisplayName:
            return "Another host already uses that name."
        case .invalidURL:
            return "Enter an https:// URL (http is allowed only for loopback development hosts)."
        case .duplicateURL:
            return "That host URL is already paired."
        case .duplicateHostIdentity:
            return "That host identity (key) is already paired on another URL."
        case .invalidHostKeyForm:
            return "The host did not return a well-formed X25519 key — pairing stopped."
        case .profileNotFound:
            return "Host profile not found."
        }
    }
}

/// URL normalization for host profiles (B1): https by default, loopback
/// http tolerated for simulator/LAN development, case + default ports +
/// trailing slash normalized so duplicates are exact. The result is the
/// immutable `urlString`; display text is never used for routing.
enum HostURLForm {
    static func normalized(_ raw: String) -> String? {
        normalized(raw, allowPlainHTTPForNonLoopback: false)
    }

    /// Legacy-migration form (B6): preserves the existing scheme verbatim
    /// (a pre-#399 daemon may be plain http on a LAN/Tailscale dev setup),
    /// so an upgraded phone keeps connecting without re-pairing.
    static func normalizedForLegacyMigration(_ raw: String) -> String? {
        normalized(raw, allowPlainHTTPForNonLoopback: true)
    }

    private static func normalized(_ raw: String,
                                   allowPlainHTTPForNonLoopback: Bool) -> String? {
        let trimmed = raw.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty else { return nil }
        var candidate = trimmed
        if !candidate.contains("://") {
            candidate = "https://" + candidate
        }
        guard var components = URLComponents(string: candidate),
              let scheme = components.scheme?.lowercased(),
              scheme == "http" || scheme == "https",
              let host = components.host?.lowercased(), !host.isEmpty else {
            return nil
        }
        components.scheme = scheme
        components.host = host
        if scheme == "http" && !allowPlainHTTPForNonLoopback {
            guard isLoopback(host) else { return nil }
        }
        // Default ports are redundant after normalization.
        if scheme == "https", components.port == 443 {
            components.port = nil
        }
        if scheme == "http", components.port == 80 {
            components.port = nil
        }
        // Userinfo and fragments never belong in a paired host URL.
        components.user = nil
        components.password = nil
        components.fragment = nil
        // A bare "/" path is the default; collapse trailing slashes so
        // "https://host" and "https://host/" are the same URL.
        let path = components.percentEncodedPath
        if !path.isEmpty && path != "/" {
            var trimmedPath = path
            while trimmedPath.hasSuffix("/"), trimmedPath.count > 1 {
                trimmedPath.removeLast()
            }
            components.percentEncodedPath = trimmedPath
        } else {
            components.percentEncodedPath = ""
        }
        guard let out = components.string, let _ = URL(string: out) else { return nil }
        return out
    }

    static func isLoopback(_ host: String) -> Bool {
        host == "localhost" || host == "::1" || host.hasPrefix("127.")
    }
}

/// Errors carrying the specific duplicate so the UI can name it.
enum HostProfileValidationError: Error, Equatable, LocalizedError {
    case duplicateURL(String)
    case duplicateHostIdentity(String)
    case duplicateDisplayName(String)

    var errorDescription: String? {
        switch self {
        case .duplicateURL(let url):
            return "A host for \(url) is already paired."
        case .duplicateHostIdentity(let name):
            return "\(name) is already paired with that host key."
        case .duplicateDisplayName(let name):
            return "A host named \(name) already exists."
        }
    }
}
