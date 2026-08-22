//! #35 phase 1: corrald's fleet registry.
//!
//! The fleet registry — `fleets.json`, today the single source of truth for
//! the separate fleet tooling — is becoming corrald's own config (issue #35
//! consolidation). This module parses, validates, and WRITES the registry:
//! `fleet add` / `fleet remove` (slice 1) and `fleet pause` / `resume` /
//! `models` (slice 2) are the commands that mutate it ([`ops`]), behind
//! atomic-write discipline and a repo-resolves-before-add check. `config::load`
//! remains the read side; mutation of running agents (spawning, watchdogs,
//! reaping, worktree pruning, the auth-gated switch) lands in later phases of
//! #35 and is out of scope — slice 2 is pure registry mutation.

pub mod config;
pub mod ops;
pub mod watch;
pub mod worktree;
