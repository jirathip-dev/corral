# Corral — P1 brief: corrald core (Rust)

> **Historical phase brief** (shipped, superseded). Kept as design
> history — current docs: README + docs/QUICKSTART.md, ARCHITECTURE.md,
> OPERATIONS.md, DEVELOPING.md.

Branch: feat/corral. Product: Corral — a tool-agnostic agent-fleet control
plane (monitor, approve, and drive AI coding agents from iOS/macOS/web/
Telegram). Neutral by design: herdr is ONE adapter of five (herdr, claude,
codex, opencode, gemini), never the product's identity.

## Goal

`corrald` — the host daemon in Rust — with the canonical agent model and a
herdr adapter that emits live agent events with ZERO polling. Deliverable:
a binary that, when run, connects to the local herdr socket and serves
`GET /snapshot` + SSE on loopback, with a monotonic `rev` cursor.

## Non-negotiable requirements

1. **Canonical agent model** (from the Opus review, D7). Each agent:
   - opaque `agent_id` (NOT pane_id — pane_id is a herdr-ism; use
     `attachment {kind: "herdr-pane", ref}`)
   - `capabilities: ["prompt","interrupt","approve","read_tail","kill","attach"]`
     — the client renders buttons from THIS, never hardcodes per-tool
   - structured `waiting_on {kind, prompt, prompt_hash, choices[]}` — status
     "blocked" is NOT one thing: approve-tool vs answer-question vs menu vs
     crash are different UIs
   - per-source monotonic `seq` (ordering; `ts` is display-only)
   - `state` (coarse enum) + `reason` (free string)
   - `parent_id` (topology: reviewer belongs to implementation)
   - `workspace {repo, branch, worktree_path, pr_number}`
   - `host` (public key identity, not hostname — D10)
2. **herdr adapter**: subscribe to the herdr socket's `events.subscribe`
   (25 event kinds already exist: pane_agent_status_changed,
   pane.output_matched, pane.scroll_changed...). ZERO polling. The adapter
   normalizes herdr events → canonical agent model. `pane.output_matched`
   is the notification trigger (server-side push, not tail scrape).
3. **Snapshot + SSE**: `GET /snapshot` returns full current state with
   monotonic `rev`. SSE stream carries `Last-Event-ID: <rev>` for resume —
   client sends its last rev, server replies full snapshot if cursor too
   old, else incremental `{rev, upd:[...], del:[...]}` (flat keyed records,
   NOT JSON Patch; JSON not CBOR). Coalesce events on a 250ms foreground
   / 2s background tick.
4. **Rust**, stdlib-heavy but pragmatic: tokio, serde, axum (or hyper), and
   a unix-socket client for herdr. No GUI. Single binary, `cargo build
   --release`. Must run on macOS now, compile for Linux later (Mac mini/VPS).
5. **Security baseline**: loopback bind only (127.0.0.1), no auth yet (that's
   P3 device signatures), but structure the code so the read path and drive
   path are separate modules from day one.

## Out of scope (later pieces)

- P2: git fsevents watcher + gh GraphQL poller
- P3: claim-based approvals + device keypair signatures
- P4: launchd service, Tailscale, iOS/macOS clients, redaction
- No web UI, no Telegram, no relay, no multi-tenancy

## Acceptance criteria (verdict gate)

1. `cargo build --release` clean, `cargo clippy` clean, `cargo test` green.
2. `corrald` connects to the real herdr socket and prints normalized agent
   events as they happen (manual check: start herdr agent, watch event flow).
3. `curl localhost:PORT/snapshot` returns valid JSON with `rev`; SSE
   reconnects with Last-Event-ID and resumes without full resnapshot when
   cursor is fresh.
4. Canonical model covers all 5 tools' states (herdr, claude, codex,
   opencode, gemini) — demonstrate with herdr's real agent list.
5. No polling anywhere in the herdr adapter (grep-able: no sleep-loops
   calling `herdr agent list`).

## How to verify

```bash
cargo build --release
cargo clippy -- -D warnings
cargo test
./target/release/corrald &   # prints events
curl localhost:PORT/snapshot
```

## Notes for the implementer

- The herdr socket is at `~/.config/herdr/herdr.sock` (JSON-RPC over unix
  socket; `herdr api schema --json` documents request/response shapes).
  Read the historical fleet-status scripts (reference only) for how agent
  state is currently scraped — you're replacing that with push.
- Keep the canonical model in its own module (`model.rs`), adapter behind a
  trait (`adapter.rs`), so P2/P3 add adapters without touching core.
- Version the schema (`schema_version` in the snapshot), additive-only.
- Don't over-engineer: this piece proves the push pipeline. If something is
  genuinely ambiguous, make the smallest defensible choice and note it.
