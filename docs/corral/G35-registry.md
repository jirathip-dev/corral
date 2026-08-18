# Corral fleet registry — corrald registry surface (#35)

Issue #35 (fleet control-plane consolidation): corrald reads the fleet
registry — `fleets.json`, today the single source of truth for the separate
fleet tooling — and, as of slice 1, also WRITES it. `list` / `check` are
read-only views; **`add` / `remove` mutate the registry** behind a
repo-resolves check, candidate validation, and an atomic temp-file+rename
write that leaves the original byte-identical on any refusal or failure.
Nothing touches a running agent. Pause/resume, model switching, spawning,
watchdogs, reaping and worktree pruning land in later phases of #35.

## Registry schema

Default path: `$CORRAL_FLEETS_PATH`, else `$HOME/.hermes/scripts/fleets.json`
(any command accepts `--registry <path>` to override).

```json
{
  "fleets": [
    {
      "name": "corral",
      "gh_repo": "jirathip-k/corral",
      "local": "~/Projects/corral",
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
- `paused` — optional bool, defaults to `false` when absent.
- `local` may start with `~/` — expanded against `$HOME`.
- Unknown fields anywhere (top level, fleet, models) → hard error, not silent
  acceptance. Duplicate fleet names → hard error.

## Commands

```
corrald fleet list [--registry <path>]
corrald fleet check [--registry <path>]
```

`list` — one greppable line per fleet:

```
corral jirathip-k/corral workers=2 paused=true orch=fable impl=opencode-go/deepseek-v4-flash review=opus
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

## Write commands (slice 1)

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
file must already exist — bootstrap one with `echo '{"fleets": []}' >
<path>`. There is no cross-process lock: concurrent writers (corrald or
the legacy tooling) can lose the load→rename race; single-writer
discipline is assumed during the migration window.

## Exit codes

| Code | Meaning |
|------|---------|
| 0    | success — `list` printed; `check` found every fleet OK; `add`/`remove` wrote the registry |
| 1    | `check`: at least one fleet failed verification; `add`/`remove`: refused (duplicate name, unresolvable repo, unknown name, no models to inherit) or the write failed — the registry is left byte-identical |
| 2    | usage error, unreadable/unparseable registry, or validation failure |

## Phase boundary (explicitly out of scope)

- No `fleet pause/resume/models/switch` — later phases.
- No status/watch/reap/prune consolidation — later phases.
- No change to the running agent, session, or `src/drive/` contract.
