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
      "orch": "orchestrator",
      "workers": ["worker-a", "worker-b"],
      "paused": true,
      "models": { "orch": "orchestrator-model", "impl": "implementation-model", "review": "review-model",
                  "impl_alt": "fallback-implementation", "impl_alt2": "last-resort-backend" }
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
  by `fleet pause`/`resume`. The skip rule: `false` is never
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
