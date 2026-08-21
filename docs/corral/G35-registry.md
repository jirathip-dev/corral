# Corral fleet registry — corrald registry surface (#35)

Issue #35 (fleet control-plane consolidation): corrald reads the fleet
registry — `fleets.json` — a format corral is taking ownership of from the
legacy fleet tooling (which still writes the legacy-path file, hence the
migration fallback below) — and, as of slices 1–2, also WRITES it. `list` /
`check` are read-only views; **`add` / `remove`** (slice 1) and **`pause` /
`resume` / `models`** (slice 2) mutate the registry behind candidate
validation and an atomic temp-file+rename write that leaves the original
byte-identical on any refusal or failure. Nothing touches a running agent —
pause/resume here are pure registry mutations; the auth-gated ops half
(halting working agents, the model-switch re-arm) is a later #35 slice.
Spawning, watchdogs, reaping and worktree pruning land in later phases of
#35 too.

## Registry schema

Default path: `$CORRAL_FLEETS_PATH`, else the corral-owned
`$HOME/.config/corral/fleets.json`; a pre-existing legacy fleet registry is
honoured as a migration fallback when — and only when — the corral-owned file
does not exist (#66); the
corral-owned dir honours `$CORRAL_CONFIG_DIR` like every other consumer
of the config dir. Corral is taking ownership of the schema: it
originated in the legacy fleet tooling, which still writes the
legacy-path file today — that is exactly why the fallback (and the loud
stderr note when it is taken) exists.
(any command accepts `--registry <path>` to override).

```json
{
  "fleets": [
    {
      "name": "corral",
      "gh_repo": "owner/repo",
      "local": "~/Projects/<repo>",
      "worktree_dir": "corral",
      "orch": "orch-corral",
      "workers": ["p4-w1", "p4-w1-reviewer"],
      "paused": true,
      "models": { "orch": "fable", "impl": "opencode-go/deepseek-v4-flash", "review": "opus",
                  "impl_alt": "opencode-go/deepseek-v4-flash", "impl_alt2": "dsh" }
    }
  ]
}
```

- `name`, `gh_repo`, `local`, `worktree_dir`, `orch` — required, non-empty strings.
- `workers` — required array of strings; may be empty.
- `models` — required object with required string keys `orch`, `impl`, `review`
  and optional string keys `impl_alt` (fallback implementer) and `impl_alt2`
  (last-resort backend). Absent alt keys stay absent through any rewrite (an explicit `null` is
  read as absent and is not written back); a present key must be non-empty
  and whitespace-free, like the other model slots.
- All `models.*` slots (required and alt) must be whitespace-free — the
  required three feed the whitespace-delimited `fleet list` line; the alt
  slots follow the same rule for consistency.
- `paused` — optional bool, defaults to `false` when absent. Set/cleared
  by `fleet pause`/`resume` (slice 2). The skip rule: `false` is never
  serialized — a resumed fleet omits the key entirely (this is the rule
  the pause/resume section refers to).
- `local` may start with `~/` — expanded against `$HOME`.
- Unknown fields anywhere (top level, fleet, models) → hard error, not silent
  acceptance. Duplicate fleet names → hard error.

## Commands

```
corrald fleet list [--registry <path>]
corrald fleet check [--registry <path>]
corrald fleet watch [--registry <path>]
```

`list` — one greppable line per fleet:

```
repo owner/repo workers=2 paused=true orch=orchestrator impl=implementer review=reviewer
```

`check` — parse + validate, then verify each fleet's `local_path()` exists,
is a directory, and holds a `.git` entry (the "repo resolves" validation):

```
ok corral
FAIL notadir: /etc/hosts is not a directory
FAIL nogit: /tmp has no .git entry
FAIL gone: cannot stat /path/to/nowhere: No such file or directory (os error 2)
```

Lines follow registry order, one per fleet.

`.git` may be a directory (ordinary clone) or a regular file (linked
worktree) — both count as resolved.

Both `ok` and `FAIL` lines go to **stdout**, deliberately, so `check` output
stays one greppable stream; the exit code is what a script should branch on.

`watch` (the #35 watchdog parity item) is READ-ONLY: one health pass over
the UNPAUSED fleets — herdr server reachability (one retry, so a
transient socket hiccup never reads as every agent missing), missing
orchestrators, three stall flavors in priority order (open PRs → fleet
workers still working → plain, naming the status; a failed gh check is
stated as unavailable, never treated as zero), and missing workers.
Paused fleets are skipped entirely — pausing genuinely silences the
watchdog. Output is sorted `PROBLEM:` lines or `ALL HEALTHY`; exit 0
healthy / 1 problems / 2 usage error. A corrupt/unreadable registry is
itself a `PROBLEM:` line on stdout with exit 1 (monitor safety: the
watchdog alerts on the failure that stops it watching). Fleet-worker
attribution matches by the fleet's own `workers[]` names or by
component-exact cwd anchored at the fleet's `local` (usable only when at
least two path components deep, below `$HOME` or outside it — legacy
rejected out-of-home locals entirely; we accept them under the same
depth rule) and `$HOME/.herdr/worktrees/<worktree_dir>`
— another fleet's `corral-x` worktree can never count for `corral`.

Declared deviations from the legacy `fleet-watch.py` (beyond the checks
themselves): one `PROBLEM:` line per problem instead of one joined line;
worker lines carry the fleet prefix; the server-down text names the
failed listing call rather than a launchd label; exit 1 when problems
exist (legacy always exited 0 — cron wrappers treating non-zero as
script failure need `|| true`); a successful zero-agent listing is
healthy, never server-down; worker names count per-fleet, not globally;
the orca-era workspaces leg is dropped; gh unavailability is stated on
every stalled flavor (informational, at the cost of an extra output flap
when the network blips). Legacy CHECKS not ported in this slice (fresh
review N2/N4 — both were added to legacy v3 after production misses, and
both remain gaps until a later slice or the legacy watcher's retirement):
stall ESCALATION (legacy re-notifies at 30m/2h/6h/24h — four buckets,
capped at 4 re-fires per stall — while corrald's dedup-friendly stable
output notifies once) and
UNSHIPPED-WORK detection (local commits with no remote branch/PR read as
a plain stall here, not as the "work never left the machine" alarm).

## Write commands (slice 1: add/remove)

```
corrald fleet add <name> --gh <owner/repo> [--local <path>] [--worktree <path>]
    [--orch <agent>] [--workers a,b,c] [--models orch=..,impl=..,review=..]
    [--registry <path>]
corrald fleet remove <name> [--registry <path>]
```

`<name>` may also be passed as `--name`, and `--worktree-dir` is an alias
for `--worktree` — both spellings match the legacy fleet CLI. Defaults:
`local` → `~/Projects/<name>`, `worktree_dir` → `<name>`, `orch` →
`orch-<name>`, `workers` → empty, `models` inherited from the **first**
fleet in the registry — `impl_alt`/`impl_alt2` included (an empty registry
requires `--models`). The alt slots are not settable from `--models`; they
inherit, or are edited in the registry directly. The repo
must resolve via `gh repo view <owner/repo>` before anything is written;
the candidate registry is validated with the same rules `load()` applies;
the write is a PID-suffixed temp file in the registry's directory,
fsynced, then renamed over the (symlink-resolved) target. The registry
file must already exist, and on a fresh machine its parent dir may not —
bootstrap with `mkdir -p ~/.config/corral && echo '{"fleets": []}' >
<path>`. There is no cross-process lock: concurrent writers (corrald or
the legacy tooling) can lose the load→rename race; single-writer
discipline is assumed during the migration window.

## Write commands (slice 2: pause/resume/models)

```
corrald fleet pause <name> [--registry <path>]
corrald fleet resume <name> [--registry <path>]
corrald fleet models <name> [--orch M] [--impl M] [--impl-alt M]
    [--impl-alt2 M] [--review M] [--registry <path>]
```

`pause` sets `"paused": true` on exactly one fleet; `resume` clears it.
Both are **idempotent**: pausing an already-paused fleet (or resuming an
unpaused one) is a no-op SUCCESS — nothing is written, exit 0, and the
message says so ("already paused"). Per the schema's skip rule, a resumed
fleet omits `paused` entirely (false is never written).

`models` updates **only the flags given**, leaving every other slot —
including the optional alt slots — untouched. At least one flag is
required (usage error, exit 2, otherwise). Empty values for the required
`orch`/`impl`/`review` slots are a usage error (exit 2); empty values for
the optional slots CLEAR them: `--impl-alt ''` removes `impl_alt` from the
written JSON, `--impl-alt2 ''` removes `impl_alt2`. `<name>` may be `all`
— the update applies to every fleet (legacy semantics; `models` only —
`pause`/`resume` take a real fleet name). `all` is therefore a RESERVED
fleet name: `validate()` refuses a registry containing it, so the wildcard
can never shadow a real fleet. `models all` against a registry with no
fleets is a refusal (exit 1), not a silent success. `models` is idempotent
like pause/resume: when every value already matches, nothing is written and
the command says so. Each affected fleet's change is printed on success:

```
board models changed: orch fable -> fable; impl sonnet -> gpt-5.6-luna; impl_alt - -> -; impl_alt2 - -> -; review opus -> opus
```

All three refuse an unknown fleet name (exit 1, `FleetNotFound`) writing
nothing, validate the candidate registry before writing, and leave the
file byte-identical on any refusal, no-op, or write failure.

## Exit codes

| Code | Meaning |
|------|---------|
| 0    | success — `list` printed; `check` found every fleet OK; `add`/`remove`/`pause`/`resume`/`models` wrote the registry (or were an idempotent no-op) |
| 1    | `check`: at least one fleet failed verification; write commands: refused (duplicate name, unresolvable repo, unknown name, no models to inherit) or the write failed — the registry is left byte-identical; `watch`: problems found — INCLUDING an unreadable/invalid registry, which `watch` reports as a `PROBLEM:` line with exit 1 (monitor safety) |
| 2    | usage error; for every subcommand EXCEPT `watch`: also an unreadable/unparseable registry or validation failure |

## Phase boundary (explicitly out of scope)

- No `fleet switch`, and no ops half of pause/resume (halting working
  agents, auth-gated model-switch re-arm) — later phases.
- No status/reap/prune consolidation — later phases (`watch` shipped
  with this slice; see the watch section above).
- No change to the running agent, session, or `src/drive/` contract.
