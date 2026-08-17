# Corral fleet registry — corrald read side (#35 phase 1)

Phase 1 of issue #35 (fleet control-plane consolidation): corrald learns to
read the fleet registry — `fleets.json`, today the single source of truth for
the separate fleet tooling — and exposes two read-only subcommands over it.
**This phase is read-only**: nothing mutates the registry or touches a running
agent. Mutation, pause/resume, model switching, spawning, watchdogs, reaping
and worktree pruning land in later phases of #35.

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
      "models": { "orch": "fable", "impl": "opencode-go/deepseek-v4-flash", "review": "opus" }
    }
  ]
}
```

- `name`, `gh_repo`, `local`, `worktree_dir`, `orch` — required, non-empty strings.
- `workers` — required array of strings; may be empty.
- `models` — required object with required string keys `orch`, `impl`, `review`.
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

## Exit codes

| Code | Meaning |
|------|---------|
| 0    | success — `list` printed; `check` found every fleet OK |
| 1    | `check`: at least one fleet failed verification |
| 2    | usage error, unreadable/unparseable registry, or validation failure |

## Phase boundary (explicitly out of scope)

- No `fleet add/remove/pause/resume/models/switch` — later phases.
- No status/watch/reap/prune consolidation — later phases.
- No change to the running agent, session, or `src/drive/` contract.
