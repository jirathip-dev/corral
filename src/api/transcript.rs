//! #63 (D35 slice 2): `GET /transcript` — grant-gated, on-demand
//! transcript pages over the slice-1 readers.
//!
//! ## Auth: the drive plane's trust decision, on a GET
//!
//! Transcripts are a superset of tail content, so the gate is the SAME
//! grant the drive plane's `read_tail` uses — same [`Capability`], same
//! [`DriveAuthorizer::verify`](crate::drive::DriveAuthorizer::verify)
//! call, same device registry, host-key surface unchanged. There is no
//! grant-gated GET precedent in this API, so the envelope rides a header:
//! the client puts the exact `SignedDrive` JSON it would POST to `/drive`
//! (capability `read_tail`, `target` = the agent id) into
//! [`TRANSCRIPT_AUTH_HEADER`]. Two deliberate differences from `/drive`,
//! both consequences of this being an idempotent READ:
//! - no replay-table claim: replaying a read returns the same page and
//!   mutates nothing, so `request_id` idempotency buys nothing here;
//! - no step-up: reads are never destructive.
//!
//! ## Flow
//!
//! agent id → [`Store`] lookup → `workspace.worktree_path` →
//! [`bind_worktree`] → [`read_page`]. Every failure is typed and mapped
//! to a status below; ambiguity returns the CANDIDATE LIST, never a
//! guess. Entries arrive already redacted (D-083 happens inside the
//! transcript module boundary); this handler adds no text of its own.
//!
//! This is an on-demand VIEW fetch — explicitly not in conflict with
//! D5's bounded-push rule: nothing here is pushed, and the phone client
//! does not call it in this phase (D16: phone stays bounded-tail only).

use std::sync::Arc;

use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Json, Response};
use serde::Deserialize;

use crate::drive::{AuthError, Capability, SignedDrive};
use crate::transcript::bind::{BindError, Candidate, bind_worktree};
use crate::transcript::{Cursor, TranscriptError, read_page};

use super::AppState;

/// Header carrying the signed envelope JSON (the same `SignedDrive` wire
/// shape `/drive` takes as its body).
pub const TRANSCRIPT_AUTH_HEADER: &str = "x-corral-drive";

/// `GET /transcript` query parameters. `cursor` is the opaque string from
/// a previous page's `next_cursor`; `limit` is capped by the module's
/// page caps regardless of what is asked.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct TranscriptQuery {
    pub agent: String,
    pub cursor: Option<String>,
    pub limit: Option<usize>,
}

#[derive(Debug)]
pub enum TranscriptApiError {
    BadRequest {
        message: String,
    },
    Auth {
        error: AuthError,
    },
    UnknownAgent {
        agent_id: String,
    },
    NoSession {
        worktree: String,
    },
    Ambiguous {
        worktree: String,
        candidates: Vec<Candidate>,
    },
    Read {
        error: TranscriptError,
    },
}

impl IntoResponse for TranscriptApiError {
    fn into_response(self) -> Response {
        let (status, kind, message, candidates) = match self {
            Self::BadRequest { message } => (StatusCode::BAD_REQUEST, "bad_request", message, None),
            Self::Auth { error } => {
                // Same status/kind mapping as the drive plane (AC1): 401
                // bad signature, 404 unknown key, 403 the rest.
                let (status, kind) = match &error {
                    AuthError::MissingSignature => (StatusCode::BAD_REQUEST, "missing_signature"),
                    AuthError::BadSignature => (StatusCode::UNAUTHORIZED, "bad_signature"),
                    AuthError::UnknownKey => (StatusCode::NOT_FOUND, "unknown_key"),
                    AuthError::Expired => (StatusCode::FORBIDDEN, "expired"),
                    AuthError::Revoked => (StatusCode::FORBIDDEN, "revoked"),
                    AuthError::NotGranted(_) => (StatusCode::FORBIDDEN, "not_granted"),
                };
                (status, kind, error.to_string(), None)
            }
            Self::UnknownAgent { agent_id } => (
                StatusCode::NOT_FOUND,
                "unknown_agent",
                format!("unknown agent: {agent_id}"),
                None,
            ),
            Self::NoSession { worktree } => (
                StatusCode::NOT_FOUND,
                "no_session",
                format!("no session store found for worktree {worktree}"),
                None,
            ),
            Self::Ambiguous {
                worktree,
                candidates,
            } => (
                StatusCode::CONFLICT,
                "ambiguous_session",
                format!("more than one session matches worktree {worktree}"),
                Some(candidates),
            ),
            Self::Read { error } => {
                let (status, kind) = match &error {
                    TranscriptError::BadCursor => (StatusCode::BAD_REQUEST, "bad_cursor"),
                    TranscriptError::StoreUnreadable { .. } => {
                        (StatusCode::SERVICE_UNAVAILABLE, "store_unreadable")
                    }
                    TranscriptError::Sqlite3Unavailable => {
                        (StatusCode::SERVICE_UNAVAILABLE, "sqlite3_unavailable")
                    }
                    TranscriptError::QueryTimeout => {
                        (StatusCode::SERVICE_UNAVAILABLE, "query_timeout")
                    }
                    TranscriptError::StoreShape => (StatusCode::BAD_GATEWAY, "store_shape"),
                };
                (status, kind, error.to_string(), None)
            }
        };
        let mut body = serde_json::json!({ "kind": kind, "message": message });
        if let Some(candidates) = candidates {
            body["candidates"] = candidates
                .iter()
                .map(|c| {
                    serde_json::json!({
                        "label": c.label(),
                        "recency_ms": c.recency_ms,
                    })
                })
                .collect();
        }
        (status, Json(body)).into_response()
    }
}

/// Parse + verify the signed envelope from the auth header. The envelope
/// must carry the `read_tail` capability and name the queried agent as
/// its `target` — a signature minted for one agent cannot page another.
fn authorize(
    state: &AppState,
    headers: &HeaderMap,
    agent_id: &str,
) -> Result<(), TranscriptApiError> {
    let Some(raw) = headers.get(TRANSCRIPT_AUTH_HEADER) else {
        return Err(TranscriptApiError::Auth {
            error: AuthError::MissingSignature,
        });
    };
    let raw = raw.to_str().map_err(|_| TranscriptApiError::BadRequest {
        message: format!("{TRANSCRIPT_AUTH_HEADER} is not valid UTF-8"),
    })?;
    let signed: SignedDrive =
        serde_json::from_str(raw).map_err(|error| TranscriptApiError::BadRequest {
            message: format!("{TRANSCRIPT_AUTH_HEADER}: {error}"),
        })?;
    if signed.envelope.capability != Capability::ReadTail {
        return Err(TranscriptApiError::BadRequest {
            message: format!(
                "capability must be {} for /transcript, got {}",
                Capability::ReadTail,
                signed.envelope.capability
            ),
        });
    }
    if signed.envelope.target != agent_id {
        return Err(TranscriptApiError::BadRequest {
            message: format!(
                "envelope target {} does not match ?agent={agent_id}",
                signed.envelope.target
            ),
        });
    }
    state
        .auth
        .authorizer
        .verify(&signed)
        .map_err(|error| TranscriptApiError::Auth { error })?;
    Ok(())
}

pub async fn transcript(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(params): Query<TranscriptQuery>,
) -> Result<Json<serde_json::Value>, TranscriptApiError> {
    if params.agent.is_empty() {
        return Err(TranscriptApiError::BadRequest {
            message: "agent must not be empty".to_string(),
        });
    }
    authorize(&state, &headers, &params.agent)?;

    // Decode the cursor BEFORE the (possibly slow) binding pass: a
    // malformed cursor is deterministic and cheap to refuse.
    let cursor = params
        .cursor
        .as_deref()
        .map(Cursor::decode)
        .transpose()
        .map_err(|error| TranscriptApiError::Read { error })?;

    let agent =
        state
            .store
            .get(&params.agent)
            .await
            .ok_or_else(|| TranscriptApiError::UnknownAgent {
                agent_id: params.agent.clone(),
            })?;
    let Some(worktree) = agent.workspace.worktree_path.clone() else {
        return Err(TranscriptApiError::NoSession {
            worktree: "(agent has no known worktree)".to_string(),
        });
    };

    let store_ref = bind_worktree(&worktree, &state.transcript_roots)
        .await
        .map_err(|error| match error {
            BindError::NoSession { worktree } => TranscriptApiError::NoSession { worktree },
            BindError::Ambiguous {
                worktree,
                candidates,
            } => TranscriptApiError::Ambiguous {
                worktree,
                candidates,
            },
            BindError::Store(error) => TranscriptApiError::Read { error },
        })?;

    let limit = params.limit.unwrap_or(crate::transcript::MAX_PAGE_ENTRIES);
    let page = read_page(&store_ref, cursor.as_ref(), limit)
        .await
        .map_err(|error| TranscriptApiError::Read { error })?;

    let store_kind = match &store_ref {
        crate::transcript::StoreRef::Opencode { .. } => "opencode",
        crate::transcript::StoreRef::Claude { .. } => "claude",
        crate::transcript::StoreRef::Codex { .. } => "codex",
    };
    Ok(Json(serde_json::json!({
        "agent": params.agent,
        "store": store_kind,
        "entries": page
            .entries
            .iter()
            .map(|e| serde_json::json!({ "role": e.role, "text": e.text, "ts": e.ts }))
            .collect::<Vec<_>>(),
        "next_cursor": page.next_cursor.as_ref().map(Cursor::encode),
        "skipped": page.skipped,
    })))
}
