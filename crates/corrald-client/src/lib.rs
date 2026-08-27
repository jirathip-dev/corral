//! Corral P4 shared client layer (W1).
//!
//! A pure client of the **frozen** corrald HTTP surface on main (P3
//! contract): `GET /snapshot`, `GET /events` (SSE with `Last-Event-ID`
//! resume), `GET /healthz`, `GET /host-key`, `POST /register`, `POST
//! /step-up`, `POST /grants`, `GET /audit`, `POST /drive`. It mirrors the
//! daemon's wire types field-for-field (`src/core/model.rs`,
//! `src/drive/mod.rs`, `src/auth/step_up.rs`) and signs envelopes over the
//! identical canonical bytes — it never shares daemon code, so the two
//! sides agree by construction and the conformance suite proves it against
//! a real corrald (see `tests/conformance.rs`, scenarios R1-R10).
//!
//! # Layers
//!
//! - [`model`] — typed read model (Snapshot/Agent/Workspace/WaitingOn/
//!   CiStatus/AgentState/Delta), additive-tolerant decoding.
//! - [`drive`] — the drive-plane wire types + canonical envelope bytes and
//!   the typed client-side error classification.
//! - [`keypair`] — Ed25519 device keypair generation + signing.
//! - [`stepup`] — `POST /step-up` proof-of-possession request.
//! - [`approval`] — approval-claim helpers (`approval_id`,
//!   `prompt_hash` over the exact snapshot prompt).
//! - [`sse`] — the reconnecting SSE client with `Last-Event-ID` resume.
//! - [`client`] — [`CorralClient`](client::CorralClient) (HTTP surface) and
//!   [`DriveClient`](client::DriveClient) (signed writes with idempotent
//!   `request_id` retries).
//!
//! # Contract rules honored here
//!
//! - Read-only default: a registered device has empty grants until the host
//!   promotes it via `POST /grants` (D13).
//! - Every write is idempotent by `request_id`; retries resend the same
//!   signed envelope and the daemon's replay table serves the first
//!   response byte-identical (exactly-once dispatch).
//! - Destructive payloads need a step-up token (`X-Step-Up-Token` header,
//!   minted via `POST /step-up`, single-use, 5 min).
//! - `prompt_hash` covers the EXACT untrimmed, redacted snapshot prompt
//!   string byte-for-byte — never raw pane text.
//! - No GUI here (W2), no polling (daemon-side) — client-side reconnect
//!   with backoff is the only retry loop.

pub mod approval;
pub mod client;
pub mod drive;
pub mod errors;
pub mod keypair;
pub mod model;
pub mod sse;
pub mod stepup;

pub use client::{
    AdminGrantsView, AuditData, CorralClient, DriveClient, GrantDevice, RegisteredDevice,
};
pub use drive::{Capability, DriveEnvelope, DriveResponse, SignedDrive, canonical_envelope_bytes};
pub use errors::{ApiError, DriveErrorKind};
pub use keypair::DeviceKeypair;
pub use model::{
    Agent, AgentState, CiStatus, Delta, GhIssueRef, Snapshot, WaitingOn, WaitingOnKind, Workspace,
};
pub use sse::{SseEvent, SseStream};
pub use stepup::StepUpRequest;
