# Issue #340 — OPERATIONS doc-truth regression (GET /fleets / fleet-ops identity)

Current-state corrections to `docs/OPERATIONS.md` after the configless
cutover (#237), the Fleet Ops CLI coupling removal (#296, merged as #298),
and the live-Herdr-workspace Issues scope (#332, merged as #339).

## What was stale and what the source says now (verified at 2c1881f)

| OPERATIONS.md claim (pre-fix) | Source truth (verified) |
|---|---|
| Issue grouping keys are "fleet-ops CLI validated fleet names (`GET /issues` + `GET /fleets`)" | `src/api/mod.rs` routes: read = `/healthz`, `/snapshot`, `/events`, `/history`, `/issues`, `/v1/worktrees`, `/v1/terminal`; write = `/drive`, `/device-token`, `/grants-read` + auth routes. NO `/fleets` route. `src/api/issues.rs` keys the view by repo and prunes to `live_workspace_repos` (`src/api/repo.rs`), the live Herdr `workspace.repo` category set (#237, #332). |
| An Issues refresh "retries a failed `GET /fleets` identity fetch" | No fleet-identity fetch exists on any path. `src/adapters/gh_plane.rs` polls GitHub from specs rebuilt from current Herdr workspaces and emits into the cache `src/api/issues.rs` reads; topology changes prune stale categories (`IssuesCache::prune_to`). A refresh re-reads the last-known projection. |
| The start-worktree slice "consumes the fleet-ops CLI validated identity (`GET /fleets` / `herdr-fleet list`)" | `src/api/drive.rs` `worktree_dispatch` accepts the request's `repo` only when it is a cached issue category or a live workspace repo, then derives the identity itself: `name`/`gh_repo`/`worktree_dir` = the repo category, `local` = `$CORRAL_REPO_ROOT` or `~/Projects/<repo>`. No CLI catalog is consulted. |
| "The only `corrald fleet` subcommand is `switch`…" / example `corrald fleet switch <name>` | `src/main.rs` dispatches only the `digest` subcommand; `parse_args` rejects anything else. `src/fleet/switch.rs` and `src/fleet/cli.rs` were removed with the Fleet Ops CLI coupling (#296/#298). `herdr-fleet` remains the fleet-ops CLI (`~/.config/fleet-operations/fleets.json`); `herdr-fleet switch <name>` is its re-arm command. |
| "Fleet-ops surfaces live in the private sidecar plugin described in #239" | The Corral plugin surface was removed end-to-end (#296/#298): `src/api/plugin.rs`, `herdr-plugin.toml`, egui plugin pane are gone. Fleet-ops surfaces live in the fleet-ops tooling (`herdr-fleet`, `fleet-watch`). |
| Worktrees are created under `<home>/.herdr/worktrees/<fleet.worktree_dir>` | `drive.rs` sets `worktree_dir: repo`, so the path is the repo category: `<home>/.herdr/worktrees/<repo>/issue-<N>-…`. |

Still-valid `herdr-fleet list` behavior is preserved: the configless section
keeps the fleet-ops CLI command list and examples; only the corrald-side
route/CLI-coupling claims were removed (no daemon route was invented).

## Doc-truth gate (AC4)

`doc-truth-gate.sh` greps `docs/OPERATIONS.md` for the removed
current-behavior claim classes and fails if any reappears:

    patterns: "GET /fleets", "corrald fleet", "registered fleet name",
              "fleet-ops CLI validated"

Gate proof — RED at the pre-fix base, GREEN at the fix head:

    $ git stash push -- docs/OPERATIONS.md
    $ bash docs/design/evidence/issue-340/doc-truth-gate.sh; echo exit=$?
    570:fleet-ops CLI validated fleet names (`GET /issues` + `GET /fleets`).
    615:startable. An Issues refresh also retries a failed `GET /fleets` identity fetch.
    644:(`GET /fleets` / `herdr-fleet list`; no corral-owned registry fields — the
    531:pause|resume|models|switch|doctor`). The only `corrald fleet` subcommand is
    543:corrald fleet switch <name>          # re-arm via the fleet-ops CLI (exit code passthrough)
    546:`corrald fleet switch` exits 0 when the fleet-ops CLI switch succeeded and
    613:display category, while each action targets the exact registered fleet name;
    570:fleet-ops CLI validated fleet names (`GET /issues` + `GET /fleets`).
    643:The start-worktree slice consumes the fleet-ops CLI validated identity
    doc-truth-gate: FAIL - docs/OPERATIONS.md claims removed fleet-ops behavior (matches above).
    exit=1
    $ git stash pop
    $ bash docs/design/evidence/issue-340/doc-truth-gate.sh; echo exit=$?
    doc-truth-gate: PASS - docs/OPERATIONS.md names no GET /fleets route, corrald fleet subcommand, or fleet-ops-CLI-validated identity.
    exit=0

## Scope notes

- Docs-only: no production code changed. Changed files: `docs/OPERATIONS.md`
  (prose only), this evidence dir, `.report.md`.
- The #324 "Provider read-tail revision contract" section lives on the live
  `impl-corral-324-rev-r1` lane and is NOT present at this lane's base; no
  revision-contract prose was touched or renumbered.
- Out-of-fence observation: `docs/ARCHITECTURE.md` lines ~159-197 still
  describe the removed `GET /fleets` view, `src/fleet/cli.rs`, and
  `corrald fleet switch` (stale since #296/#332). Not edited here — this
  issue's fence is `docs/OPERATIONS.md` only; a follow-up may be warranted.
