//! #63 (D35 slice 2): `GET /transcript` — grant-gated, on-demand
//! transcript pages over the slice-1 readers.
//!
//! ## Auth: the drive plane's trust decision, on a GET
//!
//! Transcripts are gated by the `read_tail` grant — same [`Capability`],
//! same [`DriveAuthorizer::verify`](crate::drive::DriveAuthorizer::verify)
//! call, same device registry, host-key surface unchanged. This is a
//! DELIBERATE, RECORDED scope decision (see "Grant scope" in
//! docs/OPERATIONS.md): a transcript page is strictly more than the D5
//! tail bound the grant originally covered, and every already-issued
//! `read_tail` device gains it — operators who granted `read_tail` under
//! the old meaning must re-review their grants. The issue spec (#63)
//! mandates this shape ("requires the `read_tail` grant — same trust
//! decision"); the alternative (a new `read_transcript` capability) is
//! recorded in the review file for the merge decision.
//!
//! There is no grant-gated GET precedent in this API, so the envelope
//! rides a header: the client puts the exact `SignedDrive` JSON it would
//! POST to `/drive` (capability `read_tail`, `target` = the agent id)
//! into [`TRANSCRIPT_AUTH_HEADER`]. Differences from `/drive`, each
//! deliberate:
//! - no replay-table claim. Honestly stated consequence (review): a
//!   captured header can be replayed for LIVE current content until the
//!   key expires or is revoked (checked per call) — unlike a replayed
//!   `/drive` `read_tail`, which returns the cached original response.
//!   The mitigations are revocation, the 90-day key TTL, and the audit
//!   trail below.
//! - no step-up: reads are never destructive.
//! - every SERVED page appends an [`AuditEntry`] (capability
//!   `read_tail`, the agent as target, outcome `Executed`) — the deepest
//!   read surface must not be the invisible one (review F3). Auth
//!   FAILURES are still never audited (AC5).
//!
//! Responses (success and error) carry `Cache-Control: no-store` and
//! `Vary: x-corral-drive` (review F4): this is the API's first
//! access-controlled GET, and a heuristically-cacheable 200 would let an
//! intermediary serve one device's transcript to a credential-less
//! second caller.
//!
//! ## Flow
//!
//! agent id → [`Store`] lookup → [`bind_agent`] (exact session-id rung
//! first, then the tool-restricted worktree fallback) → cursor decode
//! AGAINST the bound store (the wire cursor is fingerprinted — a rebind
//! between pages is a typed `bad_cursor`, never a silent continuation) →
//! [`read_page`]. Ambiguity returns the CANDIDATE LIST, never a guess.
//! The success body names the bound session (`session` label) and any
//! store that could not be consulted (`stores_unavailable`), so a client
//! can pin the bind and tell a complete answer from a partial one.
//! Entries arrive already redacted (D-083 inside the transcript module).
//!
//! This is an on-demand VIEW fetch — explicitly not in conflict with
//! D5's bounded-push rule: nothing here is pushed, and the phone client
//! does not call it in this phase (D16: phone stays bounded-tail only).

use std::sync::Arc;

use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Json, Response};
use serde::Deserialize;

use crate::core::util::now_millis;
use crate::drive::{AuditEntry, AuditLog as _, AuditOutcome, AuthError, Capability, SignedDrive};
use crate::transcript::bind::{BindError, Candidate, bind_agent};
use crate::transcript::{Cursor, TranscriptError, read_page};

use super::AppState;

/// Header carrying the signed envelope JSON (the same `SignedDrive` wire
/// shape `/drive` takes as its body).
pub const TRANSCRIPT_AUTH_HEADER: &str = "x-corral-drive";

/// `GET /transcript` query parameters. Everything is a string here and
/// parsed by hand in the handler (review F12): an extractor-level
/// rejection would bypass the `{kind, message}` error contract with
/// axum's plaintext 400.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct TranscriptQuery {
    pub agent: Option<String>,
    pub cursor: Option<String>,
    pub limit: Option<String>,
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

/// Both headers on EVERY response from this endpoint (review F4): the
/// body is access-controlled, so no intermediary may cache it, and the
/// credential lives in a header that is not part of the default cache
/// key.
fn with_no_store(mut response: Response) -> Response {
    let headers = response.headers_mut();
    headers.insert(header::CACHE_CONTROL, "no-store".parse().expect("static"));
    headers.insert(
        header::VARY,
        TRANSCRIPT_AUTH_HEADER.parse().expect("static"),
    );
    response
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
        with_no_store((status, Json(body)).into_response())
    }
}

/// Parse + verify the signed envelope from the auth header. The envelope
/// must carry the `read_tail` capability and name the queried agent as
/// its `target` — a signature minted for one agent cannot page another.
fn authorize(
    state: &AppState,
    headers: &HeaderMap,
    agent_id: &str,
) -> Result<SignedDrive, TranscriptApiError> {
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
    Ok(signed)
}

pub async fn transcript(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(params): Query<TranscriptQuery>,
) -> Response {
    match serve(&state, &headers, params).await {
        Ok(body) => with_no_store(Json(body).into_response()),
        Err(error) => error.into_response(),
    }
}

async fn serve(
    state: &AppState,
    headers: &HeaderMap,
    params: TranscriptQuery,
) -> Result<serde_json::Value, TranscriptApiError> {
    let agent_id = match params.agent.as_deref() {
        Some(agent) if !agent.is_empty() => agent.to_string(),
        _ => {
            return Err(TranscriptApiError::BadRequest {
                message: "agent must not be empty".to_string(),
            });
        }
    };
    let signed = authorize(state, headers, &agent_id)?;
    // limit parsed by hand (review F12) so a bad value keeps the JSON
    // error contract; the module clamps it into 1..=MAX_PAGE_ENTRIES.
    let limit = match params.limit.as_deref() {
        None => crate::transcript::MAX_PAGE_ENTRIES,
        Some(raw) => raw.parse().map_err(|_| TranscriptApiError::BadRequest {
            message: format!("limit must be a non-negative integer, got {raw:?}"),
        })?,
    };

    let agent =
        state
            .store
            .get(&agent_id)
            .await
            .ok_or_else(|| TranscriptApiError::UnknownAgent {
                agent_id: agent_id.clone(),
            })?;
    let Some(worktree) = agent.workspace.worktree_path.clone() else {
        return Err(TranscriptApiError::NoSession {
            worktree: "(agent has no known worktree)".to_string(),
        });
    };

    let outcome = bind_agent(&agent_id, &agent.tool, &worktree, &state.transcript_roots)
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

    // Cursor decode happens AFTER binding, against the bound store: the
    // wire cursor is fingerprinted (review F5), so a cursor issued for a
    // different session's file is a typed bad_cursor here.
    let cursor = params
        .cursor
        .as_deref()
        .map(|wire| Cursor::decode_for(wire, &outcome.store))
        .transpose()
        .map_err(|error| TranscriptApiError::Read { error })?;

    let page = read_page(&outcome.store, cursor.as_ref(), limit)
        .await
        .map_err(|error| TranscriptApiError::Read { error })?;

    // Review F3: the deepest read surface in the product must leave a
    // trace. One entry per SERVED page, same shape as drive's read_tail
    // audits; failures above never reach this line (AC5 holds).
    let audit_entry = AuditEntry {
        ts: now_millis(),
        key_id: signed.key_id.clone(),
        request_id: signed.envelope.request_id.clone(),
        capability: format!("{}", Capability::ReadTail),
        target: agent_id.clone(),
        outcome: AuditOutcome::Executed,
    };
    if let Err(error) = state.auth.audit.append(&audit_entry) {
        tracing::warn!(
            request_id = %audit_entry.request_id,
            error = %error,
            "transcript audit append failed; the page was already read"
        );
    }

    let session_label = Candidate {
        store: outcome.store.clone(),
        recency_ms: 0,
    }
    .label();
    let store_kind = match &outcome.store {
        crate::transcript::StoreRef::Opencode { .. } => "opencode",
        crate::transcript::StoreRef::Claude { .. } => "claude",
        crate::transcript::StoreRef::Codex { .. } => "codex",
    };
    Ok(serde_json::json!({
        "agent": agent_id,
        "store": store_kind,
        // Review F15: the bound session's identity, so a client can
        // detect a rebind or a wrong bind instead of trusting silence.
        "session": session_label,
        // Review F9: store kinds that errored during binding — a
        // complete-looking answer must not hide a store we could not ask.
        "stores_unavailable": outcome.unavailable,
        "entries": page
            .entries
            .iter()
            .map(|e| serde_json::json!({ "role": e.role, "text": e.text, "ts": e.ts }))
            .collect::<Vec<_>>(),
        "next_cursor": page.next_cursor.as_ref().map(|c| c.encode_for(&outcome.store)),
        "skipped": page.skipped,
    }))
}
