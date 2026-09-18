# #569 fix round — mechanism frames (base vs head vs fixed)

Three-arm evidence for the protected check
`FleetNotifierTests.RecentWorktreeBlockTests.testRenderedSheetDayNightMediumLargeAndAX3`,
all rendered on the same iPhone 17 Pro simulator (iPhone18,1, iOS 26.5) with the
same 390x844 pt XCTest window, one A/B arm per file. Full A/B numbers: the
round-3 receipt in `.report.md`; logs `/tmp/g569-ab.log`, `/tmp/g569-ab2.log`,
`/tmp/g569-ab3.log`.

| arm | commit | rc | the case that flips (`night-large-default`) |
|---|---|---|---|
| `base`  | `23bbfa66` (base)            | 0  | OCR `abcdef1`; header rows 435 / 502 / 581 / 642 / 706 px |
| `head`  | `bc720ce` (delivered code)   | 65 | OCR `abodef1`; header rows 440 / 511 / 590 / 651 / 715 px (+5 / +9 / +9 / +9 / +9) |
| `fixed` | `d10f0b2` (re-applied fix)   | 0  | OCR `abcdef1`; header rows 435 / 502 / 581 / 642 / 706 px — identical to base |

Measured with the same Vision settings the test uses:

- `base` vs `head`: the commit band (y 560..630) differs by 11576 px — the whole
  worktree block sits 9 px lower because the padded `RepoLabelChip` capsule
  (19.0 pt) grew the caption row past the 16 pt the caption-semibold state label
  gives it.
- `base` vs `fixed`: the commit band differs by **0 px** — the fixed chip keeps
  the Board's capsule drawing (19.0 pt measured) without inheriting the Board's
  row height, so the block returns to the base y. The remaining 17862 px of
  whole-frame difference are the chip's own chrome and the branch truncation in
  the caption row.

Files: `<arm>-17pro-<case>.png` with sha256 + byte counts in `captures.json`
(arms and commits are recorded there too). Cases: `night-large-default`
(the failing one), `night-medium-ax3`, `day-large-default`.
