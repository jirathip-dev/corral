# Corral — public-readiness + scripts review

> **Historical note — 2026-08-20:** Any cost-meter content in this historical review/brief describes Corral as it existed and the requirements under review at that time.
> The cost meter was retired by issue #107. For the current design, see `README.md`, `docs/ARCHITECTURE.md`, `docs/OPERATIONS.md`, `docs/DEVELOPING.md`, and `docs/QUICKSTART.md`.

Reviewer: Claude (Fable 5), static review, 2026-08-18. Files read in full:
`scripts/setup-corrald.sh`, `scripts/corrald-grant.sh`, `scripts/asc-beta-state.rb`,
`README.md`, `Cargo.toml`, `docs/QUICKSTART.md`, `docs/OPERATIONS.md`,
`fastlane/Fastfile`, `fastlane/.env.example`, `ios/project.yml`, `.gitignore`,
plus targeted reads of `src/main.rs`, `src/adapters/herdr.rs`, `src/core/model.rs`,
`src/fleet/config.rs`, `src/transcript/bind.rs`, `src/integrate/mod.rs` for Q8/Q9.

---

## Q1 — LICENSE

`Cargo.toml:15` declares `license = "MIT"` and there is no `LICENSE` file. That is
a real defect: GitHub won't show a license chip, `cargo publish` would warn, and
corporate users can't legally rely on a bare SPDX string in a manifest.

**Recommendation: dual-license `MIT OR Apache-2.0`**, the Rust-ecosystem default.
Corral is a daemon that shells out to claude/codex/opencode — it links nothing
proprietary and has no copyleft exposure, so permissive is right. Plain MIT is
defensible, but Apache-2.0 adds an explicit patent grant, which matters for a
control-plane daemon a company might embed in internal infra, and dual-licensing
costs nothing. Concretely:

1. Add `LICENSE-MIT` (full MIT text) and `LICENSE-APACHE` (full Apache-2.0 text),
   copyright line: `Copyright (c) 2026 Jirathip Kunkanjanathorn` (or "Corral
   contributors" if you want contributions folded in without a CLA).
2. Change `Cargo.toml:15` to `license = "MIT OR Apache-2.0"`.
3. README: add a short `## License` section plus badges:
   `![License](https://img.shields.io/badge/license-MIT%2FApache--2.0-blue)` and a
   CI badge once hosted CI (mentioned at `README.md:117`) is green.

If you'd rather keep it simple: single `LICENSE` file with MIT text and leave
`Cargo.toml` as-is. Either way the file must exist before flipping public.

## Q2 — setup-corrald.sh correctness

Overall a decent script (`set -euo pipefail`, index-loop arg parsing, `plutil
-lint` after writing the plist), but four real issues:

1. **Re-run does not apply config changes (worst bug).** Line 86:
   `launchctl bootstrap ... 2>/dev/null || launchctl kickstart -k ... || true`.
   On a second run, `bootstrap` fails ("service already loaded"), the fallback
   `kickstart -k` restarts the *already-loaded* job — launchd does **not** re-read
   the rewritten plist. So `scripts/setup-corrald.sh --bind 100.67.222.5` on a
   machine that already ran the script silently restarts the daemon on the old
   `127.0.0.1` binding. The script claims "Idempotent: safe to re-run" (line 4) —
   it's idempotent only when nothing changed. Fix: `launchctl bootout "gui/$(id
   -u)" "$PLIST" 2>/dev/null || true` before `bootstrap`, and drop the trailing
   `|| true` so a genuine bootstrap failure is fatal.
2. **Health check fails open.** Lines 90–94: on failure it prints
   `⚠ could not reach ...` and the script still exits 0. Combined with the
   swallowed errors on line 86 (`2>/dev/null` also discards launchd's actual
   error text), a totally broken install reports success to any caller checking
   `$?`. Fix: `exit 1` in the else branch, and retry the curl a few times instead
   of a single `sleep 2` (line 88) — a cold release binary + keygen can take >2s.
3. **`--bind` is unvalidated and interpolated into XML.** The heredoc at lines
   58–82 is intentionally unquoted so `$BIN`/`$HOME`/`$BIND` expand. A value like
   `--bind '127.0.0.1</string><string>--evil'` injects extra
   `ProgramArguments`. It's self-attack only (your user, your plist), and `plutil
   -lint` (line 83) catches malformed XML but *not* well-formed injection. Cheap
   fix: validate `[[ "$BIND" =~ ^[0-9a-fA-F.:]+$ ]]` after parsing. Same class of
   issue for `$REPO_DIR`/`$HOME` containing `&` or `<` — plutil catches those, so
   only the error message is bad. No TOCTOU issues of consequence (plist written
   then linted then loaded, all same-user files).
4. **`--bind` parsing edge:** line 28 `BIND="${args[$i]:-127.0.0.1}"` — a trailing
   `--bind` with no value silently defaults to loopback instead of erroring.
   Minor, but a typo'd invocation should fail loudly, not quietly bind loopback.

Also: the old project-specific launchd label (lines 40–41, echoed at
`docs/OPERATIONS.md:14`) carries a personal identifier into every public user's
LaunchAgents dir — rename to `com.corral.corrald` before release. And line 68
hardcodes the herdr socket into the plist; fine today, but it's a runtime-coupling
point to remember for Q8. The `sed -n '2,13p' "$0"` help (line 31) will drift the
moment the header comment changes length — minor.

## Q3 — corrald-grant.sh correctness

- **Admin token read (line 17)** is safe *because of* `set -e`: the `exit 1`
  inside `$( ... )` only exits the subshell, but the failed command substitution
  makes the assignment fail, and `set -e` kills the script. Correct, but fragile
  — it silently degrades to "continue with empty ADMIN" if anyone ever removes
  `set -e`. A two-line explicit check (`[[ -f ... ]] || die`) would be sturdier.
  Note also the token appears in `curl -H "Authorization: Bearer $ADMIN"`
  (line 62) — visible in `ps` output for the curl's lifetime. Loopback,
  single-user machine: acceptable; `-H @file` or `--config` would close it.
- **`--caps` is handled well**: line 55 pipes the raw value through
  `python3 ... json.dumps` as an argv element, so quotes/commas/unicode in caps
  can't break the JSON array. A malicious `--caps` value is inert data — the
  daemon's grant validation is the real gate, and the capability set is a fixed
  enum server-side (`docs/OPERATIONS.md:70-71`). No injection here.
- **`--key` is the quoting hole**: lines 50 and 56 splice `$KEY` directly into a
  hand-built JSON string (`\"key_id\":\"$KEY\"`). A key containing `"` produces
  invalid JSON or injected fields. Not a privilege escalation (the caller already
  holds the admin token), but inconsistent — you built the safe path for CAPS and
  skipped it for KEY. Build the whole body in the same python3 call.
- **curl lacks `-f`** (line 60): a 401/403/500 prints the error body but exits 0.
  Scripts wrapping this get a false success. Add `-f` (or check the response).
- Minor: `--key` as the last argument makes `shift 2` (line 23) fail with a
  cryptic `set -e` death; `--list` (lines 35–43) reads `registry.json` off disk
  rather than asking the daemon, coupling the script to the file schema — fine,
  but worth a comment.

## Q4 — asc-beta-state.rb

Token handling is sound in the ways that matter: the bearer is minted via
Spaceship (line 21–26), used only in the `Authorization` header (line 31), and
never printed. No secrets are logged. Two real issues:

1. **`ASC_KEY_PATH` resolution contradicts `.env.example`.** Line 24 resolves
   `File.basename(env.fetch("ASC_KEY_PATH"))` against `fastlane/` — i.e. it
   assumes the `.p8` lives **inside** `fastlane/`. But `fastlane/.env.example:6-7`
   says the key lives "OUTSIDE this repo — never copy the key itself in here."
   With a compliant `.env` (`ASC_KEY_PATH` pointing at a key file outside
   the repo, e.g. under `~/keys/`) this
   script — and the identical pattern at `Fastfile:25` and `Fastfile:161` — looks
   for `fastlane/AuthKey_X.p8` and fails. One of the two stories is wrong; a
   stranger following the docs hits this immediately. Either honor the absolute
   path (`File.expand_path(env.fetch("ASC_KEY_PATH"))` when absolute) or change
   `.env.example` to say "place the .p8 in fastlane/ (gitignored via *.p8)".
2. **Hardcoded `APP_ID = "6802181286"`** (line 36) — a personal ASC app id in a
   public diagnostic. Read it from env with the hardcoded value as fallback, or
   resolve it from the bundle id via the API.

Minor Ruby nits: the hand-rolled `.env` parser (lines 16–19) doesn't strip quotes
or skip `#` comments containing `=`; `Net::HTTP.start` has no open/read timeout;
the script prints tester emails (lines 54, 62) — fine locally, but the output is
PII, so never paste it into an issue. The `rescue` modifier on `JSON.parse`
(line 33) is acceptable for a diagnostic.

## Q5 — README public-readiness

What a stranger hits, top to bottom:

- **Line 3 assumes you know what herdr is.** "Corral is the control plane for the
  herdr agent fleet" is the first sentence; herdr is never defined or linked. A
  stranger needs a 2–3 sentence hook first: *the problem* (you run a fleet of
  coding agents in worktrees; you need one board, signed remote control, and cost
  visibility) — then "herdr (link) is the runtime that spawns them; corral is the
  control plane above it."
- **The "Loopback only" claim (line 57) will be false once #65 lands**, and the
  enforcement message is already stale today: `src/main.rs:571-579` refuses with
  "P1 corrald has no auth and must stay on loopback until P3 device signatures
  land" — P3 auth *is* in. Rewrite both together when #65 merges: README should
  say "binds loopback by default; non-loopback binds are restricted to private
  ranges and every write is still Ed25519-signed", and the guard's message should
  state the actual current policy. Shipping public with a security claim one
  merged PR away from false is how you earn a "misleading security docs" issue.
  Same claim also lives at `docs/QUICKSTART.md:31-32`, `docs/OPERATIONS.md:221`
  and the troubleshooting row at `docs/OPERATIONS.md:242`.
- **Retired provider-usage estimator:** issue #107 removes the estimator and
  its UI/API claims; current docs should describe the board as
  harness-agnostic and make no quota promises.
- **Missing:** license badge + CI badge; one screenshot of the egui board and one
  of the iOS notifier (a fleet dashboard repo with zero pixels is a hard sell);
  a "Status: pre-1.0, macOS-first" line — launchd, Keychain, and the iOS app are
  macOS/iOS stories and Linux support is implied only by the `keyring` features
  in `Cargo.toml:30`; a CONTRIBUTING note (even one paragraph); explicit
  statement that herdr is optional for the HTTP surface but required to see
  agents. Skip brew for now — a tap before an audience is noise.
- **Embarrassment scan:** no Thai text and no personal paths in README —
  clean. The jargon leaks elsewhere: "automation gateway" at `docs/OPERATIONS.md:23`
  (undefined, internal), "the proven sendmeter pattern" at `fastlane/Fastfile:2`
  (private project reference), your legal name in a comment at
  `fastlane/Fastfile:106`, and the old project-specific launchd label. The
  P1–P4 phase briefs linked at `README.md:158` are internal process docs; fine to
  keep, but label them "historical/internal" so nobody reads them as user docs.

## Q6 — Repo hygiene

- **`.gitignore` is genuinely good.** It covers `/target`, `build/`,
  `fastlane/certs/`, every `.env` spelling with a documented `!*.env.example`
  re-include, and `*.p8/p12/mobileprovision/cer` (lines 18–27) — and the comment
  (lines 9–13) records the #36 lesson plus a CI secret-scan that checks the git
  *index* rather than trusting ignore rules. That is exactly the right design.
  No committed secrets found in the reviewed files.
- **Team ID `9244PWFYD7`** at `ios/project.yml:15` and `fastlane/Fastfile:13`:
  team IDs are not secrets (they ship inside every distributed binary), so this
  is a reproducibility question, not a leak. A stranger cannot use your team ID,
  bundle id, or ASC app record anyway — so make `TEAM_ID` read
  `ENV["ASC_TEAM_ID"] || "9244PWFYD7"` in the Fastfile, and add three lines to
  `.env.example` or the Fastfile header saying: "to distribute your own build:
  set your own team ID, change the bundle id, create the app record once in ASC
  (the API can't — see `ensure_app_record`, Fastfile:61-69)."
- **The TestFlight story is *not* stranger-reproducible as documented** — the
  `ASC_KEY_PATH` contradiction from Q4 breaks it on step one, and
  `.env.example` never mentions the team-ID/bundle-id/app-record substitutions.
  Fix those and it's as reproducible as any fastlane lane can be, which is good
  enough.
- The comment at `Fastfile:106` naming your Distribution cert (full legal name)
  should be trimmed to "the already-installed Apple Distribution cert".

## Q7 — Overall verdict

**NOT public-ready today — but it is roughly one focused day away.** The blockers
are all packaging, not engineering: a declared-but-missing license, a security
claim about to go stale, personal identifiers baked into install paths, and a
credentials-path contradiction that breaks the one reproducible-ish pipeline.
The code-facing story (docs depth, `.gitignore` discipline, honest placeholder
labeling, verified-command quickstart) is well above the bar for a public repo.
See the MUST-FIX / SHOULD-FIX lists at the end.

## Q8 — Runtime-agnosticism (model → harness → runtime → control plane)

**Harness-agnostic: yes, already.** The core `Agent` struct
(`src/core/model.rs:168-197`) carries generic `source: String` / `tool: String`
labels; capabilities are a fixed array with the comment "never hardcoded per
tool" (`model.rs:157-165`); and there is no `match tool`-style branching anywhere
in `src/drive`, `src/approve`, or `src/api`. One precision on the brief's
framing: `apply_agent_info` (`src/adapters/herdr.rs:583`) doesn't so much
*normalize* harness kinds as **pass them through verbatim** —
`tool: tool.unwrap_or("unknown")` at `herdr.rs:688-689` — which is the correct
kind of agnosticism (uniform treatment, no per-harness logic). The
on-demand transcript reader's store binding (`src/transcript/`) is intentionally
isolated from the canonical board model because each harness has its own
session-store format.

**Runtime-bound to herdr: yes, and the coupling is localized.** The exact points:

1. **The adapter** — `src/adapters/herdr.rs`: unix-socket RPC (`:316-321`,
   `:1026`), `DEFAULT_SOCKET` (`:68`), agent-id namespacing `herdr:{v}` /
   `herdr:pane:{id}` (`:355-360`), attachment kind `"herdr-pane"` (`:707`), and
   the status vocabulary via `AgentState::from_herdr_status`
   (`src/core/model.rs:44-53` — the one herdr-named API on the core model,
   though it's a constructor, not a field).
2. **Wiring defaults in `main.rs`** — socket default (`:562-564`), and the
   worktree conventions (a repo checkout plus the Herdr worktree root;
   `:607-612`, env-overridable).
3. **Worktree layout assumption** — `src/integrate/mod.rs:121` documents
   `<root>/<repo>/<label>` and `:402` derives the repo from the first path
   component. This is the subtlest coupling: it leaks into PR/CI binding and
   transcript binding and Git facts.
4. **Fleet registry default path** — the legacy fleet registry fallback
   (`src/fleet/config.rs:180-188`), though `$CORRAL_FLEETS_PATH` already
   overrides it.
5. **The setup script** bakes the herdr socket into the launchd plist
   (`scripts/setup-corrald.sh:68`).

**A second-runtime / no-runtime mode** would be a second impl of the existing
adapter seam (`src/adapters/mod.rs`): a "static" adapter that derives agents
from `git worktree list` plus a small mapping file (worktree → tool + optional
pid/pane), emitting the same `Agent` records with `source: "static"` and no
drive capability beyond `read_tail`. Read-only fleet visibility for non-herdr
users; drive stays herdr-only until someone writes a supervising adapter.
Effort: **M** (the model needs nothing; the work is the adapter, a watcher, and
tests). It is **not a blocker** for public release — ship it as a documented
limitation in README and file it as a **separate issue**, not in #35 — #35 is
the fleet registry, and mixing "second runtime adapter" into it
would bloat a nearly-done slice.

## Q9 — Fleet registry and transcript binding

The fleet registry remains a separate CLI/configuration surface. It describes
repos, worktrees, workers, and models; it is not part of the daemon's live
agent snapshot. Transcript binding likewise stays at the read-path boundary:
the adapter supplies an agent's worktree and tool label, while the transcript
module owns store-specific lookup and redaction. Neither path adds provider
pricing or quota semantics to the canonical model.

---

## MUST-FIX (blocking)

1. **Add LICENSE file(s)** — `Cargo.toml:15` declares MIT with no LICENSE in the
   tree. Recommend `LICENSE-MIT` + `LICENSE-APACHE` and
   `license = "MIT OR Apache-2.0"`.
2. **Rename the old project-specific launchd label** →
   `com.corral.corrald` — `scripts/setup-corrald.sh:40-41`,
   `docs/OPERATIONS.md:14` (personal identifier installed onto every user's
   machine).
3. **Resolve the ASC key-path contradiction** — `fastlane/.env.example:6-7`
   ("key OUTSIDE this repo") vs the basename-into-`fastlane/` resolution at
   `fastlane/Fastfile:25`, `Fastfile:161`, `scripts/asc-beta-state.rb:24`. Pick
   one story; the current combination fails for anyone following the docs.
4. **Fix `setup-corrald.sh` reload + fail-open** — `scripts/setup-corrald.sh:86`
   (`kickstart -k` never re-reads a changed plist: `bootout` before `bootstrap`,
   drop `|| true`) and `:90-94` (health-check failure must `exit 1`).
5. **Fix the stale/soon-false loopback story in one pass with #65** —
   `README.md:57`, `docs/QUICKSTART.md:31-32`, `docs/OPERATIONS.md:221` and
   `:242`, plus the stale guard message at `src/main.rs:571-579` ("P1 … no auth …
   until P3" — P3 landed). If #65 merges after going public, this is a
   published-false security claim; land them together or reword the claim to
   "loopback by default" now.

## SHOULD-FIX

1. **`corrald-grant.sh` JSON body**: `$KEY` spliced unescaped into JSON at
   `scripts/corrald-grant.sh:50` and `:56` — build the whole body via the
   existing python3 call; add `-f` to the curl at `:60` so HTTP errors fail the
   script.
2. **Parameterize identity constants**: `TEAM_ID` at `fastlane/Fastfile:13` and
   `ios/project.yml:15` (env-overridable with your value as default), `APP_ID`
   at `scripts/asc-beta-state.rb:36`; document the "bring your own team/bundle
   id/app record" steps in `.env.example`. Trim the legal-name cert comment at
   `Fastfile:106`.
3. **README front door**: 2–3 sentence problem hook before the herdr reference
   (`README.md:3`), define/link herdr, license + CI badges, one desktop and one
   iOS screenshot, a "macOS-first, pre-1.0" support statement, and mark the
   P1–P4 briefs (`README.md:158`) as historical/internal.
4. **Validate `--bind`** in `scripts/setup-corrald.sh:28` (regex the address;
   error on missing value instead of defaulting) and lengthen/retry the health
   check at `:88-90`.
5. **De-jargon internal references**: use "automation gateway" in
   `docs/OPERATIONS.md:23` and remove private project references from the
   historical review notes; move the fleets.json default to
   `~/.config/corral/fleets.json` per Q9.

## Verdict

**NOT PUBLIC-READY today** — because of five packaging-level blockers (no
LICENSE file despite a declared MIT field, a loopback security claim one merged
PR from false with an already-stale enforcement message, a personal launchd
label, a broken-by-contradiction ASC credentials story, and a setup script whose
re-run silently ignores config changes and exits 0 on failure) — **but the gap
is about one day of mechanical work, not a rethink.** The underlying repo is
unusually strong for a pre-1.0 public flip: disciplined `.gitignore` with a CI
secret scan born from a real leak, verified-command docs, bounded transcript
reads, and a core model that is already harness-agnostic
with runtime coupling neatly localized to one adapter. Fix the five must-fixes,
add the README front door, and flip it public.
