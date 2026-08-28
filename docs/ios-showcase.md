
## iOS TestFlight showcase pipeline (#282)

The manual `.github/workflows/ios-testflight.yml` workflow has three modes:
`validate` checks the release setup without credentials, `capture` builds the
Debug-only `DemoFleet` Simulator app and reruns the showcase without an ASC
upload, and `upload` performs the existing TestFlight lane first. Only a
successful `upload` dispatched from `main` can publish Pages; a failed upload
or a capture-only run cannot publish.

The public capture allowlist is deliberately small and literal in
`scripts/ios-showcase.py`: `board`, `detail`, `issues`, and `issue-detail`.
Each is launched through an existing Debug-only route and captured with
`xcrun simctl screenshot`; no iOS UI is recreated for the web gallery. The
capture job shares the upload macOS job, so it does not buy a second macOS
runner. The `capture` mode makes a failed capture visible and retryable
without another TestFlight build.

Before artifact handoff, the same script requires exactly four PNGs,
`metadata.json`, and `index.html`. It verifies PNG signatures, chunk CRCs,
IDAT decompression, and 390×844 dimensions, then scans every artifact file for
private paths, tokens, key material, device UUIDs, and other denylisted
identifiers. Any failure is nonzero and the artifact is not uploaded.

The generated static gallery records the exact source commit SHA, UTC capture
time, and TestFlight build number when supplied (otherwise `unavailable`).
Every capture set is labeled `Simulator demo from the TestFlight source
revision`. The secret-free Ubuntu Pages job downloads only this validated
artifact, replaces only `ios/` on `gh-pages`, leaves the egui/WASM root alone,
pushes with the default `GITHUB_TOKEN`, and reads back the public URL with an
HTTP and provenance-content check.
