# #572 evidence — git-plane probe accounting is per-plane, not process-global

Fix head (code): `f0a108b8bf0a22134fce65e5507e038bf23f3dcb`
Base: `2e8ce2cedeb627bcff168a13cf2b41ee2fe79fbb` (current `integration`, includes the `main` `7d79581` merge)
Worktree: `/Users/jirathip/.herdr/worktrees/corral/impl572-testiso` (branch `g572-testiso`)
Build cache: `CARGO_TARGET_DIR=/tmp/g572-target` (removed after the lane; nothing was built into the worktree)
This evidence commit is docs-only and rides on top of the fix head; `git diff --stat f0a108b..HEAD -- src/ .github/` is empty.

Raw logs are committed losslessly as `.log.gz` (the repo's #561 convention) so log whitespace
cannot trip `git diff --check`. Read one with `gzip -dc <file>`.

## 1. The defect

The git-plane fixtures shared ONE process-global probe counter:

| site | role |
|---|---|
| `src/adapters/git_plane.rs:387` (base) | `static TEST_PROBE_RUNS: AtomicU64` |
| `git_plane.rs:2308` (base) | `TEST_PROBE_RUNS.fetch_add(1, …)` on **every admitted probe in the binary** |
| `git_plane.rs:3611` (base) | `TEST_PROBE_RUNS.store(0, …)` — `TestGitDelayReset::new`, i.e. **another test zeroes it** |
| `git_plane.rs:3711/3720` (base) | a convergence wait + an exact-count assert |
| `git_plane_issue561_tests.rs:366/373` (base) | `delta = TEST_PROBE_RUNS.load() - calls` |

Any probe admitted by any test (including a background watcher/sweep task leaked past its
fixture's guard) landed in every other reader's before/after window, and any
`TestGitDelayReset::new` zeroed every concurrent reader's window. The two CI failures below
are that interference.

## 2. The CI failure this fixes (archived, real)

`gh run view 35292288740 --repo jirathip-dev/corral --log-failed` — workflow `rust.yml`,
`headSha 2e8ce2c…` (this lane's base), job `rust`, `conclusion: failure`, run created
`2026-09-18T00:42:11Z`. Full failed-log committed as `ci-run-35292288740-failed.log.gz`;
verbatim excerpt:

```
failures:

---- adapters::git_plane::tests::g561_panic_between_cache_and_send_reemits_all_facts stdout ----

thread 'adapters::git_plane::tests::g561_panic_between_cache_and_send_reemits_all_facts' (3830) panicked at src/adapters/git_plane_issue492_tests.rs:23:6:
git-plane condition did not converge: Elapsed(())

---- adapters::git_plane::tests::g561_watcher_boot_has_zero_hydration_for_known_facts stdout ----

thread 'adapters::git_plane::tests::g561_watcher_boot_has_zero_hydration_for_known_facts' (3834) panicked at src/adapters/git_plane_issue561_tests.rs:382:5:
assertion `left == right` failed: unchanged restart must not eagerly hydrate known paths
  left: 1
 right: 0

failures:
    adapters::git_plane::tests::g561_panic_between_cache_and_send_reemits_all_facts
    adapters::git_plane::tests::g561_watcher_boot_has_zero_hydration_for_known_facts

test result: FAILED. 325 passed; 2 failed; 1 ignored; 0 measured; 0 filtered out; finished in 34.80s
error: test failed, to rerun pass `-p corrald --lib`
##[error]Process completed with exit code 101.
```

Note what the archived run actually is: the **full `-p corrald --lib` suite at DEFAULT
parallelism** on the small CI runner — not a low-thread invocation. `left: 1` is exactly one
foreign admission inside the 8th fixture's delta window.

## 3. Reproduction at the base — MY numbers (the race did NOT reproduce on this host)

Host: 10-core macOS. Same commands, same host, at base `2e8ce2c`, before any edit:

| invocation | raw exit | result line |
|---|---|---|
| `cargo test --lib -p corrald g561` | 0 | `ok. 5 passed; 0 failed; 1 ignored; 0 measured; 322 filtered out; finished in 6.32s` |
| `cargo test --lib -p corrald g561 -- --test-threads=2` | 0 | `ok. 5 passed; 0 failed; 1 ignored; 322 filtered out; finished in 6.20s` |
| `cargo test --lib -p corrald g561 -- --test-threads=4` | 0 | `ok. 5 passed; 0 failed; 1 ignored; 322 filtered out; finished in 6.17s` |
| `cargo test --lib -p corrald` (full, default) | 0 | `ok. 327 passed; 0 failed; 1 ignored; finished in 22.88s` |
| `cargo test --lib -p corrald -- --test-threads=2` (full) | 0 | `ok. 327 passed; 0 failed; 1 ignored; finished in 39.08s` |

The three-arity table from the issue (`PASS / FAIL / FAIL`) did **not** reproduce here: no
failing test name appeared in any base run. This matches the orchestrator's own eight base
runs (`.amend-1.md`), so the CI failure is environment-dependent (2-core runner scheduling)
and cannot be witnessed locally in that form. The last two rows were produced in a throwaway
detached worktree (`git worktree add --detach /tmp/g572-base 2e8ce2c`, own target dir,
removed afterwards) so the lane tree was never checked out to base.

**Therefore the owner-required "revert → the low-parallelism run fails again" witness is NOT
shown here, because it is not true on this host.** No proximity/grep substitute is offered.
The deterministic witness in §5 carries the proof of the isolation instead.

## 4. The fix

- `ProbeRuns` (`git_plane.rs:391-438`) replaces the static: one handle per `GitPlane`
  (`probe_runs` field, `git_plane.rs:659`), threaded through
  `probe_worktree_admitted(.., probe_runs: &ProbeRuns)` (`:2359-2366`, `probe_runs.record()`
  at `:2365`), `probe_worktree_with_budget(..)` (`:2311-2324`) and the test helper
  `probe_worktree(..)` (`:2342-2356`). Both production call sites pass the plane's own handle
  (`debounce` `:1337-1343`, `run_sweep`'s `paced_probes` closure `:1489-1494`).
  A probe is therefore recorded against the plane that ran it; no fixture can observe or
  zero another test's count. Production builds carry a **zero-sized** handle
  (`#[cfg(not(test))] struct ProbeRuns;`) whose `record()` is a no-op — the probe path does
  no bookkeeping and no production behaviour changes.
- The reset moves out of `TestGitDelayReset::new` (module-shared globals only) to the one
  fixture that measures its own baseline: `plane.probe_runs.reset()` in
  `queued_probe_events_coalesce_to_one_follow_up` (`:3752`).
- The #561 fixture reads its own plane's count (`git_plane_issue561_tests.rs:373`/`:380`, helper
  handle at `:369`); the invariant assertion is byte-unchanged (§7).
- The module's remaining process-global test state (`GIT_CALLS`, `TEST_GIT_*`) stays
  serialized by the existing module-level `PROBE_LOCK` (a `tokio::sync::Mutex`, the repo's
  own pattern, `git_plane.rs:375`): every test that runs a real probe or mutates those
  globals holds it for its whole body, and the two new witnesses join it. `TestGitDelayReset`
  still resets the globals it owns.
- CI (`rust.yml`): the default-parallelism `cargo test` step is kept and a **low-parallelism
  leg is added** (`test (git plane, low parallelism)` →
  `cargo test --lib -p corrald adapters::git_plane::tests -- --test-threads=2`; 48 tests,
  18.4 s locally). Additive, not a replacement.

## 5. Deterministic RED witness (the isolation itself)

Form produced: **two independent planes under test must not observe or disturb each other's
probe accounting, and a reset on one plane must not clear another's count** — the property
that fails on this host at NORMAL parallelism when the counter is process-global again.
This is the form `.amend-1.md` asked for; the flaky CI-race form is not claimed.

Witness tests (in `git_plane.rs` tests, holding `PROBE_LOCK`):
`probe_accounting_is_scoped_to_the_plane_under_test` (`:3793`) and
`probe_accounting_reset_is_scoped_to_the_plane_under_test` (`:3851`).

RED: re-globalise the counter — `ProbeRuns::new()` returns a clone of one process-wide
`OnceLock<Arc<AtomicU64>>` instead of a fresh counter (single-hunk mutation, applied by
`red-witness/red_witness.py`, the exact pre-#572 shape):

```
$ CARGO_TARGET_DIR=/tmp/g572-target cargo test --lib -p corrald probe_accounting_
RED_EXIT=101
test adapters::git_plane::tests::probe_accounting_reset_is_scoped_to_the_plane_under_test ... FAILED
test adapters::git_plane::tests::probe_accounting_is_scoped_to_the_plane_under_test ... FAILED
thread '…reset_is_scoped…' panicked at src/adapters/git_plane.rs:3878:9:
assertion `left == right` failed
  left: 2
 right: 1
thread '…is_scoped_to_the_plane_under_test' panicked at src/adapters/git_plane.rs:3812:9:
assertion `left == right` failed: a plane records exactly the probes it admitted
  left: 3
 right: 1
test result: FAILED. 0 passed; 2 failed; 0 ignored; 0 measured; 328 filtered out; finished in 0.55s
```

(`left: 3` = plane A's own probe plus plane B's two; `left: 2` = the shared count a
cross-plane reset had already corrupted. Both are the shared-counter defect, not timing.
The `git_plane.rs:` line numbers in this excerpt are from the MUTATED tree, as the test
binary reported them.)

Re-running the same mutation with the committed `red-witness/red_witness.py`
(`red-witness/red-witness-run2.log.gz`, raw exit 101) moves the assertion text — because
both witness tests charge the ONE shared counter concurrently, which is the defect itself:

```
thread '…probe_accounting_is_scoped_to_the_plane_under_test' panicked at src/adapters/git_plane.rs:3837:9:
assertion `left == right` failed: the second plane records exactly its own two probes
  left: 3
 right: 2
thread '…probe_accounting_reset_is_scoped_to_the_plane_under_test' panicked at src/adapters/git_plane.rs:3878:9:
assertion `left == right` failed
  left: 5
 right: 1
test result: FAILED. 0 passed; 2 failed; 0 ignored; 0 measured; 328 filtered out; finished in 0.50s
```

Both runs are RED with exit 101; the fixed tree is GREEN in both runs of the same two tests
(exit 0).

Restore — byte-identical (`red-witness/red-witness-hashes.txt`):

```
sha256_before  882c2bb58242539262e6e3eefa9406a941d2f5b540e3f37409a3c278a349a8e1
sha256_mutated 6bf7ab529b6048cd7cc1b02d687ee7fa58947c72c8522db744da213f5afbc338
sha256_after   882c2bb58242539262e6e3eefa9406a941d2f5b540e3f37409a3c278a349a8e1
RESTORED_IDENTICAL=True
git status --short after restore: only the orchestrator's untracked `.amend-1.md`
```

GREEN at the restored head: `cargo test --lib -p corrald probe_accounting_` → exit 0,
`ok. 2 passed; 0 failed; 328 filtered out` (`witness-green-after-restore.log.gz`), and
before the mutation the same two tests were green at default parallelism
(`witness-green-default.log.gz`, exit 0).

## 6. GREEN at the fix head (raw exits)

| invocation | raw exit | result line |
|---|---|---|
| `cargo test --lib -p corrald g561` | 0 | `ok. 5 passed; 0 failed; 1 ignored; 324 filtered out; finished in 6.46s` |
| `cargo test --lib -p corrald g561 -- --test-threads=2` | 0 | `ok. 5 passed; 0 failed; 1 ignored; 324 filtered out; finished in 6.28s` |
| `cargo test --lib -p corrald g561 -- --test-threads=4` | 0 | `ok. 5 passed; 0 failed; 1 ignored; 324 filtered out; finished in 6.32s` |
| `cargo test --lib -p corrald adapters::git_plane::tests -- --test-threads=2` (new CI leg) | 0 | `ok. 48 passed; 0 failed; 1 ignored; 281 filtered out; finished in 18.39s` |
| `cargo test --lib -p corrald` (full, default) | 0 | `ok. 329 passed; 0 failed; 1 ignored; 0 filtered out; finished in 23.04s` |
| `cargo test --lib -p corrald -- --test-threads=2` (full) | 0 | `ok. 329 passed; 0 failed; 1 ignored; 0 filtered out; finished in 38.81s` |

The low-arity runs are green BEFORE and AFTER this change on this host — stated plainly: the
CI race did not reproduce here in either state, and the deterministic witness in §5 is what
proves the isolation. (`329` = the base's `327` + the two new witness tests.)

## 7. Assertion / tolerance integrity

`assertion-integrity.txt` (regenerate with the commands inside) proves:

- every REMOVED line in `2e8ce2c..f0a108b` is either the global counter's own code or a
  call-site argument list — no line matching `assert|expect|timeout|Duration::from|ignore|panic!`
  was removed or modified;
- the protected #561 block is byte-identical at the head:

```
389-    assert_eq!(
390-        hydration_probes, 0,
391:        "unchanged restart must not eagerly hydrate known paths"
392-    );
```

- no test was removed, skipped, or `#[ignore]`d (the one `#[ignore]` in the module, the
  #561 cost measurement, is unchanged); no tolerance/window was widened.

## 8. Gates (raw exits at the fix head)

| gate | raw exit | summary |
|---|---|---|
| `cargo fmt --check` | 0 | (no output) |
| `cargo clippy --workspace --all-targets -- -D warnings` | 0 | `Finished dev profile … in 1.17s` |
| `cargo test --workspace` | 0 | 21 binaries, all `0 failed`; corrald lib `329 passed; 1 ignored`, corrald-client lib `14 passed`, doc-tests `0` |
| `cargo deny check` | 0 | `advisories ok, bans ok, licenses ok, sources ok` |
| `cargo audit` | 0 | `Loaded 1247 security advisories … Scanning Cargo.lock for vulnerabilities (356 crate dependencies)` (no vulnerabilities) |

Per-binary attribution for `cargo test --workspace` is in `gate-test-workspace.log.gz`
(`Running …` header before each `test result:` line).

## 9. Pin check

`grep -rnE "Sha256|include_bytes!|include_str!|digest|APPROVED_RELEASE_SOURCE_DIGEST"` over
the touched files (`src/adapters/git_plane.rs`, `src/adapters/git_plane_issue561_tests.rs`,
`.github/workflows/rust.yml`): **no matches**. No gate script in `scripts/`, `tools/` or
`.github/` references `git_plane.rs` (the digest-bearing gates are the iOS/demo/install
artifacts). **PIN: none.**

## 10. Reproduce

`run.sh` re-runs everything in this bundle; `red-witness/red_witness.py` applies/restores the
re-globalising mutation (it prints the sha256s and never leaves the tree modified).
`run-sh-validation.txt` is the receipt of one full execution of the committed driver at head
`e10a3e5` (base legs 0/0/0/0/0, witness green 0, RED 101 with a byte-identical restore, and
every fix-head acceptance leg 0).

## 11. Fence

Touched (fix commit): `src/adapters/git_plane.rs` (+189/−?), `src/adapters/git_plane_issue561_tests.rs`,
`.github/workflows/rust.yml`; evidence commit: `docs/evidence/issue-572/**` and `.report.md`.
No `ios/**`, no unrelated `src/**`, no production behaviour change (the production
`ProbeRuns` handle is a zero-sized no-op). No third site was needed.
