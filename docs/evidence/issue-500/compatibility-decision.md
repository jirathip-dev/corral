# #500 compatibility decision — PR-record dead gh facts (title / state / mergeable)

- Issue: #500 ("Lean GitHub: remove unused PR title/state/mergeability
  collection"), owner approval comment `5645053974` ("Approve implementation,
  review and integration-only merges for #499-501; revalidate consumers at
  current head"; quoted on the issue thread).
- Lane base (full SHA): `619b454de95f3890226e5842d964ac411dd1e0c4`
  (worktree `impl500-pr-fields`; the head that already includes #499).
- Status: **decision written BEFORE any production removal.** Base hashes
  recorded by `base-hashes.txt` in the lane evidence dir
  (`/Users/jirathip/.config/fleet-operations/run/corral/evidence/impl500-pr-fields/`):
  - `src/adapters/gh_plane.rs` `5dd1291313195668cdbc8faed648ced158e6f454f84e52a0df26307de3998c6d`
  - `src/core/events.rs` `ac09d4b359016ea2f0f8bb58b55922b40ce0ce9784e390b0e3ee4ec2def4739b`
  - `src/integrate/mod.rs` `78a5c2d056fad7fd5b9e26b2586454efba992fc327c5cc426318c3338b86fd69`
- Unit under decision (audit-489 §2, U1 slice restricted to #500): remove
  the PR-record facts H6 `GhPrState.title`, H7 `GhPrState.state`, H8
  `GhPrState.mergeable` end-to-end. The repo-level facts (#499: H2
  `default_branch`, H3 repo `ahead/behind`) are **already removed** at this
  base and are deliberately not re-opened.

## 1. Producer revalidation at the implementation head (`619b454d`)

| Fact | Producer site (current head) | Detail |
|---|---|---|
| H6 PR title | query selection `src/adapters/gh_plane.rs:858`; decode `PrWire.title: Option<String>` `:910`; mapping `src/adapters/gh_plane.rs:1213` | value only ever copied from the GraphQL response onto `GhPrState.title`; never read again anywhere in-repo |
| H7 PR state | query `:859`; wire `:911`; mapping `:1214` | the PR leg already filters `states: OPEN` (`:855`), so the field is constant `"OPEN"` for every polled PR; never read |
| H8 PR mergeable | query `:860`; wire `:912`; mapping `:1215-1218` | copied; never read |

Producer search (raw log + exits): `revalidate-producers.log`
(`rg -n 'mergeable' src tests crates`; ast-grep field reads `$A.mergeable`,
`$A.title`, `$A.state`, each with its raw exit recorded). The only
`$A.mergeable` hits in the whole workspace are the two lines of
`normalize_pr`'s own construction — zero readers.

## 2. Consumer revalidation at the implementation head (`619b454d`)

| Consumer | Site | Reads |
|---|---|---|
| Integrator gh handler | `src/integrate/mod.rs:293-307` (`handle_gh`) | `state.repo`, `state.issues` only |
| PR fold | `src/integrate/mod.rs:510-551` (`apply_pr_facts`) | `prs[].pr_number/ci_status/head_sha/head_branch/closing_issues` + local git facts; **zero** `title`/`state`/`mergeable` reads |
| Dedupe | `src/adapters/gh_plane.rs:797-803` | full-struct `PartialEq` against last-known (in-memory map, `:622`) |
| Issues view | `src/api/issues.rs` | cache written from `state.issues` only — issue `state`/`title` stay untouched |
| Persistence surfaces | `src/api`, `src/history`, `src/core/store.rs`, `src/core/model.rs` | **zero** `GhPrState`/`GhRepoState` references (consumer sweep F, `rg` exit 1 = no match) |
| Bundled client | `crates/corrald-client` | **zero** hits for `mergeable`/`GhPrState`/`pr_title`/`pr_state` (sweep G, exit 1); the R11 conformance harness queries only `number`/`headRefName`/`headRefOid`/`closingIssuesReferences` and asserts through the HTTP `Workspace` (`pr_number`/`ci_status`/`head_sha`), which never carried these PR facts |

Other constructions outside the producer/tests: `src/main.rs:560` builds an
issue-only `GhRepoState` with `..Default::default()` — unaffected.

Consumer search (raw log + exits): `revalidate-consumers.log` (sweeps E–H).

## 3. Additive-only policy and version-obligation analysis

- The event contract's rule is `src/core/events.rs:12-14` ("Additive only.
  New event kinds extend `PlaneEvent`; existing variants and field types
  never change shape."). This removal is intentionally a **subtractive
  contract change**, so it may not proceed silently — hence this document.
- Obligation surfaces checked:
  1. **In-process seam only.** `PlaneEvent` travels over the tokio mpsc
     `PlaneSink` (`src/core/events.rs:206`, channel `plane_channel`
     `:221-224`) between planes and the integrator in the same daemon
     process; `GhRepoState` is never persisted and never served (sweep F:
     no references in `src/api/`, `src/history/`, `src/core/store.rs`).
  2. **No cross-version decode.** The last-known dedupe map is a local
     `BTreeMap` inside the plane (`src/adapters/gh_plane.rs:622`), dropped
     on restart; nothing deserializes a previously-written `GhRepoState`.
     Removal cannot strand stored data.
  3. **No bundled-client obligation.** The only bundled client crate mirrors
     the HTTP surface (snapshot/SSE `Workspace`), which never carries these
     PR facts (sweep G).
  4. **External usage** remains UNKNOWN exactly as audit-489 §9 recorded;
     the owner approval scoping this lane explicitly covers the bounded
     #500 ACs.
- **Decision: PROCEED.** No consumer/version obligation was found; no
  unresolved breaking-policy blocker exists once the owner-approved
  exception is recorded here. If a future consumer or persisted copy of the
  removed facts appears, this decision must be revisited before any further
  subtraction.

## 4. Dedupe/equality semantics after removal (documented, intended)

`process_response` emits a repo only when the reduced `GhRepoState` differs
from the last one. The removed facts were never folded into workspace data;
`state` was constant `"OPEN"` by query construction. Dropping them from the
compared struct can only *shrink* the set of emissions:

- a controlled title-only rename no longer re-emits — the AC3 "no
  meaningless event" direction, pinned by
  `tests/gh_plane.rs::title_or_mergeability_only_change_does_not_emit`
  (RED/GREEN discriminating fixture);
- a mergeability flapping (UNKNOWN <-> CONFLICTING) no longer re-emits —
  same test;
- every retained PR fact (`pr_number`, `head_sha`, `head_branch`,
  `ci_status`, `closing_issues`) still compares and re-emits, and the fold
  still updates canonical state — pinned by
  `tests/gh_plane.rs::retained_pr_fact_change_still_emits_and_updates_state`.

PR and issue vectors stay sorted by number before the compare
(`src/adapters/gh_plane.rs:1165-1168`), so `UPDATED_AT` ties still cannot
flip order and defeat the dedupe.

## 5. Rollback / migration

- Rollback = source revert of the removal commit (single local commit; the
  GraphQL fragments, `PrWire` decode and `normalize_pr` mapping return
  together). No persistent data exists to migrate in either direction: no
  schema/table/db write touches `GhRepoState`, and the only stateful
  structure (last-known map) is recomputed from the first poll after
  restart.
- No replacement dummy values and no hidden extra request: the poll stays
  one aliased GraphQL POST (`src/adapters/gh_plane.rs:711-760`), one
  `repository(...)` clause per spec; the removal deletes selections, never
  adds one (asserted in the retained query test).
- No performance claim: the audit recorded no measured CPU/memory savings
  for this slice and none is made here.

## 6. Boundaries preserved (not touched by this unit)

OPEN filtering (`states: OPEN`), PR limit/order (`first: 20`,
`UPDATED_AT DESC`), PR number, head SHA, head branch, `ci_status` collapse,
closing-issue enrichment and the repo-level issue leg (issue
`state`/`title`/`labels`/`url`/`body`/`comments`) all stay. SHA-match
precedence, branch fallback, bound-PR survival and reset-on-disappearance
(`src/integrate/mod.rs:518-545`) are untouched; no HTTP wire field or local
Git-data field changes (GitPlane and `Workspace` untouched).
