# Lean audit 488 — consumers and retirement boundaries for `read_diff` and `GET /issues`

- Issue: #488 (read each GitHub issue body FIRST; no comments existed at audit time).
- Pinned head (full SHA): `d754d0af0116977b862ef0abafdf805582afdefe`.
  Every `file:line` citation in this document is a read at this exact head.
  The issue's own source anchors were written at historical `064016e0d53f5c9da46dc8d70b72eabcb24bb50d`
  and are re-resolved here against the current head.
- Status: **AUDIT ONLY — no removal authority.** Nothing in this document
  grants retirement, code changes, follow-up issues, or live operations.
  The owner decision gate is §7. Removal specs in §6 are **UNAPPROVED drafts**.
- External-usage caveat (AC2): repo-scoped searches can never prove the absence of
  external consumers. This audit ran **no** telemetry, request capture, SSH, or
  storage scan (read-only source + issues only), so any not-in-repo consumer is
  **UNKNOWN**, not confirmed-unused. "No bundled UI" is not treated as "no consumer".

Citation format: `path:line` — resolved at the pinned head above. Verification
script and raw outputs: `.audit-logs-488-490/` (see §8; logs are retained in the
lane worktree, untracked).

## 1. `read_diff` — route/capability/schema inventory

| Element | Location (file:line) | Notes |
|---|---|---|
| Capability name `read_diff` (parse/display) | `src/drive/mod.rs:70-80`, `src/drive/mod.rs:61-68` | Closed set: `read_tail`, `read_diff` only. |
| Capability enum | `src/drive/mod.rs:53-59` | `#[serde(rename_all = "snake_case")]`. |
| Typed payload kind | `src/drive/mod.rs:107-114` | `{kind:"read_diff", files?, offset?, lines?}`. |
| Bounds (schema constants) | `src/drive/mod.rs:43-46` | 128 files; 200 default / 400 max lines; 64 KiB page. |
| Query clamp | `src/drive/mod.rs:303-319` | Daemon-side clamp, never trust the client. |
| Response schema | `src/drive/mod.rs:323-336`, `src/drive/mod.rs:346-369` | diffstat + changed-files + paged lines + offsets. |
| HTTP dispatch | `src/api/drive.rs:293-306`, `src/api/drive.rs:589` | `POST /drive` only; capability parsed before authorizer. |
| Auth + refusals | `src/api/drive.rs:485-493`, `src/api/drive.rs:321-436` | Default deny; typed `unknown_capability`/`not_granted`. |
| Engine (producer) | `src/core/diff.rs:119` (entry), `src/core/diff.rs:98` (redact), `src/core/diff.rs:31` (per-line cap) | libgit2 only — no `git` subprocess on this path. |
| Adapter seam | `src/adapters/mod.rs:28-30`, `src/adapters/mod.rs:148-162` | Default `NotImplemented`; herdr overrides. |
| Adapter impl (herdr-owned paths only) | `src/adapters/herdr.rs:2648-2690`, ownership check `:2676-2680` | Path from snapshot `workspace.worktree_path`; never client-chosen. |
| Advertisement | `src/core/model.rs:170`, stamp `src/adapters/herdr.rs:1786` | `CAPABILITIES = ["read_tail","read_diff"]` on every herdr agent. |
| Guard test (advertisement) | `src/core/model.rs:265-283` | Fails if `read_diff` leaves the advertised set while the parser accepts it. |
| Grant semantics | `src/auth/registry.rs:44`, `src/auth/registry.rs:4` | Grant = `Vec<Capability>`; freshly registered device has none. |
| Registry load behavior | `src/auth/registry.rs:210-211` | Any unknown grant string → **daemon load fails closed** ("corrupt registry"). |
| Audit | `tests/drive.rs:1087` (Executed), `tests/drive.rs:1200` (Refused) | One audit entry per dispatched page; auth failures never audited. |

## 2. Consumer matrix (every surface the issue names)

Legend: producer = what writes/serves it; bundled clients = in-repo clients;
scripts/CI = scripts and workflows in this repo; external = not provable from here.

| Surface | Producer | Bundled clients | Tests | Docs | Scripts/CI | External |
|---|---|---|---|---|---|---|
| `read_diff` capability gateway (`POST /drive`) | `src/api/drive.rs` (daemon) | iOS: wire enum only, no live dispatch (`ios/FleetNotifier/Models/Models.swift:34`; sole live call site is `.readTail`, `ios/FleetNotifier/App/AppModel.swift:2823`). `crates/corrald-client`: mirror types only (`crates/corrald-client/src/drive.rs:24-27`, `:66-75`). Desktop/egui removed (#376). | `tests/drive.rs:1087,1138,1171,1200`; `tests/auth.rs:180-200`; `src/drive/mod.rs:371-456` | `docs/ARCHITECTURE.md:45-50`, `:257-273`; `docs/OPERATIONS.md:468-469`, `:491`, `:637` | none (search exit 1 — §8) | **UNKNOWN** |
| `read_diff` compute engine (libgit2) | `src/core/diff.rs` | (server-internal) | adapter tests `src/adapters/herdr.rs:6678,6724,6770,6790` | `docs/corral/DECISIONS.md:52-81` (D36) | none | n/a (internal) |
| `read_diff` grant parsing | `src/auth/registry.rs` | out-of-band `registry.json` (operator) | `tests/auth.rs:180-200`, `src/auth/registry.rs:584-647` | `docs/OPERATIONS.md:284-291` | none | **UNKNOWN** (operator-managed registries) |
| iOS demo-only read_diff fixture | `iOS DemoFleet` (DEBUG only) | iOS demo (`ios/FleetNotifier/Demo/DemoFleet.swift:142-144`; enum `#if DEBUG` at `:1`; doc "the one demo drive is read_tail" `:5-6`; demo grants `ios/FleetNotifier/App/AppModel.swift:479-486`) | `ios/FleetNotifierTests/FleetNotifierTests.swift:146,4678,6431-6436` | `ios/check-release-demo.py:56-58`, `:91-92` (historical pin notes only) | none | n/a (DEBUG) |
| Release-boundary check (iOS source pins) | `ios/check-release-demo.py` | n/a | self-test in file | n/a | iOS CI `ios-art.yml` | n/a |
| `GET /issues` route (view) | `src/api/issues.rs` (handler `:84-101`) | **no bundled client** — Issues UI removed #354 (`docs/ARCHITECTURE.md:148-160`); iOS has no `/issues` reference (search §8). | `tests/http.rs:233-242`, `:244-290`, `:541-585`, `:796-816`; unit `src/api/issues.rs:107-146` | `docs/ARCHITECTURE.md:81`, `:148-160`, `:282`, `:362`; `docs/OPERATIONS.md:275`, `:490`, `:546`, `:586-594`; `docs/QUICKSTART.md:35`; `docs/corral/P4-conformance.md:23` | none (search exit 1 — §8) | **UNKNOWN** |
| `/issues` cache (source of the view) | `Integrator` writes `Integrator::handle_gh` (`src/integrate/mod.rs:293-308`), prune `:347-348` | (internal) | `src/api/issues.rs:107-146`; `tests/http.rs:244-290` | `src/api/issues.rs:1-26` (auth-scope doc) | none | n/a |
| Repo-level gh issues fetch (dual-purpose leg) | `src/adapters/gh_plane.rs:879-887` (query), `:1152-1172` (map) | internal | `tests/gh_plane.rs` (13 tokio tests), `tests/integrate` fixtures | `docs/ARCHITECTURE.md:148-160` | none | n/a |
| Closing-issue enrichment (same fetch) | `src/adapters/gh_plane.rs:1194-1211` | rides snapshot `workspace.issues` (`src/integrate/mod.rs:550`); iOS drops the field (#354); `crates/corrald-client` retains it (`crates/corrald-client/src/model.rs:131`) | conformance R11 `crates/corrald-client/tests/conformance.rs:1056,1086` | `src/core/events.rs:96-121` | none | n/a |
| `IssuesCache::get` guard helper | `src/api/issues.rs:65-71` | **no production caller** — the worktree action it served was removed in #354; live callers are tests only (`src/integrate/mod.rs:996,1000,1033,1056,1066`; `src/main.rs:558,563`) | `tests/integrate` fixtures | `src/api/issues.rs:64-71` | none | n/a |
| `crates/corrald-client` `CAPABILITIES` const | `crates/corrald-client/src/model.rs:146-154` | **stale + unused**: 7 entries including removed `prompt/interrupt/approve/kill/attach`; no usage in the crate (only the definition matches). | n/a | `crates/corrald-client/src/model.rs:142-145` comment claims sync with daemon | none | n/a |

## 3. Bundled-client detail (negative evidence, proof extent)

- **iOS never dispatches `read_diff` over the wire.** The only production drive
  call site is `drive(capability: .readTail, …)` at `ios/FleetNotifier/App/AppModel.swift:2823`;
  `readDiff` appears only in the wire enum (`ios/FleetNotifier/Models/Models.swift:34`),
  a DEBUG demo grant set (`ios/FleetNotifier/App/AppModel.swift:479-486`, `#if DEBUG` at `:482`),
  and a never-invoked demo dispatcher arm (`ios/FleetNotifier/Demo/DemoFleet.swift:142-144`;
  the single demo caller passes `.readTail`, `ios/FleetNotifier/App/AppModel.swift:3397-3398`).
  Search: `git grep -n 'drive(capability' -- ios/` (raw log §8).
- **`crates/corrald-client` has no read_diff scenario.** `git grep -n 'read_diff\|read_issues' -- crates/corrald-client/tests/conformance.rs` → no matches (exit 1).
- **Scripts/CI do not call either surface.** `git grep -nE 'read_diff|ReadDiff' -- scripts/ .github/` → no matches; `git grep -n '/issues' -- scripts/ .github/` → no matches.
- `tests/readonly_cut.rs:262` pins that the *removed* `read_issues` drive name is refused as `unknown_capability` — i.e. `/issues` has no signed sibling.
- Historical evidence (not a current consumer): `docs/design/evidence/issue-354/README.md`,
  `docs/design/evidence/issue-267/README.md` (diff/issue design gate), and
  `docs/design/evidence/issue-316/implementation/capture.log` mention `read_diff`.

## 4. Shared data that must survive any endpoint retirement

1. **Board/recents enrichment does NOT depend on the `/issues` endpoint.**
   Repo categories are the live Herdr `workspace.repo` values (`src/api/repo.rs:18-37`,
   `src/api/repo.rs:42-50`); the integrator prunes the cache to them
   (`src/integrate/mod.rs:347-348`). Deleting only the route changes nothing for
   the board or recents.
2. **The repo-level issues fetch is dual-purpose.** The same poll's `issues` leg
   enriches each PR's `closingIssuesReferences` **state** (`src/adapters/gh_plane.rs:1194-1211`)
   before `workspace.issues` rides the snapshot (`src/integrate/mod.rs:550`).
   Removing the *fetch* (not just the route) would leave closing refs present but
   `"UNKNOWN"` — a silent truthfulness loss for a field the wire still carries.
   This is the live enrichment the issue warns about, and the reason §6's specs
   separate endpoint retirement from fetch retirement.
3. **`read_diff` feeds nothing else.** Board `dirty`/`ahead`/`behind` come from
   GitPlane status (`src/integrate/mod.rs:408-412`); recents come from `read_tail`
   (`docs/ARCHITECTURE.md:260-264`). Retiring the capability cannot remove live
   board enrichment — but see the compatibility blockers in §5 before treating
   that as sufficient to retire it.
4. Shared helpers touched by both surfaces: `live_workspace_repos`/`normalize_categories`
   (`src/api/repo.rs`) and the CORS read-plane layer (`src/api/mod.rs:148-151`).

## 5. Retain-or-retire proposals (evidence-based; owner decides)

### `read_diff` — proposed RETAIN (status quo), owner decision required
- Compatibility impact if retired: the name stops parsing → signed callers receive
  `400 unknown_capability` (`src/api/drive.rs:373-381`); advertisement must drop
  from `CAPABILITIES` (`src/core/model.rs:170`) or the guard test fails
  (`src/core/model.rs:265-283`); **any host registry granting `read_diff` fails
  daemon load** until migrated out-of-band (`src/auth/registry.rs:210-211`,
  procedure `docs/OPERATIONS.md:284-291`). `read_tail` is unaffected either way.
- Dependencies of the surface: capability/payload contract, dispatch arm, adapter
  seam + herdr impl, `src/core/diff.rs`, the `READ_DIFF_*` constants.
- Rollback consideration: a removal is a plain source revert (no persisted state,
  no data migration); audit-log history (hash-chained) is untouched.
- Rationale for retain: (a) external consumers UNKNOWN (§0 caveat); (b) removal is
  a breaking wire change with a registry-migration prerequisite; (c) the #354 cut
  deliberately kept it "for wire compatibility" (`docs/ARCHITECTURE.md:45-50`).

### `GET /issues` — proposed RETAIN the endpoint pending owner decision; if retired, **keep the fetch**
- Compatibility impact: route removal changes a documented credential-free read
  endpoint (`docs/OPERATIONS.md:490`, `docs/QUICKSTART.md:35`); external consumers
  UNKNOWN. No bundled client breaks (none consumes it).
- If retired: keep the gh `issues` leg (enrichment, §4.2) and the `IssuesCache`
  writer; drop only route+handler+view. If the **fetch** were also retired,
  snapshot closing-issue states degrade to `"UNKNOWN"` — a product-truth change
  requiring explicit owner acceptance, not an audit default.
- Rollback: source revert; cache is derived, no user data involved.

### Always-rejected baselines
- "UI absent → source cut" is **not** accepted by this audit (`no bundled UI` ≠ no consumer).
- "Repo grep found nothing → confirmed unused" is **not** accepted for external surfaces.

## 6. Appendix A — bounded removal spec drafts (**UNAPPROVED — DRAFT ONLY**)

> Owner approval for retirements is absent. These drafts exist so an approval can
> be bounded; they are **not** approved candidates, not routed, not to be filed.

### Spec R1 — retire `read_diff` end-to-end (UNAPPROVED)
- Preconditions: owner approval; registry-migration decision (hard fail vs tolerant
  parse for `"read_diff"` grants); explicit external-consumer acceptance.
- Removal set: `src/drive/mod.rs` (variant, payload, `READ_DIFF_*`, `ReadDiffQuery`,
  `ReadDiffResult`), `src/api/drive.rs` (dispatch arm), `src/adapters/mod.rs` +
  `src/adapters/herdr.rs` (seam + impl + 4 adapter tests), `src/core/diff.rs`
  (whole module), `src/core/model.rs` (`CAPABILITIES` entry; flip the guard test
  to an explicit removal assertion), iOS enum case `ios/FleetNotifier/Models/Models.swift:34` + demo arm
  `ios/FleetNotifier/Demo/DemoFleet.swift:142-144` + demo grant `ios/FleetNotifier/App/AppModel.swift:484`, `crates/corrald-client`
  mirror types (`crates/corrald-client/src/drive.rs:24-27,66-75`), stale `crates/corrald-client/src/model.rs:146-154` cleanup, docs
  (`ARCHITECTURE.md`, `OPERATIONS.md`, `QUICKSTART.md`, `DEVELOPING.md`, D36 note).
- Negative reachability gates: (a) signed `read_diff` drive returns
  `unknown_capability` before authorizer/dispatch/audit (pattern:
  `tests/readonly_cut.rs:238-267`); (b) pinned-extent source search proves no
  remaining references (same scopes as §8); (c) registry fixture containing
  `"read_diff"` behaves per the migration decision; (d) iOS source-wiring test
  asserts the removed case is gone; (e) full repo gates at the removal head.
- Rollback: revert; no persistent data touched.
- Explicitly out of scope: any `read_tail` change; any registry edit done live.

### Spec R2 — retire the `/issues` route only (UNAPPROVED)
- Keep: gh issues fetch + enrichment (§4.2), `IssuesCache` writer, integrator prune.
- Remove: `src/api/mod.rs:147` route, `src/api/issues.rs` handler/view (keep cache
  type or fold it), CORS/read-plane doc mentions, `docs/*` route references.
- Gates: route absent (404) (pattern `tests/http.rs:796-816`); snapshot enrichment
  unchanged (pattern `src/integrate/mod.rs:1009-1033`); board grouping tests unchanged.
- Data preservation: none required (derived cache).

### Spec R3 — retire the repo-level issues fetch (UNAPPROVED, NOT RECOMMENDED)
- Would flip snapshot closing-issue states to `"UNKNOWN"` (`src/adapters/gh_plane.rs:1194-1211`);
  requires explicit owner acceptance of that product-truth change; otherwise reject.

## 7. Owner decision gate (blocking)

| # | Decision | Evidence anchor |
|---|---|---|
| D1 | Retain vs retire `read_diff` (recommend retain) | §1, §5 |
| D2 | Retain vs retire the `/issues` route (recommend retain; if retire, keep fetch) | §2, §4, §5 |
| D3 | If any retirement approved: accept/deny the bounded specs R1–R3, and decide the `registry.json` migration stance | §6 |
| D4 | External-consumer posture: accept UNKNOWN risk, or commission separately authorized evidence before deciding | §0 caveat |

No item above is approved by this audit, by PASS, or by the existence of a draft spec.

## 8. Evidence, commands, and proof extent

Retained log dir: `.audit-logs-488-490/` in the lane worktree (untracked; preserved).

| Log | Command (verbatim) | Raw result |
|---|---|---|
| `488-read-diff-search.log` | `git grep -nE 'read_diff\|ReadDiff\|READ_DIFF' -- . ':!Cargo.lock'` | 181 matches / 31 files (derived: `counts.log`) |
| `488-read-issues-search.log` | `git grep -nE 'read_issues\|ReadIssues' -- .` | 13 matches |
| `488-issues-route-search.log` | `git grep -nE '/issues' -- . ':!Cargo.lock'` | 91 matches |
| `488-negative-searches.log` | `git grep -nE 'read_diff\|ReadDiff' -- scripts/ .github/` (exit 1); `git grep -n '/issues' -- scripts/ .github/` (exit 1); `git grep -n 'drive(capability' -- ios/` | raw exits preserved |
| `citations-488.log` | citation verifier (every `path:line` in this doc resolved at the pinned head) | see `.audit-logs-488-490/citations.log` |
| `counts.log` | count derivation script (code, not head) | see log |

Proof extent for negatives: searches covered **this repo at the pinned head only**
(tracked files; `Cargo.lock` excluded from the keyword scan). No host-level scan,
no network capture, no registry inspection, no sibling-repo scan was performed or
authorized. Therefore: "no in-repo consumer" is proven for the scopes above;
"no external consumer" is **not established** (UNKNOWN).

## 9. AC coverage (issue #488 checkboxes → sections)

| Checkbox | Where satisfied |
|---|---|
| Pinned-head consumer matrix: route/capability/schema, producers, bundled clients, tests, docs, scripts, external | §1, §2, §3 |
| Distinguish confirmed-unused from unknown external usage; authorized read-only evidence only | §0 caveat, §3, §8 |
| Retain/retire recommendation + compatibility/versioning impact + dependencies + rollback | §5 |
| One bounded removal spec per candidate + cleanup + negative reachability gates; no follow-up issues | §6 (all marked UNAPPROVED) |
| Shared data still needed by board/recents explained | §4 |
| Return audit + owner decision gate; no removal, no PASS-as-permission | §7, header status |

## 10. Cross-references (no duplicate invented work)

- #489 owns the producer→field→iOS mapping of Git/GitHub collection (this audit's
  §4.2 enrichment finding is a join, not a second investigation).
- #490 owns the history ring / digest audit; `/issues` and `/history` are sibling
  credential-free endpoints but have independent consumers.
- #492 (daemon memory root-cause) owns any future `src/` code investigation here;
  this document never touches code.
