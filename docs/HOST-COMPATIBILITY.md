# Host compatibility

Corral clients and `corrald` share a small, additive compatibility contract.

- `GET /version` returns `build_id`, `version`, `protocol_version`, and
  `schema_version`. The same `build_identity` object is projected into every
  `/snapshot` response and snapshot SSE frame.
- The current contract is protocol `1`, snapshot schema `5`. Additive fields
  keep the protocol generation stable; incompatible wire changes must bump
  the protocol and update `ios/host-compatibility.json`.
- Desktop, web, and iOS clients treat a missing identity as unknown and show an
  actionable update warning. A protocol mismatch or schema older than `5`
  also shows a warning; the client does not silently operate across that gap.

## Updates

`scripts/update-corral.sh` may run from a dirty feature checkout. It fetches
`origin/main`, archives that exact revision into a disposable source checkout,
and builds there. It never pulls, checks out, resets, cleans, or merges the
developer checkout. A release-shaped copy that is not a Git source checkout
fails explicitly with `release-required`; install a published bundle with
`scripts/install-corral.sh` instead.

`scripts/install-corral.sh` verifies the release checksum before swapping the
release directory. Setup failures roll back the release, launchd plists, app
bundle, and config directory snapshot, then attempt to bootstrap the previous
daemon again. Existing config and key material are not migrated or replaced by
an update.

## Promotion gate

Run the gate locally with:

    python3 scripts/check-host-compatibility.py

The iOS TestFlight workflow runs this gate before dependencies, signing, or
upload. The release workflow runs it before building and embeds the checked
`ios/host-compatibility.json` declaration in the host bundle. Promotion is
therefore blocked unless the manifest names a compatible host artifact or
contains an explicit compatibility declaration.
