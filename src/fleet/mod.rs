//! #35: corrald's fleet control plane.
//!
//! The fleet registry — `fleets.json`, today the single source of truth for
//! the separate fleet tooling — is becoming corrald's own config (issue #35
//! consolidation). This module parses, validates, and WRITES the registry:
//! `fleet add` / `remove` / `pause` / `resume` / `models` mutate it through
//! [`ops`], behind atomic-write discipline and a
//! repo-resolves-before-add check. Reading stays on [`config::load`].
//!
//! The destructive side of #35 lives beside the registry mutation: [`reap`]
//! reclaims finished and paused-idle agent panes, [`prune`] removes only
//! provably-dead worktrees, and [`switch`] re-arms the orchestrator on the
//! registry's current model map after an auth gate. These CLI operations run
//! before the tokio runtime, apply verified process/worktree identity checks,
//! and never rewrite the registry themselves.

pub mod config;
pub mod ops;
pub mod prune;
pub mod reap;
pub mod switch;
pub mod watch;
pub mod worktree;
