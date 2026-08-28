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
//!    lines are clamped to [`READ_TAIL_MAX_LINES`] (D5: 200 lines / 32 KiB).
//!    The daemon only serves a client request; it does not prefetch or push
//!    tails (the egui client may make one visible-card request after its own
//!    capability/grant checks).
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
//!    the response can carry `result.lines`. `attach` likewise routes through
//!    [`Adapter::attach`] so the response can carry a terminal handle without
//!    changing the result-less drive futures used by every other command.
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
use std::collections::{BTreeMap, HashMap, VecDeque};
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
use crate::core::blocks::segment_lines;
use crate::core::events::GhIssueRef;
use crate::core::model::Agent;
use crate::core::store::Store;
use crate::core::util::now_millis;
use crate::drive::{
    AuditEntry, AuditLog, AuditOutcome, AuthError, AuthorizedDrive, Capability, DriveEnvelope,
    DrivePayload, DriveResponse, PayloadError, READ_TAIL_DEFAULT_LINES, READ_TAIL_MAX_LINES,
    SignedDrive, UnknownCapability,
};
use crate::fleet::cli::FleetIdentity;
use crate::fleet::worktree::{
    self, GitCreator, HerdrLauncher, IssueCheck, IssueSummary, WorktreeError, WorktreeOutcome,
    WorktreeRequest,
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

    /// Atomically consume a request id for one-shot protocols that have no
    /// response to cache (for example a WebSocket session open).
    pub fn claim_once(&self, request_id: &str) -> bool {
        matches!(self.claim(request_id), Claim::Claimed)
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
    /// #113: a fleet-level worktree start (not an agent drive).
    Worktree(WorktreeRequest),
    /// #267: a fleet-level read-only issue browser fetch (not an agent
    /// drive) — dispatched before the per-agent path like `Worktree`.
    Issues,
}

/// Map a verified capability + payload onto the adapter command vocabulary.
/// Payload-bearing capabilities go through [`DrivePayload::parse`] (typed
/// mismatch refused); the three command-only capabilities take no payload.
fn command_for(
    capability: Capability,
    payload: &serde_json::Value,
    target: &str,
) -> Result<PendingCommand, PayloadError> {
    match capability {
        Capability::Prompt
        | Capability::ReadTail
        | Capability::ReadDiff
        | Capability::ReadIssues
        | Capability::Approve => {
            let parsed = DrivePayload::parse(capability, payload)?;
            Ok(match parsed {
                DrivePayload::Prompt { text } => {
                    PendingCommand::Command(DriveCommand::Prompt { text })
                }
                DrivePayload::ReadTail { lines, since_rev } => {
                    PendingCommand::Command(DriveCommand::ReadTail {
                        lines: Some(bound_tail_lines(lines)),
                        since_rev,
                    })
                }
                DrivePayload::ReadDiff {
                    files,
                    offset,
                    lines,
                } => PendingCommand::Command(DriveCommand::ReadDiff {
                    query: crate::drive::ReadDiffQuery::clamped(files, offset, lines),
                }),
                DrivePayload::ReadIssues => PendingCommand::Issues,
                DrivePayload::Approve {
                    approval_id,
                    prompt_hash,
                    choice,
                } => PendingCommand::Approve {
                    approval_id,
                    prompt_hash,
                    choice,
                },
                DrivePayload::StartWorktree { .. } => {
                    unreachable!("start_worktree is dispatched by its own arm")
                }
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
                Capability::Prompt
                | Capability::ReadTail
                | Capability::ReadDiff
                | Capability::ReadIssues
                | Capability::Approve
                | Capability::StartWorktree => unreachable!(),
            })
        }
        Capability::StartWorktree => {
            let parsed = DrivePayload::parse(capability, payload)?;
            let DrivePayload::StartWorktree {
                mode,
                repo,
                number,
                issue_url,
                name,
            } = parsed
            else {
                unreachable!("DrivePayload::parse returned the wrong variant");
            };
            let request = worktree_request(&mode, &repo, number, issue_url, name)?;
            // #113 review 2: the signed envelope `target` is the repo the
            // audit will record — it MUST equal the payload `repo` the
            // worktree is actually created against. A granted client signing
            // target=A + payload.repo=B must be refused before dispatch so
            // the audit trail reflects the real repo.
            if target != request.repo() {
                return Err(PayloadError {
                    capability: Capability::StartWorktree,
                    detail: format!(
                        "envelope target {target:?} does not match payload repo {:?}",
                        request.repo()
                    ),
                });
            }
            Ok(PendingCommand::Worktree(request))
        }
    }
}

/// Convert a `start_worktree` payload into a [`WorktreeRequest`]. An unknown
/// `kind` or a missing required field is a typed refusal (the client must
/// resend a well-formed request; nothing is created).
fn worktree_request(
    mode: &str,
    repo: &str,
    number: Option<u64>,
    issue_url: Option<String>,
    name: Option<String>,
) -> Result<WorktreeRequest, PayloadError> {
    let validate = |detail: &str| PayloadError {
        capability: Capability::StartWorktree,
        detail: detail.to_string(),
    };
    if repo.trim().is_empty() {
        return Err(validate("repo must not be empty"));
    }
    match mode {
        "issue" => {
            let number = number.ok_or_else(|| validate("issue start needs number"))?;
            if number == 0 {
                return Err(validate("issue number must be > 0"));
            }
            Ok(WorktreeRequest::Issue {
                repo: repo.to_string(),
                number,
                issue_url: issue_url.unwrap_or_default(),
            })
        }
        "free" => {
            let name = name.unwrap_or_default();
            if name.trim().is_empty() {
                return Err(validate("free start needs a name"));
            }
            Ok(WorktreeRequest::Free {
                repo: repo.to_string(),
                name,
            })
        }
        other => Err(validate(&format!(
            "unknown worktree mode {other:?}; expected \"issue\" or \"free\""
        ))),
    }
}

/// `read_tail` line bound (D5): default 50, clamped to `[1, 200]`.
fn bound_tail_lines(lines: Option<u32>) -> u32 {
    lines
        .map(|lines| lines.clamp(1, READ_TAIL_MAX_LINES))
        .unwrap_or(READ_TAIL_DEFAULT_LINES)
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
    let pending = command_for(
        capability,
        &authorized.envelope.payload,
        &authorized.envelope.target,
    )
    .map_err(|error| DriveApiError::Payload {
        error,
        request_id: Some(authorized.envelope.request_id.clone()),
    })?;

    // #113: start_worktree is a fleet-level operation, not an agent drive —
    // it must not run the per-agent (tomestone / approve / adapter) path.
    // The dispatch handles the replay/audit/write side itself.
    // #267: read_issues is the same shape — a fleet-level read, not an
    // agent drive.
    let pending = match pending {
        PendingCommand::Worktree(request) => {
            return dispatch_worktree(&state, &authorized, request).await;
        }
        PendingCommand::Issues => return dispatch_issues(&state, &authorized).await,
        other => other,
    };

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
        PendingCommand::Worktree(_) => {
            unreachable!("worktree requests are dispatched before the agent path")
        }
        PendingCommand::Issues => {
            unreachable!("read_issues is dispatched before the agent path")
        }
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
        DriveCommand::ReadTail { lines, since_rev } => {
            let requested = lines.unwrap_or(READ_TAIL_MAX_LINES);
            match state
                .adapter
                .read_tail_since(&agent_id, requested, since_rev)
                .await
            {
                Ok(lines) => {
                    // #167: serve blocks ADDITIVELY alongside the existing
                    // `lines` field. egui still renders `lines` until #168;
                    // the block renderer (iOS) consumes `blocks`. Redaction
                    // runs at the adapter boundary (D9) BEFORE these blocks
                    // are segmented, so block text rides the same redaction
                    // path as the lines.
                    let blocks = segment_lines(&lines, None);
                    (
                        true,
                        None,
                        None,
                        AuditOutcome::Executed,
                        Some(serde_json::json!({ "lines": lines, "blocks": blocks })),
                    )
                }
                Err(e) => drive_refusal(e),
            }
        }
        DriveCommand::Attach => match state.adapter.attach(&agent_id).await {
            Ok(handle) => (true, None, None, AuditOutcome::Executed, Some(handle)),
            Err(e) => drive_refusal(e),
        },
        // #232: read_diff is response-bearing like read_tail — the adapter
        // computes the bounded page (changed-files list + diffstat + unified
        // diff), redacts the lines, and we carry it back in `result`.
        DriveCommand::ReadDiff { query } => match state.adapter.read_diff(&agent_id, query).await {
            Ok(result) => (
                true,
                None,
                None,
                AuditOutcome::Executed,
                Some(
                    serde_json::to_value(result)
                        .unwrap_or_else(|_| serde_json::json!({ "error": "encode" })),
                ),
            ),
            Err(e) => drive_refusal(e),
        },
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
        DriveError::NotImplemented(_)
        | DriveError::UnknownAgent(_)
        | DriveError::StaleAgent(_)
        | DriveError::NoWorktree(_) => AuditOutcome::Refused(text.clone()),
    };
    (false, Some(text), error_kind, outcome, None)
}

/// #113: dispatch a fleet-level worktree start. Reuses the drive plane's
/// replay idempotency + audit so a duplicate tap/retry is byte-identical and
/// the write is auditable. The operation is gated upstream by the capability
/// grant (the authorizer) and by the issue selector's closed/stale guard.
async fn dispatch_worktree(
    state: &AppState,
    authorized: &AuthorizedDrive,
    request: WorktreeRequest,
) -> Result<Json<DriveResponse>, DriveApiError> {
    let request_id = authorized.envelope.request_id.clone();

    // A completed request is immutable — retries return the first response.
    if let Some(response) = state.replay.completed(&request_id) {
        return Ok(Json(response));
    }

    // Claim the id exactly once so concurrent duplicates (two taps of the
    // same logical action) cannot both dispatch a worktree. The loser gets
    // a refreshable `409 in_flight` and can retry for the stored response.
    match state.replay.claim(&request_id) {
        Claim::Done(response) => return Ok(Json(response)),
        Claim::Pending => {
            return Err(DriveApiError::InFlight { request_id });
        }
        Claim::Claimed => {}
    }

    let (ok, error, error_kind, outcome, result) = match worktree_dispatch(state, request).await {
        Ok(result) => result,
        Err(error) => {
            let outcome = worktree_outcome(&error);
            let kind = worktree_error_kind(&error);
            (
                false,
                Some(error.to_string()),
                Some(kind.to_string()),
                outcome,
                None,
            )
        }
    };

    append_audit(state.auth.audit.as_ref(), authorized, outcome);
    let rev = state.store.snapshot().await.rev;
    let response = DriveResponse {
        request_id,
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

/// #267: serve the grant-gated read-only issue browser payload. Fleet-level
/// like `dispatch_worktree` (no agent target): replay idempotency + audit,
/// then the SHARED issues view (`GET /issues`' exact builder) — one source
/// for both surfaces, so the iOS browser can never diverge from the board.
/// Read-only by construction: it is the gh poller's last-known cache.
async fn dispatch_issues(
    state: &AppState,
    authorized: &AuthorizedDrive,
) -> Result<Json<DriveResponse>, DriveApiError> {
    let request_id = authorized.envelope.request_id.clone();

    // A completed request is immutable — retries return the first response.
    if let Some(response) = state.replay.completed(&request_id) {
        return Ok(Json(response));
    }
    match state.replay.claim(&request_id) {
        Claim::Done(response) => return Ok(Json(response)),
        Claim::Pending => {
            return Err(DriveApiError::InFlight { request_id });
        }
        Claim::Claimed => {}
    }

    let view = crate::api::issues::issues_view(state).await;
    let (ok, error, error_kind, outcome, result) =
        (true, None, None, AuditOutcome::Executed, Some(view));

    append_audit(state.auth.audit.as_ref(), authorized, outcome);
    let rev = state.store.snapshot().await.rev;
    let response = DriveResponse {
        request_id,
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

/// The pure worktree dispatch: resolve the fleet, run the stale/closed guard,
/// and create + hand off. Returns the response tuple (`ok`, `error`,
/// `error_kind`, `outcome`, `result`).
async fn worktree_dispatch(
    state: &AppState,
    request: WorktreeRequest,
) -> Result<
    (
        bool,
        Option<String>,
        Option<String>,
        AuditOutcome,
        Option<serde_json::Value>,
    ),
    WorktreeError,
> {
    // #237: the fleet identity is validated by the fleet-ops CLI identity
    // path — corral never reads fleets.json. Display repo categories are
    // never actionable identities, so the request's `repo` must be a
    // CLI-validated fleet name (an UnknownFleet here is a refusal, not an
    // identity drift: the client only offers CLI-validated names).
    let fleet = state
        .fleets
        .get(request.repo())
        .map_err(|error| match error {
            crate::fleet::cli::FleetOpsError::UnknownFleet { name } => {
                WorktreeError::UnknownFleet(name)
            }
            other => WorktreeError::InvalidName(other.to_string()),
        })?;

    // #113 review 3: the issue URL in the audit/metadata is derived from the
    // daemon's authoritative issue ref, never trusted from the client. A
    // client could sign a bogus URL; the worktree action echoes the SAME
    // fetched set it validates against.
    let issue_snapshot = state.issues.snapshot();
    let all_identities = match state.fleets.list() {
        Ok(identities) => identities,
        Err(error) => {
            // The identity itself was validated above; only the alias
            // expansion is reduced. Never fabricate aliases from display
            // categories.
            tracing::warn!(error = %error, "fleet-ops CLI identity path unavailable for issue aliases");
            Vec::new()
        }
    };
    let request = match request {
        WorktreeRequest::Issue { repo, number, .. } => {
            let authoritative_url =
                authoritative_issue(&all_identities, &issue_snapshot, &fleet, number)
                    .map(|issue| issue.url)
                    .unwrap_or_default();
            WorktreeRequest::Issue {
                repo,
                number,
                issue_url: authoritative_url,
            }
        }
        free => free,
    };

    // The stale/closed-issue guard runs against the SAME repo-level issue set
    // the browser renders (the integrator's cache), never a guess.
    let issues = issue_summaries(&all_identities, issue_snapshot);
    let issue_check = IssueCheck::new(&issues);

    let creator = GitCreator;
    let launcher = HerdrLauncher;
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    let outcome = worktree::start(
        &fleet,
        &request,
        "HEAD",
        &home,
        issue_check,
        &creator,
        &launcher,
    )?;

    match outcome {
        WorktreeOutcome::Started {
            branch,
            path,
            handoff,
        } => {
            let handoff_state = match handoff {
                worktree::Handoff::Launched => "launched",
                worktree::Handoff::Deferred => "deferred",
                worktree::Handoff::Failed(msg) => {
                    return Err(WorktreeError::Launch(msg));
                }
            };
            let result = serde_json::json!({
                "state": "started",
                "branch": branch,
                "path": path.to_string_lossy(),
                "handoff": handoff_state,
            });
            Ok((true, None, None, AuditOutcome::Executed, Some(result)))
        }
        WorktreeOutcome::AlreadyStarted { branch, path } => {
            let result = serde_json::json!({
                "state": "already_started",
                "branch": branch,
                "path": path.to_string_lossy(),
            });
            Ok((true, None, None, AuditOutcome::Executed, Some(result)))
        }
    }
}

/// Return the cached issue for a fleet, including another fleet's cache entry
/// when both CLI-validated identities point at the same full GitHub
/// repository. The gh plane polls one repository once, so the first fleet's
/// issue key is the source of truth for every exact fleet-name action in
/// that repository. `identities` is the fleet-ops CLI validated catalog.
fn authoritative_issue(
    identities: &[FleetIdentity],
    snapshot: &BTreeMap<String, Vec<GhIssueRef>>,
    fleet: &FleetIdentity,
    number: u64,
) -> Option<GhIssueRef> {
    let aliases = identities
        .iter()
        .filter(|candidate| same_gh_repo(&candidate.gh_repo, &fleet.gh_repo))
        .map(|candidate| candidate.name.as_str());
    let mut names = vec![fleet.name.as_str()];
    names.extend(aliases.filter(|name| *name != fleet.name));
    names
        .into_iter()
        .filter_map(|name| snapshot.get(name))
        .flat_map(|issues| issues.iter())
        .find(|issue| issue.number == number)
        .cloned()
}

/// Expand one cached fleet issue onto every exact fleet-name alias that shares
/// its full GitHub repository. This keeps the daemon-side issue check aligned
/// with the client surface without adding duplicate entries to `GET /issues`.
fn issue_summaries(
    identities: &[FleetIdentity],
    snapshot: BTreeMap<String, Vec<GhIssueRef>>,
) -> Vec<IssueSummary> {
    let mut summaries = BTreeMap::<(String, u64), IssueSummary>::new();
    for (repo, issues) in snapshot {
        let aliases: Vec<String> = identities
            .iter()
            .find(|fleet| fleet.name == repo)
            .map(|source| {
                identities
                    .iter()
                    .filter(|candidate| same_gh_repo(&candidate.gh_repo, &source.gh_repo))
                    .map(|candidate| candidate.name.clone())
                    .collect()
            })
            .filter(|aliases: &Vec<String>| !aliases.is_empty())
            .unwrap_or_else(|| vec![repo.clone()]);
        for issue in issues {
            for alias in &aliases {
                let summary = IssueSummary {
                    repo: alias.clone(),
                    number: issue.number,
                    state: issue.state.clone(),
                };
                let key = (summary.repo.clone(), summary.number);
                if alias == &repo {
                    summaries.insert(key, summary);
                } else {
                    summaries.entry(key).or_insert(summary);
                }
            }
        }
    }
    summaries.into_values().collect()
}

fn same_gh_repo(left: &str, right: &str) -> bool {
    canonical_gh_repo(left)
        .is_some_and(|left| canonical_gh_repo(right).is_some_and(|right| left == right))
}

fn canonical_gh_repo(gh_repo: &str) -> Option<String> {
    let (owner, repo) = gh_repo.split_once('/')?;
    let (owner, repo) = (owner.trim(), repo.trim());
    if owner.is_empty() || repo.is_empty() || repo.contains('/') {
        return None;
    }
    Some(format!("{owner}/{repo}"))
}

/// Stable wire `error_kind` for a typed worktree failure.
fn worktree_error_kind(error: &WorktreeError) -> &'static str {
    match error {
        WorktreeError::UnknownFleet(_) => "unknown_fleet",
        WorktreeError::IssueNotFound { .. } => "issue_not_found",
        WorktreeError::IssueClosed { .. } => "issue_closed",
        WorktreeError::AlreadyStarted { .. } => "already_started",
        WorktreeError::InvalidName(_) => "invalid_name",
        WorktreeError::Git(_) => "git_failure",
        WorktreeError::Launch(_) => "launch_failure",
    }
}

/// Audit outcome for a worktree failure: pre-dispatch validation refusals are
/// `Refused`; a git/launch failure that consumed the id is `Failed`.
fn worktree_outcome(error: &WorktreeError) -> AuditOutcome {
    match error {
        WorktreeError::Git(_) | WorktreeError::Launch(_) => AuditOutcome::Failed(error.to_string()),
        _ => AuditOutcome::Refused(error.to_string()),
    }
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
    use std::path::PathBuf;

    fn worktree_fleet(name: &str, gh_repo: &str) -> FleetIdentity {
        FleetIdentity {
            name: name.into(),
            gh_repo: gh_repo.into(),
            local: PathBuf::from("/tmp/local"),
            worktree_dir: name.into(),
            orch: "orch".into(),
            workers: 0,
            paused: false,
        }
    }

    fn cached_issue(number: u64) -> GhIssueRef {
        GhIssueRef {
            repo: "foo".into(),
            number,
            state: "OPEN".into(),
            title: "shared issue".into(),
            labels: Vec::new(),
            url: format!("https://github.com/example/foo/issues/{number}"),
            body: None,
            comments: Vec::new(),
            comment_total: None,
        }
    }

    #[test]
    fn terminal_open_request_id_is_consumed_once() {
        let replay = ReplayTable::new();
        assert!(replay.claim_once("terminal-open-1"));
        assert!(!replay.claim_once("terminal-open-1"));
    }

    #[test]
    fn shared_gh_repo_issue_cache_serves_each_fleet_target_without_api_duplicates() {
        let identities = vec![
            worktree_fleet("alpha", "owner/foo"),
            worktree_fleet("beta", "owner/foo"),
        ];
        let issue = cached_issue(42);
        let snapshot = BTreeMap::from([
            ("alpha".to_string(), vec![issue.clone()]),
            ("beta".to_string(), vec![issue.clone()]),
        ]);

        let summaries = issue_summaries(&identities, snapshot.clone());
        assert_eq!(
            summaries.len(),
            2,
            "identical cached issues are deduplicated"
        );
        assert_eq!(
            summaries
                .iter()
                .map(|issue| issue.repo.as_str())
                .collect::<Vec<_>>(),
            vec!["alpha", "beta"],
            "one fetched issue is valid for both exact fleet targets"
        );
        assert_eq!(
            authoritative_issue(&identities, &snapshot, &identities[1], 42,),
            Some(issue),
            "the second fleet gets the same authoritative URL without a second poll"
        );
    }

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
        assert_eq!(bound_tail_lines(None), 50);
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
