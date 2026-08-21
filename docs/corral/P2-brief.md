# Corral — P2 brief: three data planes (Rust)

> **Historical phase brief** (shipped, superseded). Kept as design
> history — current docs: README + docs/ARCHITECTURE.md.

Branch: feat/corral-p2 (worktrees per workstream). P1 shipped corrald's
canonical model + herdr push adapter + snapshot/SSE (PR #2, merged).
`docs/corral/DECISIONS.md` is the authoritative design record.

## Goal

Add the other two data planes to corrald so the daemon's snapshot reflects
git + GitHub state, not just herdr agents. THREE workstreams, PARALLEL,
each in its own worktree:

### WS1 — git fsevents watcher (`git_plane.rs`)
- Watch `.git` (HEAD, index, refs) via fsevents (macOS) — debounce 300ms.
- Emit canonical events: branch switch, HEAD move, dirty/clean (index+worktree
  changed), worktree add/remove, commit on branch.
- 10s parallel `git status` sweep as safety net for missed events.
- Budget <200ms per event. Must work on the herdr-managed worktrees
  (main checkout + ~/.herdr/worktrees/*).
- No polling loop for the primary signal; the 10s sweep is the safety net.

### WS2 — GitHub GraphQL poller (`gh_plane.rs`)
- ONE aliased GraphQL query for ALL repos (sendmeter, project-hearthwild,
  synergy-costing, dotfiles, agent-ops, herdr-board, office-ops,
  synergy-services-website): PR state, CI checks, mergeability, issue refs.
- ETags on REST fallbacks. Cadence: 60s foreground / 300s background / 0 when
  no SSE client connected (SWR only on this plane).
- Budget: single round-trip per poll; measured 531ms for all repos.

### WS3 — schema extension + integration (`schema.rs` merge)
- Extend canonical agent record with `pr_number`, `ci_status`, `dirty`,
  `ahead/behind` counts, `worktree_path` — the task-centric read model
  fields from D7.
- Wire git_plane + gh_plane events into the existing snapshot/SSE pipeline
  (coalesce on the 250ms/2s tick; rev cursor covers all planes).
- Update P1's snapshot schema version (additive only, per P1 rule).

## Non-negotiable

- All three workstreams compile independently; WS3 integrates them. To keep
  the trio parallel, define the trait/event surface FIRST (a small
  `events.rs` contract doc committed before the three start) so WS1/WS2
  compile against it and WS3 merges.
- Same quality bar as P1: cargo build --release clean, clippy -D warnings
  clean, cargo test green, zero polling in the herdr adapter (P1 rule), no
  GUI, loopback bind only.
- D-083-style discipline: read-only access to GitHub (GET/GraphQL), no
  mutations from the daemon.

## Acceptance criteria (verdict gate)

1. WS1: starting a git commit in a herdr worktree emits a git event within
   <1s (300ms debounce + margin), verified with a live test.
2. WS2: one GraphQL round-trip returns PR/CI state for all 8 repos; cadence
   honors the client-connection rule (0 when no SSE client).
3. WS3: snapshot contains git + gh fields merged with herdr agent state, all
   under one monotonic rev; clients see a coherent merged view.
4. cargo build --release + clippy -D warnings + cargo test all green.
5. No polling loop in the herdr adapter (grep-able).

## How to verify

```bash
cargo build --release && cargo clippy -- -D warnings && cargo test
./target/release/corrald &    # watch events from all three planes
curl localhost:PORT/snapshot  # merged view
# live git test: touch + commit in a watched worktree, observe event
```

## Fanout note (orch-corral)

This is a 3-way parallel fanout (WS1 ∥ WS2 ∥ WS3) — the exact "many agents
for decomposed work" case. Each workstream = one implementer in its own
worktree, one adversarial reviewer per workstream (opencode, separate pane,
harsh-review brief). You integrate: merge the trait contract first, then
WS1/WS2, then WS3, run the full gate, merge once. All DeepSeek.
