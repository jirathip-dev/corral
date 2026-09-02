# Issue #354 L4 — docs + demo read-only rewrite (doc-truth evidence)

Branch g354-l4-docs-demo, exact base 2e375204c90ffa2a18f8464e77e1b2294a0cb031
(integration after L1 #355, #351 #324, L2 #356, L3 #357). Docs-only lane:
no src/, crates/, ios/, or clients/egui code changed.

## Scope

Rewrote the top-level docs that still presented the PRE-CUT surface as
current behavior, so they describe the merged read-only reality:

- `README.md` — product description: read-only monitor; removed all
  drive/steer/approve/step-up copy and the mutating diagram.
- `docs/OPERATIONS.md` — device lifecycle, out-of-band grant
  provisioning, signed reads, recents, notifications (real APNs pending
  the .p8 provisioning checkpoint), read-only API reference; removed the
  client diff-page how-to and grant-for-diff instructions.
- `docs/ARCHITECTURE.md` — read-only daemon + v2 board clients (iOS +
  egui/WASM), herdr RAW status vocabulary (working / idle / blocked /
  unknown — no done; #319/#320 wording gone), recents v1 live-tail, no
  push in egui; removed the #353-class GET /fleets / src/fleet /
  corrald-fleet side-reader claims and the Issues-tab render claims.
- `docs/QUICKSTART.md` — register → out-of-band grant → signed read_tail
  walkthrough; removed POST /grants, corrald-grant.sh, action controls,
  Face ID step-up, Issues chips/CI-glyph row copy.
- `docs/DEVELOPING.md` — module map aligned to the current tree
  (no src/approve, no step_up.rs, src/push present, tests include
  readonly_cut.rs), conformance-scenario text to the surviving arms,
  "How to add a capability" rewritten for the closed signed-read set.
- `docs/ios-showcase.md` — capture routes truth: L2 removed the
  issues/issue-detail Debug routes; only -demoMode and -corralDemoDetail
  remain; ios-showcase.py's stale allowlist needs a pipeline trim (out of
  docs scope — reported).
- `docs/design/evidence/issue-354/` — this evidence dir + the doc-truth
  gate.

## Removed-mention inventory (each pre-fix claim → source truth at base)

| Doc (base line) | Removed claim | Source truth |
|---|---|---|
| README:5 | tagline "See every agent. Steer the fleet." | read-only monitor since #354 |
| README:12,16,28-30 | prompt/interrupt/approve/kill/attach features + diagram | no mutating drive dispatch: tests/readonly_cut.rs; `Capability::FromStr` parses read_tail/read_diff only (src/drive/mod.rs:54-76) |
| README:30 | "Approvals that can't go wrong" | approve removed with the mutating plane (#354 L1) |
| README:90 | "Signed writes, default deny ... Step-up" | signed READS only; step-up removed (src/auth/step_up.rs deleted in e5a6ba5) |
| OPERATIONS:227-233 | iOS "Worktree diff (read_diff) ... phone app presents it lazily" | iOS Diff UI removed in #354 L2 (75356cc); no client dispatches read_diff |
| OPERATIONS:250,663 | "add read_diff for the diff page" | no diff page on the phone |
| OPERATIONS:519-542 | Worktree diff read-path how-to | read_diff retained daemon-side only (dispatch in src/api/drive.rs:589); no client UI after L2/L3 |
| OPERATIONS:572-573 | "core board exposes Board, Issues, Settings tabs" | egui = Board \| Settings only (L3, 2e37520; app.rs asserts no Tab::Issues) |
| OPERATIONS:631 | "The desktop Issues tab renders GET /issues" | no Issues tab in any client after L2/L3; route remains a read endpoint |
| ARCHITECTURE:132-140 | Issues tab renders the GET /issues view | see above |
| ARCHITECTURE:154-167 | Side readers: src/fleet/, `corrald fleet switch`, GET /fleets | removed with the Fleet Ops CLI coupling (#296/#298) + #354; main.rs dispatches only `digest` |
| ARCHITECTURE:186-197 | "issue grouping keys are the fleet-ops CLI validated fleet names"; GET /fleets view | issues grouped/pruned by LIVE Herdr workspace.repo categories (#332/#340); no /fleets route |
| ARCHITECTURE:268 | "The WRITE plane is device-signed everywhere" | no write plane; signed reads only |
| ARCHITECTURE:323,351 | egui "Settings hosts the admin-token audit log and grant editor"; "(Board, Audit, Registry, Settings; Issues tab read-only)" | egui Settings is connection-only; two tabs (egui README) |
| QUICKSTART:120-132 | step 5: promote via POST /grants with admin token | host-admin grant surface removed (#354 L1); registry.json out-of-band |
| QUICKSTART:177,205-226 | corrald-grant.sh --caps ...; Prompt/Interrupt/Kill/Attach/Approve UI bullets; Face ID step-up | removed with the client cut |
| QUICKSTART:216-227 | board row "issue chips, CI glyph"; order "blocked > done > working > idle" | v2 board: RAW tokens, blocked → working → idle → unknown (L2/L3 READMEs) |
| DEVELOPING:23,36-40 | lib surface incl. approve; src/approve/; auth step-up + step_up.rs | deleted in #354 L1 (src tree at base) |
| DEVELOPING:210-229 | R1–R10 approve/step-up conformance arms | surviving arms: R1/R2/R5/R10/R11 + read-only probes (crates/corrald-client/tests/conformance.rs) |
| DEVELOPING:671-702 | add-capability via POST /grants + approve seam + dispatch_worktree | out-of-band registry; no approve seam; no fleet-level caps (#354) |
| ios-showcase.md:11-15 | allowlist board/detail/issues/issue-detail Debug routes | DemoFleet routes after L2: -demoMode, -corralDemoDetail only |

Truthful kept mentions (legal by design): removal statements ("... was
removed in #354", "no `/fleets` route exists"), the daemon-retained
`read_diff` capability with no client UI, `POST /grants-read` (live,
#101), and the external fleet-ops CLI (`herdr-fleet`) which corrald does
not host.

## Doc-truth gate (AC: proof greps)

`doc-truth-gate.sh` scans the six rewritten docs for the removed
current-behavior CLAIM FORMS (mutating drive features, grant admin,
step-up, grant CLI, Issues/Terminal/Diff UI, GET /fleets / corrald fleet /
fleet-ops-CLI-validated). RED at the pre-fix base, GREEN at the fix head
(the gate loops files one at a time, so hits print line-only, and a line
matching several patterns prints once per pattern):

    $ git stash push -- README.md docs/OPERATIONS.md docs/ARCHITECTURE.md \
        docs/QUICKSTART.md docs/DEVELOPING.md docs/ios-showcase.md
    Saved working directory and index state On g354-l4-docs-demo: l4-docs-red-proof
    $ bash docs/design/evidence/issue-354/doc-truth-gate.sh; echo exit=$?
    12:- **Do something about it.** From your phone — prompt, interrupt, approve, read its output, or stop it. No SSH, no terminal needed.
    28:- **Steer it from your phone** — prompt, interrupt, approve, read output, see the agent's worktree diff (changed files + paged unified diff, #232), kill, or attach an agent, with signed, capability-gated commands.
    16:git watcher ──┤ → corrald (daemon) → ├─ signed drive: prompt · interrupt · approve · read · kill · attach
    5:**See every agent. Steer the fleet.**
    28:- **Steer it from your phone** — prompt, interrupt, approve, read output, see the agent's worktree diff (changed files + paged unified diff, #232), kill, or attach an agent, with signed, capability-gated commands.
    12:- **Do something about it.** From your phone — prompt, interrupt, approve, read its output, or stop it. No SSH, no terminal needed.
    30:- **Approvals that can't go wrong** — an approve action is bound to the exact prompt's hash, so you can't approve the wrong question.
    623:The desktop Issues tab renders the daemon's read-only `GET /issues` view —
    227:- **Worktree diff** (`read_diff`, #232): bounded diff page — diffstat,
    160:  `GET /fleets`) — CONFIGLESS (#237): corral does not own, read, or write
    195:therefore staying in the `(no repo)` orphan bucket. The `GET /fleets` view
    159:- **Fleet identities** (`src/fleet/`, `corrald fleet switch <name>`,
    164:  fleet-ops CLI validated identity path the daemon and `fleet switch`
    187:the fleet-ops CLI validated fleet names. Branches come from git HEAD facts;
    323:  Settings hosts the admin-token audit log and grant editor.
    134:Issues tab renders — separate from the per-agent `closingIssuesReferences`
    223:- **Approve / Deny / Continue** (`approve`): canned replies to a waiting
    220:- **Prompt** (`prompt`): free-text prompt to an agent.
    221:- **Interrupt** (`interrupt`), **Kill** (`kill`), and **Attach**
    222:  (`attach`): signed write controls; Kill uses Face ID step-up.
    177:audit view, and the Settings device-grant editor. It **auto-registers on localhost**
    177:audit view, and the Settings device-grant editor. It **auto-registers on localhost**
    205:scripts/corrald-grant.sh --key <phone-key-id> --caps read_tail,prompt,interrupt,approve
    674:   silently no-op. The desktop grant editor does need its closed-set
    doc-truth-gate: FAIL - a rewritten doc reintroduces removed mutating/fleet surface (matches above).
    exit=1
    $ git stash pop
    On branch g354-l4-docs-demo
    ... (docs restored)
    $ bash docs/design/evidence/issue-354/doc-truth-gate.sh; echo exit=$?
    doc-truth-gate: PASS - README + docs/ carry no removed mutating-drive, grant-admin, step-up, grant-CLI, Issues/Terminal/Diff, or GET /fleets claims as current behavior.
    exit=0

The existing issue-340 gate still passes on the rewritten OPERATIONS.md
(no GET /fleets / corrald fleet / fleet-ops CLI validated claims).

## Privacy scan (demo, AC)

    $ python3 scripts/check-demo-privacy.py clients/egui/assets/demo-fixture.json
    privacy scan: 0 forbidden matches
    exit=0
    $ python3 scripts/check-demo-privacy.py --self-test clients/egui/assets/demo-fixture.json
    privacy self-test: all identity mutations rejected, positive controls accepted
    exit=0

Fixture and scanner are L3-owned, untouched (referenced only).

## Scope notes

- Docs-only: no code changed. Files: README.md, docs/OPERATIONS.md,
  docs/ARCHITECTURE.md, docs/QUICKSTART.md, docs/DEVELOPING.md,
  docs/ios-showcase.md, this evidence dir, .report.md.
- `read_diff` still PARSES and dispatches in the daemon at this exact base
  (src/api/drive.rs:589, src/drive/mod.rs:58); docs therefore describe it
  as a daemon-retained read with no client UI rather than claiming its
  removal. Clients (iOS L2, egui L3) never dispatch it.
- Out-of-fence observation: `scripts/ios-showcase.py` still enumerates the
  pre-cut `issues`/`issue-detail` DemoFleet launch args (pipeline change,
  not docs; capture resumes only when TestFlight is approved).
- Out-of-fence observation: `src/main.rs` `--help` text still prints a
  dead `USAGE: corrald fleet switch <name> [--pane <id>]` line (code, not
  docs; dispatch rejects it — docs already name no such subcommand).
- `docs/corral/` phase-brief area (incl. P4-conformance.md, the frozen
  pre-cut wire record) is left as history, not scanned by the gate.
- Out-of-fence observation: `docs/corral/P4-conformance.md` still
  enumerates the pre-cut endpoints (/step-up, admin /grants, /fleets)
  as normative; client READMEs (ios/, clients/egui) still cite it as the
  wire contract. Not edited here — phase-brief history; a follow-up
  should re-scope or annotate it. The rewritten docs no longer point at
  it as the current contract.
