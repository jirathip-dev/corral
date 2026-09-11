# Lean consumer audits 488 / 489 / 490 — index (docs only)

Pinned head for all three audits (full SHA):
`d754d0af0116977b862ef0abafdf805582afdefe` (worktree
`/Users/jirathip/.herdr/worktrees/corral/lean-audits488490`, verified equal to
the briefed base before any write).

## What this bundle is

Three distinct, pinned-head consumer/retirement-boundary matrices requested by
issues #488, #489, #490, delivered as **docs only**:

| Audit | Deliverable | Scope |
|---|---|---|
| [#488](https://github.com/jirathip-dev/corral/issues/488) | `audit-488-read-diff-and-issues.md` | consumers + retirement boundaries for the `read_diff` capability and `GET /issues` |
| [#489](https://github.com/jirathip-dev/corral/issues/489) | `audit-489-git-github-fields.md` | producer→field→iOS consumer mapping for all Git/GitHub collected fields |
| [#490](https://github.com/jirathip-dev/corral/issues/490) | `audit-490-history-and-digest.md` | persisted history ring / `/history` / digest consumers; durable vs transient |

## What this bundle is NOT

- **No removal authority.** Every retirement spec in every file is explicitly
  **UNAPPROVED — DRAFT ONLY**. Owner approval for retirements remains a hard gate.
- **No code work**: nothing under `src/`, `ios/`, `crates/`, CI, tests, or
  architecture docs was modified. No follow-up issues were filed.
- **No live operations**: no request capture, SSH, telemetry, storage scan,
  removal, or service change was performed for these audits.
- **External usage is UNKNOWN**, not confirmed-unused, wherever in-repo evidence
  cannot prove absence (each audit states its proof extent).

## Owner decision summary (FINAL for this lane — awaiting owner)

| Ref | Decision needed | Recommendation (audit) |
|---|---|---|
| 488-D1 | Retain vs retire `read_diff` | retain (registry-migration + unknown-external blockers) |
| 488-D2 | Retain vs retire `GET /issues` route; if retired, keep the fetch | retain route; fetch must survive either way |
| 488-D3 | Accept/deny bounded specs R1–R3; registry migration stance | no action without owner decision |
| 488-D4 | External-consumer posture (UNKNOWN risk) | commission separately authorized evidence or accept |
| 489-U1..U3 | Remove unused-in-repo GH fields (`default_branch`, repo a/b, PR `title/state/mergeable`) / `ci_status` / `head_subject` | keep pending owner decision |
| 489-U4 | Repo-level issues fetch | joined with 488-D2/D4 (single decision, no duplicate) |
| 490-D1 | Ring / `GET /history` / digest keep-defer posture | keep all three |
| 490-D2 | Any future retirement: existing-user-history retention/migration decision | mandatory before any removal |
| 490-D3 | External-consumer posture for `/history` | commission evidence or accept UNKNOWN |

## Verification and retained evidence

- Every `file:line` citation resolves at the pinned head — verifier:
  `citations.log` with per-document totals (counts derived in code).
- Search/negative evidence and raw exits: `488-*.log`, `489-*.log`, `490-*.log`
  in the lane's retained log dir (`.audit-logs-488-490/`, untracked by design).
- Gates actually run for this docs-only change (no justfile in this repo — `just`
  recipe check recorded as absent): `git diff --check`, gitleaks full-tree scan
  (CI command), and the applicable source checks — raw exits in the lane report.

## Cross-reference boundaries

- #488/#489 join on one fact: the repo-level issues fetch is dual-purpose
  (endpoint view + closing-issue enrichment). The full spec lives once, in 488;
  489 links to it.
- #490 shares only the daemon with the other two; no collection or endpoint
  overlaps the ring.
- #492 (daemon memory root-cause) owns any future code investigation of
  `src/history`/adapters; #480/#453/#459 owners are unaffected by this lane.
