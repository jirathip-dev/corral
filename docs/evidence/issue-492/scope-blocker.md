# Issue 492 — scope blocker, not an implementation delivery

## Disposition

Stopped at the brief's hard-fence instruction before editing production code or running heavy work. No acceptance criterion is claimed complete. This report does not establish a fix for either git-plane death or CPU thrash.

Worktree: `/Users/jirathip/.herdr/worktrees/corral/impl492-gitplane`.
Branch: `g492-gitplane-thrash`.
Initial HEAD: `92ac61d32068c882ff8e971a1470847717e12cd7`, matching the brief exactly. Initial working tree was clean.

Read `.hermes-context.md`, the entire `.brief.md`, issue 492's body and all 13 returned comments. Read the relevant repository development guide and scoped implementation. No AGENTS.md, CLAUDE.md or justfile was present in the root discovery; no recipe was invented.

## Blocking fence conflicts and minimum authorization changes

BLOCKER: `src/api/read_timing.rs:19` — the brief explicitly requires extending the existing bounded logging mechanism, but this file is not in the allowed-file list. Its `ReadTiming` type and methods are private to the API module (`pub(super)` at lines 19 and 38; `new` is private at line 28). The git plane cannot reuse that mechanism without changing its visibility/API or relocating it, neither of which the fence permits. Minimal fix: authorize this file for narrowly scoped reuse/extension of the existing limiter and its tests; alternatively explicitly amend the brief to allow an independent git-plane limiter. Do not silently duplicate the mechanism against the brief.

BLOCKER: `src/main.rs:52` / `scripts/rotate-corral-logs.sh:28` — no separate repository logging configuration file was found that can implement the required writer cap/rotation within the fence. The daemon configures its tracing writer in `main.rs:52-56`, but this file is allowed ONLY for liveness/age/backlog wiring. The existing macOS rotation policy is code in the expressly forbidden `scripts/**`: it checks a 50 MiB threshold, is scheduled every 1800 seconds by `scripts/setup-corrald.sh:297`, and calls `launchctl kickstart -k` at `scripts/rotate-corral-logs.sh:114` to reopen the descriptor. It is not a per-write bound or a non-dropping ERROR/panic writer, and cannot be invoked against the live service under this brief. Linux output goes to journald at `scripts/setup-corrald-linux.sh:189-190`; no per-daemon size cap is configured there. Minimal fix: authorize the logger initialization in `src/main.rs` (and a narrowly named logging module/test if wanted) for a stdlib-only bounded writer with an explicit ERROR/panic failure policy, or name the exact permitted repository configuration/installer files and required policy. No installed configuration or service needs to be changed to implement/test that fix.

BLOCKER: `tests/model.rs:96` — the current snapshot serialization test constructs `Snapshot` with an exhaustive Rust struct literal. Adding the required health/counter fields to `src/core/model.rs:205` in the requested existing-field pattern necessarily requires initializing them here; serde defaults do not fill Rust struct literals. This test is not a git-plane test and is outside the literal allowlist. Minimal fix: authorize only additive initialization/assertions in `tests/model.rs`, preserving every existing assertion. A second serialization path, global state lookup during serialization, or test-only alternate schema would be inappropriate workarounds.

These are repository-source authorization conflicts, not host contention or provider failures. No host install/change is requested. The exact minimum file approvals above allow a subsequent implementation lane to proceed without pretending that existing rotation guarantees lossless logging.

## Findings retained for the implementation (static, not runtime proof)

- `src/adapters/git_plane.rs:952`: the reported event-path poison-to-panic site still uses `self.state.lock().unwrap()`.
- `src/adapters/git_plane.rs:1283-1294`: the over-budget arm holds the state mutex while emitting its WARN. Moving logging out of the critical section avoids poisoning that mutex if a writer/subscriber panics. This is a candidate hardening point, NOT proof that logging caused the historical first panic. The first panicker remains unpinned; no historical log/heap experiment was performed by this lane.
- `src/adapters/git_plane.rs:1979-1982` wraps the whole probe in the 200 ms timeout; `run_git` acquires the permit at `:2047-2050`. The permit-wait defect remains in the pinned base.
- `src/adapters/git_plane.rs:2227-2235` detaches watcher/sweep tasks without retaining their join results. `src/main.rs:460-463` observes the integrator join, not those task joins. No new supervision was implemented.
- The published snapshot boundary (`src/core/store/published.rs`) was inspected, not modified. No HTTP serialization path, SSE framing, epoch behavior or multi-worker runtime was changed.

## Commands actually run and raw results

Commands ran in the named worktree unless a file-tool read is noted. No test output was filtered to manufacture an exit status.

| Command | Raw exit | Result / retained log |
| --- | --- | --- |
| `git status --short` | 0 | empty at initial preflight |
| `git branch --show-current` | 0 | `g492-gitplane-thrash` |
| `git rev-parse HEAD` | 0 | pinned base above |
| `gh issue view 492 --json title,body,comments` | 0 | `/tmp/g492-issue.json`; body and all comments consumed |
| `gh issue view 492 --json comments -q '.comments[] | .author.login + ": " + .body'` | 0 | `/tmp/g492-comments.log` |
| `ast-grep run -p '$STATE.lock().unwrap()' -l rust src/adapters/git_plane.rs` | 0 | `/tmp/g492-lock-sites.log`; matching lock sites including line 952 and the over-budget arm |
| `ast-grep outline src/adapters/git_plane.rs` | 0 | `/tmp/g492-outline.log`; mapped watcher, sweep, probe and tests |
| `ast-grep outline src/core/store.rs src/core/model.rs src/main.rs` | 0 | `/tmp/g492-wiring-outline.log`; located current backlog/snapshot/supervisor symbols |
| `ast-grep run -p 'Snapshot { $$$FIELDS }' -l rust src/core/store/published.rs src/core/store/publication_tests.rs tests/http.rs tests/model.rs tests/store.rs` | 0 | `/tmp/g492-snapshot-constructors.log`; exhaustive literal at `tests/model.rs:96-102` |
| `ast-grep run -p 'tracing_subscriber::fmt()' -l rust src/main.rs` | 0 | `/tmp/g492-logging-entry.log`; initialization at line 52 |

Preferred tools were used: `read_file` for source and logs, `search_files` for exact logging/config text, `ast-grep` for structural questions, `terminal` for git/gh/AST commands. No fallback tools, installs or reconstructed just recipes were needed. Read-only commands were bounded by 30- or 60-second tool deadlines. No heavy command ran, so there was no cargo lock/target/cache allocation.

## Report-only validation

- `git diff --check` → raw exit 0, empty `/tmp/g492-report-diff-check.log`.
- `git diff --exit-code -- src tests crates scripts .github Cargo.toml Cargo.lock deny.toml` → raw exit 0, empty `/tmp/g492-source-unchanged.log`.
- Original `.report.md` from `git show HEAD:.report.md` compared to the updated file's prefix using Python `bytes.startswith` → raw exit 0, `PRIOR_REPORT_PREFIX_IDENTICAL=true`, `/tmp/g492-report-prefix.log`. Original-prefix SHA-256: `ea7733bcfd77e3be1d05d9b20c72fa3ca82fcc358b6235a61c4ad8bc56646463`.
- `git ls-remote --heads origin g492-gitplane-thrash` → raw exit 0 with empty output before report delivery; the lane branch was not already present on the remote.

These checks validate documentation preservation/scope only, not the requested implementation. The report-only delivery commit is identified by the final lane response rather than a self-referential commit hash in this file.

## Acceptance/gate status

| Required verification | Status |
| --- | --- |
| Poison recovery + supervised respawn, unwrap mutation RED/GREEN | NOT RUN; no implementation |
| 100 fake worktrees × 50 ms × four permits, clock mutation RED/GREEN | NOT RUN; no implementation |
| Stale-fact policy and disabling-policy mutation | NOT RUN; no implementation |
| Byte-identical mutation restores | NOT APPLICABLE; no source mutation |
| Alive/stopped real `/snapshot`, Mac then Bazzite | NOT RUN |
| Interval CPU and WARN-volume before/after | NOT MEASURED on either host |
| `cargo test --workspace` | NOT RUN, no raw exit |
| `cargo clippy --all-targets -- -D warnings` | NOT RUN, no raw exit |
| `cargo deny check` | NOT RUN, no raw exit |
| `cargo build --release` | NOT RUN, no raw exit |

All implementation, regression, runtime measurement and full-gate items remain outstanding. Owner post-deploy soak and phone-visible validation remain owner gates. No CPU reduction, new health field, stale-fact expiry, supervision behavior or ERROR/panic logging guarantee is claimed.

NON-CLAIMS: no deploy/restart, no live service/config changes, no worktree pruning, no gh-plane change, no dependency change, no PR/merge/issue mutation, no main/release/TestFlight activity, and no resolution of the multi-day heap/RSS question.
