PROTOTYPE — awaiting Guy approval

No product source changed; this is docs/evidence-only design work. Implementation remains blocked pending your explicit selection.

Artifacts
- Interactive A/B/C prototype: https://jirathips-macbook-air.tail8c3301.ts.net:8444/corral/427-filter-header/index.html
- Side-by-side comparison: https://jirathips-macbook-air.tail8c3301.ts.net:8444/corral/427-filter-header/comparison.html
- Canonical repo evidence: https://github.com/jirathip-dev/corral/tree/g427-filter-header-design/docs/design/evidence/issue-427
- Review archive: https://github.com/jirathip-dev/design-output/tree/main/corral/427-filter-header
- Review copy on host: `~/design-output/corral/427-filter-header/`

Directions
- A — Compact top-left Filters control plus native material bottom sheet. Recommended: it reclaims the board header while keeping full host health and repository labels reachable in a familiar iOS surface.
- B — Compact inline rail with independent host/repository disclosure menus. Fastest repeated switching, but it permanently spends more board height.
- C — Corral Scope Path editor. Strongest host-first mental model, but filtering temporarily replaces the monitored board.

Coverage
- 168 labeled matrix PNGs: 3 directions × 7 required states × 4 Catppuccin palettes × 2 phone widths (390×844 and 375×812).
- 6 Dynamic Type/accessibility captures plus distinct VoiceOver-label, 44 pt target, horizontal-overflow, scroll-reach, console, interaction, and contrast gates.
- Required states: populated multi-host, host-only, repository-only, both filters, connecting host, offline/stale host, and zero results.

Host/repository labels and known counts come from the issue evidence. Board rows reuse shipped demo/evidence fixtures; they are illustrative prototype content, not live telemetry.