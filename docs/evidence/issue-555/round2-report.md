# Issue #555 — round 2 implementation and verification

Source/gate head: `59e0e613bc49784c3608a7b1f9dec5ff9b62d1d5`.
Reviewed head: `60851ed946ae479ce08a114a058ec70b9ee7e61d`.
Pre-publication baseline: `11cc1d26f3b1c22716d4527fb9e7679eaa861aba`.
Worktree: `/Users/jirathip/.herdr/worktrees/corral/impl555-daemon`.
Branch: `g555-daemon-snapshot`.

## Disposition — not an unconditional performance PASS

The single-thread HTTP executor is removed; bounded logging and explicit
subscription-side-effect documentation are implemented. All required cargo
gates, the byte contract, the CPU-hog router budget, and the real-daemon smoke
passed at the source head above.

**The new relative concurrent-read guard FLAGS the fixed head's p99.** Its
measured 5.744708 ms exceeds the predeclared 5.712250 ms bound by 0.032458 ms.
The same guard rejects the reviewed head much more substantially. This flag is
retained, not rounded into a PASS, and the bound was not loosened. A proposed
same-window base/fixed/executor-only-mutation/control experiment did not start:
shared-host admission refused it. There is no new executor-only mutation proof
or clean relative-performance PASS claimed in this round.

**Freshness remains a tradeoff: fast reads can serve data about two seconds
stale.** The new changing-state/CPU-hog measurement observed buffer age p50
856 ms, p99 1984 ms, maximum 2014 ms, versus baseline 1/1/2 ms. An unchanged
buffer can age further without a new mutation being pending. Neither the
2-second debounce nor these observations are a hard real-time OS guarantee.

## F1 — executor removal and corrected disclosure

`src/main.rs:411` now uses the original `axum::serve(listener, app)` on the
normal multi-worker runtime. The entire file is byte-identical to the baseline
at `11cc1d2`; the diff command exits 0. No HTTP runtime split is retained.
Publication's existing `spawn_blocking` implementation is unchanged.

RETRACTED AS AN ACCEPTANCE JUSTIFICATION: the prior report at line 913 said
"Background planes and the publisher cannot occupy its executor queue."
It omitted the consequential fact that ALL HTTP routes shared a single thread.
The old smoke driver did not prove cross-route concurrency. The report's prior
"Status: daemon acceptance criteria verified." conclusion does not establish that
missing property. Its wire and in-process load results remain recorded, but
must not be substituted for actual-main runtime evidence.

On the repaired runtime, background tasks again share runtime workers. The
publisher's encoding remains off that runtime, and cached reads remain
independent of its store/publication locks. This is not a promise that arbitrary
blocking work cannot consume all workers. The smoke driver's misleading
"dedicated HTTP executor" description was corrected to read-readiness only.

The new `round2-http-load.py` executes real release binaries over TCP, not an
in-process router. It records all samples and fails on an exceeded relative
bound. The observed reviewed-head rejection is real; the fixed-head flag and
unexecuted additional attribution proof are explicitly disclosed below.

## F2 — bounded operator diagnostics

- `src/api/read_timing.rs:9-65`: process-wide, separate snapshot/SSE-first-frame
  budgets. Atomic reservation permits at most one aggregate per endpoint per
  five seconds, regardless of clients or concurrent callers.
- `src/api/mod.rs:185` and `src/api/events.rs:19`: replace unconditional info
  logging with those budgets. The records retain `requests`, `max_serve_ms`,
  and `max_buffer_age_ms`; revision/frame metadata is explicitly `latest_*`,
  not falsely associated with the peak request.
- Every observation contributes maxima, including unsampled slow requests.
  The next eligible request emits the aggregate. There is no new idle timer
  or task; a trailing partial window waits for another request. Concurrent
  updates may straddle aggregate windows, as documented in the module.
- The real-handler burst test observed one snapshot info record for 256
  snapshot requests and one SSE-first-frame record for 256 SSE requests.
  The release-binary matrix observed one snapshot aggregate per run, versus
  384 per run at the reviewed head, with the same 384 requests per run.

The two new limiter tests exercise an eight-thread/8000-observation burst and
clock-window/unsampled-peak behavior. They and the public-handler test pass.
The optional old-source replay of the public-handler test was behind the
blocked mutation build and was NOT executed; no assertion-level RED is claimed
for that replay.

## F3 and F4 — explicit semantics, unchanged behavior

`src/core/store.rs:516-520` now begins "Subscribe to live deltas AND wake the
publication coalescer" and explains registration, notification, shortening a
pending background debounce, and the absence of synchronous flushing. This
uses the contract's documentation option; the public name and behavior remain
unchanged.

`src/core/store.rs:40-45` prominently distinguishes fast response time from
roughly two-second stale data and potentially older unchanged buffers. Existing
250 ms foreground / 2 s background debounce and the HTTP test's 3 s publication
deadline remain unchanged.

## Changed files and test ranges

Relative to the reviewed head, the source/driver changes are:

- `src/main.rs:411`: restore original HTTP serving.
- `src/api/mod.rs:44,185-205`: register/use the timing limiter.
- `src/api/events.rs:107–118`: budget first-frame info records.
- `src/api/read_timing.rs:1-123`: limiter and its unit tests.
- `src/core/store.rs:40-45,516-520`: freshness and subscription documentation.
- `tests/read_logging.rs:1-77`: new real-router logging regression.
- `docs/evidence/issue-555/round2-build.py:1-62`: pinned archive builds and
  the additional, unexecuted mutation/replay helpers.
- `docs/evidence/issue-555/round2-http-load.py:1-206`: real-binary matrices
  and executable relative guard.
- `docs/evidence/issue-555/round2-method.md`: reproduction commands and scope.
- `docs/evidence/issue-555/daemon-smoke.py:2`: correct its evidence description.

All 77 lines of the new test file belong to the logging witness and its capture
writer; test code also occupies `src/api/read_timing.rs:69-123`. No existing
assertion was weakened or skipped. `tests/http.rs`, `tests/snapshot_load.rs`,
and `src/core/store/publication_tests.rs` are byte-unchanged from the reviewed
head. The prior 3-second HTTP visibility bound and independent byte oracles
were rerun unchanged.

No edits to `crates/**`, `ios/**`, `scripts/**`, `.github/**`, Cargo manifests or
lockfile, `deny.toml`, or `fastlane/**`; no dependencies added. The final
closeout adds only this report, the append to `.report.md`, and raw evidence.

## Required gates — actual commands and raw exits

All ran from the named worktree at `59e0e61`. No justfile exists in this checkout.
The reused `daemon-run.py` records commands, source heads, raw exits, disk/host
observations and deadlines under `flock /tmp/n.lock`, with
`CARGO_TARGET_DIR=/tmp/g555-daemon-target`. To keep the limited disk budget,
the gate session set `CARGO_PROFILE_DEV_DEBUG=0`, `CARGO_PROFILE_TEST_DEBUG=0`,
and `CARGO_INCREMENTAL=0`; assertions were not disabled and release
optimization settings were not changed.

| Command | Raw exit | Actual result |
| --- | --- | --- |
| `cargo fmt --all --check` | 0 | No formatting diagnostics. |
| `cargo test --workspace` | 0 | 488 passed, 0 failed, 11 ignored, summed from all 29 test-result records. |
| `cargo clippy --all-targets -- -D warnings` | 0 | Completed successfully. |
| `cargo deny check` | 0 | Completed; duplicate-version and unmatched-license warnings retained in raw log. No configuration changes. |
| `cargo build --release` | 0 | Release build completed. |
| `cargo test --release --lib api::read_timing -- --nocapture` | 0 | 2 passed. |
| `cargo test --release --test read_logging -- --nocapture` | 0 | 1 passed; one info record per 256 requests for each endpoint. |
| `cargo test --release --lib core::store::publication_tests -- --nocapture` | 0 | 5 passed; unchanged 64-state byte contract and lock/race tests. |
| `cargo test --release --test http -- --nocapture` | 0 | 23 passed. |
| `cargo test --release --test snapshot_load --no-run` | 0 | Load test compiled. |
| `cargo test --release --test snapshot_load -- --ignored --nocapture` | 0 | 512 samples, ten nice-0 CPU hogs, budget passed, revisions progressed. |
| `python3 docs/evidence/issue-555/daemon-smoke.py` | 0 | `/snapshot`, `/host-key`, `/events` returned 200; timing/age info observed; fixture reaped. |

The gate session completed with raw exit 0. Admission failures elsewhere are
not substituted for these completed commands, nor are the performance flags
hidden by this table.

## Byte equality and changing-state router load

Actual test output:

    G555_BYTE_EQUALITY states=64 snapshots=64 sse_frames=64 empty=1 legacy_header_cases=16 epochs=64 revisions=64

The unchanged test exercises Store publication and the real router. It compares
complete cached JSON bodies and initial SSE frames with independent prior
axum `Json` / `Event::json_data` encoders for the same agent state and capture
time. It includes empty state, randomized revisions/epochs/content, escaping,
and legacy headers; the cache is not its own decoded-state oracle.

The changing-state test is distinct from TCP timing: its clock measures router
service time. Its source hash remains exactly the pre-change baseline's
`08774414aa7681db131a8c4a18f86131832785adab4ad9a2f77384a8e3ebe27b`.

| Router-accounting metric | Pre-change `11cc1d2` | Round 2 `59e0e61` |
| --- | ---: | ---: |
| Samples / nice-0 hogs | 512 / 10 | 512 / 10 |
| Serve p50 ms | 43.870792 | 0.008042 |
| Serve p99 ms | 45.687750 | 0.031833 |
| Buffer age p50 ms | 1 | 856 |
| Buffer age p99 ms | 1 | 1984 |
| Buffer age maximum ms | 2 | 2014 |

The existing baseline is retained from the pre-change run, not reconstructed.
The round-2 run satisfies the 20/50 ms router budget and observes revisions
1 and 2. Its host receipt confirms ten nice-0 hogs and 0% idle CPU. This does
not prove actual-main concurrent-route performance or owner warm returns.

## Actual-main TCP comparison and the retained FLAG

One identical final harness measured all three binaries. The baseline and
reviewed binaries were built from pinned `git archive` sources without
switching this worktree's branch. Every matrix has three fixture runs, each
with 128 `/snapshot` samples in each of three phases:

- idle: no competing history reader;
- single: one continuous `/history?limit=5000` reader;
- concurrent: eight such readers.

Every fixture has 1024 retained history events, no live inputs, a new loopback
connection per request with `Connection: close`, 2-second request deadlines,
and a one-second loaded warmup. Each background reader must progress during
the measured samples. TCP setup/body receipt are timed; JSON inspection is
after that timing. Nearest-rank percentiles are calculated per run, then the
median of the three run percentiles is reported. All samples are retained.

| Binary | Idle p50/p99 ms | Single p50/p99 ms | Concurrent p50/p99 ms |
| --- | ---: | ---: | ---: |
| Baseline `11cc1d2` | 0.193417 / 0.259375 | 0.128500 / 1.016458 | 0.497417 / 1.856125 |
| Reviewed `60851ed` | 0.197334 / 0.327708 | 1.089125 / 1.356375 | 7.844167 / 9.876958 |
| Fixed `59e0e61` | 0.196833 / 0.304292 | 0.190584 / 1.825042 | 1.441292 / 5.744708 |

These are measurements, not an assertion that the fixed numbers meet every
relative criterion. Unchanged-buffer age in these idle-data fixtures reached
19.861 ms at baseline, 3652.472 ms reviewed, and 2762.332 ms fixed. With no new
state mutation, age need not stay below the mutation-to-publication debounce.
The changing-state age table above is the relevant freshness tradeoff.

The relative guard was fixed before measuring the repair:

    candidate concurrent p50 <= 2 * baseline p50 + 1 ms
    candidate concurrent p99 <= 2 * baseline p99 + 2 ms

For the recorded baseline, those bounds are 1.994834117591381 ms and
5.712249919772148 ms. The checker requires matching harness hashes and complete
3-by-128 phase populations before comparing them.

Actual check commands (from the worktree):

    python3 docs/evidence/issue-555/round2-http-load.py check /tmp/g555-r2-http-base-close.json /tmp/g555-r2-http-reviewed.json
    raw exit 1: concurrent /snapshot regression exceeds 2x baseline + 1/2 ms p50/p99

    python3 docs/evidence/issue-555/round2-http-load.py check /tmp/g555-r2-http-base-close.json /tmp/g555-r2-http-fixed.json
    raw exit 1: concurrent /snapshot regression exceeds 2x baseline + 1/2 ms p50/p99

The fixed p50 passes; p99 exceeds its bound by 0.03245808370411396 ms. Neither
result is a green relative gate. The literal issue p99 ceiling of 50 ms is a
separate, looser condition. No tolerance was changed to hide this flag.

### Paired diagnostic blocked — no attribution claim invented

Because the baseline and fixed measurements had compilation, CPU-hog work and
host changes between them, a single follow-up experiment was specified before
running it: compile variants first, then record base/fixed/executor-only-mutant/
fixed control under one held lock, using the unchanged harness and bound. This
would test a time-window variability hypothesis, not assume it true.

The first mutant-build admission returned 75 after 180 seconds. A further
bounded 900-second observation also returned 75 before starting the experiment.
No mutant binary was built, no old-source logging replay ran, and no paired
measurements or post-mutation controls occurred. The scripts are included for
reproduction, not presented as executed proof. There was no worktree mutation
to restore, and source equality was verified at cleanup.

BLOCKER: `.brief.md:9-11` — sibling heavy jobs prevented an admitted paired
verification window — minimal remedy is a quiet serialized window to execute
the documented binary/control experiment. Host variability is NOT established
as the cause of the fixed-head flag. The pending performance judgment is
explicitly handed back rather than continuing an unbounded retry loop.

## Diagnostics, provenance and cleanup

- Initial R2 baseline probe: raw exit 1, `OSError: [Errno 49] Can't assign
  requested address`. It was an incomplete client/socket attempt, not a
  latency regression RED. The probe was corrected to send `Connection: close`,
  matching the review client's close behavior. All three completed matrices
  use that same final harness; the failed attempt remains separately archived.
- Earlier baseline-build admission attempts returned 75 before cargo started;
  a later admitted build completed. The final gate startup wrapper also retains
  its initial refusal to proceed after the incomplete baseline. Those are not
  Rust failures and are not counted as executed gates.
- Relative checker failures and the later 180/900-second admission refusals
  are preserved, including the outer wrapper's assertion/exit where applicable.
- `round2/manifest.json` lists 99 raw artifacts with verified byte lengths and
  SHA-256 hashes. Logs are losslessly gzip-compressed; JSON remains readable.
  `round2/summary.json` aggregates 18 receipts and the three complete matrices
  (3456 timed snapshot samples), explicitly marking the initial incomplete
  matrix as incomplete. These are real executions against explicitly synthetic
  history fixtures, not fabricated timing results.
- `round2/g555-r2-inspection-final.json` records exact commands and raw exits:
  `git diff --exit-code 11cc1d26f3b1c22716d4527fb9e7679eaa861aba HEAD -- src/main.rs`
  exited 0; `ast-grep run -l rust -p 'axum::serve($$$)' src/main.rs` exited 0
  and found the restored call at line 411; `ast-grep outline
  src/api/read_timing.rs tests/read_logging.rs --view signatures` exited 0 and
  found the limiter/test declarations. The focused `pub fn subscribe` ast-grep
  pattern exited 0 and found the existing subscribe/notify sequence at 521-525
  (`g555-r2-subscribe-structure.log.gz`). No skill-loading claim substitutes for
  these actual invocations.
- Prior `.report.md` bytes are retained as the prefix: 71662 bytes, SHA-256
  `79ec60d978888113b548331a05cf34d7b51ad35db94eaa1de31f0eb4b316f1c3`.
  The correction is this appended round, not a silent rewrite of that history.
- Cleanup under `/tmp/n.lock` verified the owned marker, no active target
  process, and unchanged source before removing only `/tmp/g555-daemon-target`.
  Target absence and cleanup raw exit 0 are recorded. Scratch daemons and hogs
  were reaped by their completed drivers. No sibling process/tree was removed.

## Remaining boundaries

DELTA-FIRST: still skipped; existing same-epoch incremental resume is retained.
No new delta-then-snapshot ordering, wire schema/version change, header meaning
change, or SSE framing change.

NOT EXERCISED: the additional paired executor-only attribution experiment and
old-source logging replay; a clean relative-performance PASS; iOS recorded
fixtures; owner warm returns under real fleet/Tailscale load; live drive/git
pressure; hosted/Linux CI; installed/deployed daemon behavior.

NON-CLAIMS: no iOS/Xcode/simulator run by this lane, no deployment or live
configuration/daemon change, no CI verification, no PR/merge/issue mutation,
no main-branch promotion, release, or TestFlight. This is a source/evidence
handoff with an explicit performance FLAG, not an unconditional acceptance
or production-readiness claim.
