//! Corral P4 desktop client (`corrald-ui`): a dark-dashboard fleet board
//! speaking corrald's read plane (snapshot/SSE with Last-Event-ID resume)
//! and its signed drive plane (device-keypair writes, claim-based
//! approvals, idempotent retries, transparent step-up).
//!
//! The daemon's HTTP surface is the contract (`docs/corral/P4-conformance.md`).
//! The binary lives in `main.rs`; this lib exposes the wire/protocol/drive
//! layers so they carry unit tests and the conformance suite.

pub mod app;
pub mod drive;
pub mod infer;
pub mod keys;
pub mod model;
pub mod protocol;
pub mod state;
pub mod theme;
pub mod ui;
