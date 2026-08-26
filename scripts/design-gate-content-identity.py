#!/usr/bin/env python3
"""Compute the non-circular implementation identity for #206 evidence.

The manifest is intentionally explicit and excludes every generated evidence
path. It covers the egui source, the daemon-authoritative registry rules, the
build lockfiles, the capture/verification scripts, and the approved prototype.
The digest is therefore stable across a commit while still changing whenever
the implementation content used by the native capture changes.
"""

from __future__ import annotations

import hashlib
from pathlib import Path
import sys


FILES = (
    "Cargo.lock",
    "Cargo.toml",
    "clients/egui/Cargo.toml",
    "clients/egui/src",
    "docs/design/corral-ux-egui-redesign-prototype.html",
    "scripts/design-gate-content-identity.py",
    "scripts/design-gate-evidence.sh",
    "scripts/native-window-probe.swift",
    "scripts/test-design-gate-egui-integration.sh",
    "scripts/verify-design-gate-egui-evidence.py",
    "src/fleet/config.rs",
    "src/fleet/ops.rs",
    "src/main.rs",
)


def files_for(root: Path) -> list[Path]:
    paths: set[Path] = set()
    for entry in FILES:
        path = root / entry
        if path.is_dir():
            paths.update(candidate for candidate in path.rglob("*") if candidate.is_file())
        elif path.is_file():
            paths.add(path)
        else:
            raise SystemExit(f"implementation identity input is missing: {entry}")
    return sorted(paths)


def main() -> int:
    if len(sys.argv) != 2:
        raise SystemExit(f"usage: {Path(sys.argv[0]).name} REPOSITORY")
    root = Path(sys.argv[1]).resolve()
    entries: list[tuple[str, str]] = []
    for path in files_for(root):
        relative = path.relative_to(root).as_posix()
        digest = hashlib.sha256(path.read_bytes()).hexdigest()
        entries.append((relative, digest))
    manifest = "".join(f"{relative}\0{digest}\n" for relative, digest in entries)
    content_digest = hashlib.sha256(manifest.encode("utf-8")).hexdigest()
    print(f"sha256:{content_digest}")
    for relative, digest in entries:
        print(f"- `{relative}` — `{digest}`")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
