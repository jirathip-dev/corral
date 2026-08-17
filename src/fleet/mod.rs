//! #35 phase 1: corrald's fleet registry.
//!
//! The fleet registry — `fleets.json`, today the single source of truth for
//! the separate fleet tooling — is becoming corrald's own config (issue #35
//! consolidation). This module parses, validates, and (slice 1) WRITES the
//! registry: `fleet add` / `fleet remove` are the first commands that mutate
//! it ([`ops`]), behind atomic-write discipline and a repo-resolves-before-add
//! check. `config::load` remains the read side; mutation of running agents
//! (pause/resume, model switching, spawning, watchdogs, reaping, worktree
//! pruning) lands in later phases of #35 and is out of scope.

pub mod config;
pub mod ops;
