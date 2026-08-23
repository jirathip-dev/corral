# Corral fleet registry — corrald registry surface (#35)

Issue #35 (fleet control-plane consolidation): corrald reads the fleet
registry — `fleets.json` — a format corral is taking ownership of from the
legacy fleet tooling (which still writes the legacy-path file, hence the
migration fallback below) — and also WRITES it. `list`, `check`, and
`watch` are read-only views; **`add` / `remove`** and **`pause` / `resume` /
`models`** mutate the registry behind candidate validation and an atomic
temp-file+rename write that leaves the original byte-identical on any
refusal or failure. Registry mutation never touches a running agent.
The destructive ops half — `switch`, `reap`, and `prune` — is implemented
beside it: those commands can halt a verified agent pane or remove a
provably-dead worktree, but they never rewrite the registry themselves.

Corral is a **tolerant subset reader**: fleet-operations owns the full
schema (model roles, `reasoning_effort`, top-level `admit`, and future
additions), while corral consumes only the identity fields it needs. Unknown
keys at the registry, fleet, or `models` level are accepted and retained
across a corral rewrite, so a fleet-ops schema addition cannot empty
`GET /issues` or silently disappear through `fleet models`. This tolerance
does not weaken validation of fields Corral owns: an unknown key one edit
away from `paused`, `impl_alt`, or another owned name is refused loudly
rather than silently defaulting (`pausd`, `imp1_alt`, `puased`,
`imlp_alt`), while genuinely farther foreign keys stay accepted. One edit
includes substitution, insertion, deletion, and adjacent transposition.

## Path resolution

The registry name is always `fleets.json`; the file itself is resolved by
`corrald` in this order:

1. `$CORRAL_FLEETS_PATH` when set. This env value always wins, including when
   the file does not exist, so a test or one-off override is unambiguous.
2. The corral-owned config file `$CORRAL_CONFIG_DIR/fleets.json`, where
   `$CORRAL_CONFIG_DIR` defaults to `$HOME/.config/corral`
   (`~/.config/corral/fleets.json`). The relocated config dir is honoured like
   every other Corral consumer.
3. The legacy `~/.hermes/scripts/fleets.json` fallback (#66) — used only when
   the corral-owned file does **not** exist. When it is taken, `corrald` prints
   a loud stderr note asking the operator to migrate it.

If no file exists, `corrald` still targets the corral-owned path and reports a
normal missing-file error; it never silently creates an empty registry or
switches away from a configured fallback. Every `corrald fleet` command
additionally accepts `--registry <path>` to bypass the ladder for that
invocation.

The fleet-operations side resolves the same canonical
`~/.config/corral/fleets.json` through its own resolver, with `FLEETS_JSON` /
`HERMES_FLEETS_JSON` test overrides; its legacy file is never used by default.
Both sides deliberately point at one live file, so a Corral `--registry`
override affects only that Corral CLI invocation, not `herdr-fleet`, the
controllers, or the daemon.

## Registry schema

The checked-in `fleets.example.json` at the Corral repo root is the complete
seed template below. It uses placeholder repo/path/agent values and contains no
secrets; replace them before using it as more than a parse check.

```json
{
  "fleets": [
    {
      "name": "example",
      "gh_repo": "example/example",
      "local": "~/Projects/example",
      "worktree_dir": "example",
      "orch": "orch-example",
      "workers": [],
      "paused": false,
      "models": {
        "orch": "codex/deepseek-v4-flash-vision-exp",
        "impl": "codex/deepseek-v4-flash-vision-exp",
        "review": "codex/deepseek-v4-flash-vision-exp",
        "impl_alt": "opencode-go/deepseek-v4-flash",
        "impl_alt2": "codex/deepseek-v4-flash",
        "reasoning_effort": {
          "orch": "medium",
          "impl": "max",
          "review": "high"
        }
      }
    }
  ],
  "admit": {
    "enabled": true,
    "budget_mb": 16000,
    "working_charge_mb": 1000,
    "idle_charge_mb": 150,
    "sim_max": 1,
    "min_spawn_gap_sec": 45,
    "burst": 3,
    "pressure_refuse_at": "warn",
    "swapout_rate_refuse": 2000,
    "load_per_cpu_critical": 2.5
  }
}
```

The top level is a `fleets` array plus the optional fleet-operations-owned
`admit` object. `fleets` may be empty for a bootstrap registry, but a missing
or non-array value fails both parsers.

### Per-fleet fields

- `name` — required, non-empty string. Corral-owned; duplicate names are a hard
  error. `all` is reserved by the `fleet models` wildcard and cannot be used.
- `gh_repo` — required string in `owner/repo` form (Corral-owned), no internal
  whitespace. This is the GitHub identity used for attribution and checks.
- `local` — required string path to the primary checkout; may start with `~/`,
  expanded against `$HOME`. `fleet check` verifies it exists with a `.git`
  entry.
- `worktree_dir` — required string naming the fleet's worktree root component
  under the worktrees root; Corral validates it as a single path component.
- `orch` — required string naming the registered orchestrator agent.
- `workers` — required array of strings; may be empty. Each entry is
  non-empty.
- `paused` — optional bool, defaults to `false`. `fleet pause`/`resume` set and
  clear it. The skip rule is fixed: `false` is never serialized, so a resumed
  fleet omits the key rather than writing `"paused": false`.
- `models` — required object; see below.

### Per-fleet `models`

- Required slots: `orch`, `impl`, `review` — each a non-empty, whitespace-free
  model id string. The required three feed the whitespace-delimited
  `corrald fleet list` line.
- Optional `impl_alt` (fallback implementer) and `impl_alt2` (last-resort
  backend), same non-empty, whitespace-free rules. Absent alt keys stay absent
  through a Corral rewrite; an explicit `null` is read as absent and not
  written back.
- `models.reasoning_effort` — optional runtime object keyed by the same three
  roles: `orch`, `impl`, `review`. Fleet-operations validates each present
  value against `none`, `minimal`, `low`, `medium`, `high`, `xhigh`, `max`,
  or `ultra`, requires it for a `codex/...` role, and rejects it for a bare
  Claude or opencode-backed role. Corral preserves the object verbatim and
  does not model its contents as typed fields.

### Top-level `admit`

`admit` is fleet-operations-owned admission-control configuration. Corral
preserves the whole object across a rewrite without interpreting it. The
complete key set used by the shared loader:

| Key | Type | Meaning |
|-----|------|---------|
| `enabled` | bool | Master admission switch; explicit `false` blocks new spawns at the shared load/swap gate. |
| `budget_mb` | number | Peak-MB weighted budget: working agents × `working_charge_mb`, idle agents × `idle_charge_mb`, plus the new spawn's weight must be less than this; the loader refuses once the projection reaches it. |
| `working_charge_mb` | number | Charge applied per live working agent. |
| `idle_charge_mb` | number | Flat charge applied per idle agent. |
| `sim_max` | integer | Hard semaphore limiting concurrent simulator users. |
| `min_spawn_gap_sec` | number | Minimum spacing in the spawn pacing token bucket. |
| `burst` | integer | Number of tokens/spawns allowed inside the pacing window. |
| `pressure_refuse_at` | string | Memory-pressure level at which a spawn is refused; the live loader uses `warn` or `critical`. |
| `swapout_rate_refuse` | number | Swap-out rate threshold in pages per 2-second sample; above it a spawn is refused. |
| `load_per_cpu_critical` | number | Maximum 1-minute load average per CPU before a spawn is refused. |

Omitted `admit` keys fall back to the fleet-operations `load_admit.py`
defaults (`budget_mb` 9000, `working_charge_mb` 1500, `idle_charge_mb` 300,
`sim_max` 1, `min_spawn_gap_sec` 45, `burst` 3, `pressure_refuse_at` `warn`,
`swapout_rate_refuse` 2000, `load_per_cpu_critical` 2.5). The checked-in
example deliberately sets a tighter working charge and a different budget; an
`allow` override or other forward key is preserved by Corral even though it is
not modeled here.

### Forward compatibility

Unknown fields anywhere — top level, fleet, or `models` — are retained through
a Corral rewrite, so a fleet-operations schema addition cannot empty
`GET /issues` or silently disappear through `fleet models`. `admit` and
`reasoning_effort` are explicitly recognized, not lumped in the unknown map.
The typo guard remains: an unknown key one edit away from a Corral-owned field
(including adjacent transposition, e.g. `pausd`, `imp1_alt`, `puased`,
`imlp_alt`) is refused loudly rather than silently defaulted. Genuinely
forward-compatible foreign keys stay accepted.

## Setup and handoff

Fresh-machine setup from the Corral checkout:

```sh
REPO_ROOT="$(git rev-parse --show-toplevel)"   # the Corral checkout
CONFIG_DIR="${CORRAL_CONFIG_DIR:-$HOME/.config/corral}"
mkdir -p "$CONFIG_DIR"
cp -n "$REPO_ROOT/fleets.example.json" "$CONFIG_DIR/fleets.json"
corrald fleet list
corrald fleet check   # placeholders must be replaced before this passes
```

The seed is a schema template, not a runnable live entry: replace
`example/example`, `~/Projects/example`, `orch-example`, and the model ids
with the host's real fleet before relying on `check`, `watch`, or the control
plane. `corrald fleet list --registry "$REPO_ROOT/fleets.example.json"` is a
useful parse-only check that never touches the live file. `cp -n` never
overwrites an existing canonical registry; use `herdr-fleet doctor --seed
--force` only after reviewing the backup behavior.

On a machine with the legacy `~/.hermes/scripts/fleets.json` but no corral-owned
file, `corrald fleet list` finds and reads it automatically and prints the
migration note (#66). Migrate deliberately rather than letting the fallback stay
ambiguous: `herdr-fleet doctor` reports canonical/legacy/seed state, then
`herdr-fleet doctor --migrate` copies the legacy file to the canonical path
(`--force` first backs up a differing canonical file). The legacy file is never
deleted. Use `mv`, not `cp`, if migrating that file manually: a copy would leave
legacy tooling writing one path while Corral reads the other.

After setup, all of these consume the same file:

- `corrald fleet list/check/watch` read it; `add/remove/pause/resume/models`
  rewrite it atomically. All accept `--registry <path>` for an invocation-level
  override.
- `herdr-fleet list/doctor/check/models/...` and the fleet-operations
  controllers use the shared resolver's canonical path
  (`~/.config/corral/fleets.json`; `FLEETS_JSON` / `HERMES_FLEETS_JSON` for
  tests/instrumentation). `herdr-fleet doctor --seed` seeds from the
  fleet-operations template; this checked-in template is kept in schema
  lockstep with it.
- The `corrald` daemon's `GET /issues` loads the same default path, inserts a
  key for every fleet name, and workspace attribution uses each fleet's
  `gh_repo`/`local` identity. The daemon has no `--registry` flag, so a CLI-only
  override cannot silently change what it attributes.

The canonical path may also be overridden for Corral with
`$CORRAL_FLEETS_PATH` or `$CORRAL_CONFIG_DIR`; the checked-in template itself is
never read as the live registry just because it exists in the source tree.

## HTTP read view (#135)

The daemon serves a read-only `GET /fleet-registry` projection of the same
file consumed by `GET /issues` and workspace attribution. It is a deliberate
non-auth loopback/private-tailnet surface, like the other read GETs: it never
mutates the registry or GitHub and must not be exposed on a public interface.
Success returns HTTP 200:

```json
{
  "status": "ok",
  "path": "/absolute/path/to/fleets.json",
  "error": null,
  "fleets": [{ "name": "corral", "gh_repo": "owner/repo", "local": "…", "worktree_dir": "…", "orch": "…", "workers": [], "paused": false, "models": { "orch": "…", "impl": "…", "review": "…", "impl_alt": null, "impl_alt2": null, "reasoning_effort": {} } }]
}
```

An IO, parse, or validation failure is still HTTP 200 with `status="error"`,
a human-readable `error`, and `fleets=[]`, so the desktop Registry tab shows
the failure rather than silently appearing to have no fleets. The board
fetches this view alongside `/issues` and exposes a manual refresh; the
optional pause/resume/models drive path is explicitly out of scope for #135.

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

## Registry write commands (add/remove)

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
requires `--models`). `models.reasoning_effort` is inherited with the rest of
the model object. The alt slots and `reasoning_effort` are not settable from
Corral's `--models`; they inherit, or are edited in the registry directly. The repo
must resolve via `gh repo view <owner/repo>` before anything is written;
the candidate registry is validated with the same rules `load()` applies;
the write is a PID-suffixed temp file in the registry's directory,
fsynced, then renamed over the (symlink-resolved) target. The registry
file must already exist, and on a fresh machine its parent dir may not —
bootstrap with `mkdir -p ~/.config/corral && echo '{"fleets": []}' >
<path>`. There is no cross-process lock: concurrent writers (corrald or
the legacy tooling) can lose the load→rename race; single-writer
discipline is assumed during the migration window.

## Registry write commands (pause/resume/models)

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
including the optional alt slots and `models.reasoning_effort` — untouched. At least one flag is
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
file byte-identical on any refusal, no-op, or write failure. `admit` and
`models.reasoning_effort` survive every one of these rewrites; use
`herdr-fleet models --orch-effort/--impl-effort/--review-effort` or edit the
file directly to change reasoning efforts.

## Fleet operations (switch/reap/prune)

### Auth-gated model switch

```
corrald fleet switch <name> [--pane <id>] [--registry <path>]
```

`switch` is the re-arm half of pause/resume. The model map is the source of
the new command line, so it:

1. validates every role's model id against the harness mapping — unqualified
   ids imply `claude`; `codex/<model>` implies `codex`; the known opencode
   provider prefixes (`commandcode`, `deepseek`, `openai`, `opencode`,
   `opencode-go`) imply `opencode`; any other `provider/model` is refused
   before anything is touched;
2. checks authentication for **every** implied harness — `claude auth
   status` (`loggedIn: true`), `codex login status`, or `opencode auth list`
   — and refuses before killing the incumbent if any check is false or
   unavailable;
3. kills only the registered `orch` incumbent, and only after the reaper's
   verified-pane identity check (argv0 allowlist, exclusion of the pane
   shell, sane pgid, exact cwd match); it never auto-discovers and kills an
   unregistered orchestrator;
4. sends `TERM` to that verified process group, then only sends `KILL` after
   checking that one of the verified pids still belongs to the same group;
5. resolves the destination pane from `--pane`, or from a single herdr pane
   whose cwd equals the fleet's `local` (ambiguous/missing is a refusal),
   and starts `fleet.orch` on `models.orch` through `herdr agent start`;
6. leaves the registry and its `paused` flag untouched — after either a
   failure or a success the fleet stays paused until an explicit
   `fleet resume`.

### Reaper

```
corrald fleet reap <fleet|all> [--apply] [--max-done N]
    [--max-fraction F] [--registry <path>]
```

`reap` is dry-run by default; `--apply` is the only mode that signals
anything. It targets `done`/`completed` agents plus `idle` agents whose cwd
belongs to a `paused: true` fleet; the canonical paused orchestrator is not
treated as an idle victim. A `done` agent whose pane still contains a live
claude/codex process is a resumable session and is skipped.

Every victim passes the same process-identity checks as `switch` before
TERM, and every agent/pane is re-fetched and re-validated immediately before
signalling. `TERM` is followed by a recheck; `KILL` is sent only when a
previously verified pid still belongs to the original process group. An
unreadable/unavailable herdr agent listing refuses the whole run.

The shrink guard counts all finished agents — including `done` agents whose
pane id is gone or uninspectable, so a broken pane cannot bypass it — and
refuses before any kill when the count exceeds `--max-done` (default 5) or
`--max-fraction` of the fleet (default 0.25, fraction floor at 2).

### Worktree pruning

```
corrald fleet prune [--apply|--yes] [--max-prune N] [--min-age DAYS]
    [--worktrees <path>] [--registry <path>]
```

`prune` is dry-run by default; `--apply`/`--yes` are the only modes that
remove anything. A worktree is a candidate only when ALL of these are
verified:

1. clean git tree — nothing tracked, modified, or untracked except the root
   `.brief.md` scaffold, which is moved aside immediately before removal and
   restored if git refuses;
2. no herdr agent cwd inside it;
3. no open PR on its branch, and the `gh pr list` check itself succeeded
   (an unreadable/unavailable result keeps it);
4. HEAD is an ancestor of `origin/staging` when that ref exists, else of
   `origin/main`, and is not equal to the integration tip;
5. no protected gitignored files (env/secrets/session/PR-review patterns)
   and no skip-worktree/assume-unchanged index marks;
6. the resolved path is exactly `<worktrees root>/<fleet.worktree_dir>/<one
   branch component>`, so a fleet root cannot authorize a sibling path.

`--min-age` (default 1 day) keeps recently touched HEADs, and
`--max-prune` (default 10) is checked on apply before anything is removed.
Removal is always a NON-FORCE `git worktree remove`, revalidated
immediately before each deletion, so git remains the final authority.

## Exit codes

| Code | Meaning |
|------|---------|
| 0    | success — read command completed; registry write happened (or was an idempotent no-op); `switch` re-armed the orchestrator; reap/prune completed/planned a run |
| 1    | `check`: at least one fleet failed verification; registry writes: refused or failed, with the registry byte-identical; `watch`: problems found — INCLUDING an unreadable/invalid registry, which `watch` reports as a `PROBLEM:` line with exit 1 (monitor safety); `switch`/`reap`/`prune`: operational refusal or failure (auth failed, shrink guard, identity check failed, cap, git/gh/herdr failure) |
| 2    | usage error; for every subcommand EXCEPT `watch`: also an unreadable/unparseable registry or validation failure |

## Scope boundary

- The registry remains the single source of truth; `switch`, `reap`, and
  `prune` never rewrite it, and `reap`/`prune` only touch verified
  processes/worktrees.
- No change to how running agents execute, how sessions persist, the
  `src/drive/` contract, or READ-ONLY GitHub access.
- CLI execution happens before the tokio runtime; these commands never talk
  to a running daemon.
