# Release notes

## Unreleased

### iOS

- **Recent-output sheet: the repo has ONE identity across the app (#569).**
  The sheet's flat muted repo label is gone. The repo now renders as the
  Board's own hue chip (`RepoLabelChip`, the same `repoHue(for:among:)`
  resolution the Board, filter chips and subgroup headers use), so a repo
  keeps its colour everywhere; Other/unknown stays the surface2 gray, never
  an accent ring hue. The pane reference became a capsule and the existing
  PR/CI binding capsule is unchanged.
- **Recent-output sheet: state panels paint no background (#569).** The
  `.empty`, `.loading`, generic-error and permission-denial panels lost their
  opaque base slabs: the copy sits directly on the translucent sheet surface
  on the header's 16/10 grid, with measured AA contrast over the rendered
  backdrop in Day and Night. `.empty` is now a deliberate state (small icon +
  headline + one short next step). Sheet corner convention, reused wherever a
  container is kept: capsules for chips, radius-10 continuous cards for
  container surfaces.
