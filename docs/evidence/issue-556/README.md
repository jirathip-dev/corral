# Issue 556 evidence

The implementation and gate report is `../../../.report.md`. This directory contains
production-daemon observations, explicitly fictional binding controls, regression
probes, and native component renders. It is not deployment evidence.

## Reproduce

Run from the repository root. Observe the lane's no-competing-heavy-job preflight;
all heavy work uses `flock /tmp/n.lock`. Rust artifacts use
`CARGO_TARGET_DIR=/tmp/g556-target` and native builds use `/tmp/g556-dd`.

```sh
CARGO_TARGET_DIR=/tmp/g556-target flock /tmp/n.lock cargo build --release
flock /tmp/n.lock python3 docs/evidence/issue-556/daemon-evidence.py \
  /tmp/g556-target/release/corrald none --seconds 300 --output /tmp/g556-none-repro
CARGO_TARGET_DIR=/tmp/g556-target flock /tmp/n.lock \
  python3 docs/evidence/issue-556/regression-evidence.py
```

The mutation driver temporarily edits the lane's production files, restores their
exact bytes in `finally`, and records restored SHA-256 values. Run it only in an
owned, otherwise-idle worktree. The three mutations bypass file permissions,
stale-binding publication, and the response-redaction call respectively. Restored
selected tests run once at the end. These are mutation REDs, not an old-base run.

For the credential paths, supply `G556_LIVE_TOKEN` to the evidence driver through
the operator's environment, never argv. Use mode `env` or `file`, `--seconds 90`,
and the branch/number of an actually open PR via `--branch` and `--pr`. Mode
`refused --seconds 5` uses only an obviously fake credential with mode 0644.
The daemon's own PATH is `/usr/bin:/bin`, with no `gh`, and its environment excludes
host service credentials and proxies. It uses an isolated config directory, port,
Herdr Unix socket, and temporary git checkout. It never connects to the installed
Corral service or restarts it.

## Evidence interpretation

- `none/` is a five-minute real-binary observation, with an SSE subscriber and
  nonempty git-backed agents. `connections.log` contains repeated raw
  `lsof -nP -a -p PID -i` output and exits. Sampling alone cannot exclude a
  connection that vanished between samples; the regression transport spy also
  proves zero calls with no token, including after a subscriber connects.
- `env/` and `file/`, when present, contain LIVE GitHub responses folded onto
  FICTIONAL Herdr agents and checkouts. The bound checkout intentionally matches
  a real PR by branch, not by the fake commit SHA. Its peer has no matching PR.
  They are not observations of an existing fleet lane. `live-pr.json` identifies
  the PR observed. `before.json` precedes the first SSE-triggered poll.
- `env-first-alias/` retains the failed first live binding observation (driver
  exit 1). Its synthetic rows used `/tmp/...` while #492's fresh fact keys used
  `/private/tmp/...`; the additional raw-spelling entries were `stale: true`, so
  publication correctly followed that metadata and hid the binding. The driver
  now advertises physical fixture paths. Alias normalization is NOT repaired by
  this lane; a caller supplying such an alias can still lose a fresh binding.
  `first-daemon-runs.json` preserves the failed attempt separately from the four
  successful cases in `daemon-runs.json`.
- The no-token before/after assertion compares `agents` and `rev`, not clock
  metadata: `generated_at` and fact ages naturally change. History file sizes
  before/after measure transition-journal bytes, not diagnostic-log bytes.
- Fixture JSON extracted from regression stdout is labelled `fixture-*`; it
  exercises the real integrator, pre-serialized publication, SSE delta stream,
  and resume buffer with synthetic GitHub data, including age >=120 seconds,
  absent facts, a stopped git plane, and recovery without another GitHub poll.
- `ios/` contains ImageRenderer attachments of the unchanged production
  `WorkspaceLine`, not full-screen captures or physical-device evidence.
  The four cases are unbound, orphan CI, bound #556, and cleared/stale. The
  manifest records original XCTest attachment identities and pixel hashes.
- `.log.gz` files are losslessly compressed raw stdout/stderr (gzip mtime zero).
  Decompress with `gzip -dc FILE.log.gz`. JSON gate receipts give raw child
  exits and original `/tmp/g556-*.log` locations. Expected mutation exits are 101.

The owner-only post-deploy Mac/Bazzite RSS/CPU comparison, actual-host installation
without `gh`, physical iOS behavior, and promotion are not claimed here.
