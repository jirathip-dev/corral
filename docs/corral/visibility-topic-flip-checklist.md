# Go-public flip checklist

This is the human gate for the repository visibility change and marketplace
indexing. The repository-side work can be reviewed and merged independently;
the final flip is never performed by a script or plugin action.

## Repository-side checks

- [ ] W1: Guy confirms the repository description and `herdr` topic.
- [x] W2: README leads with Corral as the control plane for a herdr fleet;
  harness-agnostic wording is supporting text and the cost-meter caveat is
  retained.
- [x] W3: `herdr-plugin.toml` links setup and read-only status actions using
  auditable argv arrays.
- [ ] W1–W3 are merged to `main`.
- [x] W4 docs pass: public docs contain no machine-specific internal paths or
  hostnames.
- [ ] W4 secret sweep: rerun the index and reachable-history checks immediately
  before the flip; never print secret-shaped values in evidence.

## Human-only final steps

- [ ] Guy changes the GitHub repository visibility to public.
- [ ] Guy sets the GitHub repository social preview to
  `assets/icon/social-preview.png`.
- [ ] Guy adds the `herdr-plugin` topic. This topic intentionally remains absent
  until the visibility flip because it triggers marketplace indexing.
- [ ] Confirm the next marketplace index refresh discovers `herdr-plugin.toml`.

The plugin manifest is not a substitute for the human approval or the
marketplace pickup check.
