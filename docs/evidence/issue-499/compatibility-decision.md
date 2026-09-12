# #499 compatibility decision — repo-level dead gh facts (default branch + constant counters)

- Issue: #499 ("Lean GitHub: remove unused repository default-branch and constant
  counters"), owner approval comment `5645053551` (Approve implementation,
  review and integration-only merges for #499–501; revalidate consumers at
  current head).
- Lane base (full SHA): `9abfa8975c630a5afdf8baa7e25c58753f9e78e1`
  (worktree `impl499-repo-facts`).
- Status: **decision written BEFORE any production removal.** At authoring
  time `src/core/events.rs` and `src/adapters/gh_plane.rs` were unmodified;
  base hashes recorded by `.logs-499/base-hashes.txt`:
  - `src/core/events.rs` `cf8ec86a926d291df820904b7ec7b1930630e117cf0e23549f06e74fafd8f28e`
  - `src/adapters/gh_plane.rs` `968891751ac0302555c44056f9123ef7478bb95ec1a67f2200197f4de74a032d`
- Unit under decision (audit-489 §2 U1, restricted to the #499 slice): remove
  `GhRepoState.default_branch` (H2) and the repo-level constant-zero
  `ahead`/`behind` counters (H3) end-to-end. PR `title/state/mergeable` (H6–H8)
  are #500's slice and are deliberately untouched here; client-capability edits
  are #501's slice.

## 1. Producer revalidation at the implementation head (`9abfa89`)

| Fact | Producer site (current head) | Detail |
|---|---|---|
| H2 default branch | query selection `src/adapters/gh_plane.rs:855` (`defaultBranchRef { name }` inside the `GhPlaneRepo` fragment, `build_query` `:843-888`); decode `RepoWire.default_branch_ref` `:896-898`; wire struct `DefaultBranchRefWire` `:902-907`; mapping `:1177-1183` | value only ever copied from the GraphQL response onto `GhRepoState.default_branch`; never read again anywhere in-repo |
| H3 repo ahead/behind | mapping `src/adapters/gh_plane.rs:1184-1187` — literal `ahead: 0, behind: 0` with the in-code note "GitHub's API has no ahead/behind vs default branch concept; local tracking info is WS1's job" | constants by construction; no GitHub input ever feeds them |
| Contract fields | `src/core/events.rs:187-195` (`GhRepoState { repo, default_branch, ahead, behind, prs, issues }`) | serialized/deserialized only over the in-process plane channel |

Producer search (raw log + exit): `.logs-499/revalidate-consumers.log`
(`rg -n 'default_branch|defaultBranchRef|DefaultBranchRef' src tests crates docs/...`).

## 2. Consumer revalidation at the implementation head (`9abfa89`)

| Consumer | Site | Reads |
|---|---|---|
| Integrator gh handler | `src/integrate/mod.rs:293-307` (`handle_gh`) | `state.repo`, `state.issues` only (`self.issues.update(&state.repo, issues)` `:306`) |
| Repo re-fold | `src/integrate/mod.rs:421-442` (`reapply_repo`) | repo key + PR/issue facts |
| PR fold | `src/integrate/mod.rs:510-563` (`apply_pr_facts`) | `prs[].pr_number/ci_status/head_sha/head_branch/closing_issues` + local git facts; **no** `default_branch`/`ahead`/`behind` reads |
| Issues view | `src/api/issues.rs:92-101` (`issues_view`) | cache written from `state.issues` only |
| Dedupe | `src/adapters/gh_plane.rs:798-801` | full-struct `PartialEq` compare against last-known (in-memory map, `:622`) |
| Bundled client crate | `crates/corrald-client` | mirrors the HTTP `Workspace` wire model only; **no** `GhRepoState`/`default_branch` reference (search above, zero hits) |

Reader-site search for the removed facts narrowed to direct field access
(`rg 'state\.(default_branch|ahead|behind)|\.default_branch\b' src tests crates`,
exit recorded in the log): zero production reads. All remaining hits are
constructors/assertions in tests and fixtures (the #499 fixture-expectation
cleanup), e.g. `tests/gh_plane.rs:769-774`, `src/adapters/gh_plane.rs:1634-1636`,
`src/integrate/mod.rs:777` etc.

## 3. Additive-only policy and version-obligation analysis

- The event contract's rule is `src/core/events.rs:12-14`: "Additive only. New
  event kinds extend `PlaneEvent`; existing variants and field types never
  change shape." This removal is intentionally a **subtractive contract
  change**, so it may not proceed silently — hence this document.
- Obligation surfaces checked:
  1. **In-process seam only.** `PlaneEvent` travels over the tokio mpsc
     `PlaneSink` (`src/core/events.rs:206`, channel `plane_channel`
     `:221-224`) between planes and the integrator in the same daemon
     process; `GhRepoState` is never persisted
     and never served (no `PlaneEvent`/`GhRepoState` references in
     `src/api/`, `src/history/`, or any on-disk store — search log).
  2. **No cross-version decode.** The last-known dedupe map is a local
     `BTreeMap` inside `run_forever` (`src/adapters/gh_plane.rs:622`), dropped
     on restart; nothing deserializes a previously-written `GhRepoState`.
     Removal cannot strand stored data.
  3. **No bundled-client obligation.** The only bundled client crate mirrors
     the HTTP surface (snapshot/SSE `Workspace`), which never carries these
     repo-level facts.
  4. **External usage** remains UNKNOWN exactly as audit-489 §9 recorded;
     the owner approval scoping this lane explicitly covers the bounded #499
     ACs, and the audit's U1 owner decision gate is satisfied for this slice
     by approval `5645053551` (scoped to #499-501 only).
- **Decision: PROCEED.** No consumer/version obligation was found; no
  unresolved breaking-policy blocker exists once the owner-approved exception
  is recorded here and on the `GhRepoState` item itself. If a future consumer
  or persisted copy of the removed facts appears, this decision must be
  revisited before any further subtraction.

## 4. Dedupe semantics after removal (documented, intended)

`process_response` emits a repo only when the reduced `GhRepoState` differs
from the last one. The removed facts were either never folded
(`default_branch`) or constants (repo `ahead/behind` = 0). Dropping them from
the compared struct can only *shrink* the set of emissions for a fact no
surface consumes (e.g. a default-branch rename alone no longer re-emits); it
cannot suppress or reorder any PR/issue/CI emission. PR and issue vectors stay
sorted by number before the compare (`src/adapters/gh_plane.rs:1175-1176`), so
`UPDATED_AT` ties still cannot flip order and defeat the dedupe.

## 5. Rollback / migration

- Rollback = source revert of the removal commit (single local commit; the
  GraphQL fragment and mapping return together). No persistent data exists to
  migrate in either direction: no schema/table/db write touches `GhRepoState`,
  and the only stateful structure (last-known map) is recomputed from the
  first poll after restart.
- No replacement dummy values and no hidden extra request: the poll stays one
  aliased GraphQL POST (`src/adapters/gh_plane.rs:706-763`), one
  `repository(...)` clause per spec; the removal deletes a selection, never
  adds one.

## 6. Boundaries preserved (not touched by this unit)

Local GitPlane ahead/behind/dirty (`GitStatus` `src/core/events.rs:42-52`,
fold `src/integrate/mod.rs:408-412`), repo identity/grouping, SHA+branch PR
matching (`src/integrate/mod.rs:514-531`), OPEN filtering, read_tail/read_diff,
polling cadence/backoff/partial-null cache behavior, security/wire/identity/
reconnect boundaries: all untouched; regression coverage listed in the lane
report.
