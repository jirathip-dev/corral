# Go-public flip checklist

This is the human gate for the repository visibility change and marketplace
indexing. The repository-side work can be reviewed and merged independently;
the final flip is never performed by a script or plugin action.

## Repository-side checks

- [x] W2: README leads with Corral as the control plane for a herdr fleet;
  harness-agnostic wording is supporting text (the provider cost meter was
  retired by issue #107 and the transcript reader by #241 — no session-store
  caveat remains).
- [x] W3: `herdr-plugin.toml` links setup and read-only status actions using
  auditable argv arrays; validated as well-formed TOML against the herdr
  0.7.x manifest schema (`contexts` values and `min_herdr_version` are
  recognized by the installed herdr binary).
- [ ] W1: Guy confirms + applies the repository description and topics
  (`herdr` present; `herdr-plugin` deferred to the flip).
- [ ] W1–W3 are merged to `main` (W2/W3 are already on `main` in this
  worktree; W1 is applied to GitHub directly by Guy).
- [x] W4 docs pass: public docs contain no machine-specific internal paths or
  hostnames (re-checked 2026-08-23).
- [ ] W4 secret sweep: rerun the index and reachable-history checks immediately
  before the flip; never print secret-shaped values in evidence. The
  `tracked-secrets` index check passes in this worktree (no tracked
  `.env` / `*.key` / `*.crt` / `*.pem` / `*.p12` / `*.mobileprovision` /
  `*.cer` files); the gitleaks full-tree scan runs in CI (`secret-scan.yml`)
  and must be green at flip time.

## W1 values (apply to GitHub)

Repository description:

> Control plane for your herdr fleet: live board, signed remote drive from any
> device, event history. Works with Claude Code, Codex, OpenCode. Rust daemon +
> egui board + iOS notifier.

Topics:

- Ensure the `herdr` topic is present.
- Do **not** add the `herdr-plugin` topic until the visibility flip (it
  triggers marketplace indexing).

> The issue brief's original description mentioned a "per-provider cost
> meter"; that feature was retired by issue #107, so it is intentionally
> omitted here.

## Human-only final steps

- [ ] Guy changes the GitHub repository visibility to public.
- [ ] Guy sets the GitHub repository social preview to
  `assets/icon/social-preview.png`.
- [ ] Guy adds the `herdr-plugin` topic. This topic intentionally remains absent
  until the visibility flip because it triggers marketplace indexing.
- [ ] Confirm the next marketplace index refresh discovers `herdr-plugin.toml`.

The plugin manifest is not a substitute for the human approval or the
marketplace pickup check.
