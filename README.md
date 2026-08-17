# herdr-board

Read-only Tailnet board for the herdr agent fleet: worktree status, live agent
output, issue linkage, merge readiness. Served on the Tailscale interface only.

## Status

Under construction — see `docs/PLAN.md`.

## Cost meter (G34)

`GET /cost` reports per-provider (opencode/claude/codex) USD spend and %
of a configured cap over rolling 5h/weekly/monthly windows, read-only and
bounded from each provider's own session store — see `src/cost/mod.rs` for
the data flow and `docs/corral/DECISIONS.md` (D34) for the design writeup.
Per-provider caps default to clearly-marked placeholders; configure real
ones via `CORRAL_COST_CAP_<PROVIDER>_<WINDOW>_USD` (e.g.
`CORRAL_COST_CAP_CLAUDE_WEEKLY_USD=100`) — see `src/cost/config.rs` for the
full var list. The board's per-agent cost column and a `tracing::warn!`
watchdog before any window nears exhaustion both come from the same meter.
