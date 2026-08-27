# Corral decisions

This file records decisions that still shape the current control plane. Phase
briefs and review notes are historical context; the README, architecture,
operations, and development guides describe the supported system.

## D34 (retired 2026-08-20) — remove provider-usage estimation

The provider-specific spend estimator from the earlier G34 workstream was
retired in issue #107. It inferred dollars, plan percentages, and exhaustion
alerts from external tool session stores, which could not represent a
provider's subscription quota reliably. Corral must not present those
estimates as fleet state or quota truth.

The canonical agent model and snapshot wire shape therefore contain no
provider pricing, quota, or spend fields. The daemon has no estimator route,
background scan, alert watchdog, pricing table, cap configuration, or desktop
usage panel. The egui, iOS, and shared client models mirror the same
harness-agnostic agent shape.

Transcript reading remained a separate, on-demand, grant-gated capability
(issue #63): its store binding was bounded and read-only, its roots were
configurable with `CORRAL_OPENCODE_DB`, `CORRAL_CLAUDE_DIR`, and
`CORRAL_CODEX_DIR`, and all entries were redacted before leaving the
transcript module. Transcript data never fed the board model. The whole
transcript/full-chat surface was retired in 2026-08-27 by D35 below.

## D35 (retired 2026-08-27) — remove transcript / full-chat surface

Guy (2026-08-27): all fleets now run on hermes lanes; `corrald`'s
`GET /transcript` bind ladder only resolved stores for opencode / claude /
codex and no hermes store arm existed, so `/transcript` served zero live
agents. The surface was dropped END-TO-END instead of adding a hermes
store:

- corrald: `GET /transcript` route, handler, paging/auth, and
  `src/transcript/` (store binding, per-store page readers, `TranscriptRoots`,
  the `CORRAL_*_DIR` env hooks, `TranscriptLimiter`) were removed.
  Pane-tail blocks (`TranscriptBlock` + segmentation) moved to
  `src/core/blocks.rs` because the `/drive` `read_tail` response still
  serves them additively.
- iOS FleetNotifier and the egui board: the Full chat action, transcript
  panes/pages, and transcript fetch helpers were removed; the Recent output
  surface renders `read_tail` (live bounded tail + block markers) only.
- `read_tail` capability, grant, and `/drive` delivery are UNCHANGED —
  the bounded redacted pane tail is the only agent output a client can
  read.

The paged older-history UX shipped by #205 is superseded by this removal
(#205 remains closed; the removal was recorded in #241).

## D36 (2026-08-28) — read_diff: worktree diff on the board (#232)

Guy (2026-08-26 → 2026-08-27): Approve is blind today — the approver sees
branch/PR/CI and the live tail, but never what the agent changed in its
herdr worktree, and approvals happen BEFORE push. Diff is evidence for a
steer decision, so Corral gets a surface for it; file browsing/editing
stays in the IDE/terminal lane (explicit non-goal).

- **One shared endpoint.** new `read_diff` capability → bounded
  `/drive` response: diffstat + changed-files list + paged unified diff
  (client walks `next_offset`; files capped 1..=128, lines 1..=400,
  default 200, per-line 4096-char truncation, 64 KiB page budget).
  Computed via libgit2 (vendored) — never a git subprocess — via
  `diff_tree_to_workdir_with_index` (tracked changes vs HEAD, staged +
  unstaged; untracked excluded).
- **Herdr-owned paths only.** the adapter resolves the path from
  snapshot state (`workspace.worktree_path`), verifies it is under the
  configured worktrees root, and refuses anything else with
  `no_worktree` (`ok:false`). Client-supplied paths are never accepted.
- **Granted like read_tail.** new readonly capability, default-empty,
  per-device, audited (one audit entry per served page), redacted at the
  adapter boundary; 403 `not_granted` without the grant.
- **Lazy client surfaces.** egui: `DIFF` column (+N/−M), row-expand diff
  section (files | paged diff, "Load next"); iOS: ± Diff button (V2
  toolbar) → diff sheet (header, diffstat, files, paged diff). Never
  prefetched fleet-wide (same D5 stance as read_tail).
- **git2 chosen over shelling out.** the git_plane shells out to `git`
  for status under a four-command admission budget; an on-demand diff
  does not fit that budget, and libgit2 gives a subprocess-free, vetted
  diff engine for exactly the worktree-vs-HEAD view this feature needs.
