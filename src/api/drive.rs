//! Drive plane (P3 W1): `POST /drive`.
//!
//! The write side of corrald: an authenticated, capability-gated command
//! endpoint over the P3 contract in [`crate::drive`]. Handler flow:
//!
//! 1. Deserialize the signed envelope. `capability` is read as a plain
//!    string first so unknown capability names surface as the typed
//!    `unknown_capability` refusal instead of a generic parse error; the
//!    typed envelope handed onward re-serializes to the same canonical
//!    bytes the signature covers (field order mirrors [`DriveEnvelope`]).
//! 2. [`DriveAuthorizer::verify`] gates every request. W3 implements the
//!    device-key verifier; W1 builds against the trait ([`StubAuthorizer`]
//!    is the loopback placeholder). Default deny: a key without the
//!    capability comes back `NotGranted` and is refused here. Auth failures
//!    are never audited (AC5: the audit grows only on writes).
//! 3. Payload parse: [`DrivePayload::parse`] for prompt/read_tail/approve;
//!    interrupt/kill/attach take no payload (`null` or `{}`). `read_tail`
//!    lines are clamped to [`READ_TAIL_MAX_LINES`] (D5: 200 lines /
//!    32 KiB, never prefetch).
//! 4. Idempotency claim on [`ReplayTable`], keyed by `request_id` (bounded,
//!    LRU-ish). The claim is atomic with the table lookup: exactly one
//!    caller ever dispatches for a given id, even under concurrent
//!    duplicates (the loser gets `409 in_flight` and can retry for the
//!    stored response). Replays return the first response byte-identical.
//! 5. Dispatch via [`Adapter::drive`] and await the source outcome. The
//!    adapter resolves the canonical `agent_id` to its own transport target —
//!    the daemon never sends keys by coordinates (D8), and W1 never sees pane
//!    ids. Exception: `read_tail` routes through
//!    [`Adapter::read_tail`], which returns the redacted, bounded tail so
//!    the response can carry `result.lines` (the one capability whose whole
//!    point is a response).
//! 6. Audit: [`AuditLog::append`] exactly once per dispatched write —
//!    success (`Executed`) or typed refusal at dispatch (`Refused` /
//!    `Failed`). An `append` failure is logged, never allowed to fail the
//!    response: the write already happened, and the replay entry keeps
//!    retries from re-sending.
//! 7. The response carries the store's new monotonic rev, captured after
//!    dispatch via [`Store::snapshot`]. The write path itself flows through
//!    the store (adapter events); W1 only reads the resulting rev.
//!
//! # HTTP model
//!
//! Pre-dispatch client errors are 4xx with a typed JSON body
//! `{kind, message, request_id?}`:
//!
//! - `400 bad_request` — malformed envelope, empty request_id/target.
//! - `400 unknown_capability` — capability string outside the contract.
//! - `422 payload` — payload shape does not match the capability.
//! - `403 auth` — signature/key/grant failure (incl. `NotGranted`).
//! - `409 in_flight` — same request_id currently being dispatched.
//!
//! Everything that reaches dispatch returns `200` with the contract's
//! [`DriveResponse`]; dispatch-level refusals (unknown agent, stale agent,
//! command not implemented, transport failure) are `ok: false` with a
//! human-readable `error` plus stable `error_kind` in that same body.
//! Storing and replaying the exact body keeps idempotent retries byte-identical.

use std::collections::hash_map::Entry as HashMapEntry;
use std::collections::{HashMap, VecDeque};
use std::future::Future;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::Json;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use serde::Serialize;
use tracing::warn;

use crate::adapters::{Adapter, DriveCommand, DriveError};
use crate::approve::{ApprovalError, check_approval_claim};
use crate::core::model::Agent;
use crate::core::store::Store;
use crate::core::util::now_millis;
use crate::drive::{
    AuditEntry, AuditLog, AuditOutcome, AuthError, AuthorizedDrive, Capability, DriveEnvelope,
    DrivePayload, DriveResponse, PayloadError, READ_TAIL_MAX_LINES, SignedDrive, UnknownCapability,
};

use super::AppState;

/// Read-path dispatch stub: refuses every drive command with a typed error.
/// Used by [`super::AppState::default`] so read-only construction needs no
/// adapter wiring.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoopAdapter;

impl Adapter for NoopAdapter {
    fn source(&self) -> &'static str {
        "noop"
    }

    fn start(self: Arc<Self>, _store: Store) {}

    fn drive<'a>(
        &'a self,
        _agent_id: &'a str,
        _command: DriveCommand,
    ) -> futures::future::BoxFuture<'a, Result<(), DriveError>> {
        Box::pin(async { Err(DriveError::NotImplemented("noop adapter")) })
    }

    fn knows_agent(&self, _agent_id: &str) -> bool {
        false
    }
}

/// Cap of the replay table (LRU-ish eviction; D3 keeps it bounded).
const REPLAY_CAP: usize = 4096;
/// A claimed-but-never-completed entry is evicted after this long. A claim
/// is held only for the duration of one synchronous dispatch, so a survivor
/// means a handler died mid-dispatch; cleared lazily on the next access,
/// never polled.
const CLAIM_STALE: Duration = Duration::from_secs(30);

// ---------------------------------------------------------------------------
// Idempotency table
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
enum Entry {
    /// A request has claimed the id and is dispatching.
    Claimed { since: Instant },
    /// First (and only) dispatch outcome, returned verbatim on replay.
    Done(DriveResponse),
}

/// Bounded idempotency table keyed by `request_id`.
///
/// `claim` is atomic: at most one caller ever wins [`Claim::Claimed`] for a
/// given id, which is what guarantees the command is dispatched at most once
/// even when duplicates arrive concurrently. `complete` stores the response
/// under the claim; `Done` entries are served until LRU eviction, so a retry
/// returns the first response byte-identical. No polling anywhere: claims
/// complete synchronously, stale claims are reaped on access.
#[derive(Debug, Default)]
pub struct ReplayTable {
    inner: Mutex<ReplayInner>,
}

#[derive(Debug, Default)]
struct ReplayInner {
    entries: HashMap<String, Entry>,
    /// Recency order, oldest first.
    order: VecDeque<String>,
}

#[derive(Debug)]
pub enum Claim {
    /// The id already has a stored response — return it.
    Done(DriveResponse),
    /// The id is currently being dispatched by a concurrent duplicate.
    Pending,
    /// This caller won the claim and must dispatch exactly once.
    Claimed,
}

impl ReplayTable {
    pub fn new() -> Self {
        Self::default()
    }

    fn claim(&self, request_id: &str) -> Claim {
        let mut inner = self.inner.lock().expect("replay table poisoned");
        inner.evict_stale_claims();
        match inner.entries.entry(request_id.to_string()) {
            HashMapEntry::Occupied(occupied) => match occupied.get().clone() {
                Entry::Done(response) => {
                    inner.touch(request_id);
                    Claim::Done(response)
                }
                Entry::Claimed { .. } => Claim::Pending,
            },
            HashMapEntry::Vacant(vacant) => {
                vacant.insert(Entry::Claimed {
                    since: Instant::now(),
                });
                inner.order.push_back(request_id.to_string());
                inner.evict_to_cap();
                Claim::Claimed
            }
        }
    }

    /// Return a completed response without claiming a new dispatch. This
    /// lookup deliberately happens before live-target validation: a retry of
    /// a request that already completed must remain byte-identical even if
    /// the agent disappeared after the original dispatch.
    fn completed(&self, request_id: &str) -> Option<DriveResponse> {
        let mut inner = self.inner.lock().expect("replay table poisoned");
        inner.evict_stale_claims();
        let response = match inner.entries.get(request_id) {
            Some(Entry::Done(response)) => Some(response.clone()),
            Some(Entry::Claimed { .. }) | None => None,
        };
        if response.is_some() {
            inner.touch(request_id);
        }
        response
    }

    fn complete(&self, request_id: &str, response: DriveResponse) {
        let mut inner = self.inner.lock().expect("replay table poisoned");
        inner
            .entries
            .insert(request_id.to_string(), Entry::Done(response));
        inner.touch(request_id);
        inner.evict_to_cap();
    }
}

impl ReplayInner {
    /// Move a key to the back of the recency order (LRU-ish touch).
    fn touch(&mut self, request_id: &str) {
        self.order.retain(|id| id != request_id);
        self.order.push_back(request_id.to_string());
    }

    fn evict_to_cap(&mut self) {
        while self.entries.len() > REPLAY_CAP {
            let Some(oldest) = self.order.pop_front() else {
                break;
            };
            self.entries.remove(&oldest);
        }
    }

    fn evict_stale_claims(&mut self) {
        let stale: Vec<String> = self
            .entries
            .iter()
            .filter_map(|(id, entry)| match entry {
                Entry::Claimed { since } if since.elapsed() >= CLAIM_STALE => Some(id.clone()),
                Entry::Claimed { .. } | Entry::Done(_) => None,
            })
            .collect();
        for id in stale {
            self.entries.remove(&id);
            self.order.retain(|candidate| candidate != &id);
        }
    }
}

// ---------------------------------------------------------------------------
// Wire parsing
// ---------------------------------------------------------------------------

/// Wire form with `capability` as a plain string, so an unknown capability
/// name surfaces as the typed `unknown_capability` refusal before the
/// contract's enum deserialization would fold it into a generic error.
/// Field order mirrors [`DriveEnvelope`] exactly: the typed envelope built
/// from this re-serializes to the same canonical bytes a signature covers.
#[derive(Debug, Deserialize)]
struct SignedDriveWire {
    key_id: String,
    signature: String,
    envelope: EnvelopeWire,
}

#[derive(Debug, Deserialize)]
struct EnvelopeWire {
    request_id: String,
    capability: String,
    target: String,
    payload: serde_json::Value,
    #[serde(default)]
    rev: Option<u64>,
}

/// What the drive handler will dispatch after the claim-check phase.
///
/// Approve is special: the command cannot be constructed here because the
/// claim check (W2) must validate the approval against the store first, and
/// the validated `choice` (not the raw payload) must be what gets dispatched
/// so menu membership holds.
enum PendingCommand {
    Command(DriveCommand),
    Approve {
        approval_id: String,
        prompt_hash: String,
        choice: String,
    },
}

/// Map a verified capability + payload onto the adapter command vocabulary.
/// Payload-bearing capabilities go through [`DrivePayload::parse`] (typed
/// mismatch refused); the three command-only capabilities take no payload.
fn command_for(
    capability: Capability,
    payload: &serde_json::Value,
) -> Result<PendingCommand, PayloadError> {
    match capability {
        Capability::Prompt | Capability::ReadTail | Capability::Approve => {
            let parsed = DrivePayload::parse(capability, payload)?;
            Ok(match parsed {
                DrivePayload::Prompt { text } => {
                    PendingCommand::Command(DriveCommand::Prompt { text })
                }
                DrivePayload::ReadTail { lines } => {
                    PendingCommand::Command(DriveCommand::ReadTail {
                        lines: Some(bound_tail_lines(lines)),
                    })
                }
                DrivePayload::Approve {
                    approval_id,
                    prompt_hash,
                    choice,
                } => PendingCommand::Approve {
                    approval_id,
                    prompt_hash,
                    choice,
                },
            })
        }
        Capability::Interrupt | Capability::Kill | Capability::Attach => {
            if !is_empty_payload(payload) {
                return Err(PayloadError {
                    capability,
                    detail: format!("no payload expected for {}, got {}", capability, payload),
                });
            }
            Ok(match capability {
                Capability::Interrupt => PendingCommand::Command(DriveCommand::Interrupt),
                Capability::Kill => PendingCommand::Command(DriveCommand::Kill),
                Capability::Attach => PendingCommand::Command(DriveCommand::Attach),
                Capability::Prompt | Capability::ReadTail | Capability::Approve => unreachable!(),
            })
        }
    }
}

/// `read_tail` line bound (D5): default 200, clamped to `[1, 200]`.
fn bound_tail_lines(lines: Option<u32>) -> u32 {
    lines
        .map(|lines| lines.clamp(1, READ_TAIL_MAX_LINES))
        .unwrap_or(READ_TAIL_MAX_LINES)
}

fn is_empty_payload(payload: &serde_json::Value) -> bool {
    payload.is_null() || payload.as_object().is_some_and(|object| object.is_empty())
}

// ---------------------------------------------------------------------------
// Handler
// ---------------------------------------------------------------------------

/// Typed pre-dispatch refusals. Dispatch-level outcomes travel inside
/// [`DriveResponse`] (`ok: false` + `error`), so replay stays byte-identical.
#[derive(Debug)]
pub enum DriveApiError {
    BadRequest {
        message: String,
        request_id: Option<String>,
    },
    UnknownCapability {
        capability: String,
        request_id: Option<String>,
    },
    Payload {
        error: PayloadError,
        request_id: Option<String>,
    },
    Auth {
        error: AuthError,
        request_id: Option<String>,
    },
    /// Destructive payload without a step-up token (W3).
    StepUpRequired {
        request_id: Option<String>,
    },
    /// Step-up token invalid, spent, expired, or key-bound to another device.
    StepUpFailed {
        error: String,
        request_id: Option<String>,
    },
    /// Claim-based approval refusal (W2): typed, never a 500. These refusals
    /// do NOT occupy the replay slot (parse-before-claim rule) and are NOT
    /// appended to the audit log (AC5: audit grows only on writes — a refused
    /// approval never dispatched).
    Approval {
        error: ApprovalError,
        request_id: Option<String>,
    },
    /// The store has no record for the target agent (claim-check prerequisite).
    UnknownAgent {
        agent_id: String,
        request_id: Option<String>,
    },
    /// The target was present when the client selected it but disappeared or
    /// migrated before dispatch. This is a refreshable conflict, not a
    /// generic not-found or transport error.
    StaleAgent {
        agent_id: String,
        request_id: Option<String>,
    },
    InFlight {
        request_id: String,
    },
}

#[derive(Debug, Serialize)]
struct DriveErrorBody {
    kind: &'static str,
    message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    request_id: Option<String>,
}

impl IntoResponse for DriveApiError {
    fn into_response(self) -> Response {
        let (status, kind, message, request_id) = match self {
            Self::BadRequest {
                message,
                request_id,
            } => (StatusCode::BAD_REQUEST, "bad_request", message, request_id),
            Self::UnknownCapability {
                capability,
                request_id,
            } => (
                StatusCode::BAD_REQUEST,
                "unknown_capability",
                format!("unknown capability: {capability}"),
                request_id,
            ),
            Self::Payload { error, request_id } => (
                StatusCode::UNPROCESSABLE_ENTITY,
                "payload",
                error.to_string(),
                request_id,
            ),
            Self::Auth { error, request_id } => {
                // W3's typed auth mapping (AC1): distinct status per error
                // class — 401 bad signature, 404 unknown key, 403 the rest.
                let (status, kind) = match &error {
                    AuthError::MissingSignature => (StatusCode::BAD_REQUEST, "missing_signature"),
                    AuthError::BadSignature => (StatusCode::UNAUTHORIZED, "bad_signature"),
                    AuthError::UnknownKey => (StatusCode::NOT_FOUND, "unknown_key"),
                    AuthError::Expired => (StatusCode::FORBIDDEN, "expired"),
                    AuthError::Revoked => (StatusCode::FORBIDDEN, "revoked"),
                    AuthError::NotGranted(_) => (StatusCode::FORBIDDEN, "not_granted"),
                };
                (status, kind, error.to_string(), request_id)
            }
            Self::StepUpRequired { request_id } => (
                StatusCode::FORBIDDEN,
                "step_up_required",
                "destructive payload needs a step-up token (POST /step-up, X-Step-Up-Token header)"
                    .to_string(),
                request_id,
            ),
            Self::StepUpFailed { error, request_id } => (
                StatusCode::UNAUTHORIZED,
                "step_up_failed",
                error,
                request_id,
            ),
            Self::Approval { error, request_id } => {
                let (status, kind) = match error {
                    ApprovalError::NoWaitingApproval => {
                        (StatusCode::CONFLICT, "no_waiting_approval")
                    }
                    ApprovalError::StaleApproval => (StatusCode::CONFLICT, "stale_approval"),
                    // The wrong-question race kill — must be distinct from
                    // stale so clients can tell "I answered late" from
                    // "I answered the wrong prompt".
                    ApprovalError::HashMismatch => (StatusCode::CONFLICT, "hash_mismatch"),
                    ApprovalError::ChoiceNotInMenu => {
                        (StatusCode::UNPROCESSABLE_ENTITY, "choice_not_in_menu")
                    }
                    ApprovalError::CannotApproveKind(_) => {
                        (StatusCode::UNPROCESSABLE_ENTITY, "cannot_approve_kind")
                    }
                };
                (status, kind, error.to_string(), request_id)
            }
            Self::UnknownAgent {
                agent_id,
                request_id,
            } => (
                StatusCode::NOT_FOUND,
                "unknown_agent",
                format!("unknown agent: {agent_id}"),
                request_id,
            ),
            Self::StaleAgent {
                agent_id,
                request_id,
            } => (
                StatusCode::CONFLICT,
                "stale_agent",
                format!("stale agent: {agent_id}; refresh the fleet snapshot"),
                request_id,
            ),
            Self::InFlight { request_id } => (
                StatusCode::CONFLICT,
                "in_flight",
                format!("request {request_id} is being dispatched by a concurrent duplicate"),
                Some(request_id),
            ),
        };
        (
            status,
            Json(DriveErrorBody {
                kind,
                message,
                request_id,
            }),
        )
            .into_response()
    }
}

/// Classify the result of each approval store read at the moment it returns.
/// The initial stale check is only a fast path; a target can disappear while
/// the async store lookup is in flight, in which case the adapter tombstone
/// must upgrade `None` from a generic 404 to a refreshable 409.
fn classify_approval_lookup(
    adapter: &dyn Adapter,
    agent_id: &str,
    request_id: &str,
    agent: Option<Agent>,
) -> Result<Agent, DriveApiError> {
    match agent {
        Some(agent) => Ok(agent),
        None if adapter.is_stale_agent(agent_id) => Err(DriveApiError::StaleAgent {
            agent_id: agent_id.to_string(),
            request_id: Some(request_id.to_string()),
        }),
        None => Err(DriveApiError::UnknownAgent {
            agent_id: agent_id.to_string(),
            request_id: Some(request_id.to_string()),
        }),
    }
}

async fn validated_approval_command<F, Fut>(
    adapter: &dyn Adapter,
    mut get_agent: F,
    agent_id: &str,
    request_id: &str,
    approval_id: &str,
    prompt_hash: &str,
    choice: &str,
) -> Result<DriveCommand, DriveApiError>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Option<Agent>>,
{
    let agent = classify_approval_lookup(adapter, agent_id, request_id, get_agent().await)?;
    check_approval_claim(
        agent_id,
        agent.waiting_on.as_ref(),
        approval_id,
        prompt_hash,
        choice,
    )
    .map_err(|error| DriveApiError::Approval {
        error,
        request_id: Some(request_id.to_string()),
    })?;

    // Re-read immediately before dispatch. Crucially, this read uses the same
    // tombstone-aware classification as the first read, so disappearance in
    // either async store window is a refreshable stale conflict.
    let agent = classify_approval_lookup(adapter, agent_id, request_id, get_agent().await)?;
    let approved = check_approval_claim(
        agent_id,
        agent.waiting_on.as_ref(),
        approval_id,
        prompt_hash,
        choice,
    )
    .map_err(|error| DriveApiError::Approval {
        error,
        request_id: Some(request_id.to_string()),
    })?;
    Ok(DriveCommand::Approve {
        choice: approved.choice,
    })
}

pub async fn drive(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
    body: Bytes,
) -> Result<Json<DriveResponse>, DriveApiError> {
    let wire: SignedDriveWire =
        serde_json::from_slice(&body).map_err(|error| DriveApiError::BadRequest {
            message: error.to_string(),
            request_id: None,
        })?;

    let request_id = wire.envelope.request_id.clone();
    if request_id.is_empty() {
        return Err(DriveApiError::BadRequest {
            message: "request_id must not be empty".to_string(),
            request_id: None,
        });
    }
    let target = wire.envelope.target.clone();
    if target.is_empty() {
        return Err(DriveApiError::BadRequest {
            message: "target must not be empty".to_string(),
            request_id: Some(request_id.clone()),
        });
    }
    let capability: Capability =
        wire.envelope
            .capability
            .parse()
            .map_err(
                |error: UnknownCapability| DriveApiError::UnknownCapability {
                    capability: error.0,
                    request_id: Some(request_id.clone()),
                },
            )?;

    let signed = SignedDrive {
        key_id: wire.key_id,
        signature: wire.signature,
        envelope: DriveEnvelope {
            request_id,
            capability,
            target,
            payload: wire.envelope.payload,
            rev: wire.envelope.rev,
        },
    };

    let authorized =
        state
            .auth
            .authorizer
            .verify(&signed)
            .map_err(|error| DriveApiError::Auth {
                error,
                request_id: Some(signed.envelope.request_id.clone()),
            })?;

    // Step-up gate (W3): destructive payloads need a single-use, short-TTL
    // token minted via POST /step-up. Step-up is part of AUTH — failures
    // are NOT audited (AC5) and do not occupy the replay slot.
    if state.auth.step_up.required(&authorized.envelope) {
        let token = headers
            .get(crate::auth::http::STEP_UP_HEADER)
            .and_then(|v| v.to_str().ok());
        match token {
            None => {
                return Err(DriveApiError::StepUpRequired {
                    request_id: Some(authorized.envelope.request_id.clone()),
                });
            }
            Some(token) => {
                if let Err(e) = state.auth.step_up.spend(&authorized.key_id, token) {
                    return Err(DriveApiError::StepUpFailed {
                        error: e.to_string(),
                        request_id: Some(authorized.envelope.request_id.clone()),
                    });
                }
            }
        }
    }

    // Parse BEFORE claiming: a payload error is deterministic and must not
    // occupy the id's slot.
    let pending = command_for(capability, &authorized.envelope.payload).map_err(|error| {
        DriveApiError::Payload {
            error,
            request_id: Some(authorized.envelope.request_id.clone()),
        }
    })?;

    // A completed request is an immutable response, even if its target has
    // disappeared since the original dispatch. Peek before any current
    // store/tombstone validation so retries remain byte-identical.
    let agent_id = authorized.envelope.target.clone();
    if let Some(response) = state.replay.completed(&authorized.envelope.request_id) {
        return Ok(Json(response));
    }

    // A tombstone must win before approve claim validation: the store may
    // already have removed the blocked row, but the adapter still knows this
    // was a real target and can give the client a refreshable 409.
    if state.adapter.is_stale_agent(&agent_id) {
        return Err(DriveApiError::StaleAgent {
            agent_id: agent_id.clone(),
            request_id: Some(authorized.envelope.request_id.clone()),
        });
    }

    // Claim check (W2, D8): the approve reply is validated against the
    // agent's CURRENT waiting approval BEFORE a new replay claim, so a stale
    // hash / stale approval can never occupy the id's slot or dispatch.
    // Refusals here are client errors: no replay entry, no audit entry.
    let command = match pending {
        PendingCommand::Command(command) => command,
        PendingCommand::Approve {
            approval_id,
            prompt_hash,
            choice,
        } => {
            let store = state.store.clone();
            validated_approval_command(
                state.adapter.as_ref(),
                || store.get(&agent_id),
                &agent_id,
                &authorized.envelope.request_id,
                &approval_id,
                &prompt_hash,
                &choice,
            )
            .await?
        }
    };

    // Re-check immediately before claiming replay state to cover a target
    // that disappeared while an approve claim was being validated.
    if state.adapter.is_stale_agent(&agent_id) {
        return Err(DriveApiError::StaleAgent {
            agent_id,
            request_id: Some(authorized.envelope.request_id.clone()),
        });
    }

    match state.replay.claim(&authorized.envelope.request_id) {
        Claim::Done(response) => return Ok(Json(response)),
        Claim::Pending => {
            return Err(DriveApiError::InFlight {
                request_id: authorized.envelope.request_id,
            });
        }
        Claim::Claimed => {}
    }

    let (ok, error, error_kind, outcome, result) = match command {
        // read_tail is the one capability whose whole point is a response:
        // the adapter fetches, redacts (D9) and bounds (D5) the tail and we
        // carry it back in `result.lines`; other drive commands await their
        // source RPC through the same outcome-bearing adapter future.
        DriveCommand::ReadTail { lines } => {
            let requested = lines.unwrap_or(READ_TAIL_MAX_LINES);
            match state.adapter.read_tail(&agent_id, requested).await {
                Ok(lines) => (
                    true,
                    None,
                    None,
                    AuditOutcome::Executed,
                    Some(serde_json::json!({ "lines": lines })),
                ),
                Err(e) => drive_refusal(e),
            }
        }
        other => match state.adapter.drive(&agent_id, other).await {
            Ok(()) => (true, None, None, AuditOutcome::Executed, None),
            Err(e) => drive_refusal(e),
        },
    };

    append_audit(state.auth.audit.as_ref(), &authorized, outcome);

    let rev = state.store.snapshot().await.rev;
    let response = DriveResponse {
        request_id: authorized.envelope.request_id.clone(),
        ok,
        error,
        error_kind,
        rev,
        result,
    };
    state
        .replay
        .complete(&authorized.envelope.request_id, response.clone());
    Ok(Json(response))
}

/// Map a dispatch-level [`DriveError`] onto the response: `ok:false` + the
/// typed error text + the matching audit outcome (transport → `Failed`,
/// everything else → `Refused`). `result` is always `None` on refusal.
fn drive_refusal(
    e: DriveError,
) -> (
    bool,
    Option<String>,
    Option<String>,
    AuditOutcome,
    Option<serde_json::Value>,
) {
    let text = e.to_string();
    let error_kind = Some(e.wire_kind().to_string());
    let outcome = match &e {
        DriveError::Transport(_) => AuditOutcome::Failed(text.clone()),
        DriveError::NotImplemented(_) | DriveError::UnknownAgent(_) | DriveError::StaleAgent(_) => {
            AuditOutcome::Refused(text.clone())
        }
    };
    (false, Some(text), error_kind, outcome, None)
}

fn append_audit(audit: &dyn AuditLog, authorized: &AuthorizedDrive, outcome: AuditOutcome) {
    let entry = AuditEntry {
        ts: now_millis(),
        key_id: authorized.key_id.clone(),
        request_id: authorized.envelope.request_id.clone(),
        capability: authorized.envelope.capability.to_string(),
        target: authorized.envelope.target.clone(),
        outcome,
    };
    if let Err(error) = audit.append(&entry) {
        warn!(
            request_id = %entry.request_id,
            error = %error,
            "audit append failed; the write was already dispatched"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::drive::canonical_envelope_bytes;
    use serde_json::json;

    #[derive(Debug)]
    struct TombstonedAdapter;

    impl Adapter for TombstonedAdapter {
        fn source(&self) -> &'static str {
            "test"
        }

        fn start(self: Arc<Self>, _store: Store) {}

        fn drive<'a>(
            &'a self,
            _agent_id: &'a str,
            _command: DriveCommand,
        ) -> futures::future::BoxFuture<'a, Result<(), DriveError>> {
            Box::pin(async { Err(DriveError::NotImplemented("test")) })
        }

        fn knows_agent(&self, _agent_id: &str) -> bool {
            false
        }

        fn is_stale_agent(&self, _agent_id: &str) -> bool {
            true
        }
    }

    #[test]
    fn wire_envelope_round_trips_to_identical_canonical_bytes() {
        let typed = DriveEnvelope {
            request_id: "r-1".to_string(),
            capability: Capability::Prompt,
            target: "herdr:a".to_string(),
            payload: serde_json::json!({ "kind": "prompt", "text": "go" }),
            rev: Some(7),
        };
        let wire: SignedDriveWire = serde_json::from_value(json!({
            "key_id": "test-key",
            "signature": "dGVzdC1zaWc",
            "envelope": serde_json::to_value(&typed).unwrap(),
        }))
        .unwrap();
        let capability: Capability = wire.envelope.capability.parse().unwrap();
        let signed = SignedDrive {
            key_id: wire.key_id,
            signature: wire.signature,
            envelope: DriveEnvelope {
                request_id: wire.envelope.request_id,
                capability,
                target: wire.envelope.target,
                payload: wire.envelope.payload,
                rev: wire.envelope.rev,
            },
        };
        assert_eq!(
            canonical_envelope_bytes(&signed.envelope),
            canonical_envelope_bytes(&typed),
            "the envelope W1 hands to the authorizer must serialize to the bytes a signature covers"
        );
    }

    #[test]
    fn tail_lines_are_bounded() {
        assert_eq!(bound_tail_lines(None), READ_TAIL_MAX_LINES);
        assert_eq!(bound_tail_lines(Some(5)), 5);
        assert_eq!(bound_tail_lines(Some(0)), 1);
        assert_eq!(bound_tail_lines(Some(100_000)), READ_TAIL_MAX_LINES);
    }

    #[tokio::test]
    async fn approval_reads_reclassify_second_read_disappearance_as_stale() {
        let adapter = TombstonedAdapter;
        let live = Agent {
            agent_id: "herdr:race".to_string(),
            source: "herdr".to_string(),
            tool: "opencode".to_string(),
            state: crate::core::model::AgentState::Blocked,
            reason: None,
            seq: 1,
            ts: 0,
            capabilities: Vec::new(),
            waiting_on: Some(crate::core::model::WaitingOn {
                kind: crate::core::model::WaitingOnKind::AnswerQuestion,
                prompt: "continue?".to_string(),
                prompt_hash: "sha256:x".to_string(),
                approval_id: "herdr:race:sha256:x".to_string(),
                choices: Vec::new(),
            }),
            cost: None,
            parent_id: None,
            host: None,
            workspace: Default::default(),
            attachment: None,
            display_name: None,
            title: None,
        };
        // Script the actual two approval reads: the first sees the blocked
        // record; disappearance before the second returns None. This is the
        // deterministic interleaving at the async store/classification
        // boundary, without relying on scheduler timing.
        let reads = Arc::new(Mutex::new(vec![Some(live), None]));
        let read_source = reads.clone();
        let result = validated_approval_command(
            &adapter,
            move || {
                let agent = read_source.lock().unwrap().remove(0);
                async move { agent }
            },
            "herdr:race",
            "req-race",
            "herdr:race:sha256:x",
            "sha256:x",
            "yes",
        )
        .await;
        assert!(matches!(
            result,
            Err(DriveApiError::StaleAgent { agent_id, request_id })
                if agent_id == "herdr:race" && request_id.as_deref() == Some("req-race")
        ));
    }
}
