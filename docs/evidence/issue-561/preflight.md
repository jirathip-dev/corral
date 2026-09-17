# Preflight at 4cc316599b0ca1d69039aceb7dbb44193be48e52

Worktree: /Users/jirathip/.herdr/worktrees/corral/impl561-hydrate
Branch: g561-hydrate. Initial `git status --porcelain`: empty.
`gh issue view 561 --repo jirathip-dev/corral --json number,title,body`: exit 0; issue asks to preserve cached facts over supervisor restart.

Pin/digest check performed BEFORE test or implementation edits:
- search_files(path='scripts', pattern='git_plane\\.rs|sha256|sha-256|digest|shasum'): 104 matches; no Rust-source pin. design-gate-content-identity.py:25-36 enumerates only iOS inputs and capture tooling.
- search_files(path='.', pattern='git_plane\\.rs|Rust.source|rust.source|source.digest|source_digest|rglob\\(.\\*\\.rs|glob\\(.\\*\\*/\\*\\.rs'): 126 matches; git-plane references are documentation and historical evidence, not gates. ios/release_source_manifest.py is the Swift app manifest, not a Rust manifest.
- search_files(path='tests', pattern='sha256|digest|include_bytes|include_str|git_plane.rs'): 31 matches; data/wire hashes and JSON fixture includes, not Rust source fingerprints.
- search_files(path='.', file_glob='*.rs', pattern='Sha256|sha256|include_bytes!|include_str!'): hashes are runtime data/fixtures, not source pins.
- search_files(path='tools', pattern='src/|rglob|glob|PIN|digest'): 9 matches, icon checks only; no Rust inputs.
- Read .github/workflows/rust.yml in full: fmt, deny, audit, clippy, release build, tests, coverage; no source digest gate.
Result: no gate found pinning git_plane.rs or digesting the Rust source set. No out-of-fence pin update required.

`git ls-files '*AGENTS.md' '*CLAUDE.md' '*justfile*' '*Justfile*'` and local/ancestor file checks: none. No just recipe available. docs/DEVELOPING.md:135-159 and .github/workflows/rust.yml are the gate references.

Structural queries executed (exit 0):
- `ast-grep outline src/adapters/git_plane.rs`: supervise:775, run_watcher:865, run_status_sweep:1371, apply_probe:1444, rescan:1578, rescan_with_mode:1591, paced_probes:2186, Plane::start:2486.
- `ast-grep run -p '$X.supervise($$$)' -l rust src tests`: production start at git_plane.rs:2488; two #492 test calls.
- `ast-grep outline src/core/store.rs --match GitPlaneHealth --view expanded`: GitPlaneHealth:68, progress:95, last_event_age_ms:101. Read-only; no core edits.
- `ast-grep run -p '$X.facts.$F($$$)' -l rust src/core/store.rs src/adapters/git_plane.rs`: health writers at reset, removal, successful publication and rescan; reader store.rs:600.
- `ast-grep run -p '$X.send($$$)' -l rust src/adapters/git_plane.rs`: probe and topology publications identified.
- `ast-grep run -p '$X.rescan($$$)' -l rust src/adapters/git_plane.rs`: rescan-sites.txt.
- `ast-grep run -p '$X.debounce($$$)' -l rust src/adapters/git_plane.rs`: debounce-sites.txt.
- `ast-grep run -p '$X.apply_probe($$$)' -l rust src/adapters/git_plane.rs`: apply-sites.txt.

Read-only inventory census (Python pathlib, immediate children only):
    herdr_worktrees = [p for r in (Path.home()/'.herdr/worktrees').iterdir()
                      if r.is_dir() for p in r.iterdir()
                      if p.is_dir() and (p/'.git').exists()]
    projects_roots = [p for p in (Path.home()/'Projects').iterdir()
                     if p.is_dir() and (p/'.git').exists()]
Output: herdr_worktrees=107 Projects_roots=23 total=130.
This is a filesystem census, not a claim about the live daemon's current enabled inventory.
Benchmark uses 130 seeded temporary paths (main + 129 linked worktrees) in one repo, not live repositories, sockets, or a daemon restart. One repo does not reproduce the host's multi-repo topology cost or large working directories.
