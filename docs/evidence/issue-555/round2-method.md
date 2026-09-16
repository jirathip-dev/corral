# #555 round-2 reproduction

The single-thread HTTP runtime was removed. HTTP uses the original multi-worker
runtime; publication still uses `spawn_blocking`. This does not reserve CPU for
reads or make blocking work elsewhere harmless. In particular, exhausting all
runtime workers remains possible. The regression below checks the escaped
cross-route case, not an OS scheduling guarantee.

## Freshness is a separate budget

A fast `/snapshot` can contain data about two seconds stale after a change
(background debounce), versus freshly flushed data at base. Foreground debounce
is 250 ms. The existing HTTP test allows three seconds for publication including
encoding/scheduling; that is a test deadline, not a hard OS SLA. With NO changes,
an unchanged buffer's timestamp can age beyond two seconds without missing any
new state. The idle history fixture below is exactly such a case. Judge change
visibility using the busy-writer load's age/revision evidence, not idle buffer age.

## Cross-route check

`round2-http-load.py` uses the reviewer's real-binary method: a synthetic 1024-row
history ring (four segments of 256), `/history?limit=5000` CPU/JSON work, a new
loopback TCP connection for each request, and INFO logging to an ordinary file.
There are no live repositories, herdr socket, credentials or executable helpers.
Every history worker must complete requests DURING the snapshot sample window;
errors are fatal, never discarded. Each request has a two-second network timeout.

Each fresh daemon runs these phases, three times total, retaining all 128 samples
per phase per run:

- idle: serial `/snapshot` calls, no competing readers;
- single: serial `/snapshot` calls with one competing history reader;
- concurrent: serial `/snapshot` calls with eight competing history readers.

The separate `history_single_ms` is one warm-up history request. Times include
TCP/HTTP and full response-body receipt, unlike the original in-process router
accounting. Quantiles use nearest rank; the comparison uses the median of the
three runs' p50 and p99. All runs are retained, not a fastest-run selection.

The bound was fixed before measuring: concurrent candidate p50 <= 2 * base p50
+ 1 ms, p99 <= 2 * base p99 + 2 ms. The additive margins avoid amplifying
sub-millisecond host/loopback scheduling noise; they are not the original
20/50 ms absolute budget. `check` asserts both inequalities and exits nonzero on
a violation. A matching harness SHA-256 and complete sample inventory are also
mandatory. The reviewed binary and an isolated executor-only reintroduction are
negative controls, not accepted candidates.

Run from the named worktree, with no sibling cargo/Xcode work. `daemon-run.py`
acquires `/tmp/n.lock`, records host/disk state, uses only its marked
`/tmp/g555-daemon-target`, bounds each command and records its raw exit. Archive
builds never change this worktree or any branch. Example commands (substitute a
fresh output label rather than overwriting an earlier evidence run):

```sh
python3 docs/evidence/issue-555/daemon-run.py r2-base-build python3 docs/evidence/issue-555/round2-build.py 11cc1d26f3b1c22716d4527fb9e7679eaa861aba base
python3 docs/evidence/issue-555/daemon-run.py --timeout 120 r2-http-base python3 docs/evidence/issue-555/round2-http-load.py record --binary /tmp/g555-daemon-target/r2-base-corrald --source-ref 11cc1d26f3b1c22716d4527fb9e7679eaa861aba --output /tmp/g555-r2-http-base.json
python3 docs/evidence/issue-555/daemon-run.py r2-release cargo build --release
python3 docs/evidence/issue-555/daemon-run.py --timeout 120 r2-http-fixed python3 docs/evidence/issue-555/round2-http-load.py record --binary /tmp/g555-daemon-target/release/corrald --source-ref "$(git rev-parse HEAD)" --output /tmp/g555-r2-http-fixed.json --bounded-logs
python3 docs/evidence/issue-555/round2-http-load.py check /tmp/g555-r2-http-base.json /tmp/g555-r2-http-fixed.json
```

`round2-build.py 60851ed reviewed` supplies the reviewed binary. After the fixed
head is committed, `round2-build.py HEAD mutant` builds that head with ONLY the
old HTTP-executor hunk restored; record and check it with the identical load
harness. The build receipt pins source ref and binary SHA-256. Both controls must
be measured rather than inferred from source. `round2-build.py 60851ed logging-red`
also runs the new real-handler logging test against the old implementation.

## Logging and existing gates

Snapshot and SSE-first-frame observations each share one process-wide aggregate,
not one budget per client. Atomic admission permits one info emission per five
seconds per endpoint; high-frequency callers cannot multiply that rate. Records
contain request count, maximum serve time and maximum buffer age since the last
emission. Maxima include unsampled calls. They are opportunistic: the next read
emits, no idle timer runs, and a final partial window waits for another read.
Concurrent updates may straddle reporting windows; maxima and `latest_*` metadata
are not a single request's tuple. SSE's serve time includes a legitimate wait for
a first delta/keepalive on a current cursor, as before this round.

Focused commands:

```sh
cargo test --release --lib api::read_timing -- --nocapture
cargo test --release --test read_logging -- --nocapture
cargo test --release --lib core::store::publication_tests -- --nocapture
cargo test --release --test http -- --nocapture
```

The publication suite still exercises 64 randomized snapshot bodies and complete
SSE frames against independent prior encoders. `tests/snapshot_load.rs` is held
byte-identical to round 1; run its original nice-0 CPU-hog / busy-writer leg with:

```sh
python3 docs/evidence/issue-555/daemon-run.py --hog --budget --timeout 120 r2-hog cargo test --release --test snapshot_load -- --ignored --nocapture
```

Build the ignored load target before that leg, so compilation is not part of the
measurement. Keep the original baseline record at `11cc1d26...` and compare the
new result to it. Full gates remain `cargo fmt --all --check`,
`cargo clippy --all-targets -- -D warnings`, `cargo test --workspace`,
`cargo deny check`, and `cargo build --release`, each separately logged with a raw
exit. No iOS, hosted-CI, installed-daemon or physical warm-return claim follows
from these local gates.
