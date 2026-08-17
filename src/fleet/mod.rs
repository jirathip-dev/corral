//! #35 phase 1: corrald's read side of the fleet registry.
//!
//! The fleet registry — `fleets.json`, today the single source of truth for
//! the separate fleet tooling — is becoming corrald's own config (issue #35
//! consolidation). This module parses and validates it ([`config::load`]);
//! nothing here mutates the registry or touches a running agent. Mutation
//! (add/remove, pause/resume, model switching, spawning, watchdogs, reaping,
//! worktree pruning) lands in a later phase of #35 and is out of scope.

pub mod config;
