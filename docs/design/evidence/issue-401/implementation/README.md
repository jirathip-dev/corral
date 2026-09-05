# Corral #401 — multi-host board + Settings UX (host filters, stale/offline, accessibility)

Native evidence of the #401 change (iOS `ios/FleetNotifier/**`), captured from
the final committed state of branch `g401-host-ux` on a fresh **iPhone 16
simulator (iOS 26.5)**. Frames are the deterministic DEBUG multi-host evidence
drivers (launch-arg gated, marker-polled `simctl io screenshot`, then `sips`
resize to 390x844 — the locked phone-size gate; this host has no 390x844
device, so captures are iPhone 16 @3x resized, consistent with the issue-362+
conventions). Content is synthetic only (`demo-host-*`, `demo-*` lanes — the
same vocabulary as the single-host demo fixture).

The seeded state (DEBUG `AppModel.enterMultiHostDemo()` — fresh simulator
only, no real pairing touched): **Host A** live (active store), **Host B**
offline with RETAINED stale rows (last seen ~6m, `stream disconnected — host
unreachable`), **Host C** key mismatch (B4 paused, fails closed). Host A and
Host B share the raw agent id `herdr:demo-orbit-blocked` and the
`demo-orbit`/`demo-atlas` repos — the composite identity, merged repo
subgroups, row badges, and live-before-stale ranking are all on screen.

## Frames

Board driver (`-corralDemoMultiHostBoardEvidence`):

- `phase-1-mh-board-all-mocha.png` — All Hosts + All Repos (D1 default state):
  host chip row ABOVE the repo chip row (All 5 · Host A 3 live · Host B 2
  offline — health text on the chip, never color alone), repo chips with
  UNIFIED counts (D4), compact board summary `1 host offline · 1 host key
  mismatch` (D7), blocked section's `demo-orbit` subgroup MERGING Host A's
  live + Host B's stale equal-raw-id rows with textual `Host A` / `Host B`
  badges (D5/D6/C2), `stale · last seen 6m ago` on the retained row (C6).
- `phase-2-mh-board-host-a-mocha.png` — Host A filter selected: rows/badges
  of Host A only (badges hidden — D6), repo chips rescoped to A's repos with
  per-host counts (D4), Host B's chip still visible (a host is never hidden
  because another filter excludes it — D4), summary still shown.
- `phase-3-mh-board-all-latte.png` — All Hosts in Latte (partial-offline
  presentation on the light theme).
- `phase-4-mh-board-host-b-latte.png` — Host B filter: the retained stale
  board alone (live-before-stale ranking leaves only stale rows), Latte.
- `phase-5-mh-board-host-repo-latte.png` — Host A + repo `demo-atlas`: host
  AND repo filters apply together (D4), Latte.
- `phase-6-mh-board-done.png` — back to All Hosts (Mocha) after the sequence.

Settings driver (`-corralDemoMultiHostSettingsEvidence`):

- `phase-1-mh-settings-mocha.png` — the Hosts section (scrolled into view):
  one row per host in store order (drag-to-reorder affordance = Edit, D2)
  with health + name header, URL, fingerprint (copyable), key id, grants
  expiry, Host B's `stream disconnected — host unreachable` error, per-host
  Retry/Rename/Remove host, Host C's red key-mismatch guidance (D7/B4) and
  the `Active` tag on the single-host runtime's profile (Mocha).
- `phase-2-mh-settings-latte.png` — same Hosts list after a live flavor flip
  to Latte.
- `phase-3-mh-settings-done.png` — board restored after Settings closes.

Add Host driver (`-corralDemoMultiHostAddEvidence`):

- `phase-1-mh-add-entry-mocha.png` — Add Host sheet phase 1: the URL field
  was driven through the real binding and the NAME field is prefilled from
  it (`demo-host-d` from `demo-host-d.tail0123.ts.net`) — the #399 rev B3
  carry-over, through the real `HostURLForm.displayNameCandidate` onChange.
- `phase-2-mh-add-confirm-latte.png` — phase 2: fingerprint confirmation
  (synthetic X25519 fixture key, fingerprint derived through
  `HostKeyTrust.fingerprint`), full fingerprint + Copy + Registration-token
  field + `Confirm fingerprint & register`, Latte.
- `phase-3-mh-add-done.png` — the sheet dismissed after the sequence.

## Audit (Vision OCR of the frames)

- All-Hosts frames show BOTH host chips with lane counts and the offline
  chip's textual `offline` label; the row badges are text capsules; the
  summary line is the only board-level outage surface (no per-retry banner).
- Equal raw agent id `herdr:demo-orbit-blocked` renders as TWO rows (Host A
  claude live, Host B codex stale) inside ONE `demo-orbit` subgroup — same
  repo from several hosts shares one subgroup; no host sections/tabs.
- The stale row keeps its last-reported `blocked` state chip and shows
  `stale · last seen 6m ago` — never recast, age text present.
- One-host-filtered frames hide row badges and rescope repo counts.
- Settings rows carry the full D7 surface incl. the red key-mismatch text;
  Add-host phase 1 proves the URL→name prefill; phase 2 shows the derived
  fingerprint before any token entry.
- Accessibility/theme coverage is additionally enforced by the unit suite:
  the health→Catppuccin-token mapping (`BoardModel.hostHealthToken`) is the
  single source for chips + Settings rows (all four flavors resolve the same
  tokens), chips keep ≥44 pt targets + VoiceOver labels/values/selected
  traits (source-wiring pins), and the 22 new #401 tests cover the host chip
  guard (single-host F1 layout), the stale branch, and the removed-host
  recents route.

## Capture commands (capture.log)

Fresh simulator `Corral401` (iPhone 16, iOS 26.5, UDID
7F6AD3BA-7AE8-4C73-A7FC-E6765604B929), DEBUG build from this branch
(`/tmp/401-dd`), one launch per driver with marker polling +
`simctl io screenshot` per phase + `sips` resize to 390x844. Full details in
`capture.log`.

SHA-256s: `SHA256SUMS.txt` (all artifacts above).
Evidence conventions follow docs/design/evidence/issue-362/385/386/388/389.
