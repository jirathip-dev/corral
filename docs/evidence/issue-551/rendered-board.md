# Issue 551 — rendered board evidence across the warm-return boundary

These two frames are REAL rendered pixels of the production `FleetView` board
(`UIGraphicsImageRenderer` + `window.drawHierarchy(in:afterScreenUpdates:)`,
1179x2556 px = iPhone 16 @3x), captured by
`ForegroundReconnectTests.testWarmReturnRetainsRenderedStatesUntilVerifiedReplacement`
in the same run that the board assertions execute in. They are not mockups and
not recomposed images.

## Provenance

- Source head: the lane head recorded in `.report.md` (`git rev-parse HEAD`).
- Gate that produced them: G1 focused (`/tmp/g551-focused.log`), test
  `FleetNotifierTests.ForegroundReconnectTests.testWarmReturnRetainsRenderedStatesUntilVerifiedReplacement`.
- Extraction (attachments live in the gate's `.xcresult`):

```
xcrun xcresulttool export attachments \
  --path /tmp/g551-dd/Logs/Test/Test-FleetNotifier-<stamp>.xcresult \
  --output-path /tmp/g551-att
```

| Frame | Attachment name | Committed file | sha256 | Pixels |
| --- | --- | --- | --- | --- |
| Warm return, verification still pending (2 hosts connecting) | `551-warm-retained-unverified` | `board-retained-unverified.png` | `d1c133f4bd76fc5a5b531d589e58f3342d37b675c279d04dd9dbc2c077b17bd7` | 1179x2556 |
| After the first host's frame is verified and applied | `551-warm-applied-verified` | `board-applied-verified.png` | `776d9ce2524a8b3b62fd33bca2055c7cdf8c96eaec561080670f84a5e026a1fe` | 1179x2556 |

## What the frames show (read directly from the pixels)

`board-retained-unverified.png` — the board immediately after the production
`.background` → `.active` seam, while BOTH host keys are still unverified and no
frame has been applied:

- header connection line: **`2 hosts connecting`** — the same string the owner
  reported for the failing return;
- section **`working (2)`** with two rows;
- each row's chip reads **`working`** (NOT `unknown`), with `revision 6`, the
  host badge (`Fixture 1` / `Fixture 0`) and the explicit
  **`stale · last seen 0s ago`** marker.

So with both hosts unverified the board still renders the last-known rows with
their last-reported state, marked stale — no `N unknown` flash and no Live
labelling while the key check is pending. This is the run-time contrast to the
owner's screenshot (all 55 rows `unknown`).

`board-applied-verified.png` — after `Fixture 0`'s host key verifies and its
first frame is applied:

- header: `1 host connecting` (the other host is still unverified);
- `Fixture 0`'s row is now `revision 7` and carries NO stale marker (the
  verified replacement applied);
- `Fixture 1`'s row is untouched and still `revision 6` + `stale` — one host's
  verification cannot replace another host's retained rows.

## Fixture artifacts visible in the frames (not production defects)

The fixture stamps `ts`/`seq` with the frame revision number (6, 7), i.e.
milliseconds after the Unix epoch. `20711d 9h` (time-in-state) and
`stale · last seen 0s ago` are therefore the fixture's clock, not production
timing; the board is rendering the fixture's own values verbatim.
