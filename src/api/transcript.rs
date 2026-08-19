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
//! - replay-BOUNDED, not replay-table'd (fresh review F3): the envelope
//!   payload is transcript-scoped (`{ts, cursor, limit}`) and the
//!   signature covers the whole envelope — [`DriveEnvelope::payload`](crate::drive::DriveEnvelope::payload)
//!   is inside [`canonical_envelope_bytes`](crate::drive::canonical_envelope_bytes) —
//!   so ONE signature buys exactly ONE page, and only within the 60s
//!   `|now - ts|` window the step-up / device-token handlers already
//!   enforce. A captured header replays that one page at most, for 60
//!   seconds, never live current content of the still-growing history
//!   and never a different page (there is no URL knob: `cursor`/`limit`
//!   query params are refused outright). Paging means re-signing per
//!   page with the new cursor. The remaining mitigations are revocation
//!   (checked per call), the 90-day key TTL, and the audit trail below.
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
//! header signature + freshness + signed `{ts, cursor, limit}` payload →
//! agent id → [`Store`] lookup → [`bind_agent`] (exact session-id rung
//! first, then the tool-restricted worktree fallback; memoized per agent
//! for the life of a page sequence — fresh review F5) → cursor decode
//! AGAINST the bound store (the wire cursor is fingerprinted, so it can
//! only ever resume in the file that minted it — never a silent
//! continuation in another) → [`read_page`]. Ambiguity returns the
//! CANDIDATE LIST, never a guess. The success body names the bound
//! session (`session` label) and any store that could not be consulted
//! (`stores_unavailable`), so a client can pin the bind and tell a
//! complete answer from a partial one. Entries arrive already redacted
//! (D-083 inside the transcript module).
//!
//! This is an on-demand VIEW fetch — explicitly not in conflict with
//! D5's bounded-push rule: nothing here is pushed, and the phone client
//! does not call it in this phase (D16: phone stays bounded-tail only).

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;

use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode, header};
use axum::response::{IntoResponse, Json, Response};
use serde::{Deserialize, Serialize};

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
    // cursor/limit are NOT part of the URL contract anymore (fresh
    // review F3): they live in the SIGNED envelope payload, so the
    // fields are only kept so the handler can refuse them with a typed
    // 400 instead of silently ignoring a page-parameter attempt.
    pub cursor: Option<String>,
    pub limit: Option<String>,
}

/// Freshness window for the signed `ts` (fresh review F3) — the SAME 60s
/// `|now - ts|` skew the step-up and device-token handlers enforce
/// (`src/api/mod.rs` [`super::DEVICE_TOKEN_MAX_SKEW_SECS`]). A captured
/// header stops being replayable 60 seconds after it was signed, in
/// both clock directions.
const TRANSCRIPT_MAX_SKEW_SECS: u64 = super::DEVICE_TOKEN_MAX_SKEW_SECS;

/// The transcript-scoped envelope payload (fresh review F3). The client
/// signs this INSIDE the normal `x-corral-drive` envelope; because
/// `DriveEnvelope::payload` is covered by
/// [`canonical_envelope_bytes`](crate::drive::canonical_envelope_bytes),
/// the existing drive signature machinery already binds a signature to
/// exactly one page — no `DriveEnvelope` change, `/drive` untouched.
///
/// `ts` is unix seconds; `cursor`/`limit` are the page parameters that
/// used to be query strings. `limit` defaults to the module page cap;
/// `cursor` absent = the newest page.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct TranscriptPayload {
    pub ts: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u64>,
}

fn parse_transcript_payload(
    payload: &serde_json::Value,
) -> Result<TranscriptPayload, TranscriptApiError> {
    serde_json::from_value(payload.clone()).map_err(|error| TranscriptApiError::BadRequest {
        message: format!(
            "signed envelope payload must be {{\"ts\": <unix secs>, \"cursor\": <opaque|absent>, \"limit\": <1..=50|absent>}}: {error}"
        ),
    })
}

/// Bind memoization (fresh review F5, efficiency half). Paging an agent
/// costs one full bind (up to three store scans and several sqlite3
/// spawns) per page WITHOUT this — a codex page-2 re-walks
/// `~/.codex/sessions`. The first page of a sequence already pays for
/// the bind; later pages reuse it.
///
/// "Life of a page sequence" is defined by the fingerprint gate, not a
/// wall clock: a cursor is only ever valid for the store that minted it
/// (the wire cursor carries [`store_fingerprint`](crate::transcript::store_fingerprint)),
/// so a memo entry is reused ONLY when the request's cursor fingerprint
/// matches the memoized store — a match IS the store-identity check, and
/// the cursor can then never read a different file. A cursor-less
/// request always re-binds (the bind must stay current — a new session
/// can become newest between sequences) and refreshes the entry; a
/// fingerprint mismatch falls through to a fresh bind and the usual
/// `bad_cursor` path.
///
/// Bounded (fresh review F5 — memoization must not be a DoS vector
/// either): FIFO cap of [`BIND_MEMO_MAX_ENTRIES`] entries; overflow
/// evicts the oldest. Entries hold a bind OUTCOME (store paths, rung,
/// `stores_unavailable`) — never transcript content.
const BIND_MEMO_MAX_ENTRIES: usize = 64;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct BindMemoKey {
    agent_id: String,
    tool: String,
    worktree: String,
}

#[derive(Debug, Default)]
struct BindMemoInner {
    map: HashMap<BindMemoKey, crate::transcript::bind::BindOutcome>,
    order: VecDeque<BindMemoKey>,
}

fn bind_memo() -> &'static std::sync::Mutex<BindMemoInner> {
    static MEMO: std::sync::OnceLock<std::sync::Mutex<BindMemoInner>> = std::sync::OnceLock::new();
    MEMO.get_or_init(|| std::sync::Mutex::new(BindMemoInner::default()))
}

/// Reuse the memoized bind only when the wire cursor's fingerprint
/// matches the memoized store (see [`bind_memo`] for why that is the
/// whole identity check). Otherwise fall through to a fresh bind.
fn memo_lookup(key: &BindMemoKey, wire: &str) -> Option<crate::transcript::bind::BindOutcome> {
    let memo = bind_memo()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let outcome = memo.map.get(key)?;
    Cursor::decode_for(wire, &outcome.store).ok()?;
    Some(outcome.clone())
}

/// Record a successful bind. FIFO-bounded: a NEW key past the cap
/// evicts the oldest; updating an existing key never grows the map.
fn memo_store(key: BindMemoKey, outcome: crate::transcript::bind::BindOutcome) {
    let mut memo = bind_memo()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    use std::collections::hash_map::Entry;
    match memo.map.entry(key) {
        Entry::Occupied(mut occupied) => {
            occupied.insert(outcome);
            return;
        }
        Entry::Vacant(vacant) => {
            let key = vacant.key().clone();
            vacant.insert(outcome);
            memo.order.push_back(key);
        }
    }
    while memo.order.len() > BIND_MEMO_MAX_ENTRIES {
        if let Some(oldest) = memo.order.pop_front() {
            memo.map.remove(&oldest);
        }
    }
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
                // Fresh review F2: `StoreUnreadable`'s Display embeds the
                // absolute host store path and (for opencode) up to 2KiB
                // of sqlite3 stderr. That diagnostic was written for the
                // operator's log and must not ride a 503 body to a remote
                // client — this is the first surface that serializes the
                // error onto the wire. Log the detail, send a generic
                // message. The other variants' Displays are static text.
                let message = match &error {
                    TranscriptError::StoreUnreadable { .. } => {
                        tracing::warn!(error = %error, "transcript store unreadable");
                        "the session store could not be read (detail in the daemon log)".to_string()
                    }
                    other => other.to_string(),
                };
                (status, kind, message, None)
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
    // Fresh review F4: the audit trail is this endpoint's stated replay
    // mitigation, so every entry must be correlatable to a request —
    // `/drive` already rejects an empty request_id, and this surface
    // additionally caps its length because the id is copied verbatim
    // into the hash-chained log, one entry per served page.
    if signed.envelope.request_id.is_empty() {
        return Err(TranscriptApiError::BadRequest {
            message: "request_id must not be empty".to_string(),
        });
    }
    if signed.envelope.request_id.len() > 128 {
        return Err(TranscriptApiError::BadRequest {
            message: "request_id must be at most 128 bytes".to_string(),
        });
    }
    state
        .auth
        .authorizer
        .verify(&signed)
        .map_err(|error| TranscriptApiError::Auth { error })?;
    Ok(signed)
}

/// Fresh review F5: cap on concurrent `/transcript` serves. One request
/// can run up to three blocking store scans plus several sqlite3
/// children on the tokio blocking pool the WHOLE daemon shares —
/// uncapped, a couple hundred concurrent requests (one captured header
/// suffices, finding 3) would saturate it for 10s stretches. The
/// per-scan `ScanBudget` bounds one walk, not N of them; this bounds N.
/// Process-wide static (the blocking pool it protects is process-wide).
/// Try-acquire: over-cap callers get an immediate 503 `busy`, never an
/// invisible queue. The efficiency half of F5 (one bind per page
/// sequence instead of one per page) is [`bind_memo`] — fingerprint
/// gated, FIFO-bounded, below.
const TRANSCRIPT_MAX_CONCURRENT: usize = 8;
static TRANSCRIPT_GATE: tokio::sync::Semaphore =
    tokio::sync::Semaphore::const_new(TRANSCRIPT_MAX_CONCURRENT);

pub async fn transcript(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(params): Query<TranscriptQuery>,
) -> Response {
    let _permit = match TRANSCRIPT_GATE.try_acquire() {
        Ok(permit) => permit,
        Err(_) => {
            let body = serde_json::json!({
                "kind": "busy",
                "message": "too many concurrent transcript reads; retry shortly",
            });
            return with_no_store((StatusCode::SERVICE_UNAVAILABLE, Json(body)).into_response());
        }
    };
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
    // Fresh review F3: cursor/limit are signed into the header payload —
    // a URL knob would let one captured header page an unbounded history
    // inside the freshness window. Refuse the old query strings outright
    // (typed, never a silent ignore).
    if params.cursor.is_some() || params.limit.is_some() {
        return Err(TranscriptApiError::BadRequest {
            message: "cursor and limit are part of the signed x-corral-drive header payload, not the query string — re-sign per page".to_string(),
        });
    }
    let signed = authorize(state, headers, &agent_id)?;
    // Fresh review F3: the signed, transcript-scoped payload is the page
    // authority. Parsed AFTER the signature verify (it is signed
    // content), with the same 60s freshness rule as /step-up and
    // /device-token — `|now - ts| > 60` means the header is a stale
    // capture, refused even though the signature itself is valid.
    let payload = parse_transcript_payload(&signed.envelope.payload)?;
    if super::now_secs().abs_diff(payload.ts) > TRANSCRIPT_MAX_SKEW_SECS {
        return Err(TranscriptApiError::BadRequest {
            message: format!(
                "stale transcript request: |now - ts| > {TRANSCRIPT_MAX_SKEW_SECS}s — re-sign with a fresh ts"
            ),
        });
    }
    // limit parsed from the SIGNED payload so a bad value keeps the JSON
    // error contract (review F12); the module clamps it into
    // 1..=MAX_PAGE_ENTRIES.
    let limit = match payload.limit {
        Some(raw) => usize::try_from(raw).unwrap_or(usize::MAX),
        None => crate::transcript::MAX_PAGE_ENTRIES,
    };
    // Fresh review F7: structural cursor validation (framing, prefixes,
    // hex) needs no store — run it BEFORE the bind (or memo hit) so a
    // malformed cursor is a cheap 400, not a full bind (up to three
    // store scans and several sqlite3 spawns) followed by a 400. The
    // fingerprint half still has to wait for the bound store, below.
    let cursor_wire = payload.cursor.as_deref();
    if let Some(wire) = cursor_wire {
        Cursor::validate_wire(wire).map_err(|error| TranscriptApiError::Read { error })?;
    }

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

    // Fresh review F5 (efficiency half): reuse the memoized bind when
    // this request's cursor was minted for it — cursor-bearing requests
    // are mid-sequence, so the bind is known and the store walk is
    // redundant. Cursor-less requests (sequence starts) always re-bind
    // so the bind stays current, then refresh the memo.
    let bind_key = BindMemoKey {
        agent_id: agent_id.clone(),
        tool: agent.tool.clone(),
        worktree: worktree.clone(),
    };
    let outcome = match cursor_wire.and_then(|wire| memo_lookup(&bind_key, wire)) {
        Some(memoized) => memoized,
        None => {
            let fresh = bind_agent(&agent_id, &agent.tool, &worktree, &state.transcript_roots)
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
            memo_store(bind_key, fresh.clone());
            fresh
        }
    };

    // Fingerprint verification happens AFTER binding, against the bound
    // store (review F5): a cursor issued for a different session's file
    // is a typed bad_cursor here. Structure was already validated above;
    // on the memo-hit path this is the second (cheap, pure-string)
    // parse, kept uniform so one code path does the fingerprint check.
    let cursor = cursor_wire
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
        // R4: distinguishable from a bounded /drive read_tail entry — an
        // operator must be able to tell "40 full-history pages" from "40
        // 32KiB tails". The capability field is a String; this is the
        // additive spelling (`<grant>:<surface>`).
        capability: format!("{}:transcript", Capability::ReadTail),
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
        // Review R1: which ladder rung answered — "session_id" is exact,
        // "worktree" is best-effort (same-tool co-residents without
        // session-id hints share that rung's candidate set).
        "bind": outcome.rung,
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
