# Issue 539 — curl-only installer evidence

Implementation head: d588c98845ccb9f87c0603891b043c987bce3532
Branch: g539-curl-install
Base: 05dafa0801c264de193e3f518914738c5ed1357d
The later delivery commit adds evidence/report only; its SHA is the final lane message.

Reproduce from the worktree with `python3 docs/evidence/issue-539/run-proof.py`.
The completed invocation exited 0. Commands have deadlines (240 seconds per
local operation; 300 seconds for the remote proof). No test command was piped
through a filter to infer PASS. `commands.json` records driver argv, environment,
raw exits and working directories. `scenario-results.json` records EVERY installer
scenario's expanded command, raw exit and actual diagnostics; `macos.log.gz` and
`bazzite.log.gz` are the unabridged transcripts, including HTTP curl argv and PATH
lookups. Read gzip text with Python's `gzip.open(path, "rt")`.

## Results

- macOS host: `Darwin arm64`; `/bin/bash scripts/test-install-corral-linux.sh`
  exit 0, 1275 executed assertions,
  113 installer invocations (100 resolution cases,
  13 preserved-behavior cases). Both Linux-dispatch and native Darwin-dispatch
  installer paths ran; macOS `plutil` checked generated daemon plists.
- Bazzite host: `Linux x86_64`; same command, exit 0,
  671 executed assertions,
  64 recorded invocations (50 resolution cases,
  13 preserved-behavior cases and `systemd-analyze verify`, itself exit 0).
- Installer fixture binaries are deliberately inert; systemctl, launchctl and
  health checks are stubbed. This proves resolution, checksums, staging and the
  existing service-helper integration, not that a real daemon binary was launched.
- True pre-fix control: exact base installer + new suite in a disposable scripts
  tree exited 1; first installer exited 97 at poison `gh api`, rather than 0.
  `base-forbidden.log` records the real intercepted command. The suite had
  completed 1 assertion before rejecting `test 97 -eq 0`. The lane's sources were
  never swapped; all 7 copied script hashes matched the tested tree afterwards.

## Resolution cases and raw exits

Every row ran in the poison-gh and truly-absent-gh environments on Linux;
macOS additionally ran every row through Darwin dispatch. `resolution_case`
expands to `env -i HOME=<case> PATH=<controlled> TMPDIR=<case>/tmp
CORRAL_INSTALL_DIR=<case>/install CORRAL_CONFIG_DIR=<case>/config ...
/bin/bash scripts/install-corral.sh`. Exact per-case assignments and output,
not guessed example invocations, are in `scenario-results.json`. Scenario
assertion counts are emitted at runtime as `SCENARIO_ASSERTIONS`; suite totals
also count the preserved unit-file, hash, rollback and uninstall checks.

| Scenario | Installer raw exit | Observed message/result |
| --- | --- | --- |
| `latest` | 0 | Installed Corral (verified SHA-256; managed-service stub reached) |
| `pinned-tag` | 0 | Installed Corral (verified SHA-256; managed-service stub reached) |
| `pinned-url` | 0 | Installed Corral (verified SHA-256; managed-service stub reached) |
| `latest-env` | 0 | Installed Corral (verified SHA-256; managed-service stub reached) |
| `relative-location` | 0 | Installed Corral (verified SHA-256; managed-service stub reached) |
| `slash-tag` | 0 | Installed Corral (verified SHA-256; managed-service stub reached) |
| `repo-override` | 0 | Installed Corral (verified SHA-256; managed-service stub reached) |
| `rate-403-bare` | 3 | !! release rate limit/access refusal (HTTP 403): latest endpoint |
| `rate-403-delay` | 3 | !! release rate limit/access refusal (HTTP 403): latest endpoint;    Retry-After: 120 (wait until this delay/date before retrying) |
| `rate-403-date` | 3 | !! release rate limit/access refusal (HTTP 403): latest endpoint;    Retry-After: Thu, 17 Sep 2026 12:00:00 GMT (wait until this delay/date before retrying) |
| `rate-403-reset` | 3 | !! release rate limit/access refusal (HTTP 403): latest endpoint;    X-RateLimit-Reset: 1790000000 (Unix seconds; do not retry before reset) |
| `rate-429-bare` | 3 | !! release rate limit/access refusal (HTTP 429): latest endpoint |
| `rate-429-delay` | 3 | !! release rate limit/access refusal (HTTP 429): latest endpoint;    Retry-After: 120 (wait until this delay/date before retrying) |
| `rate-429-date` | 3 | !! release rate limit/access refusal (HTTP 429): latest endpoint;    Retry-After: Thu, 17 Sep 2026 12:00:00 GMT (wait until this delay/date before retrying) |
| `rate-429-reset` | 3 | !! release rate limit/access refusal (HTTP 429): latest endpoint;    X-RateLimit-Reset: 1790000000 (Unix seconds; do not retry before reset) |
| `dns` | 4 | !! release network/TLS failure (curl exit 6) |
| `offline` | 4 | !! release network/TLS failure (curl exit 7) |
| `missing-latest` | 5 | !! release/tag or asset missing (HTTP 404) |
| `missing-redirect` | 5 | !! missing release tag redirect from jirathip-dev/corral |
| `missing-tag` | 5 | !! release/tag or asset missing (HTTP 404) |
| `missing-checksum` | 5 | !! release/tag or asset missing (HTTP 404) |
| `download-rate` | 3 | !! release rate limit/access refusal (HTTP 429): https://github.com/jirathip-dev/corral/releases/download/v0.1.0/corral-v0.1.0-linux-x86_64.tar.gz;    Retry-After: 120 (wait until this delay/date before retrying) |
| `download-offline` | 4 | !! release network/TLS failure (curl exit 7) |
| `server-error` | 6 | !! unexpected release HTTP status 503 |
| `checksum-mismatch` | 1 | !! SHA-256 mismatch — refusing to install |

Preserved-behavior raw exits (Bazzite): `fresh`=0, `systemd-analyze`=0, `migrate`=0, `reinstall`=0, `update`=0, `rollback-existing`=1, `checksum`=1, `unhealthy`=1, `recover`=0, `uninstall`=0, `nohome`=2, `noarch`=2, `freebsd-selftest`=0, `noplat`=2.
macOS has the same exits except that `systemd-analyze` is explicitly skipped
because it is absent. The added existing-release rollback checks restore the
v2 binary hash after an unhealthy v1 update, remove `.previous`, stop the failed
service, and preserve existing key bytes. Uninstall preserves those bytes too.
Checksum mismatch checks require no install root and no service calls. Every
failed resolution checks no install root, no service call, no source-tool or gh
invocation, preserved config/key bytes and cleaned installer temporary files.

## gh absence versus poison shim

An executable poison `gh` and empty `command -v gh` cannot coexist on the SAME
PATH. The suite therefore installs the poison shim first and exercises two
honest environments rather than hiding that contradiction:

1. Poison leg: `<work>/poison-bin:<work>/stub-bin:/usr/bin:/bin`.
   `command -v gh` exits 0 and prints exactly `<work>/poison-bin/gh`.
   The shim logs then exits 97 if invoked. The cumulative forbidden log remains
   empty while latest/tag/URL installs succeed.
2. Absent leg: `<work>/stub-bin:/usr/bin:/bin`.
   Non-login `/bin/bash --noprofile --norc -c 'command -v gh'` exits 1 with
   EMPTY stdout, checked inside every case. Installs still succeed.

`macos-forbidden.log` and `bazzite-forbidden.log` are actual saved 0-byte logs;
no gh/cargo/rustc/git invocation occurred in the successful suites. Credentials
are absent because each installer uses `env -i` and a fake HOME. A fixture brew
path prevents Darwin setup's existing PATH helper from introducing host Homebrew.
The local `gh issue view` used for brief discovery is NOT part of these test
environments or a claim that gh is uninstalled on either host.

## Bazzite isolation and cleanup

Before/after roots: live `~/.local/share/corral`, `~/.config/corral`,
`~/.config/systemd/user/corrald.service`, and `~/.local/bin/gh`.
The owner-installed gh was never moved, renamed, removed or invoked remotely.
`bazzite-isolation.json.gz` contains both exact manifests: file SHA-256, mode,
size, mtime_ns, symlink-target hashes and directory metadata. Path identifiers
are hashed to avoid publishing config/device filenames. Both manifests contain
5767 entries; aggregate digest before AND after:
`aae33212865d6921f67bf752dd6a89f0daf94643b6f163e04ca05fde84c5623f`.
Both service probes exited 0, `active`; MainPID=3887997 and
ActiveEnterTimestampMonotonic=1081826071056 were identical. Full manifests and
service records compare equal, not just a subset of binaries.

Created on Bazzite: `/tmp/corral539-proof-pc4ute00`, marker `.created-for-539`,
a tar of the 7 named scripts, copied scripts and proof driver, fake HOME and
TMPDIR, fixture release bundles/checksums/binaries, PATH stubs (including poison
gh), per-scenario fake install/config/unit roots and keys, and evidence logs.
The suite removed its sandbox `/tmp/corral539-proof-pc4ute00/tmp/tmp.gYtjvwSbfS`
(including all staged releases, fixtures, keys and stubs) after copying logs.
The driver fetched receipts, then removed ONLY its marked root and all remaining
contents. `remote-cleanup.log.gz` enumerates the remaining exact removed paths;
`remote-cleanup-readback.log.gz` proves the root absent (exit 0) and service
still active. The suite's TMPDIR was empty. No actual systemd/launchd mutation,
stop, reconfiguration or real Corral installation occurred.

The macOS outer `/var/folders/4k/fbv06j2j5bbd3h4wgbwk1ccm0000gn/T/corral539-local-4wh1l09n`
was also removed by TemporaryDirectory; per-case sandbox paths and cleanup
assertions are in the logs. Initial focused-run diagnostic logs are archived
separately and are not substituted for the final suite.

## Exact gate/driver commands

| Label | Raw exit | Command |
| --- | --- | --- |
| `syntax` | 0 | `/bin/bash -c 'for f in scripts/*.sh; do bash -n "$f" &#124;&#124; exit; done; printf "bash -n: all scripts OK\n"'` |
| `shellcheck-installer` | 0 | `shellcheck scripts/install-corral.sh` |
| `shellcheck-suite` | 0 | `shellcheck -e SC2016 scripts/test-install-corral-linux.sh` |
| `macos-path` | 1 | `/bin/bash --noprofile --norc -c 'command -v gh'` |
| `macos` | 0 | `/bin/bash scripts/test-install-corral-linux.sh` |
| `base-source` | 0 | `git show 05dafa0801c264de193e3f518914738c5ed1357d:scripts/install-corral.sh` |
| `base-red` | 1 | `/bin/bash scripts/test-install-corral-linux.sh` |
| `remote-create` | 0 | `ssh -o BatchMode=yes -o ConnectTimeout=10 bazzite 'mktemp -d /tmp/corral539-proof-XXXXXXXX'` |
| `remote-marker` | 0 | `ssh -o BatchMode=yes -o ConnectTimeout=10 bazzite 'touch /tmp/corral539-proof-pc4ute00/.created-for-539'` |
| `remote-copy` | 0 | `scp -o BatchMode=yes -o ConnectTimeout=10 /var/folders/4k/fbv06j2j5bbd3h4wgbwk1ccm0000gn/T/corral539-local-4wh1l09n/scripts.tar.gz bazzite:/tmp/corral539-proof-pc4ute00/scripts.tar.gz` |
| `remote-extract` | 0 | `ssh -o BatchMode=yes -o ConnectTimeout=10 bazzite 'tar -xzf /tmp/corral539-proof-pc4ute00/scripts.tar.gz -C /tmp/corral539-proof-pc4ute00'` |
| `bazzite-driver` | 0 | `ssh -o BatchMode=yes -o ConnectTimeout=10 bazzite 'env PATH=/usr/bin:/bin /usr/bin/python3 /tmp/corral539-proof-pc4ute00/bazzite-proof.py /tmp/corral539-proof-pc4ute00'` |
| `remote-receipt` | 0 | `scp -o BatchMode=yes -o ConnectTimeout=10 bazzite:/tmp/corral539-proof-pc4ute00/results/isolation.json /Users/jirathip/.herdr/worktrees/corral/impl539-curl-install/docs/evidence/issue-539/bazzite-isolation.json` |
| `remote-fetch-suite.log` | 0 | `scp -o BatchMode=yes -o ConnectTimeout=10 bazzite:/tmp/corral539-proof-pc4ute00/results/suite.log /var/folders/4k/fbv06j2j5bbd3h4wgbwk1ccm0000gn/T/corral539-local-4wh1l09n/suite.log` |
| `remote-fetch-bazzite-exits.tsv` | 0 | `scp -o BatchMode=yes -o ConnectTimeout=10 bazzite:/tmp/corral539-proof-pc4ute00/results/scenario-logs/exits.tsv /Users/jirathip/.herdr/worktrees/corral/impl539-curl-install/docs/evidence/issue-539/bazzite-exits.tsv` |
| `remote-fetch-bazzite-forbidden.log` | 0 | `scp -o BatchMode=yes -o ConnectTimeout=10 bazzite:/tmp/corral539-proof-pc4ute00/results/scenario-logs/forbidden.log /Users/jirathip/.herdr/worktrees/corral/impl539-curl-install/docs/evidence/issue-539/bazzite-forbidden.log` |
| `remote-cleanup` | 0 | `ssh -o BatchMode=yes -o ConnectTimeout=10 bazzite '/usr/bin/python3 -c '"'"'import pathlib,shutil; p=pathlib.Path('"'"'"'"'"'"'"'"'/tmp/corral539-proof-pc4ute00'"'"'"'"'"'"'"'"'); assert (p/'"'"'"'"'"'"'"'"'.created-for-539'"'"'"'"'"'"'"'"').is_file(); items=sorted(str(x.relative_to(p)) for x in p.rglob('"'"'"'"'"'"'"'"'*'"'"'"'"'"'"'"'"')); print('"'"'"'"'"'"'"'"'REMOVED_ROOT'"'"'"'"'"'"'"'"', p); print('"'"'"'"'"'"'"'"'CREATED_AND_REMOVED'"'"'"'"'"'"'"'"', items); shutil.rmtree(p); assert not p.exists(); print('"'"'"'"'"'"'"'"'CLEANUP_VERIFIED absent'"'"'"'"'"'"'"'"')'"'"''` |
| `remote-cleanup-readback` | 0 | `ssh -o BatchMode=yes -o ConnectTimeout=10 bazzite 'test ! -e /tmp/corral539-proof-pc4ute00 && systemctl --user is-active corrald'` |
| `diff-check` | 0 | `git diff --check` |

`run-proof.py` itself: exit 0. Syntax printed `bash -n: all scripts OK`.
Both final ShellCheck logs are empty. The suite's SC2016 exclusion is solely
for the existing single-quoted fixture writers, not a newly suppressed rule.
There is no justfile or AGENTS.md/CLAUDE.md in this worktree/ancestor search;
no `just` recipe was invented. No Rust build/daemon smoke or hosted workflow
was claimed. `python3 ios/check-release-demo.py` was NOT run: no pinned Swift
source or release-manifest file changed.

## Earlier diagnostics and metadata correction

Initial `bash -n scripts/install-corral.sh` and
`bash -n scripts/test-install-corral-linux.sh`: each 0.
Initial `shellcheck scripts/install-corral.sh scripts/test-install-corral-linux.sh`:
1, existing SC2016 fixture diagnostics; raw log `initial-shellcheck.log.gz`.
Initial `CORRAL_TEST_EVIDENCE_DIR=/tmp/corral539-macos-focused env PATH=/usr/bin:/bin
/bin/bash scripts/test-install-corral-linux.sh`: 0, 1266 executed assertions;
`initial-focused.log.gz`. The final suite then added existing-release rollback
and explicit remaining assertion counts before the one full two-host proof.

Structural discovery:
- `ast-grep run --lang bash --pattern 'resolve_release_urls() { $$$BODY }'
  scripts/install-corral.sh`: exit 8, query parser rejected multiple nodes.
- `ast-grep run --lang bash --kind function_definition --json=compact
  scripts/install-corral.sh`: exit 0, 12 functions including resolver at base
  line 332. An initial Python pretty-printer exited 1 (quoting SyntaxError);
  the corrected printer listed all 12 functions.
- `ast-grep run --lang bash --pattern 'gh $$$ARGS' scripts/setup-corrald.sh
  scripts/lib-corral-update-path.sh`: exit 1, no executable gh call matches.
- `ast-grep run --lang bash --kind list --json=compact
  scripts/test-install-corral-linux.sh`: exit 0; existing OR-list assertions
  were used to place runtime assertion counters without weakening them.

The evidence driver initially stored the mutable env dictionary by reference:
changing the base-proof output directory altered two saved metadata entries
retroactively. The driver now stores `env.copy()`; only those two
`CORRAL_TEST_EVIDENCE_DIR` metadata values were corrected to the actually used
`macos-logs` directory. Raw suite logs, exits and outputs were never changed.
This evidence-only fix was AST-parsed; it did not cause a redundant full rerun.

## Dependency and scope contracts

Resolver before:
```
tag="$(gh api "repos/$RELEASE_REPO/$endpoint" --jq '.tag_name')"
asset_list="$(gh api "repos/$RELEASE_REPO/releases/tags/$tag" --jq '.assets[] | [.name, .browser_download_url] | @tsv')"
```
Resolver/download after (one shared HTTP helper):
```
fetch_release "https://github.com/$RELEASE_REPO/releases/latest" /dev/null --head
curl -q -sS --connect-timeout 15 --max-time 300 --max-redirs 10 \
  "$mode" --dump-header "$WORK_DIR/http.headers" --output "$output" \
  --write-out '%{http_code}' "$url"
ASSET_URL="https://github.com/$RELEASE_REPO/releases/download/$tag/$ASSET_NAME"
CHECKSUM_URL="$ASSET_URL.sha256"
fetch_release "$ASSET_URL" "$BUNDLE_FILE" --location
fetch_release "$CHECKSUM_URL" "$CHECKSUM_FILE" --location
```
Latest uses the website's tag redirect; pinned tags form release asset URLs
without discovery; pinned URLs still append `.sha256`. `--release`/`--url`
parsing and mutual exclusion are unchanged. Slash-to-underscore asset naming
matches the existing release workflow. No JSON endpoint remains.

403/429 refuse immediately, preserving Retry-After (seconds or HTTP date) and
X-RateLimit-Reset in the loud diagnostic. There is NO automatic retry or sleep:
the installer does not send a request ahead of the server's retry boundary and
instructs the caller to wait. Headers are case-insensitive and only the final
HTTP response counts. Downloads also use this classifier, so an error after
resolution is not hidden. Curl ignores curlrc (`-q`) and has bounded connect,
request and redirect limits. Offline/DNS/TLS failures report the original curl
exit; missing releases/assets, HTTP errors and SHA mismatch are distinguishable.
No fallback source-build path was added.

Production dependency set does not grow: Bash + curl + existing Unix tools
(awk, basename, dirname, sed, tr, mktemp, tar, grep, head, id, uname, mkdir, rm,
mv, chmod, and existing platform service/helper utilities). SHA-256 still uses
shasum OR sha256sum OR openssl, unchanged. Resolver adds NO jq/Python/Rust/git or
GitHub authentication; removes gh. awk/mktemp/rm were already installer
requirements. Python/SSH/ShellCheck here are proof-driver tools, not installation
requirements.

Production diff: scripts/install-corral.sh +71/-29;
scripts/test-install-corral-linux.sh +276/-41. All other changes are this
issue's evidence and the append-only report entry. `scripts/update-corral.sh`,
setup helpers, runtime GitHub code, iOS, crates, src and workflows are byte
unchanged. No new automatic updater or packaging framework was introduced;
existing release setup behavior was preserved rather than changed out of scope.

Report prefix at base: 188196 bytes,
SHA-256 `4cafd683386e25247759f40d1a6c1d81ffb17b1120afd2203b62023360365537`. The closeout verifies this entire
prefix against the final archive, checks the changed-path fence and runs
`git diff --check` and `git status --porcelain`. File/line inventory is
`files.txt`; final archive append/status receipt is `closeout.json`.

NON-CLAIMS: no device evidence, no hosted-CI claim, no merge, no push, no release,
no runtime-GitHub change, no authenticated runtime/core-board audit, no live
public asset download, and no real daemon launch. These are hermetic installer
proofs on two actual hosts. Stopping at the brief's fence.
