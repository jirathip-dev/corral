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
   CORRAL_UI_SCREENSHOT_DELAY_MS=60000 \
   CORRAL_UI_SCREENSHOT_AGENT=herdr:01a03388-6a3e-7732-94a4-91bde62aea4d \
   RUST_LOG=info ./target/release/corrald-ui
   ```

   The target env var only selects the live agent. It does not fetch content.
   After the native window rendered, a macOS Quartz mouse event clicked the
   visible Cards divider control at native screen coordinate `{x: 762, y: 268}`
   (the window was at `{x: 75, y: 41}` with a title bar). This is the normal
   Cards interaction path; the click produced the signed `read_tail` and
   transcript requests.

The native PNG is 2640×1720 (Retina scale). The exact final evidence is
[`live-after.png`](live-after.png), at
`/Users/jirathip/.herdr/worktrees/corral/g201-egui-visual-conformance/docs/design/evidence/issue-201/live-after.png`,
SHA-256
`8e97e3668cd903466c38170b7c0f407179bbe83ef4c2f8a1a9fbca8fc23c00aa`.
The pre-change native comparison is [`live-before.png`](live-before.png).

## Live daemon proof

No daemon rebuild or restart was needed. The already-running daemon remained
populated throughout the work.

Final read-only checks:

- `GET http://127.0.0.1:8474/healthz` → `ok`
- `GET /snapshot` → schema 5, rev 15942, 32 agents: blocked 0, done 8,
  idle 17, working 7
- Selected real target:
  `herdr:01a03388-6a3e-7732-94a4-91bde62aea4d`, working, `orch-corral`,
  codex, capabilities include `read_tail`, `interrupt`, `kill`, and `attach`

The capture log records the real path:

```text
native screenshot evidence selected live agent; Cards fetch remains user-driven
read_tail result applied ... lines=51
transcript result folded ... outcome=AppliedOk entries=0 has_error=false
requesting viewport screenshot
screenshot saved — exiting
```

The captured session record is `/tmp/corral-ui-live-after.log`.

The transcript page was successfully authorized but contained zero role
entries for this live session. The screenshot therefore uses the 51 lines
returned by the real `read_tail`; no demo or fabricated content was added.
The returned tail visibly contains a styled tool/status line, the
`› Ask Codex to do anything` empty-prompt marker, and the real model/path
context line. The empty-prompt marker is deliberately not classified as a
human message, so the capture has no fabricated user block; a real typed
prompt marker would use the right-inset user tint.

## Implemented conformance

- Preserved the current-main 42/58 master/detail layout and flat Cards default.
- Board mode removes duplicate global `board / issues / audit` navigation;
  the detail-owned Board/Issues/Audit strip is reachable in both Cards and
  Table with the teal underline. Registry and Settings remain global
  utilities.
- The production chip sequence is `Needs you` then `All` when a real blocked
  bucket exists; the active All chip emits a visible dark `All` label. Needs
  you is the only red-tinted chip and All is the only working-blue chip.
  Flexsort and the dynamic Working/Idle/Review chip set are gone. This live
  daemon snapshot had zero blocked agents, so the zero-state rule correctly
  hid Needs you in the captured frame.
- Cards/Table/Interrupt/Kill share one inline Cards control row. A pending
  Kill confirmation is rendered below the disabled trigger, expires, and is
  cleared when selection changes; the original Kill pointer coordinate cannot
  confirm by double-click.
- Master rows are dense, state-tinted, and keep the collapsed `Idle / done`
  tail.
- Cards ends at the styled Recent output surface; the legacy topology/drive
  detail card and recent-drive dump no longer consume the pane. Table bounds
  transcript content to a reflowed 520 px body.
- Recent output paints the supported teal live dot and readable
  `229 earlier lines · Load earlier` divider without unsupported glyph boxes.
  That literal is prototype-prescribed display copy, not a count derived from
  the live 51-line tail; the visible `Load earlier` control is the real fetch
  route and provides disabled/error feedback when unavailable.
- Load earlier sends the real `read_tail` drive plus the transcript's opaque
  older-page cursor after the first page; it uses `None` only for the initial
  page. Disabled guidance names capability tokens (`read_tail`, `interrupt`,
  `kill`), never display labels such as `Load earlier`.
- Real tail semantics feed the same block renderer as transcript entries:
  typed `›`/prompt payloads are right-inset user-tint blocks, bullet/status and
  command markers are monospace tool blocks, and ordinary lines—including the
  empty Codex prompt marker—are proportional agent blocks. The final capture
  visibly proves the real tool and agent treatments without inventing a user
  message.
- Transcript panes remain newest-first in storage: Cards renders the newest
  six entries oldest-to-newest with the newest at the bottom, and appending an
  older page does not displace that visible newest window. The live transcript
  response used for the PNG had zero role entries, so this ordering contract
  is covered by rendered test data only and is not claimed as screenshot
  evidence.
- Destructive controls are enabled only for advertised, granted capabilities;
  Kill requires explicit confirmation and Cancel is covered by real pointer
  interaction tests. The keychain warning remains a normal one-shot toast and
  was allowed to expire before evidence capture; it was not structurally
  suppressed for the screenshot.

## Verification

Passing checks on the final source:

- `cargo fmt --all -- --check`
- `cargo clippy -p corrald-ui --all-targets -- -D warnings`
- `cargo test -p corrald-ui --lib` — 141 passed
- `cargo test -p corrald-ui --test conformance` — 6 passed
- `cargo build --release -p corrald-ui`

Notable contracts include:

- `board_surface_has_no_duplicate_global_board_issue_audit_navigation`
- `toolbar_search_accepts_real_click_and_text_events`
- `board_toolbar_has_required_chips_and_detail_owns_view_actions`
- `recent_tail_classifies_terminal_semantics_into_chat_styles`
- `recent_transcript_renders_newest_window_in_stable_order_after_older_page`
- `cards_load_earlier_dispatches_real_read_tail_and_transcript`
- `right_pane_gates_interrupt_kill_and_requires_kill_confirmation`
- `show_table_has_a_real_cards_round_trip`
- `right_tabs_reach_issues_and_audit_and_reset_on_board_return`
- `master_headers_align_identity_and_state_time_with_dense_rows`
- `extreme_master_card_age_is_clipped_inside_bound`
