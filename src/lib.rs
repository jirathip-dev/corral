//! Corral host daemon library.
//!
//! P1: canonical agent model + herdr adapter (events.subscribe over the herdr
//! unix socket, with a bounded trusted-catalog refresh) + `GET /snapshot`
//! with monotonic `rev` + SSE resume on loopback.

pub mod adapters;
pub mod api;
pub mod approve;
pub mod auth;
pub mod core;
pub mod drive;
pub mod fleet;
pub mod history;
pub mod integrate;
pub mod plugin;
pub mod push;
