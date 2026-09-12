# Lean audit 489 — Git and GitHub collection mapped to current visible product fields

- Issue: #489 (issue body read first; no comments at audit time).
- Pinned head (full SHA): `d754d0af0116977b862ef0abafdf805582afdefe`.
  Every `file:line` citation is a read at this exact head; the issue's historical
  anchors (`064016e0d5…`) are re-resolved here.
- Status: **AUDIT ONLY — no removal authority, no collector changes, no new
  adapters, no follow-up filing.** Decision list in §7; drafts in §8 are
  **UNAPPROVED**. External usage is **UNKNOWN** where it cannot be proven from
  this repo (no telemetry/capture/SSH/storage scan performed).
- Performance discipline (AC3): this audit makes **no runtime-cost claims**.
  Schedules below are code constants (facts), not measurements; any cost number
  would require a separately authorized bounded observation and none is presented.

## 1. Collected-field inventory (producers)

### 1a. Git plane (`src/adapters/git_plane.rs`) — event contract `src/core/events.rs:55-94`

| # | Collected fact | Producer location | Event wire field |
|---|---|---|---|
| G1 | Worktree topology add/remove (path identity) | `src/core/events.rs:80-83`; adapter emits on topology scans | `WorktreeAdded/Removed { worktree }` |
| G2 | Dirty state (staged + unstaged/untracked) | `src/core/events.rs:37-47` (`dirty_index`, `dirty_worktree`), `is_dirty()` `:50-52` | `DirtyChanged.status` |
| G3 | Ahead/behind vs upstream | `src/core/events.rs:44-46` | `DirtyChanged.status` |
| G4 | Branch name | `src/core/events.rs:66-74` (HeadMoved), `:85-93` (CommitOnBranch) | `branch` |
| G5 | HEAD commit (full SHA) | same events (`commit`) | `commit` |
| G6 | Commit subject (first line) | same events (`subject`, additive since G21) | `subject` |
| G7 | 5s child timeout / 4-command permit budget / fsevents debounce (producer mechanics) | `src/adapters/git_plane.rs:226` (5s), `:234` (4 permits), `:113` (300 ms debounce), `:224` (200 ms event budget), `:248` (256-event batch), `:241` (1 s rescan throttle), `:245` (400 ms rescan retry) | n/a (internal) |

### 1b. GitHub plane (`src/adapters/gh_plane.rs`) — `GhRepoState` `src/core/events.rs:187-195`

| # | Collected fact | Producer location | Wire field |
|---|---|---|---|
| H1 | Repo key (identity/grouping) | `GhRepoSpec` `src/adapters/gh_plane.rs:123`; scope from live Herdr workspaces `src/adapters/gh_plane.rs:147`, refresh `:600-607` | `repo` (+ aliases `:805-822`) |
| H2 | Default branch | query `src/adapters/gh_plane.rs:855`, map `:1179-1183` | `default_branch` |
| H3 | Repo ahead/behind | **hardcoded 0** `src/adapters/gh_plane.rs:1184-1187` ("WS1's job; cheaply unavailable in-query") | `ahead`, `behind` |
| H4 | PR list (OPEN, newest-updated first, cap 20) | query `src/adapters/gh_plane.rs:856-878` (`PR_LIMIT` `:105`) | `prs[]` |
| H5 | PR number | `src/adapters/gh_plane.rs:1229` | `GhPrState.pr_number` |
| H6 | PR title | `src/adapters/gh_plane.rs:1230` | `GhPrState.title` |
| H7 | PR state | `src/adapters/gh_plane.rs:1231` (query already filters `states: OPEN`) | `GhPrState.state` |
| H8 | PR mergeable | `src/adapters/gh_plane.rs:1232-1235` | `GhPrState.mergeable` |
| H9 | PR CI verdict (collapsed rollup; 50 contexts cap) | collapse `src/adapters/gh_plane.rs:1095-1132`, map `:1236` (`CONTEXTS_LIMIT` `:116`) | `GhPrState.ci_status` |
| H10 | PR head SHA | `src/adapters/gh_plane.rs:1237` | `GhPrState.head_sha` |
| H11 | PR head branch | `src/adapters/gh_plane.rs:1238` | `GhPrState.head_branch` |
| H12 | PR closing issues (number/title/url/labels; state enriched) | query `src/adapters/gh_plane.rs:864-866`, map `:1199-1226` (`CLOSING_ISSUES_LIMIT` `:114`) | `GhPrState.closing_issues` |
| H13 | Repo-level issues (10 newest-updated, OPEN+CLOSED) | query `src/adapters/gh_plane.rs:879-887` (`ISSUE_LIMIT` `:107`) | `issues[]` |
| H14 | Issue number/state/title | map `src/adapters/gh_plane.rs:1159-1163` | `GhIssueRef.*` |
| H15 | Issue labels (name+color, first 10) | `src/adapters/gh_plane.rs:1164`, `labels_from` | `GhIssueRef.labels` |
| H16 | Issue URL | `src/adapters/gh_plane.rs:1165` | `GhIssueRef.url` |
| H17 | Issue body | `src/adapters/gh_plane.rs:1166` | `GhIssueRef.body` |
| H18 | Issue comments (newest-first window, cap 30) + authoritative total | `src/adapters/gh_plane.rs:1167-1168` (`COMMENTS_LIMIT` `:111`) | `comments[]`, `comment_total` |

### 1c. Producer schedules and client-absence behavior (AC3; code facts, not measurements)

- **Git plane**: push-first via fsevents (`src/adapters/git_plane.rs:113`) with bounded safety nets:
  10 s topology (`:121`), 60 s status sweep (`:117`), 15 min cold rediscovery
  (`:124`), 500 ms startup rescan (`:128`); four shared command permits (`:234`);
  5 s child timeout (`:226`); missing-source backoff 10 s/60 s/5 min (`:251-254`)
  with 15 min expiry (`:259`). Every path shares one command budget
  (`docs/ARCHITECTURE.md:107-137`).
- **GitHub plane (SWR / client-absence)**: `cadence_step` `src/adapters/gh_plane.rs:398-437`
  — **zero network polling until the first SSE client has EVER connected**
  (`RecheckSubscribers` `:644-646`); first-ever join and every reconnect trigger an
  immediate fetch (`cadence_step` `src/adapters/gh_plane.rs:405-414`); while connected: foreground 60 s (`:92`); after the
  first-ever client, with none live: background 300 s (`:94`); in-process wake slice
  2 s (`:98`; wake-slice mechanism `src/adapters/gh_plane.rs:646-653`); HTTP timeout 30 s (`:100`); failure backoff starts 5 s, doubles,
  capped at the cadence (`:103`, `:438-443`); one GraphQL round-trip per poll
  (`:706-760`); per-alias null data skips that repo, keeping last-known state (`:790-796`).
- Neither plane's cost is measured in this audit; no estimate is presented as a result.

## 2. Producer → canonical field → iOS consumer matrix (every field)

Fold locations: `src/integrate/mod.rs` (`reapply_path` `:354-417`,
`apply_pr_facts` `:493-551`, `reapply_repo` `:421-442`, `reset_worktree` `:444-473`).

| Wire field | Canonical field (`src/core/model.rs`) | Fold rule (integrate) | iOS consumer (file:line) | Classification |
|---|---|---|---|---|
| G1 worktree topology | `workspace.worktree_path` survives; facts cached/pruned | `src/integrate/mod.rs:267-289`; removal resets derived fields `:444-473` | path → basename `ios/FleetNotifier/UI/FleetViews.swift:562-574`; cache `ios/FleetNotifier/Profiles/BoardCache.swift:80` | **keep** (identity) |
| G4 branch | `workspace.branch` `src/core/model.rs:111` | `src/integrate/mod.rs:380-390` (F7: detached `"HEAD"` → `None`); attribution fallback `:334-343`; generation reset `:172-190` | row text `ios/FleetNotifier/UI/FleetViews.swift:484-495`; basename suppression `:567`; notifications `ios/FleetNotifier/Notifications/PushPayload.swift:59`; cache `ios/FleetNotifier/Profiles/BoardCache.swift:79`; cache-rebuilt model `ios/FleetNotifier/Profiles/HostStreamCoordinator.swift:103` | **keep** (rendered) |
| G2 dirty (+G3 covered below) | `workspace.dirty` `src/core/model.rs:119-120` | `src/integrate/mod.rs:408-412` (`status.is_dirty()`) | `ios/FleetNotifier/UI/FleetViews.swift:517-520` (`dirty` badge) | **keep** (rendered) |
| G3 ahead/behind | `workspace.ahead/.behind` `src/core/model.rs:121-126` | `src/integrate/mod.rs:409-411` | `ios/FleetNotifier/UI/FleetViews.swift:522-526` (`↑a↓b`) | **keep** (rendered) |
| G5 commit | `workspace.head_sha` `src/core/model.rs:127-132` | `src/integrate/mod.rs:392-398`; also the **PR matching key** `:514-524` | none (iOS model has no `headSha`; search §9) | **keep** (hidden-but-required: PR matching + wire parity) |
| G6 subject | `workspace.head_subject` `src/core/model.rs:133-138` | `src/integrate/mod.rs:399-407` (redacted, D9) | none (not in iOS model) | **uncertain** (display text with no current client reader; wire parity only) |
| H10 head SHA | (matching input) | `src/integrate/mod.rs:516-524` (precedence 1) | n/a | **keep** (matching key) |
| H11 head branch | (matching input) | `src/integrate/mod.rs:525-531` (precedence 2, #22 fallback) | n/a | **keep** (matching key) |
| H5 PR number | `workspace.pr_number` `src/core/model.rs:114` | `src/integrate/mod.rs:547` | `ios/FleetNotifier/UI/FleetViews.swift:511-515` (`#N`) | **keep** (rendered) |
| H9 CI verdict | `workspace.ci_status` `src/core/model.rs:115-117` | `src/integrate/mod.rs:548`, mapped `:556-563` (unrecognized → `Unknown`) | decoded `ios/FleetNotifier/Models/Models.swift:46,76-78`; **no reader anywhere in the app** (search §9) | **uncertain** (transported, not rendered; wire parity) |
| H12 closing issues | `workspace.issues` `src/core/model.rs:146-150` | `src/integrate/mod.rs:508-509`, `:550` | iOS **dropped** the field (#354 L2; comment `ios/FleetNotifier/Models/Models.swift:42-44`); `crates/corrald-client` retains it (`crates/corrald-client/src/model.rs:131`) and conformance R11 pins the mirror (`crates/corrald-client/tests/conformance.rs:1056,1086`) | **uncertain** (wire join; no iOS consumer; shared with audit-488 §4.2) |
| debug PR-match source | `workspace.pr_match_source` `src/core/model.rs:139-145` | `src/integrate/mod.rs:536-549` | none (documented "not a render-driver") | **keep** (debug-only by design) |
| H1 repo key | `workspace.repo` `src/core/model.rs:111` | attribution `src/integrate/mod.rs:337`, `:379`; `src/core/workspace.rs:180` (`repo_for`) | grouping/pills `ios/FleetNotifier/UI/BoardModel.swift:36,60,71,422,430`; chip `ios/FleetNotifier/UI/FleetViews.swift:481`; notifications `ios/FleetNotifier/Notifications/PushPayload.swift:58` | **keep** (identity/grouping) |
| H2 default branch | none (never folded) | — | — | **confirmed unused** (in-repo; carried on `GhRepoState`) |
| H3 repo ahead/behind | none (never folded; always 0) | — | — | **confirmed unused** (in-repo; constant-0 producer) |
| H6 PR title | none (never folded) | — | — | **confirmed unused** (in-repo; participates only in poll-dedupe equality) |
| H7 PR state | none (never folded; query filters OPEN) | — | — | **confirmed unused** (in-repo) |
| H8 PR mergeable | none (never folded) | — | — | **confirmed unused** (in-repo) |
| H13–H18 repo-level issues | ride `GET /issues` (audit-488) + enrich H12 states `src/adapters/gh_plane.rs:1194-1211` | cache writer `src/integrate/mod.rs:299-306` | no bundled client (audit-488 §2) | **uncertain**; **external UNKNOWN** |
| `git_plane_backlog` snapshot flag | `Snapshot.git_plane_backlog` `src/core/model.rs:213-215` | set `src/core/store.rs:355,431`; flag `src/core/store.rs:120-121`, wired `src/main.rs:422` | none (iOS has no reference) | **keep** (stale-state honesty; server-side) |

Hidden-but-required identity/grouping dependencies (AC1) — call-outs:
- Repo identity is **path-derived** and configless: `src/core/workspace.rs:45,180,202`;
  board categories are the live `workspace.repo` union `src/api/repo.rs:18-37,42-50`.
- Branch facts are **generation-scoped**: cleared on plane restart
  (`src/integrate/mod.rs:172-190`) and preserved across herdr record rebuilds
  (`src/adapters/herdr.rs:1924-1954`) — attribution correctness depends on it.
- Branch is a value that currently **bypasses redaction** by an in-code TODO
  (`src/integrate/mod.rs:384-389`; `head_subject` is redacted `:399-407`).
  Recorded as an existing, code-documented observation — not a fix in this audit.

## 3. Endpoint consumers (AC1 "endpoint consumers")

| Endpoint/route | What Git/GitHub field reaches here | Consumer | Citation |
|---|---|---|---|
| `GET /snapshot`, SSE `/events` | every folded `Workspace` field above | iOS (SSE + snapshot), `crates/corrald-client` | `docs/ARCHITECTURE.md:58-82`, `:332-345` |
| `GET /issues` | H13–H18 repo-level issues (grouped by H1 repo key) | none bundled (audit-488; external UNKNOWN) | `src/api/issues.rs:84-101` |
| `read_diff` (`POST /drive`) | none (G1 topology informs worktree ownership only) | none live (audit-488 §3) | `src/adapters/herdr.rs:2648-2690` |
| `corrald digest` (offline) | none (history ring is state transitions; no git/gh payload) | operator CLI | `src/history/digest.rs:1-11` |

## 4. Classification summary (AC2)

- **keep**: G1–G6 where folded and consumed (repo/branch/dirty/ahead/behind/pr_number,
  head_sha/head_branch as matching keys), H1, H5, H9 (wire parity), H10–H12 (H12 = keep
  only if the 488 fetch survives), `git_plane_backlog`, `pr_match_source` (debug-only).
- **confirmed unused (in-repo)**: H2 default branch, H3 repo ahead/behind (hardcoded 0),
  H6 PR title, H7 PR state, H8 mergeable. "Confirmed unused" covers **in-repo consumers
  only**; all remain on the internal event contract (`src/core/events.rs`), whose rules
  are additive-only (`src/core/events.rs:12-14`), so removal is a contract change, not a field drop.
- **uncertain**: H9 `ci_status` client rendering, H12/`workspace.issues` iOS-side absence,
  H13–H18 (no bundled consumer; **external UNKNOWN**), G6 `head_subject` display intent.

## 5. Polling/subprocess schedule record (AC3)

Recorded in §1c with exact constants. Client-absence behavior is SWR: **no GitHub
network traffic at all until a first SSE client connects**; afterwards background
300 s cadence while nobody is live, 60 s while a client is connected. GitPlane is
push-driven with the bounded safety nets above. No measurement claims are made.

## 6. Regression surface a future cut must keep green (AC4 scaffold)

Named in-repo suites that already prove the required behaviors and must remain
green for any approved unit: repo/branch attribution and board fields —
`tests/integration.rs` (16 tokio tests; synthetic plane events, incl. branch/repo/
dirty/pr/ci fan-out), `src/core/workspace.rs` tests; multi-host identity —
iOS `MultiHost*` projections in `ios/FleetNotifierTests/FleetNotifierTests.swift`
(host chip/section/filter wiring) and `ios/FleetNotifier/UI/BoardModel.swift:36,60,71,422,430`;
recent output — recents suites (read_tail path, untouched by this audit);
gh mapping — `tests/gh_plane.rs` (13 tokio tests), `tests/git_plane.rs` +
`src/adapters/git_plane.rs` inline tests.

## 7. Retain/remove decision list (owner gate; nothing approved here)

| # | Unit | Recommendation | Impact if removed | Owner decision |
|---|---|---|---|---|
| U1 | GH fields with no in-repo consumer: `default_branch` (H2), repo `ahead/behind` (H3), PR `title/state/mergeable` (H6–H8) | recommend **keep** until a bundled client need is proven absent *and* external usage accepted | internal event-contract change (additive-only rule), GraphQL fragment edit, dedupe equality changes | required |
| U2 | CI verdict `ci_status` (H9) | recommend **keep** (wire parity; crates client + conformance model it) | iOS would lose a field it already ignores; client crate mirror drifts | required |
| U3 | `head_subject` (G6) | recommend **keep** (documented G21 addition; redacted) | wire parity loss; PR line-2 identity use intended | required |
| U4 | Repo-level issues fetch (H13–H18) | **joined with audit-488 D2/D4** — no independent decision here | closing-issue state enrichment degrades (`src/adapters/gh_plane.rs:1194-1211`); `/issues` view empties | required (488) |
| — | Any collector removal | **not authorized** by this audit | — | owner must approve explicitly |

## 8. Appendix A — bounded specs (UNAPPROVED — DRAFT ONLY)

> For U1/U2/U3 only, and only *after* an explicit owner decision that includes the
> external-unknown posture. No follow-up issue filing is authorized.

- **Spec S1 (U1 fields)** — UNAPPROVED: drop the listed GraphQL selections
  (`src/adapters/gh_plane.rs:855,859-861`) and their wire fields; keep dedupe semantics by
  comparing the reduced struct; gates: `tests/gh_plane.rs` mapping tests updated,
  `tests/integration.rs` PR folding unchanged, `crates/corrald-client` conformance
  R11 unchanged (it does not read title/state/mergeable), full repo gates at the
  change head; rollback = revert (no persistent state).
- **Spec S2 (U2/H9 or U3/G6)** — UNAPPROVED, **not recommended**: would require iOS
  model + crates-client mirror edits and a wire-compat note; no data migration.
- **Spec S3 (U4)** — UNAPPROVED and **owned by audit-488** (link, do not duplicate).

## 9. Evidence, commands, and proof extent

Retained logs: `.audit-logs-488-490/` (untracked, preserved).

| Log | Command (verbatim) | Raw result |
|---|---|---|
| `489-negative-searches.log` | `git grep -n 'headSha\|head_sha\|headSubject\|head_subject' -- ios/FleetNotifier/ ios/FleetNotifierTests/` (exit 1); `git grep -n 'CiStatus\|ciStatus' -- ios/FleetNotifier/` (9 hits: 3 demo seed lines 40/276/311 + 6 model lines 24/46/55/60/66/78 — the :60/:66 init pair counted as the two separate matched lines it is, not merged; no UI reader); `git grep -n 'gitPlaneBacklog' -- ios/` (exit 1) | raw exits preserved |
| `489-schedule-constants.log` | `git grep -nE 'const [A-Z_]+.*=\|Duration::from_secs\(' -- src/adapters/git_plane.rs src/adapters/gh_plane.rs` | see log |
| `citations-489.log` | citation verifier at pinned head | see log |

Proof extent for negatives: the three searches above cover the iOS app and
`crates/` at the pinned head only. They prove "no in-repo reader"; they do **not**
establish anything about external clients (UNKNOWN).

## 10. AC coverage (issue #489 checkboxes → sections)

| Checkbox | Where satisfied |
|---|---|
| Producer→field→iOS mapping incl. hidden identity/grouping + endpoint consumers | §1, §2, §3 |
| keep / confirmed unused / uncertain with exact evidence at a pinned head | §2, §4 |
| Polling/subprocess schedules + client-absence; no unmeasured cost presented as result | §1c, §5 |
| Smallest removable units + tests proving attribution/multi-host/board/recents stay correct | §6, §7, §8 |
| Herdr stays authoritative; reconciliation/backoff/concurrency/stale-state preserved | §1c, §2 (`git_plane_backlog`, generation-scoped branch), §6 |
| Retain/remove decision list + bounded specs only; owner approval before removal/filing | §7, §8, header |

## 11. Cross-references (no duplicate invented work)

- #488 owns `read_diff` / `/issues` retirement boundaries (this audit contributes
  the H12/H13 field join and the enrichment dependency).
- #490 owns the history ring; no git/gh payload flows there (§3).
- #441 (Herdr compatibility) and #443 (optional adapter idea, unrouted) are
  referenced by the issue; no second runtime/adapter is proposed here.
- #492 owns code-level daemon investigations; this audit read-only.
