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
  pause, and forwarding code paths have focused atomic-update tests.
- No iOS surface and no #215 behavior is included in this bundle.

The generated `conformance.md` in each tab directory records the exact
working-tree HEAD and command line for that capture. Native screenshots are
not fixture PNGs: they come from the running egui binary after its health
check and selected `/snapshot` agent validation.
