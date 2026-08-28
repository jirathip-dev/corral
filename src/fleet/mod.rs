//! #35 + #237: corrald's fleet control plane — CONFIGLESS.
//!
//! Since #237, Corral does not own, read, or write `fleets.json`. The fleet
//! registry is fleet-ops' opinionated config (per-role models, admit,
//! paused); corrald never touches it. Actionable fleet identities come
//! exclusively from the fleet-ops CLI validated identity path
//! ([`cli`]) — the same shell-out pattern as the existing `herdr` shell-outs.
//!
//! The destructively-oriented CLI operations (`switch`) sit beside the
//! identity provider. `switch` delegates the whole re-arm to the fleet-ops
//! CLI (`herdr-fleet switch`), which is lanes-aware (hermes profile in the
//! brief) and validates identities itself.

pub mod cli;
pub mod health;
pub mod switch;
pub mod worktree;
