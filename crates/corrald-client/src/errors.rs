//! Client-side error model for the frozen corrald HTTP surface.
//!
//! The drive endpoint maps refusals onto status + a typed `kind` in a JSON
//! body `{kind, message, request_id?}`. This module classifies the full
//! matrix exhaustively (the reviewer-visible piece: every status/kind pair
//! the daemon can emit has a variant). Unknown kinds (a daemon that grew a
//! new refusal) still surface with their status + message — never lost to a
//! catch-all decode failure.
//!
//! The register/step-up/grants/audit endpoints use a plain `{error: ...}`
//! body; those map to [`ApiError::Plain`].

use std::fmt;

use serde_json::Value;

/// Every typed drive refusal the daemon emits (P4-conformance.md
/// normative list), keyed by (status, kind) in [`classify_drive_kind`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DriveErrorKind {
    // 400
    BadRequest,
    UnknownCapability,
    MissingSignature,
    // 401
    BadSignature,
    StepUpFailed,
    // 404
    UnknownKey,
    UnknownAgent,
    // 403
    Expired,
    Revoked,
    NotGranted,
    StepUpRequired,
    // 409
    InFlight,
    StaleAgent,
    NoWaitingApproval,
    StaleApproval,
    HashMismatch,
    // 422
    Payload,
    ChoiceNotInMenu,
    CannotApproveKind,
}

impl fmt::Display for DriveErrorKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.wire_name())
    }
}

impl DriveErrorKind {
    pub fn wire_name(&self) -> &'static str {
        match self {
            Self::BadRequest => "bad_request",
            Self::UnknownCapability => "unknown_capability",
            Self::MissingSignature => "missing_signature",
            Self::BadSignature => "bad_signature",
            Self::StepUpFailed => "step_up_failed",
            Self::UnknownKey => "unknown_key",
            Self::UnknownAgent => "unknown_agent",
            Self::Expired => "expired",
            Self::Revoked => "revoked",
            Self::NotGranted => "not_granted",
            Self::StepUpRequired => "step_up_required",
            Self::InFlight => "in_flight",
            Self::StaleAgent => "stale_agent",
            Self::NoWaitingApproval => "no_waiting_approval",
            Self::StaleApproval => "stale_approval",
            Self::HashMismatch => "hash_mismatch",
            Self::Payload => "payload",
            Self::ChoiceNotInMenu => "choice_not_in_menu",
            Self::CannotApproveKind => "cannot_approve_kind",
        }
    }
}

/// Classify a `(status, kind)` pair into the typed refusal. Exhaustive over
/// the normative matrix; anything else is `None` (caller keeps the raw
/// message — a daemon newer than this client).
pub fn classify_drive_kind(status: u16, kind: &str) -> Option<DriveErrorKind> {
    use DriveErrorKind as K;
    Some(match (status, kind) {
        (400, "bad_request") => K::BadRequest,
        (400, "unknown_capability") => K::UnknownCapability,
        (400, "missing_signature") => K::MissingSignature,
        (401, "bad_signature") => K::BadSignature,
        (401, "step_up_failed") => K::StepUpFailed,
        (404, "unknown_key") => K::UnknownKey,
        (404, "unknown_agent") => K::UnknownAgent,
        (403, "expired") => K::Expired,
        (403, "revoked") => K::Revoked,
        (403, "not_granted") => K::NotGranted,
        (403, "step_up_required") => K::StepUpRequired,
        (409, "in_flight") => K::InFlight,
        (409, "stale_agent") => K::StaleAgent,
        (409, "no_waiting_approval") => K::NoWaitingApproval,
        (409, "stale_approval") => K::StaleApproval,
        (409, "hash_mismatch") => K::HashMismatch,
        (422, "payload") => K::Payload,
        (422, "choice_not_in_menu") => K::ChoiceNotInMenu,
        (422, "cannot_approve_kind") => K::CannotApproveKind,
        _ => return None,
    })
}

/// A typed drive refusal: status + kind + daemon message + request_id (when
/// the daemon echoed it).
#[derive(Debug, Clone)]
pub struct DriveRefusal {
    pub status: reqwest::StatusCode,
    /// `None` when the body had no recognizable `kind` (e.g. a plain
    /// `{error: ...}` or a future refusal kind).
    pub kind: Option<DriveErrorKind>,
    pub message: String,
    pub request_id: Option<String>,
}

impl fmt::Display for DriveRefusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match (&self.kind, &self.request_id) {
            (Some(kind), Some(rid)) => {
                write!(f, "drive refused: {} ({}) [{}]", kind, self.message, rid)
            }
            (Some(kind), None) => write!(f, "drive refused: {} ({})", kind, self.message),
            (None, Some(rid)) => {
                write!(
                    f,
                    "drive refused: HTTP {} ({}) [{}]",
                    self.status, self.message, rid
                )
            }
            (None, None) => write!(f, "drive refused: HTTP {} ({})", self.status, self.message),
        }
    }
}

impl std::error::Error for DriveRefusal {}

/// Every client-side failure mode of the crate.
#[derive(Debug)]
pub enum ApiError {
    /// HTTP-level failure (connect refused, timeout, TLS, ...). The `status`
    /// field is set when the server answered with a non-2xx we did not
    /// classify further.
    Transport(reqwest::Error),
    /// The server answered non-2xx with a typed drive body.
    Drive(DriveRefusal),
    /// The server answered non-2xx with a plain `{error: ...}` body
    /// (register/step-up/grants/audit endpoints).
    Plain {
        status: reqwest::StatusCode,
        error: String,
    },
    /// A step-up-gated retry hit `step_up_failed`. Per the daemon's
    /// ordering (step-up gate before the replay claim), an earlier attempt
    /// of this retry loop consumed the token — the write may already have
    /// dispatched. The outcome is unknown to this client; callers should
    /// re-read the snapshot/rev rather than resubmit.
    AmbiguousWrite { request_id: String },
    /// The server answered 2xx but the body did not decode.
    Decode(serde_json::Error),
    /// Base URL was not a valid `http(s)://` URL.
    Url(String),
}

impl fmt::Display for ApiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Transport(e) => write!(f, "transport: {e}"),
            Self::Drive(refusal) => write!(f, "{refusal}"),
            Self::Plain { status, error } => {
                write!(f, "HTTP {status}: {error}")
            }
            Self::AmbiguousWrite { request_id } => write!(
                f,
                "ambiguous write outcome for {request_id}: an earlier attempt may have dispatched"
            ),
            Self::Decode(e) => write!(f, "decode: {e}"),
            Self::Url(e) => write!(f, "bad base url: {e}"),
        }
    }
}

impl std::error::Error for ApiError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Transport(e) => Some(e),
            Self::Decode(e) => Some(e),
            _ => None,
        }
    }
}

impl From<reqwest::Error> for ApiError {
    fn from(e: reqwest::Error) -> Self {
        Self::Transport(e)
    }
}

impl From<serde_json::Error> for ApiError {
    fn from(e: serde_json::Error) -> Self {
        Self::Decode(e)
    }
}

/// Parse a drive-endpoint error body `{kind, message, request_id?}`.
/// Falls back to a plain `{error: ...}` shape when `kind` is absent.
pub fn parse_drive_refusal(status: reqwest::StatusCode, body: &[u8]) -> DriveRefusal {
    let parsed: Option<Value> = serde_json::from_slice(body).ok();
    let kind_str = parsed
        .as_ref()
        .and_then(|v| v.get("kind"))
        .and_then(Value::as_str);
    let message = parsed
        .as_ref()
        .and_then(|v| v.get("message"))
        .and_then(Value::as_str)
        .or_else(|| {
            parsed
                .as_ref()
                .and_then(|v| v.get("error"))
                .and_then(Value::as_str)
        })
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| String::from_utf8_lossy(body).into_owned());
    let request_id = parsed
        .as_ref()
        .and_then(|v| v.get("request_id"))
        .and_then(Value::as_str)
        .map(ToOwned::to_owned);
    let kind = kind_str.and_then(|k| classify_drive_kind(status.as_u16(), k));
    DriveRefusal {
        status,
        kind,
        message,
        request_id,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_matrix_classifies() {
        for (status, kind) in [
            (400, "bad_request"),
            (400, "unknown_capability"),
            (400, "missing_signature"),
            (401, "bad_signature"),
            (401, "step_up_failed"),
            (404, "unknown_key"),
            (404, "unknown_agent"),
            (403, "expired"),
            (403, "revoked"),
            (403, "not_granted"),
            (403, "step_up_required"),
            (409, "in_flight"),
            (409, "stale_agent"),
            (409, "no_waiting_approval"),
            (409, "stale_approval"),
            (409, "hash_mismatch"),
            (422, "payload"),
            (422, "choice_not_in_menu"),
            (422, "cannot_approve_kind"),
        ] {
            let got = classify_drive_kind(status, kind).expect("classified");
            assert_eq!(got.wire_name(), kind, "wire name must echo the kind");
        }
    }

    #[test]
    fn wrong_status_for_kind_is_not_classified() {
        assert_eq!(classify_drive_kind(401, "bad_request"), None);
        assert_eq!(classify_drive_kind(403, "hash_mismatch"), None);
        assert_eq!(classify_drive_kind(500, "internal"), None);
        assert_eq!(classify_drive_kind(200, "bad_request"), None);
    }

    #[test]
    fn refusal_parses_both_body_shapes() {
        let typed = parse_drive_refusal(
            reqwest::StatusCode::FORBIDDEN,
            br#"{"kind":"step_up_required","message":"destructive payload needs a step-up token","request_id":"r-9"}"#,
        );
        assert_eq!(typed.kind, Some(DriveErrorKind::StepUpRequired));
        assert_eq!(typed.request_id.as_deref(), Some("r-9"));

        let plain = parse_drive_refusal(
            reqwest::StatusCode::UNAUTHORIZED,
            br#"{"error":"bad registration token"}"#,
        );
        assert_eq!(typed.request_id.as_deref(), Some("r-9"));
        assert_eq!(plain.kind, None);
        assert_eq!(plain.message, "bad registration token");
        assert!(plain.request_id.is_none());
    }
}
