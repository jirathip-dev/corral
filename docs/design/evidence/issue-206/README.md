# Issue #206 native egui evidence

This bundle contains one native screenshot-to-prototype comparison for each
workspace destination: `board/`, `issues/`, `registry/`, and `settings/`.
Each directory contains the approved-prototype render, the native
`corrald-ui` render, the stamped comparison, the generated conformance note,
and the native capture log.

## Provenance

- Prototype source: `docs/design/corral-ux-egui-redesign-prototype.html`.
- Prototype source SHA-256: `dbcb7d3a8d848247c4f68e6a190e5094734b988c7292b144b0076bda41f827d6`.
- Native process: the release `target/release/corrald-ui` built from this
  worktree.
- Daemon path: a fresh release `corrald` on loopback with a fresh scratch
  registry and three scratch Git repositories.
- Herdr path: a deterministic fake Herdr Unix socket that returns three real
  agent records, one for each scratch repository.
- Issues path: `/snapshot`, registration, `/host-key`, `/events`, and
  `/fleet-registry` are served by the scratch daemon. Only `/issues` is
  supplied by a loopback proxy so the native tab can show a deterministic
  multi-repository open/closed issue snapshot without GitHub credentials; it
  is not GitHub evidence.
- Registry path: the fixture is a real `fleets.json` loaded by the daemon;
  `corrald fleet check --registry` is run before capture, and the UI's save,
  pause, candidate-rejection, stale-draft, and forward-key paths have focused
  tests. “Send to fleet” is explicitly validation-only because no daemon
  distribution endpoint exists in this layout-only scope.
- Implementation identity: each conformance note records a SHA-256 digest of
  an explicit manifest covering the egui source, daemon registry rules, build
  inputs, capture/verification scripts, and approved prototype. Generated
  `docs/design/evidence/issue-206/` is excluded, so the identity is stable and
  non-circular. Verification recomputes the digest and refuses stale bundles;
  it never rewrites them.
- Verification: `scripts/test-design-gate-egui-integration.sh` is read-only by
  default and asserts that `git status --porcelain` is unchanged. Native
  capture/publication requires the explicit `--publish` mode.
- Native readiness: each capture records an exact-PID CoreGraphics window list
  plus structured process/window visibility, frontmost, key/main, and
  active-space probes. The screenshot dispatch, later eframe Screenshot event,
  non-empty PNG, and dimensions are all required before publication.
- No iOS surface and no #215 behavior is included in this bundle.

The generated `conformance.md` in each tab directory records the capture's
parent HEAD for context, the content-addressed implementation identity and
manifest for actual provenance, the command line, and the native binary hash.
Native screenshots are not fixture PNGs: they come from the running egui
binary after its health check and selected `/snapshot` agent validation.
