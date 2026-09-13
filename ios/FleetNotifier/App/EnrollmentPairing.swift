import Foundation

// MARK: - QR enrollment pairing (#486: the Add Host flow's scan path)

/// One in-progress QR enrollment exactly as the Add Host sheet renders it.
///
/// Deliberately carries NO pairing material: the scanned single-use
/// redemption `code` never enters this value, so no view — and no
/// screenshot of one — can render or copy it. The code lives only in the
/// model's private transient slot for the duration of the pairing and is
/// dropped by every exit (success, discard, stop, failure).
struct EnrollmentDraft: Equatable {
    /// User-visible host name; prefilled from the code's endpoint when the
    /// draft had none, editable through the existing entry phase.
    var displayName: String
    /// The normalized endpoint the code promised (and the live host
    /// answered on for the key check).
    var urlString: String
    /// The host's X25519 public key, verified EQUAL to the live
    /// `GET /host-key` answer before this draft existed.
    var hostKeyB64: String
    /// Human-comparable fingerprint of that key.
    var fingerprint: String
    /// The code's deadline (unix seconds) — the whole mint → redeem →
    /// approve flow shares one.
    var expiresTs: UInt64
    /// The code's frozen scope. Always `read_tail`: pairing is read-only.
    var scope: String

    var phase: Phase

    /// The flow's explicit states. `failed` and `interrupted` are terminal
    /// for THIS code: recovery is a freshly minted code — never an empty
    /// board, a silent success, or a half-paired profile.
    enum Phase: Equatable {
        /// Identity + read-only scope on screen; nothing sent to the host.
        case reviewing
        /// Redeemed; waiting for the host owner's explicit approval.
        case waiting
        /// Explicit, actionable failure text.
        case failed(String)
        /// The wait was stopped before approval: nothing was paired.
        case interrupted
    }
}

/// Scan-time rejections. Every case states the recovery, and every message
/// is composed locally: a scanned payload (or a host answer) can never be
/// echoed into the UI, and no case carries the redemption code.
enum EnrollmentScanRejection: Error, Equatable, LocalizedError {
    /// The scanned text is not a v1 enrollment payload.
    case malformedCode
    /// The payload's endpoint is unusable as a Corral host URL.
    case unusableEndpoint
    /// `GET /host-key` could not be reached.
    case unreachableHost
    /// The live host answered with an unusable identity key.
    case hostKeyUnusable
    /// The code's host key does not match the host's LIVE key.
    case hostKeyMismatch
    /// A pinned profile already holds this host key (duplicate host).
    case duplicateHost(String)
    /// The endpoint is paired with a DIFFERENT key (reinstall/rotation).
    case changedHostKey(String)
    /// Host profiles are unavailable on this device.
    case storeUnavailable
    /// The device key could not be loaded.
    case deviceKeyUnavailable
    /// The host never approved before the code's deadline.
    case approvalTimedOut
    /// The owner stopped the wait before approval.
    case interrupted
    /// The pairing was superseded by another Add Host action.
    case superseded

    var errorDescription: String? { message }

    var message: String {
        switch self {
        case .malformedCode:
            return "That QR is not a Corral host code. Scan the code the host minted for this device."
        case .unusableEndpoint:
            return "That code points at an address Corral cannot use — mint a fresh code on the host."
        case .unreachableHost:
            return "Could not reach the host to verify it — check it is online and on the same network, then scan again."
        case .hostKeyUnusable:
            return "The host answered with an unusable identity key — pairing stopped."
        case .hostKeyMismatch:
            return "The code's host key does not match the host's live key. The code may be stale or tampered with — do not pair; mint a fresh code on the host."
        case .duplicateHost(let name):
            return "\(name) is already paired with that host key — there is nothing to add."
        case .changedHostKey(let name):
            return "\(name) is already paired at that address with a different key. If the host was reinstalled or its key rotated, remove that host entry and scan a fresh code."
        case .storeUnavailable:
            return "Host profiles are unavailable on this device — pairing stopped."
        case .deviceKeyUnavailable:
            return "This device's key could not be loaded — pairing stopped."
        case .approvalTimedOut:
            return "The host did not approve this device before the code expired. Mint a fresh code on the host and scan again."
        case .interrupted:
            return "Pairing stopped before the host approved — nothing was paired. Mint a fresh code and scan again."
        case .superseded:
            return "Pairing was superseded by another Add Host action — nothing was paired."
        }
    }
}

extension EnrollmentApprovalDenial {
    /// User-facing, actionable text for one approval denial. Composed
    /// locally: the wire module's denial payloads are never rendered.
    var enrollmentMessage: String {
        switch self {
        case .notRedeemed:
            return "The host never saw this device's request — mint a fresh code and scan again."
        case .alreadyProbed:
            return "Pairing recovery already ran once for this code — mint a fresh code and scan again."
        case .revoked:
            return "The host reports this device's registration as revoked. Ask the host owner to approve it again (fresh code → approve), then scan the new code."
        case .missingReadTailGrant:
            return "The host approved this device without the read-only grant Corral needs — nothing was paired. Ask the host owner to approve the read_tail scope."
        case .refused:
            return "The host refused the recovery read — pairing did not complete. Mint a fresh code and scan again."
        case .transport:
            return "The host could not be reached while pairing — check it is online, then mint a fresh code and scan again."
        }
    }
}

extension EnrollmentClientError {
    /// User-facing, actionable text for one enrollment-client failure.
    /// `.server` failures carry only the wire module's locally composed
    /// text (never host-supplied text); transport and malformed answers are
    /// replaced with local copy here.
    var enrollmentMessage: String {
        switch self {
        case .transport:
            return EnrollmentScanRejection.unreachableHost.message
        case .server(_, let code, let message):
            switch code {
            case "enroll_unknown_code":
                return "The host does not know this code — it may have been minted on another host. Mint a fresh code on this host."
            case "enroll_redeemed":
                return "That code was already used — mint a fresh code on the host."
            case "enroll_expired":
                return "That code expired — mint a fresh code on the host."
            case "already_registered":
                return "This device key is already registered on that host. If it is not paired in Corral anymore, ask the host owner to revoke the old registration, then mint a fresh code."
            case "bad_public_key":
                return "The host rejected this device's key — pairing stopped."
            case "bad_name":
                return "The host rejected the host name — rename it and try again."
            case "malformed_request":
                return "The host rejected the pairing request as malformed — pairing stopped."
            default:
                return message
            }
        case .malformedResponse:
            return "The host answered with an unrecognized pairing response — pairing stopped."
        case .unexpectedState:
            return "The host answered with an unrecognized enrollment state — pairing stopped."
        }
    }
}
