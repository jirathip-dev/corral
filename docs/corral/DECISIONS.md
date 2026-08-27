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
