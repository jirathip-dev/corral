# Design system — Corral #427 filter/header

## Surface

**Monitor.** Preserve the live board’s scan order and density; filtering is supporting chrome, not the content hero.

## Existing product language retained

- SwiftUI/SF typography and native top bar.
- Catppuccin Latte, Frappé, Macchiato, and Mocha tokens from `AppTheme.swift`.
- Mauve = interaction/selection accent; teal remains the working-state hue.
- Flat base rows, thick status header, demoted repository band, hairline separators.
- Settings gear remains top-right.
- Existing pull-to-refresh/automatic-stream copy remains visible.
- Existing translucent-sheet availability contract: Liquid Glass on iOS 26+, themed 80% material fallback below it.

## Type hierarchy

| Role | Prototype size | Posture |
|---|---:|---|
| filter/header title | 16 pt | semibold/bold, compact |
| scope/editor title | 17–18 pt | bold |
| status section | 16 pt | bold |
| option/row primary | 13 pt | semibold |
| scope label | 10–11 pt | mono uppercase, tracked |
| metadata | 10–11 pt | Catppuccin subtext1 |
| accessibility mode | 1.25× base | wrapping, scroll-preserved |

Use SF/system fonts in production. Monospace is reserved for scope labels and board metadata already expressed as system/agent identifiers.

## Spacing

Base rhythm: 4, 8, 12, 16, 20 pt.

- top bar control: minimum 44 pt;
- option row: 50 pt minimum, 60 pt in accessibility mode;
- section boundary: 8 pt surface break;
- content gutters: 16–18 pt;
- board controls never force horizontal page overflow.

## Shape and elevation

- header controls: native borderless hit areas;
- selection rows: flat 12% accent tint + trailing check;
- sheet: 22 pt top radius + 37×5 drag handle;
- no decorative cards or gratuitous shadows;
- elevation comes from material, scrim, surface tier, and the sheet edge.

## Color contract

All values are in `sources/catppuccin-palette.json` and are byte-matched to `AppTheme.swift` at `c3141f94bea97cc08476123e6e491e6fa24a5a55`.

- background: `base`;
- header/rail: `mantle`;
- separator: `surface0` / translucent `surface1`;
- status header: `surface1` on dark flavors, `surface0` on Latte so normal-size text remains WCAG AA;
- primary text: `text`;
- metadata: `subtext1`;
- accent/action/selection: `mauve`;
- health/status colors: supplementary marks only; textual `live`, `connecting`, `offline`, and `stale` always remain.

`contrast-checks.json` records 28 AA checks across all four palettes. The measured minimum is 4.68:1.

## Motion posture

Selection applies immediately. Avoid ceremonial motion.

- sheet presentation: native system motion in implementation;
- inline disclosure: brief system transition only;
- Scope Path swap: direct navigation/surface transition;
- `prefers-reduced-motion: reduce`: all prototype animation/transition removed.

## Accessibility contract

- every button/control ≥44×44 pt;
- visible and VoiceOver-scoped `All hosts` / `All repositories`;
- distinct `Clear host`, `Clear repository`, `Reset all filters`, `Close filters`, and `Settings` labels;
- selection communicated by text, highlight, check, and accessibility state—not color alone;
- Dynamic Type wraps filter option copy and preserves independent vertical scrolling;
- terminal options remain reachable at 375×812.
