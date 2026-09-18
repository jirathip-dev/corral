# #561 reproduction and interpretation

All commands run in `/Users/jirathip/.herdr/worktrees/corral/impl561-hydrate`.
The helpers use only temporary Git repositories and, for health assertions, an
in-process store served on an ephemeral loopback port. No live daemon is used.

## Reproduce

```
python3 docs/evidence/issue-561/run.py focused-reproduction 180 cargo test -p corrald g561_ -- --nocapture
python3 docs/evidence/issue-561/run.py cost-reproduction 360 cargo test -p corrald g561_restart_cost_measurement -- --ignored --nocapture
python3 docs/evidence/issue-561/gates.py reproduction-
```

The cost test is intentionally ignored in the ordinary suite. It is explicitly
executed in before-cost.log and after-cost.log, one test in each, raw exit 0.
Every command-receipt `.json` records the exact argv, worktree, raw subprocess
exit, timeout flag and elapsed wall time. Raw logs are committed losslessly as
`.log.gz` to preserve whitespace without failing `git diff --check`; decompress
them with Python `gzip.decompress` or `gzip -dc`. Local raw `.log` files were
removed only after verifying their lossless gzip round-trip. `artifact-manifest.json`
records both raw and compressed SHA-256s. Compiler errors in focused-first.log
and focused.log are retained, not presented as behavioral REDs.

The behavioral RED is red.log: production code at
4cc316599b0ca1d69039aceb7dbb44193be48e52 plus the new test include/test file,
BEFORE changing production behavior. red-head.txt and red-source-diff.txt.gz record
that boundary. It executes one test and fails with `generation restart cleared
cached commit`, left None, right the temporary repository's real HEAD. A held
four-permit admission budget prevents eager re-hydration from hiding the reset.
The same exact command is green.json/green.log after the fix; final-green is its final-code rerun.

## Measurement definition

A read-only filesystem census counted 107 herdr worktree directories and 23
immediate Projects checkouts. This is NOT a daemon inventory API observation.
Seed 130 paths (one main checkout plus 129 detached linked worktrees) in one
temporary repository using the existing scratch_repo helper, then fully warm
facts. For each of three trials, force a normal critical-child exit in the REAL
supervisor, wait for its generation to advance, and observe 65 seconds from that
point. The wall window includes the restart/backoff before those 65 seconds.
Read getrusage(RUSAGE_SELF) + getrusage(RUSAGE_CHILDREN) before/after the window;
subtract both user and system CPU times. Children means reaped Git subprocesses.
Divide those CPU seconds by the measured wall seconds for one-core percentage.
This is cumulative CPU accounting, not a ps-percentage snapshot.

Both runs use the same debug test harness, fixture size, command and window,
on the same Mac. The before binary was compiled/run before production edits;
subsequent edits did not change that already-running binary. The cost-test body
is unchanged apart from formatting. Setup/initial hydration are outside the
measurement; normal topology scans, FSEvents and paced status work ARE included.
Raw sample records and programmatically computed means/maxima are in
measurements.json; there are exactly three records per side.

No pre-run host-load readings were captured. The shared host was NOT isolated:
a reading during the after run was 73.32 / 49.09 / 38.50 (1/5/15-minute load).
Do not interpret the overlapping noisy samples as a universal speedup or a
production daemon CPU cap. The fixture also does not reproduce 23 repositories'
separate topology costs or large working directories. `skipped` in each sample
is the lifetime cumulative count, including initial hydration, not a delta.

The deterministic achieved bound is ZERO eager watcher hydration probes for a
fully observed, unchanged inventory. The watcher bootstrap regression pins it;
the generation regression separately pins preservation of commit/status/subject
and exact observation timestamps. This does NOT mean zero Git work: normal
paced status revalidation and topology safety nets deliberately continue.

Observed after envelope: restart transition at most 1.358s; total window at most
66.371274s and CPU at most 35.538879s. Before means: 32.263057 CPU seconds /
66.015457 wall seconds; after means: 25.410430 / 66.225335. Observed mean CPU
reduction 21.239856%; wall-clock transition did NOT improve in these samples.
The measured envelope and zero-eager-probe bound are stated beside RESTART_DELAY;
the freshness qualifications are beside SWEEP_INTERVAL in git_plane.rs.

## Call-site inventory

The ast-grep commands/results are in preflight.md and *-sites.txt. A literal
search followed the structural pass to include calls inside tokio::select!
macro token bodies, which ast-grep's Rust call pattern does not traverse.
Final source line numbers:

- Converted supervise:786: reset only after JoinError::is_panic at :837 or :874
  (including panic results drained while aborting sibling tasks). A normal
  critical-loop exit or heartbeat stall retains facts and observation ages.
- Converted watcher bootstrap :916-927: only unobserved or interrupted
  publications are debounced; an unchanged complete cache causes no hydration.
- Converted shared apply_probe :1476, :1547-1604: mark publication pending before
  updating the cache; clear only after all events were sent and the cached
  observation still matches. A cancellation between cache update and send is
  replayed even when the next probe is equal. Both existing callers (:1293,
  :1458) get this handling; neither call's execution budget changes.
- Deliberately retained watcher event debounce :964: genuinely changed metadata
  must still refresh promptly. Retained command rescan :1006 and throttled rescan
  :1246: topology events must discover additions/removals.
- Deliberately retained startup/topology/rediscovery added-path debounce calls
  :1349, :1376, :1395 and rescans :1343, :1370, :1389: new paths hydrate and
  removed paths leave the maps; failed sources keep their existing backoff.
- Deliberately retained full paced status-sweep rescan :1421 and probe :1446:
  preservation is not indefinite staleness. Every path is read after the
  500ms startup delay, paced over 60s with four command permits; a changed HEAD
  still triggers a fresh subject read. Scan/permit wait is additional, and
  over-budget or failed probes retain old observation ages and retry, never
  pretending stale data is fresh.
- Plane::start :2529 still starts exactly one supervisor; no wire, store or
  other adapter changes. Existing #492 health publication semantics remain.

The panic regression injects a real supervised panic in the tracing writer
AFTER the cache update and BEFORE sending. It requires fresh HeadMoved AND
DirtyChanged for both the interrupted and unaffected worktree. Panic recovery
still invalidates/re-reads all cached facts, unlike normal generation restart.

## Gate failure retained and fixture correction

The first full ordinary workspace run passed (509 passed, 12 ignored). The first
coverage run did NOT pass: coverage.json records raw exit 101, with the new
freshness test timing out at 65s (317 passed, 1 failed, 1 ignored in the lib).
The failed run did not capture per-probe diagnostics, so its exact cause is not
proved; this is not attributed conclusively to host load. A successful probe
within one round is not guaranteed when the existing 200ms budget rejects work.

The correction changes only that test's existing SweepIntervals seam to a 1s
status cadence, retaining the production startup/topology settings, real Git,
real supervisor restart, and every fact/event/health assertion. It explicitly
pins the production 60s constant and adds timeout state/skipped diagnostics.
The fixture now demands convergence within 6s, including retry allowance; this
is NOT a measured 6s production freshness guarantee. The initial ordinary
focused run exercised the unchanged production cadence and observed convergence
in 585ms, alive=true, age_ms=5, skipped=1 (the test deliberately adds one skipped
probe). The corrected instrumented focused run observed 1169ms, alive=true,
age_ms=4, skipped=1. It ran one test, raw exit 0; its raw log is
freshness-instrumented-test.log.gz. The preceding failed command spelling is
also retained (freshness-instrumented.json, raw exit 1, no tests executed).

A NEW complete serial invocation, `python3 docs/evidence/issue-561/gates.py final-`,
then passed all eleven commands at source HEAD
f6ecffdbbc7ce8c36affda75bc6af7e54b146d40. The final workspace and instrumented
suites each passed 509 tests, with 12 ignored; the separate #492 filter passed
7, and the survival regression passed 1. Core coverage was 90.35% lines / 87.11%
functions (floors 85/82); client coverage 42.21% / 37.98% (floors 40/35).
No gate was spliced together from the earlier stopped invocation. No production
source changed after the cost measurement except its documented bound comment;
the final test-only correction did not change the cost test body or intervals.

CI equivalence is local macOS, not a hosted Linux run. The brief's workspace fmt,
all-feature clippy and workspace tests are supersets of rust.yml's corresponding
commands; deny, audit, release build and both coverage thresholds were run.
Actions setup, Linux package installation, coverage artifact upload and hosted
CI were not run. A release BUILD is not a release/publication.
