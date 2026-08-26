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
  distribution endpoint exists in this layout-only scope. Registry mutations
  use the client-local registry path, are refused for non-loopback hosts, and
  compare canonical/normalized client and daemon-reported paths before any
  write; a mismatch names both paths and refuses the mutation.
- Implementation identity: each conformance note records a SHA-256 digest of
  an explicit manifest covering only the egui client, native
  capture/verification scripts, approved prototype, and a narrow fingerprint
  of the selected eframe/wgpu Cargo.lock package records. Generated
  `docs/design/evidence/issue-206/` and unrelated daemon/workspace files are
  excluded, so an unrelated merge cannot invalidate an otherwise identical
  capture. The selected dependency fingerprint catches renderer lockfile-only
  changes without hashing the broad workspace lockfile. Each conformance note
  also records the native UI binary, runtime daemon binary, and fixture
  registry SHA-256 values used by that capture. Verification recomputes the
  implementation digest, checks those runtime provenance fields and every
  non-self-referential artifact hash, and refuses stale or swapped bundles; it
  never rewrites them.
- Verification: `scripts/test-design-gate-egui-integration.sh` is read-only by
  default and asserts that `git status --porcelain` is unchanged. Native
  capture/publication requires the explicit `--publish` mode.
- Native readiness: each capture records an exact-PID CoreGraphics window list
  plus structured process/window visibility, frontmost, key/main, and
  exact-target on-screen observations, with only a non-target window count
  retained for privacy. There is no synthetic active-space gate: the public
  macOS probe cannot provide a reliable independent Space-membership result.
  The screenshot dispatch, later eframe Screenshot event, non-empty PNG, and
  dimensions are all required before publication.
- No iOS surface and no #215 behavior is included in this bundle.

The generated `conformance.md` in each tab directory records the capture's
parent HEAD for context, the content-addressed implementation identity and
manifest for actual provenance, stable `scripts/test-design-gate-egui-integration.sh --publish`
reproduction guidance, the native/daemon/fixture hashes, and exact artifact
dimensions and hashes. `conformance.md` is the manifest itself, so its own
self-referential hash is intentionally omitted. If the default renderer
cannot complete on a host, set `CHROME_BIN` to a complete GUI-capable
Chrome/Chromium executable. Native screenshots are not fixture PNGs: they
come from the running egui binary after its health check and selected
`/snapshot` agent validation.
