# Corral — public-readiness + scripts adversarial review (Fable)

> **Historical note — 2026-08-20:** Any cost-meter content in this historical review/brief describes Corral as it existed and the requirements under review at that time.
> The cost meter was retired by issue #107. For the current design, see `README.md`, `docs/ARCHITECTURE.md`, `docs/OPERATIONS.md`, `docs/DEVELOPING.md`, and `docs/QUICKSTART.md`.

Reviewer: run headless, reason statically, READ the actual files, cite exact
lines. Do NOT run shell commands or read files with tools — reason STATICALLY
from the file contents (the sandbox denies tool use).

## Context

Corral (`owner/repo`) is an agent-fleet control plane daemon
(`corrald`, Rust) with a desktop egui client, an iOS FleetNotifier app, and a
planned public release. This branch does not claim physical-device or
TestFlight verification. The repo wants to
become PUBLIC on GitHub. Current state on `main`:

- `Cargo.toml` declares `license = "MIT"` but **there is no LICENSE file**.
- README.md is thorough but has a "Loopback only — refuses to bind any
  routable interface" security claim that will be FALSE once #65 lands
  (bind-relax to allow Tailscale/private IPs — the guard's P3 reason is
  stale; P3 auth IS in).
- Three new shell scripts + one Ruby script just landed in `scripts/`
  (commit 500b3cc + f689d53):
  - `scripts/setup-corrald.sh` — build + launchd agent install + health check
  - `scripts/corrald-grant.sh` — list/grant/revoke device capabilities
  - `scripts/asc-beta-state.rb` — TestFlight beta state diagnostic
- `fastlane/` has the TestFlight lane (`Fastfile`, `.env.example`).

## Files to review (read each)

1. `scripts/setup-corrald.sh`
2. `scripts/corrald-grant.sh`
3. `scripts/asc-beta-state.rb`
4. `README.md`
5. `Cargo.toml` (license field + workspace)
6. `docs/QUICKSTART.md` + `docs/OPERATIONS.md` (the new "One-shot setup" section)
7. `fastlane/Fastfile` (for the public story: does it leak team IDs / is it
   reproducible by a stranger?)
8. `ios/project.yml` (bundled identifiers, team ID — public-readiness)

## Specific questions (answer all, numbered)

1. **LICENSE**: What license fits a Rust agent-orchestration daemon that
   shells out to claude/codex/opencode and drives a user's local agent
   fleet? Is MIT right, or is there a better default (Apache-2.0, dual)?
   What LICENSE file content should be added (full MIT text, copyright
   line format)? Any README badge worth adding?
2. **setup-corrald.sh correctness**: any shell bugs, TOCTOU, insecure
   heredoc expansion (the plist template interpolates `$BIN`/`$HOME`/`$BIND`
   — any injection or quoting hole)? Is the `launchctl bootstrap` fallback
   logic correct (it swallows errors)? Does the health check fail open?
   Does it handle `--bind` properly?
3. **corrald-grant.sh correctness**: does the JSON body construction via
   python3 subprocess handle quoting safely? Is the admin token read safe?
   Any capability-injection risk (a malicious `--caps` value)?
4. **asc-beta-state.rb**: is the ASC token handling sound (reads
   fastlane/.env, no secrets logged)? Any Ruby issues?
5. **README public-readiness**: what's MISSING for a stranger cloning this?
   (Badges? Screenshots? Architecture diagram? "What problem does it
   solve?" hook? Install via brew? Docs link structure?) Is the "Loopback
   only" claim now misleading? What about the Cost-meter placeholder
   honesty — keep or hide? Anything that would embarrass a public release
   (personal paths, Thai/English mix, herdr-specific jargon)?
6. **Repo hygiene for public**: .gitignore correctness (fastlane/.env,
   *.p8, build/, target/)? Any committed secrets or personal identifiers in
   fastlane/ or docs? Should the ios/ team ID (9244PWFYD7) be a variable?
   Is the TestFlight/ASC story (fastlane/.env.example) reproducible by a
   stranger without the key?
7. **Overall verdict**: is this repo PUBLIC-READY today? If not, what's the
   TOP 5 must-fix list (numbered, concrete, with file:line refs)? What's
   the top 5 should-fix?
8. **Runtime-agnosticism (Guy's corrected framing)**: Precise terminology —
   the stack is model → harness → runtime → control plane:
   - **Harness** = the wrapper giving a model tools: Claude Code, Codex CLI,
     OpenCode. Corral ALREADY treats these as interchangeable (the adapter
     normalizes claude/codex/opencode agent kinds uniformly via
     `apply_agent_info`; core model has a `tool` label, no harness logic).
   - **Runtime** = the layer that spawns/supervises harnesses: herdr.
   - **Control plane** = corral itself.
   Given this framing: is corral HARNESS-agnostic already (yes?) and
   RUNTIME-bound to herdr at the adapter? Where is the herdr-runtime coupling
   exactly (adapter? registry? worktree conventions?)? What would a
   "second-runtime / no-runtime" mode look like (e.g. derive agents from git
   worktrees + a mapping file, or read another runtime's socket)? Is
   runtime-agnosticism a blocker for public release or a documented
   limitation? Rate effort (S/M/L) and recommend whether it belongs in #35
   or a separate issue.
9. **Fleet registry + cost attribution**: the agent→worktree association
   for per-agent cost uses `fleets.json` (herdr convention). Is a
   `FleetRegistry` trait (herdr impl + generic impl) the right decoupling
   seam? What's the minimal change to make the cost meter fully
   runtime-agnostic without breaking the herdr path?

## Output

Write your review to `docs/corral/public-review.md`:
- Numbered answers to Q1–Q7.
- A "MUST-FIX (blocking)" numbered list with file:line citations.
- A "SHOULD-FIX" numbered list.
- A one-paragraph overall verdict: PUBLIC-READY or NOT with the reason.
Aim for 1200-2500 words. Be concrete and opinionated — this decides a real
release. Cite exact lines from the files you read.
