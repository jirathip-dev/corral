# Corral — public-readiness re-review (Fable round 2, delta)

> **Historical note — 2026-08-20:** Any cost-meter content in this historical review/brief describes Corral as it existed and the requirements under review at that time.
> The cost meter was retired by issue #107. For the current design, see `README.md`, `docs/ARCHITECTURE.md`, `docs/OPERATIONS.md`, `docs/DEVELOPING.md`, and `docs/QUICKSTART.md`.

Reviewer: Claude (Fable 5), static review, 2026-08-18. Baseline:
`docs/corral/public-review.md`. Delta files read in full: `README.md`,
`Cargo.toml`, `LICENSE-MIT`, `LICENSE-APACHE`, `scripts/setup-corrald.sh`,
`scripts/corrald-grant.sh`, `scripts/asc-beta-state.rb`, `fastlane/Fastfile`,
`fastlane/.env.example`, `docs/OPERATIONS.md`, `docs/ARCHITECTURE.md`; targeted
reads of `docs/QUICKSTART.md`, `docs/corral/DECISIONS.md`, `src/main.rs`,
`src/cost/mod.rs`, `src/cost/agent_cache.rs`, `src/adapters/herdr.rs`,
`src/fleet/config.rs`.

---

## 1. Round-1 MUST-FIX dispositions

**MF1 — LICENSE: RESOLVED.** `LICENSE-MIT` (copyright line
`Copyright (c) 2026 Jirathip Kunkanjanathorn`, `LICENSE-MIT:3`) and
`LICENSE-APACHE` (full Apache-2.0 text) both exist; `Cargo.toml:15` is
`license = "MIT OR Apache-2.0"`; README has the badge (`README.md:3`) and a
License section (`README.md:188-190`). Exactly as recommended.

**MF2 — launchd label: RESOLVED.** `com.corral.corrald` at
`scripts/setup-corrald.sh:48-49` and `docs/OPERATIONS.md:14`. No remaining
project-specific launchd labels in any file I read.

**MF3 — ASC key-path contradiction: RESOLVED.** `.env.example:6-8` now
documents the actual contract ("resolved relative to fastlane/ by the lane
(basename is taken...)"), matching the resolution at `fastlane/Fastfile:27`,
`Fastfile:163`, and `scripts/asc-beta-state.rb:24`. The chosen story is
"key lives in fastlane/, gitignored via `*.p8`" — internally consistent.
Residual (accepted, documented): an absolute path to a key *outside* the repo
still silently resolves to `fastlane/<basename>`; the doc now says so, so a
stranger no longer hits a contradiction, they hit a documented behavior.

**MF4 — setup-corrald.sh reload + fail-open: RESOLVED.**
`launchctl bootout` before writing/bootstrapping the plist
(`setup-corrald.sh:69`, with the correct rationale in the comment at
`:66-68`); bootstrap failure is now fatal with visible stderr (`:100` — the
`2>/dev/null || true` is gone); the health check retries 10×1s and exits 1 on
failure (`:103-115`); `--bind` errors on a missing value (`:28-30`) and is
regex-validated before hitting the plist heredoc (`:32-36`). All four round-1
issues fixed. (New nits from these fixes: see findings N3–N4.)

**MF5 — loopback story: RESOLVED in docs, STILL-OPEN in the guard message
(deferred by design).** The claim is now stated correctly everywhere a
stranger reads: "Loopback by default, public refused... Private/tailnet
(100.x/10.x) binds are planned via #65" (`README.md:71-73`), the same framing
plus "will still sit behind the full device-signature + grants plane" at
`docs/ARCHITECTURE.md:150-153`; `docs/QUICKSTART.md:31-32` and
`docs/OPERATIONS.md:222-223` say loopback-only, which is *true today* (the
guard at `src/main.rs:574-580` really does refuse routable binds). The stale
message text itself ("P1 corrald has no auth ... until P3", `main.rs:576-577`)
is unchanged — deliberately deferred to land with #65 per the brief. Fine:
the enforced policy is real and correctly documented; only an internal error
string lags. Not a blocker, but do land it with #65 as planned.

## 2. NEW findings (from the fixes and fresh eyes)

**N1 — README front door: "signed remote control from your phone" overclaims
today.** `README.md:7-8` sells phone remote control in sentence two, but a
phone cannot reach a loopback-only daemon — every network bind is blocked
until #65 (the script's own `--bind 100.67.222.5` example says "needs #65",
`setup-corrald.sh:10`), and the APNs reply path is honestly labeled "not
device-verified yet" (`README.md:105`). The security section is honest; the
pitch is ahead of it. One-line fix: qualify the hook, e.g. "signed remote
control from your phone (devices connect over loopback today; tailnet binds
land with #65)". SHOULD-FIX before flip.

**N2 — the herdr link needs verification.** `README.md:18` links herdr to
`https://github.com/dcolinmorgan/herdr`. I cannot verify URLs statically, and
everything else in this repo treats herdr as your own runtime — if that URL
is wrong or 404s, the repo's single most load-bearing external link (defining
the required runtime, in paragraph three) is broken on day one. Verify it
resolves to *your* herdr before flipping public. If herdr is not public yet,
say so instead of linking ("herdr (not yet public)").

**N3 — `--bind` validation accepts values the daemon rejects, and IPv6 breaks
the health check.** `setup-corrald.sh:34` has a hostname branch
(`^[a-zA-Z0-9.-]+$`) and the comment says "or hostname", but `corrald` parses
`--bind` with `bind.parse::<IpAddr>().expect(...)` (`src/main.rs:567-568`) —
a hostname makes the daemon panic and crash-loop under `KeepAlive`. Failure
is loud (health check exits 1 after 10s) but misdiagnosed. Also an IPv6 bind
(`::1` passes the first regex) produces the invalid URL
`http://::1:8474/healthz` at `:105` — curl needs `http://[$BIND]:...`. Drop
the hostname branch and bracket IPv6 in the health-check URL, or document
IPv4-only. Minor.

**N4 — bootout→bootstrap window (the race the brief asked about): acceptable,
one hardening suggestion.** The daemon is down from `:69` (bootout) until
`:100` (bootstrap) — the plist write and `plutil -lint` sit inside that
window, and a lint failure under `set -e` leaves the daemon down. That is the
correct trade: failure is loud (non-zero exit) instead of the old silent
stale-config restart, and the expensive step (`cargo build`, `:61`) runs
*before* the bootout, keeping the window small. One real flake: `launchctl
bootout` can return before teardown completes on some macOS builds, so the
immediate bootstrap occasionally fails spuriously ("Bootstrap failed: 5") —
a single retry-after-1s on bootstrap would absorb it. PLAUSIBLE, minor.

**N5 — corrald-grant.sh revoke path: correct.** Both bodies are built wholly
in python3 with values passed as argv (`corrald-grant.sh:56`, `:59`) — quotes
and unicode in `--key`/`--caps` are inert for grant *and* revoke, and the
emitted shapes match the daemon's verified contract
(`docs/OPERATIONS.md:79`, `:88`). `curl -fsS ... || exit 1` (`:63-66`) makes
HTTP errors fatal. Two accepted trade-offs: `-f` discards the daemon's typed
error body (you get "HTTP error" not "unknown key_id"), and `--key` as the
final argument still dies on `set -u` unbound `$2` (`:23`) — both minor,
pre-existing-class.

**N6 — internal-jargon stragglers round 1 missed** (no secrets, all polish):
`docs/ARCHITECTURE.md:5-7` cites the repository decision record
(`docs/corral/DECISIONS.md`) as "Design authority";
`docs/corral/DECISIONS.md:1-8` opens as an internal mirror addressed to
"Guy/orchestrator" while `README.md:147` sends users there for the D34
writeup; `src/cost/mod.rs:95` says "a provider Guy doesn't use";
`docs/OPERATIONS.md:26` still says `kickstart -k` "reloads" the daemon —
contradicting the (correct) comment at `setup-corrald.sh:66-68` that
kickstart never re-reads a changed plist ("restarts" is the accurate word).

**N7 — README has two `## Status` sections** (`README.md:125`, `:180`). The
`[Status](#status)` anchor at `:105` resolves to the first (phase progress),
not the pre-1.0/macOS-first statement the link intends. Merge them. Trivial.

Fastlane delta is clean: `TEAM_ID` env-overridable (`Fastfile:15`),
"sendmeter" gone (`Fastfile:2`), legal name replaced with `<NAME>`
(`Fastfile:107`), `APP_ID` overridable via `CORRAL_ASC_APP_ID`
(`asc-beta-state.rb:36-38`).

## 3. Q8/Q9 + ARCHITECTURE.md "Stack terminology" accuracy

**Accurate, one wording nuance.** The four-layer table
(`ARCHITECTURE.md:13-18`) is correct: harness examples are the actual
supported three; the runtime row correctly names the adapter as *the*
coupling point; "configured, not coupled" for models is right. Verified
claims: `source: "herdr"` / `tool: tool.unwrap_or("unknown")` pass-through at
`src/adapters/herdr.rs:688-689` (harness-agnostic core confirmed — though
"normalizes ... uniformly via `apply_agent_info`" slightly overstates it; the
tool label is passed through *verbatim*, which is the stronger form of
agnosticism — no per-harness logic exists to normalize with). The cost-meter
paragraph (`:27-33`) is fully accurate: `CORRAL_OPENCODE_DB` /
`CORRAL_CLAUDE_DIR` / `CORRAL_CODEX_DIR` exist exactly as named
(`src/cost/mod.rs:100-119`), and `store_found: false` is real
(`mod.rs:94-96`).

**Q9 confirmed: the cost meter does not touch fleets.json.** Attribution
keys on `format!("{tool}:{worktree_path}")`
(`src/cost/agent_cache.rs:41-43`), accumulated from each provider store's own
`workspace_path` (`:88-94`), joined to agents in the adapter
(`herdr.rs:681-685`). `fleets.json` appears only in `src/fleet/`, and its
default path is still `~/.hermes/scripts/fleets.json`
(`src/fleet/config.rs:181-188`) — correctly deferred to #66, and
`docs/OPERATIONS.md:212-213` documents the env override, so a stranger is not
blocked, just pointed at a legacy default.

## 4. Verdict

**PUBLIC-READY** — with one pre-flip check and one recommended one-line edit,
neither of which stops a stranger from cloning, building, and using it. All
five round-1 must-fixes are genuinely resolved in the files (not just
claimed): dual license in place, personal label gone, the ASC key story
internally consistent, the setup script now correct-on-rerun and loud on
failure, and the loopback security claim true-as-written everywhere a user
reads it (the stale guard *message* rides with #65, which is fine because the
enforced policy is real). The new findings are polish: verify the
`dcolinmorgan/herdr` URL actually points at your herdr before flipping (N2 —
the only finding I'd hold the button for, since I can't check it statically),
and soften "remote control from your phone" until #65 lands (N1). Everything
else — hostname/IPv6 bind edge, bootout flake retry, jargon stragglers,
duplicate Status heading — is post-flip cleanup. Flip it.
