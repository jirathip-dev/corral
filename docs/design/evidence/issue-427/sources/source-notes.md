# Source notes

Base: `c3141f94bea97cc08476123e6e491e6fa24a5a55`

## Authoritative inputs

- `issue-427-locked-spec.md` — copied byte-for-byte from the fleet-operations lock.
- `issue-427.json` — title, URL, and body read from GitHub before authoring.
- `AppTheme-palette-excerpt.txt` — exact Catppuccin table lines 90–136.
- `FleetViews-filter-excerpt.txt` — exact board/header/filter implementation lines 739–1113.
- `BoardModel-filter-excerpt.txt` — exact host/repository projection lines 263–425.
- `catppuccin-palette.json` — the four complete 26-token tables copied from the base source.

## Visual baselines

- `reference-issue-401-all-mocha-390x844.png` — existing multi-host board, all scopes, Mocha.
- `reference-issue-401-host-repo-latte-390x844.png` — existing selected host+repository board, Latte.

These images are existing repo evidence, not generated or altered by this lane.

## Content provenance

- host/repository labels and known counts: GitHub issue #427 physical evidence;
- stale 6m wording and multi-host row structure: issue #401 implementation evidence;
- synthetic board row vocabulary: shipped `DemoFleet.swift` debug fixture;
- `g427-filter-header-design`: this lane identifier, used as an evidence label only;
- no external images, fonts, packages, analytics, or telemetry were added.
