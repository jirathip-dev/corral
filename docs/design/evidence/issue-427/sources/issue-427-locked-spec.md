# Corral #427 — locked design-gate specification

Status: DESIGN GATE LOCKED FOR PROTOTYPE ONLY — implementation is not approved.
Issue: https://github.com/jirathip-dev/corral/issues/427
Repository: jirathip-dev/corral

## Locked decisions

### A. Exploration breadth

Produce three materially distinct phone-width directions:

- A — compact top-left `Filters` affordance opening a native filter sheet;
- B — compact inline filter rail;
- C — one designer wild-card direction.

C may be bold, but must remain Corral-specific, native iOS, Catppuccin-themed,
accessible, and within the existing host/repository data and interaction
contract. The designer recommends one direction only after evidence; Guy's
approval is still required.

### B. Filter semantics and labels

- Host and repository filters remain two independent selections.
- Use explicit scoped actions: `All hosts` and `All repositories`.
- Never present two unexplained `All` labels.
- Selection applies immediately.
- Provide one visible `Reset all filters` action plus scoped clear controls for
  Hosts and Repositories.
- Provide an obvious dismiss/close path.
- Preserve current filter data behavior, composite host identity, stream
  lifecycle, grants, and stale/offline semantics.

### C. Header and surface

- Reclaim the empty top-left header area with one control labeled in the form
  `Filters · 2` when two filters are active.
- Include a concise selected summary such as `Bazzite · corral` before opening.
- Expand to the full scoped lists inside the filter surface.
- Keep Settings on the top-right.
- Do not restore the removed generic `Fleet` title.
- The sheet direction is a native bottom sheet with a drag handle and visible
  board context behind it.
- Use the active theme's translucent/material treatment, with a themed fallback
  surface below the availability gate. The result must be visually verified;
  source modifiers alone are insufficient.

### D. Host status and grouping

- Keep connecting/offline/stale status textual in a compact summary or banner;
  never rely on color alone.
- Inside the filter surface, order Host scope first, including health text, then
  Repository scope.
- Keep Host and Repository visually independent throughout.

### E. Evidence coverage

The prototype evidence must show the full state matrix:

- populated multi-host board;
- host-only filtering;
- repository-only filtering;
- both filters active;
- connecting host;
- offline/stale host;
- zero results.

Cover all four Catppuccin palettes: Latte, Frappé, Macchiato, and Mocha,
including light/dark contrast checks.

Prove both reference phone widths: 390×844 and 375×812. Include Dynamic Type /
accessibility sizes, distinct VoiceOver labels, and 44 pt minimum hit targets.

## Designer deliverables

Docs/evidence-only; do not modify `src/`, `ios/` production code, project files,
or runtime behavior.

- Labeled A/B/C prototype outputs and comparison sheet.
- Phone-sized PNG evidence with labels identifying variant, palette, width, and
  state.
- Interactive or static prototype source under the design-output issue folder.
- README describing the recommendation, rejected tradeoffs, and all state/theme
  coverage.
- Capture log and reproducibility/asset hashes where the design harness uses
  them.
- Canonical evidence path: `docs/design/evidence/issue-427/`.
- Review copy: `~/design-output/corral/427-filter-header/`.
- Post exactly one issue comment beginning `PROTOTYPE — awaiting Guy approval`
  with the artifact links and recommendation, then stop.

## Approval and handoff

This specification authorizes a design prototype only. It does not authorize
source implementation, a production PR, or a design-gate-cleared comment.
After Guy selects a direction, record a separate issue comment beginning
`DESIGN GATE CLEARED — variant <A|B|C>` with the approved evidence paths; only
then may the orchestrator create a bounded implementation route.

## Scope fence

Do not alter host/repository data, composite identity, stream lifecycle, grants,
stale/offline semantics, Settings destructive controls, or the removed Fleet
title. Do not invent metrics or runtime states. Do not use color alone to
communicate host health. Do not route implementation from this prototype brief.
