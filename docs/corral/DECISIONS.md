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

Transcript reading remains a separate, on-demand, grant-gated capability. Its
store binding is bounded and read-only, its roots are configurable with
`CORRAL_OPENCODE_DB`, `CORRAL_CLAUDE_DIR`, and `CORRAL_CODEX_DIR`, and all
entries are redacted before leaving the transcript module. Transcript data is
never folded into the board model.
