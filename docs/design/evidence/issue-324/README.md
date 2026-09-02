# #324 — provider read-tail revision contract: measured probe evidence

Status: **IMPLEMENTED with simulated-provider evidence.** The Corral adapter
now implements the provider read-tail revision contract end-to-end
(`src/adapters/herdr.rs`), but the live upstream Herdr provider cannot
exercise the incremental path — that limitation is recorded honestly below
and the measured probe uses a simulated contract-honoring provider over a
mock unix socket against the REAL adapter code.

## Upstream blocker (verified, honest)

The live Herdr 0.8.2 provider does NOT support revisions. Independent probes
of its socket (`~/.config/herdr/herdr.sock`, `agent.read`, pane `w12E:p6`)
returned `revision: 0` and a byte-identical 138-line body for all three
variants:

- no `rev` parameter
- `rev = 1`
- `rev = 999999`

Re-confirmed read-only at this lane's base (58a41f0) against the live
socket: `revision: 0` and a byte-identical body for all three variants —
see `live-herdr-0.8.2-probe.md`.

So against Herdr 0.8.2 the adapter keeps the existing bounded full-page
behavior (legacy fallback), and `source_rev` on the wire echoes the client's
cached revision exactly as before #324. No live-provider incremental
behavior is claimed. The incremental contract activates only against
providers that honor it.

## The contract (defined in this lane)

- `agent.read` responses carry `read.revision`: a monotonic
  per-agent/output-source revision — NOT the fleet snapshot revision.
- `revision` 0/absent = legacy provider → bounded full-page fallback, no
  provider revision.
- Request `rev` == provider `revision` → UNCHANGED: provider returns empty
  `text`; the adapter returns an explicit empty result with the same
  revision, no page re-transferred.
- Anything else (first read, advanced output, provider wrap/restart with a
  reset counter) → bounded window plus the provider's current revision.

Wire shapes: request `{"kind":"read_tail","lines","since_rev"}`; response
result `{"lines","blocks","source_rev"}`. Normative docs:
`docs/corral/P4-conformance.md`, `docs/OPERATIONS.md`
("Provider read-tail revision contract (#324)").

## Measured probe (real adapter, simulated provider)

`probe.sh` runs the committed probe test
(`adapters::herdr::tests::probe_read_tail_bytes_first_unchanged_changed`):
the REAL `HerdrAdapter::read_tail_since_with_rev` path performs three reads
against a mock socket whose fixture provider implements the contract. The
fixture pane tail is a full bounded window (200 lines ≈ 18 KiB), standing in
for a long-running pane. Raw wire bytes (JSON-RPC line incl. newline) per
exchange, from `probe.log`:

| Read | Request bytes | Response bytes | Lines returned | source_rev |
|---|---|---|---|---|
| first (no cached rev) | 105 | 20260 | 200 | 59 |
| unchanged (cached rev 59) | 114 | 62 | 0 | 59 |
| changed (cached rev 59) | 114 | 20260 | 200 | 60 |

The unchanged read transfers 62 bytes instead of the 20,260-byte full page
(~99.7% saving) and returns an explicit empty result with the same
`source_rev`; the changed read returns the bounded window with the strictly
newer revision 60. Reproduction: `docs/design/evidence/issue-324/probe.sh`
from the repo root (log written to `probe.log`).

## Regression coverage

- `contract_tail_*` unit tests: legacy (absent + revision 0), unchanged,
  changed, wrap/restart, redaction/line-clamp preservation.
- `read_tail_since_with_rev_*` socket tests: first read, unchanged (explicit
  empty, no page), changed (newer revision), legacy fallback, wrap/restart.
- RED/GREEN mutation probes (scratch copy, byte-identical restores):
  ignoring `rev` on the wire, ignoring the revision decision, and
  substituting the fleet snapshot revision all go RED on the tests above —
  see `.report.md` for exact commands, exits, and assertion messages.

## Scope

Only `src/adapters/herdr.rs`, the two protocol docs, this evidence bundle,
and `.report.md` changed. No updater scripts, egui fonts, iOS code, GitHub
polling scope, or terminal lifecycle; no live socket mutation; no live app
touch.
