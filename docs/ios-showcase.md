
## iOS TestFlight showcase pipeline (#282)

The manual `.github/workflows/ios-testflight.yml` workflow has three modes:
`validate` checks the release setup without credentials, `capture` builds the
Debug-only `DemoFleet` Simulator app and reruns the showcase without an ASC
upload, and `upload` performs the existing TestFlight lane first. Only a
successful `upload` dispatched from `main` can publish Pages; a failed upload
or a capture-only run cannot publish.

The showcase pipeline is TestFlight-gated: the #354 read-only cut queue
holds TestFlight (no main promotion until the cut outcome is approved), so
no new capture has been published since the cut.

### Capture routes after the #354 L2 client cut

The public capture allowlist lives in `scripts/ios-showcase.py`. Before the
#354 L2 cut it launched four Debug routes (`board`, `detail`, `issues`,
`issue-detail`); the L2 client removed the Issues UI, so the app's
Debug-only demo routes are now exactly two:

- `-demoMode` — the read-only board (repo groups, raw herdr state chips).
- `-corralDemoDetail` — the same board with the featured agent's recents
  sheet open (the deterministic evidence route; see ios/README.md).

`scripts/ios-showcase.py` still enumerates the pre-cut `issues` /
`issue-detail` entries with their old launch arguments; those arguments no
longer resolve to an Issues surface in the L2 app, so a capture run must
trim the allowlist to `board` and `detail` first. That script change is a
pipeline change (not docs) and lands with the pipeline lane that resumes
captures after TestFlight is approved.

Each capture is launched through an existing Debug-only route and captured
with `xcrun simctl screenshot`; no iOS UI is recreated for the web gallery.
The capture job shares the upload macOS job, so it does not buy a second
macOS runner. The `capture` mode makes a failed capture visible and retryable
without another TestFlight build.

Before artifact handoff, the same script requires the allowlisted PNGs,
`metadata.json`, and `index.html`. It verifies PNG signatures, chunk CRCs,
IDAT decompression, and 390×844 dimensions, then scans every artifact file
for private paths, tokens, key material, device UUIDs, and other denylisted
identifiers. Any failure is nonzero and the artifact is not uploaded.

The generated static gallery records the exact source commit SHA, UTC capture
time, and TestFlight build number when supplied (otherwise `unavailable`).
Every capture set is labeled `Simulator demo from the TestFlight source
revision`. The secret-free Ubuntu Pages job downloads only this validated
artifact after checking out `gh-pages`, replaces only `ios/`, leaves the
rest of the Pages root alone,
pushes with the default `GITHUB_TOKEN`, and reads back the public URL with an
HTTP/content-type/non-empty response check for the page and every allowlisted
PNG. Capture-retry Pages runs additionally query GitHub Actions via `gh api`:
they publish only when the artifact SHA matches a prior successful main-branch
run whose upload step succeeded. Visual mobile/desktop rendering verification
is performed by the orchestrator post-merge.
