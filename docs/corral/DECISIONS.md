# Corral DECISIONS.md (G34 + D34)

## D34 (2026-08-17, G34 implementer) — cost/usage-% meter design

**Issue:** #34 — per-provider (opencode/claude/codex) cumulative USD + % of
configured cap over rolling 5h/weekly/monthly windows, surfaced on the
board and dashboard, with an alert before exhaustion.

**Resolved dependency:** the issue said it "blocks on #27" (the full-chat
transcript viewer). It does not — #27 is deferred and this workstream
builds its own read-only session-store readers (`src/cost/{opencode,
claude,codex}.rs`), independent of any transcript-viewing feature.

**Store readers:**
- **opencode** (`src/cost/opencode.rs`): shells out to the system `sqlite3`
  CLI (`-readonly -json`, `.timeout` pragma) rather than adding a sqlite
  crate — none was in `Cargo.toml`, and the brief said a genuinely-missing
  crate needs orchestrator sign-off before adding. Every query is bounded
  by a SQL `WHERE time_created/time_updated BETWEEN ...` clause — never an
  unbounded scan of the 13GB+ `opencode.db`. Schema is feature-detected:
  current installs carry per-message `data` JSON (`tokens`, `cost`,
  `modelID`, `time.created`, `path.cwd`); if `message.data` is absent
  (older schema), it falls back to the per-session cumulative columns.
  **If corrald ever needs to run on a host without the `sqlite3` binary,
  that's the trigger to add `rusqlite` — not a silent default.**
- **claude** (`src/cost/claude.rs`): walks `~/.claude/projects/**/*.jsonl`,
  skipping files whose mtime predates the query window (minus a 1-day
  margin), streaming line-by-line. Prices via `message.usage` tokens ×
  `src/cost/pricing.rs`'s Claude rate table.
- **codex** (`src/cost/codex.rs`): walks `~/.codex/sessions/**/rollout-*.jsonl`,
  tracks the active model from `turn_context.payload.model`, and prices
  `token_count` events' `info.last_token_usage` (the incremental delta —
  `total_token_usage` is cumulative per session and summing it would
  double-count) via an OpenAI/codex rate table.

**D-083 (no chat content/secrets through the meter path):** every reader
only ever touches `usage`/`model`/`cwd`/`timestamp` fields — never
`message.content`/`response_item` text. This is enforced by construction
(no reader function ever calls `.get("content")`), verified by
`cost::claude::tests::d083_message_content_never_reaches_a_usage_event` and
`tests/cost.rs::d083_injected_message_content_never_reaches_the_response_body`
(an end-to-end check through `GET /cost`).

**Pricing table** (`src/cost/pricing.rs`): Claude prices are the
first-party API rates from Anthropic's own model catalog (Opus 5 $5/$25,
Sonnet 5 $3/$15, Fable 5/Mythos 5 $10/$50, Haiku 4.5 $1/$5 per MTok, as of
2026-08-17); cache tiers are derived via the documented 1.25x (5m TTL) /
2x (1h TTL) write and 0.1x read multipliers over the input rate. Codex
prices were fetched live from `developers.openai.com/api/docs/pricing` the
same day (gpt-5.6-sol/-terra/-luna, gpt-5.5, gpt-5.4, gpt-5.3-codex,
gpt-5(-mini)). **These are snapshots, not a live feed** — an unrecognized
model id returns `None` (unpriced — contributes $0, never a fabricated
number) rather than guessing. Older/deprecated Claude aliases
(`claude-opus-4-5`, `-4-1`, `-3-*`, etc.) are intentionally absent from the
table for the same reason.

**Caps are placeholders — open question.** Real opencode-go /
claude / codex plan limits are unknown to this workstream. `src/cost/config.rs`
ships order-of-magnitude placeholder caps ($5 / $35 / $140 for 5h/weekly/
monthly) so the % and alert machinery is demonstrably functional out of the
box, and every `GET /cost` window carries `cap_is_placeholder: true` until
overridden via `CORRAL_COST_CAP_<PROVIDER>_<WINDOW>_USD` env vars (see
`src/cost/config.rs` module docs for the full var list, and
`CORRAL_COST_ALERT_THRESHOLD_PCT` / `CORRAL_COST_WARN_THRESHOLD_PCT` for
the alert thresholds, default 90%/70%).

**D30 (board cost column):** `src/adapters/herdr.rs`'s `build_agent` now
reads `Agent.cost` from a process-global cache
(`src/cost/agent_cache.rs`), keyed by `(tool, worktree_path)` and
refreshed every 5 minutes by a background loop spawned from `main.rs`.
"Cumulative" here means "summed over the trailing 30 days", not true
all-time — an unbounded full-history scan of `opencode.db` on every
herdr pane event would violate the bounded-read constraint this whole
workstream is built around. A process-global static (rather than a field
threaded through `HerdrAdapter::new`) is a deliberate minimal-footprint
choice: every existing test that constructs a `HerdrAdapter` directly is
untouched, and the cache degrades to `None` (today's pre-G34 behavior)
until the loop has run once. The egui client's board panel already
renders `agent.cost` when present (built in an earlier P4 PR) — no
client-side change was needed for D30 to show up end-to-end.

**Alert before exhaustion:** `GET /cost` carries a `status`
(`ok`/`warning`/`problem`) per provider/window, computed the same way the
alert watchdog logs it — `problem` at/above `alert_threshold_pct` (default
90%), `warning` at/above `warn_threshold_pct` (default 70%). A background
task (`cost::spawn_alert_watchdog`, spawned from `main.rs` on the same
5-minute interval as the D30 cache) additionally `tracing::warn!`s the
moment a window crosses into `problem`, mirroring fleet-watch's
"flag before agents idle" shape but for spend rather than liveness.

**Not built in this pass — flagged, not silently dropped:** the "dashboard
tiles per provider" *client* surface (a new egui panel polling `GET
/cost`). The wire contract is done, stable, and tested; the panel itself
needs visual verification this environment can't do for a desktop egui
app, and P4's client stack was explicitly frozen as a prior-PR contract.
Follow-up work, not a scope cut nobody will notice.
