# Reproduce Corral #427 design evidence

Requirements already present on the design host:

- Python 3
- Pillow
- Playwright-cached `chrome-headless-shell`

From either the canonical repo evidence directory or the design-output review copy:

```sh
python3 scripts/build.py
python3 scripts/capture.py --force --workers 6
python3 scripts/verify.py
```

Expected terminal result:

- capture: `captured 175/175; failures=0`
- verification: `status: pass`
- images: 168 matrix + 6 accessibility + 1 comparison
- DOM cases: 60, zero failures
- contrast checks: 28, all pass

## Capture dimensions

Matrix files are produced at native CSS pixel scale, not resized after capture:

- 390×844
- 375×812

The comparison sheet is 1440×1080.

## Interactive review

Open `index.html`. The left-side controls switch direction, palette, and state. On a phone-width viewport the controls disappear and the prototype fills the viewport.

Stable direction files also exist:

- `variant-a.html`
- `variant-b.html`
- `variant-c.html`

Query contract used by the harness:

```text
?capture=1&variant=A&palette=mocha&state=both-filters
```

Supported states:

```text
populated
host-only
repository-only
both-filters
connecting-host
offline-stale-host
zero-results
```

Accessibility evidence adds `a11y=1`; scrolled-end proof adds `scrollEnd=1`; deterministic interaction proof adds `selftest=1`.

## Determinism

`scripts/build.py` is the single generator for all HTML. Do not patch generated HTML directly. Any visual change must:

1. patch `scripts/build.py`;
2. regenerate all HTML;
3. recapture all 175 images;
4. rerun `scripts/verify.py`;
5. visually inspect the regenerated contact sheets;
6. regenerate hashes in `asset-manifest.json` via the same verifier.
