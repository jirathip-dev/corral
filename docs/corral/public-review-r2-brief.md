# Corral — delta re-review after MUST-FIX round (Fable round 2)

You reviewed this repo before. Your round-1 review is at
`docs/corral/public-review.md` (baseline). The fixes landed in commits
`1c9bda6`, `2f30383`, `6de5eb6`. Re-review the DELTA: verify each of your
round-1 findings against the current files, and hunt for NEW holes the fixes
may have introduced.

## What changed since your round 1

1. **LICENSE**: `LICENSE-MIT` + `LICENSE-APACHE` added; `Cargo.toml` →
   `license = "MIT OR Apache-2.0"`.
2. **launchd label**: the old project-specific label → `com.corral.corrald`
   (`scripts/setup-corrald.sh`, `docs/OPERATIONS.md`).
3. **setup-corrald.sh**: bootout-before-bootstrap (re-run applies new
   --bind), bootstrap failure fatal, health check retries 10x + `exit 1` on
   failure, `--bind` validated (regex, missing value errors).
4. **corrald-grant.sh**: whole JSON body built in python3 (safe for quoted
   --key/--caps), `curl -f` (HTTP errors fail).
5. **README**: problem-hook front door, herdr link + runtime note, license
   badge, Status (pre-1.0 macOS-first), License section, P1–P4 briefs marked
   historical.
6. **fastlane/Fastfile**: TEAM_ID env-overridable (`ASC_TEAM_ID`); legal-name
   + "sendmeter pattern" de-jargoned. `fastlane/.env.example` ASC_KEY_PATH
   contract fixed (resolved relative to fastlane/, basename).
7. **asc-beta-state.rb**: APP_ID env-overridable (`CORRAL_ASC_APP_ID`).
8. **docs/OPERATIONS.md**: "automation gateway" wording;
   launchd label updated.
9. **NOT changed (deliberately — orch-territory, filed as issues)**: the
   stale loopback guard message at `src/main.rs:571-579` (lands with #65);
   the fleets.json default path (filed #66); the `ios/project.yml` hardcoded
   team ID (reproducibility, not a leak — TEAM_ID env-override in the
   Fastfile covers the lane).

## What to verify (numbered)

1. **All 5 round-1 MUST-FIXes**: confirm each is actually resolved in the
   current files (cite the fixed lines). Any that are only partially fixed?
2. **NEW holes from the fixes**: did bootout-before-bootstrap introduce a
   race (bootout kills a running daemon, then bootstrap fails → daemon
   down)? Is the health-check retry loop + `exit 1` correct? Did the
   python3-body change in corrald-grant.sh break the revoke path (quote
   handling)? Does the README front door overclaim (e.g. "remote control
   from your phone" — but the phone is blocked until #65)?
3. **Q8/Q9 confirmation**: corral is harness-agnostic (generic tool label),
   runtime-bound to herdr at the adapter; cost meter does NOT use
   fleets.json (keys on tool:worktree_path from provider stores). Confirm
   the ARCHITECTURE.md "Stack terminology" section I added (commit d5633d4)
   is accurate — any errors in the model→harness→runtime→control-plane
   table or the cost-meter claims?
4. **Remaining blockers**: is the repo PUBLIC-READY now, or is there
   anything left that blocks flipping it public TODAY? Be precise: only
   things that would actually stop a stranger from cloning, building, and
   using it.

## Output

Write to `docs/corral/public-review-r2.md`:
- Per-finding disposition: RESOLVED / PARTIAL / STILL-OPEN (with file:line).
- NEW findings (if any), numbered, with file:line.
- Q8/Q9 + ARCHITECTURE.md accuracy check.
- Final verdict: PUBLIC-READY / NOT with the reason, one paragraph.

Reason statically from file contents — do NOT run shell commands or use
file-read tools (sandbox denies them). Do not stop until the file exists.
Aim for 800-1500 words.
