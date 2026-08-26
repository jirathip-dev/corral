#!/usr/bin/env python3
"""Compute the non-circular implementation identity for #206 evidence.

The manifest is intentionally explicit and excludes every generated evidence
path and unrelated workspace code. It covers only the egui client, the native
capture/probe/verifier tooling, and the approved prototype. Runtime inputs
such as the daemon, fixture repositories, and the built binary are recorded
separately in each conformance file, so an unrelated merge cannot invalidate
an otherwise identical capture.
"""

from __future__ import annotations

import hashlib
from pathlib import Path
import sys


FILES = (
    "clients/egui/Cargo.toml",
    "clients/egui/src",
    "docs/design/corral-ux-egui-redesign-prototype.html",
    "scripts/design-gate-content-identity.py",
    "scripts/design-gate-evidence.sh",
    "scripts/native-window-probe.swift",
    "scripts/test-design-gate-egui-integration.sh",
    "scripts/verify-design-gate-egui-evidence.py",
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
