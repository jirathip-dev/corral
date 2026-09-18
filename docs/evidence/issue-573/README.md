# #573 — watcher boot hydrated one known path (Linux inotify registration artifacts)

Base (the failing head): `21ada0a719491cda8a3764ee32c31574689a144e` (current `integration`).
Arbiter: hosted `rust` job, CI run **35298137718** (`gh run view 35298137718 --repo jirathip-dev/corral --log-failed`):

```
thread 'adapters::git_plane::tests::g561_watcher_boot_has_zero_hydration_for_known_facts' panicked at src/adapters/git_plane_issue561_tests.rs:389:5:
assertion `left == right` failed: unchanged restart must not eagerly hydrate known paths
  left: 1     right: 0
test result: FAILED. 328 passed; 1 failed; 1 ignored; finished in 28.82s
```

The owner had already ruled out arity/`--test-threads` knobs and "green locally": the same
suite on the same head is 329/0 in 24.6 s on the macOS host and 328/1 in 28.8 s on the Linux
runner. #572 is in effect at this head (per-plane `ProbeRuns`), so a delta of exactly 1 cannot
be a foreign admission: the probe belongs to the plane under test.

## 1. The orchestrator's hypothesis is REFUTED

The hypothesis was: `apply_probe`'s facts are not visible in `lock_state()` synchronously when
the boot filter (`git_plane.rs:970-976`) reads them, so the path looks unknown and gets
debounced.

Evidence against it (all raw outputs archived here):

1. `apply_probe` (`src/adapters/git_plane.rs`) writes `st.commit` / `st.status` /
   `st.branch` inside one `lock_state()` critical section, sends its events, and clears
   `publication_pending` only after the cached facts still match the probe
   (`:1597-1658`). The fixture `await`s that call before spawning the watcher, so the facts are
   in the map — under the same mutex — before the boot filter can run.
2. `red-witness/boot_initial_probe.py` prints the boot filter's actual short-circuit set at the
   moment it runs (temporary `#[cfg(test)]` `eprintln!`, both mutations reverted
   byte-identically afterwards). At the pre-fix head, with the witness fixture armed:

```
G573_BOOT_INITIAL=[]          <- the boot filter had NOTHING to hydrate (path known)
raw_exit=101
  left: 1                     <- a probe still escaped
 right: 0
```

An empty short-circuit set with a non-zero probe delta means the probe did **not** come from the
boot filter. The escaping probe comes from the event path, and the events exist because of the
watcher's own registration pass.

## 2. The real mechanism: registration-time `Access(Open)` events on Linux

Call graph at boot (pre-fix head):

```
run_watcher (git_plane.rs:941)
  -> self.rescan(&sink)                          (registry known; no probes)
  -> register_new_commondir_watchers(...)        (notify: RecommendedWatcher::new + watch(cd, Recursive))
       notify 8.2.0 inotify backend -> EventLoop::add_watch -> WalkDir over the commondir
       walkdir opens EVERY directory it descends into (fs::read_dir) BEFORE yielding it;
       the commondir's own watch is already live by then, so the kernel reports each open
       as IN_OPEN on the parent watch with the child's name  ->  notify maps it to
       EventKind::Access(AccessKind::Open(AccessMode::Any)) with that child path
  -> boot filter (short circuit; empty for a known path)
  -> loop: progress(0) -> select! -> event_rx -> handle_fs_event_batch(batch)
       -> map_event_path("<commondir>/refs")   -> ["refs"] with an empty rest -> the main checkout
       -> debounce(repo)                        (pending set; dedupes the burst to ONE task)
       -> sleep(DEBOUNCE=300ms) -> probe_worktree_with_budget -> probe_runs.record()   <-- +1
```

Measured with a minimal reproduction of `new_commondir_watcher` (`mechanism/probe-src`, one
`notify` watcher on `<repo>/.git`, `Recursive`, all events printed for 2 s):

| host | backend | events after registration |
|---|---|---|
| macOS (this host) | `Fsevent` | **0** |
| Linux (Bazzite node, kernel 6.17) | `Inotify` | **14**, all `Access(Open(Any))`: `.git/hooks`, `.git/info`, `.git/refs`, `.git/refs/heads`, `.git/refs/tags`, `.git/objects{,/pack,/info,/<xx>×3}`, `.git/logs`, `.git/logs/refs`, `.git/logs/refs/heads` |

Raw: `mechanism/macos-fsevent.log`, `mechanism/linux-inotify.log`.

`map_event_path` resolves several of those to the main checkout:
`.git/refs` and `.git/logs` hit `Some("refs") | Some("logs")` with an empty remainder
(`["HEAD"] | [] => main`), and `.git/refs/heads`, `.git/refs/tags`, `.git/logs/refs`,
`.git/logs/refs/heads` fall through to "every worktree". All of them coalesce into ONE
`debounce(repo)` because `debounce` inserts into `state.pending` and sets `rerun` for the rest.

The kernel semantics are documented, not inferred: `inotify(7)` lists `IN_OPEN` among the
events marked with an asterisk, and states that "when monitoring a directory, the events marked
with an asterisk above can occur for files in the directory". notify's inotify mask includes
`IN_OPEN` (`notify-8.2.0/src/inotify.rs`, `add_single_watch`), and its event loop maps it to
`Access(Open(Any))`.

### The exact window in which the probe escaped

The test measures from `progress_ms[0] > 0` for `DEBOUNCE + 200 ms`:

- `progress(0)` is set at the top of the first watcher loop iteration (`git_plane.rs:982`),
  i.e. after registration and after the (empty) boot filter.
- The registration events are already queued in `event_tx` at that point (the probe log above
  shows them at `t_ms=0`), so the first `select!` iteration processes them and calls
  `debounce(repo)`.
- The debounce task sleeps `DEBOUNCE` (300 ms) and then records the probe — 300 ms into the
  500 ms measurement window. The second mapped event only sets `rerun`, whose follow-up probe
  lands at ~600 ms, outside the window. That is why the delta is exactly **1**.

### Why the hosted runner exposes it and this host does not

The difference is the **platform backend, not scheduling**: macOS `RecommendedWatcher` is the
FSEvents backend and FSEvents has no "file was opened" concept, so the registration walk emits
nothing (0 events measured). Linux uses inotify, whose `IN_OPEN` is a real event, and it is
delivered for the children of a watched directory. Both CI runs of this head on Linux
(2e8ce2c, 21ada0a) failed with `left: 1`; the macOS host is green in both states.

## 3. The fix

`handle_fs_event_batch` now ignores events that cannot represent a change
(`src/adapters/git_plane.rs`):

```rust
for event in events {
    // An access is not a change (#573). ...
    if matches!(event.kind, notify::EventKind::Access(_)) {
        continue;
    }
```

- An open (or a read-close) never changes a worktree's facts; only create/remove/modify events
  can. The mask notify subscribes to on inotify delivers `Modify(Data)` for writes, so dropping
  `Access` loses no change signal (the FSEvents backend emits no `Access` events at all, so
  macOS behaviour is unchanged).
- This is also the root cause of a second Linux-only defect: every probe reads
  `.git/index`/`.git/HEAD`/refs, so pre-fix each probe's own opens re-armed the debounce and
  kept a self-sustaining probe loop alive. After the fix the loop is gone.
- Production probe call sites and their ordering are untouched: the filter sits upstream of
  `debounce` in the same event handler; `debounce`, `run_sweep` and `run_status_sweep` probe
  paths are unchanged.

## 4. Deterministic RED witness (form (a))

The witness is a test-only boot hook: `GitPlane.registration_open_events` (an `AtomicBool`
field, `#[cfg(test)]` only — production carries no field, no branch, no state) is armed by the
new fixture, and `run_watcher` then injects one `Access(Open(Any))` event per directory under
each commondir into the watcher's event channel, i.e. exactly what the Linux backend delivers.
`inject_registration_open_events` is `#[cfg(test)]`.

Tests added (additions only) to `src/adapters/git_plane_issue561_tests.rs`:

- `g573_watcher_boot_ignores_registration_open_events` — the boot witness (same shape as the
  #561 fixture: rescan, probe, apply, boot, measure `DEBOUNCE + 200 ms`, assert 0 hydration
  probes).
- `g573_access_events_are_not_change_signals` — discrimination: an `Access(Open)` event for a
  mapped path yields no debounce target, while a `Modify(Data)` event for the same path still
  maps to its worktree.

`red-witness/red_witness.py` reverts ONLY the production filter hunk, runs the witness, and
restores the file byte-identically (`red-final.log`):

```
before_sha256   = 5ac3c4c1e7d1f35ee71ddd52f1782bd14dcaf99bb47c84fc32c107a34c4d967f
mutated_sha256  = 0bcce8131e3b751b0689abde0bf26e66c51de6616b9c1b3e7f5d8456bc6f456e
raw_exit        = 101
assertion       = registration-time access events are not changes and must not hydrate known paths
  left: 1  right: 0
restored_sha256 = 5ac3c4c1e7d1f35ee71ddd52f1782bd14dcaf99bb47c84fc32c107a34c4d967f
byte_identical_restore = True
```

`red-witness/red-witness-hashes.txt` records the same values plus the driver/log hashes.

The witness fails with the same `left: 1 / right: 0` shape as the hosted failure, and the fix
(not the environment) is what makes it pass.

## 5. Linux end-to-end validation (the CI platform)

The base head was checked out and built on a Linux node (Bazzite, kernel 6.17, rust 1.97.1 —
the pinned toolchain), then the real #561 fixture was run:

| state | command | raw exit | result line |
|---|---|---|---|
| pre-fix (3 runs) | `cargo test --lib -p corrald g561_watcher_boot` | 101, 101, 101 | `0 passed; 1 failed; 329 filtered out; finished in 0.62-0.63s` |
| pre-fix (4th, logged) | same, `--nocapture` → `linux/pre-fix-g561.log.gz` | 101 | `left: 1 right: 0`, 0.63 s |
| fixed | same → `linux/fixed-g561.log.gz` | 0 | `1 passed; 0 failed; finished in 1.02s` |
| fixed | `g573_` → `linux/fixed-g573.log.gz` | 0 | `2 passed; 0 failed` |
| fixed | `cargo test --lib -p corrald` → `linux/fixed-lib-default.log.gz` | 0 | `331 passed; 0 failed; 1 ignored; finished in 23.03s` |
| fixed | `cargo test --lib -p corrald adapters::git_plane::tests -- --test-threads=2` (the #572 leg) → `linux/fixed-module-t2.log.gz` | 0 | `50 passed; 0 failed; 1 ignored; finished in 18.22s` |

The 0.62-0.63 s pre-fix duration matches the hosted failure (the failing test finished 0.63 s
after the previous test released the lock) — the same escape, reproduced off-CI.

## 6. Gates and suites (raw exits)

Local macOS host at the fixed head (`CARGO_TARGET_DIR=/tmp/corral-573/target`), see `gates/`:

| gate | raw exit |
|---|---|
| `cargo fmt --check` | 0 |
| `cargo clippy --workspace --all-targets -- -D warnings` | 0 |
| `cargo deny --locked --workspace check` | 0 (`advisories ok, bans ok, licenses ok, sources ok`) |
| `cargo audit --deny warnings` | 0 |
| `gitleaks dir . --config .gitleaks.toml --redact --no-banner --exit-code=1` | 0 (`no leaks found`) |
| tracked secret files (`git ls-tree` basename scan) | none |
| `cargo test --lib -p corrald` (default) | 101 (`329 passed; 2 failed` under load 61.6, see below), then 0 (`331 passed; 0 failed; 1 ignored`, 23.93 s at load 19.9) |
| `cargo test --lib -p corrald -- --test-threads=2` | 0 (`331 passed; 0 failed; 1 ignored`) |
| `cargo test --lib -p corrald -- --test-threads=4` | 101 (`330 passed; 1 failed` under load 61.6-75.8, see below), then 0 (`331 passed; 0 failed; 1 ignored`, 30.85 s at load 12.0) |
| `g492` | 0 |
| `g560` | 0 |
| `g561` | 0 |
| `probe_accounting_` (#572 witness pair) | 0 |
| `g573_` (#573 witness pair) | 0 |
| `cargo build --release -p corrald` | 0, and the test-only hook leaves no symbol/string in the binary (`production-hook-absence.txt`) |

Load-sensitive flake disclosure (every raw log kept under `gates/` and `flakes/`): this host was
under an extreme external load for part of the session (1-minute load averages 12-82; the fleet
runs here). Three suite runs failed with pure starvation symptoms and passed on a load-dip
re-run at the same bytes:

- head default, load 61.6: `g561_generation_preserves_facts` (`:68`) and
  `g561_panic_between_cache_and_send_reemits_all_facts` (`:300`, the 8 s re-emission expect).
- head `--test-threads=4`, load 61.6-75.8: `core::store::publication_tests::g560_dead_plane_ages_reuse_agent_encoding` (`:443`).
- head `--test-threads=4`, load 55.3 (earlier run): `g561_changed_inventory_and_missed_change_converge`
  and `g561_generation_preserves_facts`, both `git-plane condition did not converge` (`:23`).

No non-starvation assertion failed in any run. The same class reproduces at the BASE head:
`cargo test --lib -p corrald g492` failed 2/10 base runs
(`g492_permit_queue_is_not_execution_budget_and_sweep_is_paced` at `:246`,
`g492_supervisor_respawns_poisoned_child_and_wire_reports_stopped` at `:116`) — base logs in
`flakes/`.

## 7. Assertion integrity / no weakening

`assertion-integrity.txt`: the protected assertion in
`git_plane_issue561_tests.rs` (`assert_eq!(hydration_probes, 0, "unchanged restart must not
eagerly hydrate known paths")`) is byte-identical to the base; the #561 test body is untouched;
no test was removed, skipped, `#[ignore]`d, and no tolerance, window or timeout was widened.
The diff to the test file is additions only.

## 8. What was not verified

- The hosted run itself: the acceptance gate is the green `rust` job on the promotion PR after
  merge; the branch is unpushed and no hosted verdict is claimed here.
- `g561_restart_cost_measurement` (the `#[ignore]`d 3×65 s cost witness) was not re-run.
- The Linux node is not a GitHub runner: it reproduces the mechanism and the fixture outcome on
  the same OS/backend, but it is not the hosted environment.
