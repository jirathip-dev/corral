# Issue #201 egui conformance evidence

## Scope and sources

This run targets current-main only. The stale release Corral.app v0.1.0
repo-grouped ellipsis grid and E6 are out of scope; they are not used as a
fix target or as prototype truth.

Read completely before implementation:

- Conductor report: `/Users/jirathip/Projects/corral/docs/design/corral-ux-pixel-diff.md`
- Authoritative prototype spec: `/Users/jirathip/Projects/corral/docs/design/corral-ux-prototype-spec.md`
- Rendered prototype truth: `/tmp/prototype-egui-crop.png`

The exact spec is also preserved at
[`docs/design/corral-ux-prototype-spec.md`](../../corral-ux-prototype-spec.md).
The approved prototype is 1160×631; the worktree copy is byte-identical.

## Browser and rendering path

Browser discovery was unavailable: `getDefault()` reported “No browser is
available” and `agent.browsers.list()` returned `[]`. No unrelated browser
control fallback was used.

Evidence came from the legitimate native path:

1. `clients/egui/src/main.rs` launches eframe with the native wgpu renderer
   and a 1320×860 logical viewport.
2. `clients/egui/src/app.rs` handles the env-gated
   `CORRAL_UI_SCREENSHOT` command, requests an eframe viewport screenshot,
   polls wgpu for readback, writes PNG, and exits.
3. The release binary was run as:

   ```sh
   CORRAL_UI_SCREENSHOT="$PWD/docs/design/evidence/issue-201/live-after.png" \
   CORRAL_UI_SCREENSHOT_DELAY_MS=20000 \
   CORRAL_UI_SCREENSHOT_AGENT=herdr:01a03388-6a3e-7732-94a4-91bde62aea4d \
   RUST_LOG=info ./target/release/corrald-ui \
     > /tmp/corral-ui-live-after.log 2>&1
   ```

The native PNG is 2640×1720 (Retina scale). The exact final evidence is
[`live-after.png`](live-after.png), SHA-256
`dbb3e6e1762f422660117654f22888c66107645638e64dd8b8c9acd317517f85`.
The pre-change native comparison is [`live-before.png`](live-before.png).

## Live daemon proof

No daemon rebuild or restart was needed. The already-running daemon remained
populated throughout the work.

Final read-only checks:

- `GET http://127.0.0.1:8474/healthz` → `ok`
- `GET /snapshot` → schema 5, rev 14173, 27 agents: done 2, idle 17,
  working 8
- Selected real target:
  `herdr:01a03388-6a3e-7732-94a4-91bde62aea4d`, working, codex, repo
  `corral`, capabilities include `read_tail`, `interrupt`, `kill`, and
  `attach`

The capture log records the real path:

```text
native screenshot evidence selected live agent and requested read_tail + transcript
read_tail result applied ... lines=51
transcript result folded ... outcome=AppliedOk entries=0 has_error=false
requesting viewport screenshot
screenshot saved — exiting
```

The transcript page was successfully authorized but contained zero role
entries for this live session. The screenshot therefore uses the 51 lines
returned by the real `read_tail`; no demo or fabricated content was added.
The returned tail visibly contains the terminal status line, the `› Ask Codex
to do anything` prompt marker, and the real model/path context line.

## Implemented conformance

- Preserved the current-main 42/58 master/detail layout and flat Cards default.
- Board mode removes duplicate global `board / issues / audit` navigation;
  the detail pane owns Board/Issues/Audit with the teal underline. Registry
  and Settings remain global utilities.
- The active All chip emits a visible dark `All` label; Needs you is the only
  red-tinted chip and All is the only working-blue chip. Flexsort and the
  dynamic Working/Idle/Review chip set are gone.
- Master rows are dense, state-tinted, and keep the collapsed `Idle / done`
  tail.
- Cards ends at the styled Recent output surface; the legacy topology/drive
  detail card and recent-drive dump no longer consume the pane. Table bounds
  transcript content to a reflowed 520 px body.
- Recent output paints the supported teal live dot and readable
  `229 earlier lines · Load earlier` divider without unsupported glyph boxes.
- Real tail semantics feed the same block renderer as transcript entries:
  `›`/prompt lines are right-inset user-tint blocks, bullet/status and command
  markers are monospace tool blocks, and ordinary lines are proportional agent
  blocks. The visible `you` label is an inferred role treatment of the real
  terminal prompt marker, not fabricated message text.

## Verification

Passing checks on the final source:

- `cargo fmt --all -- --check`
- `cargo clippy -p corrald-ui --all-targets -- -D warnings`
- `cargo test -p corrald-ui --lib` — 133 passed
- `cargo test -p corrald-ui --test conformance` — 6 passed
- `cargo build --release -p corrald-ui`

Notable contracts include:

- `board_surface_has_no_duplicate_global_board_issue_audit_navigation`
- `toolbar_search_accepts_real_click_and_text_events`
- `board_toolbar_has_required_chips_and_detail_owns_view_actions`
- `recent_tail_classifies_terminal_semantics_into_chat_styles`
- `extreme_master_card_age_is_clipped_inside_bound`
