# Lean audit 490 — do persisted history and digests have a current product consumer?

- Issue: #490 (issue body read first; no comments at audit time).
- Pinned head (full SHA): `d754d0af0116977b862ef0abafdf805582afdefe`.
  Every `file:line` citation is a read at this exact head; the issue's historical
  anchors (`064016e0d5…`) are re-resolved here.
- Status: **AUDIT ONLY.** No history deletion, no notification rewrite, no live
  storage inspection, no follow-up routing. "Audit completion does not authorize
  execution" (issue #490). Decision gate §7; drafts in §8 are **UNAPPROVED**.
- **User-history preservation is a hard constraint of this audit**: the ring is
  read-only from the audit's perspective; nothing here deletes or mutates it.
- External-usage caveat: repo-scoped evidence cannot prove absence of external
  clients (no telemetry/capture/SSH authorized) — any not-in-repo reader is
  **UNKNOWN**.

## 1. Component inventory (writers, storage, readers)

| Component | Location (file:line) | Role |
|---|---|---|
| `HistoryEvent` schema | `src/history/ring.rs:28-46` | `ts`, `pane_id`, `agent_id?`, `old_status?`, `new_status`, `source`, `repo?` — one status transition |
| Ring (rotating JSONL segments) | `src/history/ring.rs:1-9`, `:96-155` | `seg-<seq>-<start_ts>.jsonl` under the history dir; append-only; newest is active |
| Insertion choke point (writer) | `src/core/store.rs:135-195` (push `:195`); design `src/history/mod.rs:23-31` | `Store::apply` `Change::Upsert` compares old vs new state; only actual transitions append; `Remove` emits nothing |
| Store wiring | `src/core/store.rs:88` (in-memory default), `:107-110` (`with_history_dir`), `:116-117` (`history()`); daemon: `src/main.rs:276` | Daemon persists under `<config-dir>/history` |
| Config dir default | `src/main.rs:44-49` | `$CORRAL_CONFIG_DIR` else `~/.config/corral` |
| Rotation/prune policy (defaults) | `src/history/ring.rs:52-67` (struct), `:69-80` (defaults), `:84-94` (`should_rotate`), `:321-342` (prune) | 4 segments × 256 events × 256 KiB ≈ 1024 events, hard 2 MiB total; 24 h max segment age |
| Durability details | `src/history/ring.rs:163-181` (push), `:210-258` (load/reopen), `:381-396` (torn line skipped) | Survives restart; best-effort append (kept in memory on write failure); dir failure degrades to in-memory (`:140-155`) |
| Reader 1: `GET /history` | route `src/api/mod.rs:146`; handler `:164-185`; limits `:73-75` | Credential-free read; `?since=<epoch-millis>`, `?limit=` (default 1000, cap 5000); oldest first |
| Reader 2: `corrald digest` (offline CLI) | `src/main.rs:59-62`, `:73-125`; `DIGEST_DEFAULT_WINDOW` `:40-41` | Loads the same ring segments directly — no daemon, no socket; `--since`, `--config-dir` |
| Digest computation | `src/history/digest.rs:1-11` (conventions), `:24-39` (`AgentDigest`), `:60-69` (`Digest`), `:75-…` (`compute`) | Per-agent transitions, closed blocked spans, open-span reporting, work per repo |
| Notifications (independent) | `src/push/mod.rs:73-86` (`Shadow` dedupe), `:65-69` (60 s reconcile tick) | Transition notifier keeps its OWN in-memory dedupe/hash state; it does **not** read the ring (search §9) |
| SSE resume backlog (transient, different object) | `src/core/store.rs:29` (`HISTORY_CAP=1024`), `:41-42`, `:288-291` | Bounded in-memory `(rev, delta)` ring for SSE resume — **not** the event ring; the only shared word is "history" |

## 2. Durable vs transient separation (AC2)

| Concern | Durable | Transient | Evidence |
|---|---|---|---|
| Status-transition record | **Rotating JSONL ring** (survives restarts, bounded to ~1024 events / 2 MiB) | — | `src/history/ring.rs:1-9`, `:69-80`; `tests/history.rs:140` (restart), `:165` (torn tail), `:194` (rotate/prune) |
| Live recents (`read_tail`) | not stored | On-demand bounded tail (200 lines / 32 KiB), never prefetched; recents v1 is live-tail only | `docs/ARCHITECTURE.md:20-22`, `:260-264` |
| SSE recovery | not stored | in-memory delta backlog capped at 1024 (`HISTORY_CAP`), full resnapshot beyond | `src/core/store.rs:29`, `:288-291`; `src/api/mod.rs:192-266` |
| Notification dedup | not stored (episodes lost on restart by design) | `Shadow` per-agent state | `src/push/mod.rs:73-86` |
| Digest artifact | computed offline from the ring on demand | — | `src/main.rs:73-125` |

Conclusion: the ring is the **only durable** history object, and its only readers
are the two above; recents/reconnect/dedup do **not** depend on it.

## 3. Consumer trace (AC1) — daemon, iOS, push, scripts, docs, external

| Consumer surface | Verdict | Evidence |
|---|---|---|
| Daemon `GET /history` | **consumer** (credential-free read) | `src/api/mod.rs:146,164-185` |
| Daemon `corrald digest` | **consumer** (offline CLI; the documented cron/launchd artifact) | `src/main.rs:73-125`; `src/history/mod.rs:33-54` |
| iOS app | **no consumer** — no `/history`/history API use anywhere; recents are live-tail only | search exit 1 (§9); `ios/FleetNotifier/App/AppModel.swift:2766` (comment "recents v1 = LIVE TAIL ONLY") |
| `crates/corrald-client` | **no consumer** | `git grep -rni 'history' -- crates/` → no matches |
| Push/notifications | **no dependency** on the ring (own `Shadow`) | `src/push/mod.rs:73-86`; `git grep 'store.history\|history()' -- src/push/ src/main.rs` → only main.rs store usage at `:276` |
| Scripts / CI | **no consumer** (no `corrald digest`, no `/history` in `scripts/` or `.github/`) | search exit 1 (§9) |
| Docs | describe the operator usage (`curl /history`, `corrald digest`) | `docs/OPERATIONS.md:350-375`, `:489`, `:498`; `docs/ARCHITECTURE.md:77-80`, `:351`, `:365`; `docs/DEVELOPING.md:24,46-47` |
| External clients | **UNKNOWN** — documented as a credential-free endpoint "for read-only clients and scripts"; no in-repo script uses it and no external scan was authorized | `docs/OPERATIONS.md:489-491`; §0 caveat |

## 4. Retention / configuration / defaults (AC3)

- Ring: 4 segments × 256 events × 256 KiB per segment, hard 2 MiB budget, 1024-event
  memory bound, 24 h segment-age rotation (`src/history/ring.rs:69-80`). Configurable
  in code via `RotationPolicy` (`src/history/ring.rs:52-67`); **no env/CLI knob** exists — daemon and
  digest both use `RotationPolicy::default()` (`src/main.rs:121`, `src/core/store.rs:107-110`).
- Location: `<config-dir>/history` (`src/main.rs:276`; digest `--config-dir` `:91-97`).
- API retention window: `?since`/`?limit` only; server keeps ~1024 events.
- Operational burden — demonstrable: bounded by policy and covered by the flood
  regression `tests/history.rs:469` ("stress flood stays bounded under 2 MiB");
  live-daemon smoke incl. digest `tests/history.rs:744`; the on-disk footprint is
  ≤ ~2 MiB by construction (`src/history/ring.rs:69-80`). **Unmeasured**: actual
  runtime cost/CPU of appends was not measured (no live observation authorized);
  no estimate is presented as a result.
- Log rotation is a **separate** concern (the daemon's launchd log, not the ring):
  `docs/OPERATIONS.md:375-388`, `scripts/rotate-corral-logs.sh`.

## 5. Existing-data preservation implications (AC4)

- The ring is user/fleet history. Any retirement recommendation must state what
  happens to **existing segments**: none of the keep options below deletes it, and
  no option may delete it without the explicit owner data-migration/retention gate.
- The audit itself preserves everything: no segment read for mutation, no prune,
  no storage inspection beyond the source.

## 6. Recommendations (keep/defer/retire per component; owner decides)

| Component | Recommendation | Compatibility / preservation implication |
|---|---|---|
| Ring (writer + JSONL storage) | **KEEP** | Removing it stops `GET /history` and `corrald digest`; existing segments would need an owner-approved migration/retention decision before any disable |
| `GET /history` | **KEEP** (no bundled consumer, but documented + external UNKNOWN; cheap, credential-free, bounded) | Retirement is a documented-endpoint break for unknown external readers |
| `corrald digest` CLI | **KEEP** | Offline operator tool; no daemon dependency; removing it loses the only reporting reader |
| Notifications | **KEEP, untouched** (issue #490: "notifications remain optional, not deleted"; independent of the ring) | — |
| `HISTORY_CAP` SSE backlog | **KEEP** (needed for reconnect/flood recovery) | Not the event ring; do not conflate |

No component is recommended for retirement. If the owner ever wants to retire the
`GET /history` endpoint alone, the ring + digest remain fully functional (they share
storage, not the route); that split is the natural bounded unit (§8).

## 7. Owner decision gate (blocking)

| # | Decision | Anchors |
|---|---|---|
| D1 | Confirm keep/defer posture for ring + `/history` + digest | §3, §6 |
| D2 | If any retirement is ever proposed: explicit existing-data retention/migration decision (no silent deletion of user history) | §5 |
| D3 | External-consumer posture (UNKNOWN today) before any endpoint retirement | §3 |

Nothing here is approved; audit PASS is not a removal permission.

## 8. Appendix A — minimal bounded follow-up plan (UNAPPROVED — DRAFT ONLY)

- **Spec H1 — retire only `GET /history`** (UNAPPROVED, only if owner later asks):
  remove the route/handler (`src/api/mod.rs:146,164-185`), keep ring + digest +
  `RotationPolicy`; gates: digest CLI test (`tests/history.rs:427`) stays green,
  restart/torn-tail/rotate tests stay green, route-absent probe (pattern
  `tests/http.rs:796-816`), docs updated; rollback = revert; **no segment deletion**.
- **Spec H2 — retire ring + digest** (UNAPPROVED, NOT recommended): requires the D2
  data-retention decision for existing `seg-*.jsonl` and the 488-style external
  acceptance; would also strand the `/history` docs. Not drafted further by design.
- Explicitly out of scope: notification dedup rewrite, storage migration, any
  file deletion in this lane.

## 9. Evidence, commands, and proof extent

Retained logs: `.audit-logs-488-490/` (untracked, preserved).

| Log | Command (verbatim) | Raw result |
|---|---|---|
| `490-negative-searches.log` | `git grep -rni 'history' -- crates/` (exit 1); `git grep -n 'history\|/history' -- ios/FleetNotifier/` (1 comment-only hit); `git grep -n 'store.history\|history()' -- src/push/ src/main.rs` (exit 1); `git grep -rn 'corrald digest\|/history' -- scripts/ .github/` (exit 1) | raw exits preserved |
| `490-tests.log` | `grep -n 'async fn \|fn ' tests/history.rs` | test inventory (restart `tests/history.rs:140`, torn tail `:165`, rotate/prune `:194`, flood `:469`, digest CLI `:427`, live smoke `:744`) |
| `citations-490.log` | citation verifier at pinned head | see log |

Proof extent for negatives: searches covered this repo at the pinned head
(tracked files) plus the iOS app and crate sources. They do **not** establish the
absence of external readers (UNKNOWN).

## 10. AC coverage (issue #490 checkboxes → sections)

| Checkbox | Where satisfied |
|---|---|
| Trace `/history`, rotating JSONL, ring, digest through daemon/iOS/push/scripts/docs/external | §1, §3 |
| Separate durable reporting from transient recents/SSE/dedup state | §2 |
| Retention/configuration/defaults + demonstrable burden; unknowns labeled | §4 |
| keep/defer/retire per component + compatibility/data-preservation implications; no user-history deletion | §5, §6 |
| Minimal follow-up plan with board/recents/reconnect regressions + data-migration gate | §8, §6 |
| Owner approval before any removal; audit ≠ authorization | §7, header |

## 11. Cross-references (no duplicate invented work)

- #488 owns `read_diff` / `/issues`; #489 owns Git/GitHub collection mapping.
  Neither flows through the ring (§3), so no shared removal unit exists.
- #29 / #397 are referenced by the issue body (notification lineage); notifications
  are explicitly out of scope here except to record their independence (§2).
- #492 owns daemon memory root-cause code investigation; this audit is read-only
  and does not duplicate it.
