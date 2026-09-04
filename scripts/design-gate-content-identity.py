#!/usr/bin/env python3
"""Compute non-circular implementation identities for design-gate evidence.

The identity scope is the current product implementation: the iOS
FleetNotifier client (source, tests, Xcode project, release-wiring docs and
scripts) plus the design-gate capture generator itself. Generated evidence
paths and unrelated workspace code are excluded, so an unrelated merge
cannot invalidate an otherwise identical capture. Runtime inputs such as the
daemon, fixture repositories, and built binaries are recorded separately in
conformance. The #376 egui removal deleted the desktop client and its
Cargo.lock renderer packages, so no lockfile fingerprint is part of the
identity any more (the renderer-fingerprint concept was egui/wgpu-only).
"""

from __future__ import annotations

import argparse
import hashlib
from pathlib import Path

# The iOS implementation scope: every changed product input, its regression
# coverage and release wiring, plus the capture generator. Generated
# PNGs/logs and the evidence directories are never inputs to their own
# identity.
CURRENT_FILES = (
    "ios/FleetNotifier",
    "ios/FleetNotifierTests",
    "ios/FleetNotifier.xcodeproj",
    "ios/project.yml",
    "ios/README.md",
    "ios/check-release-demo.py",
    "ios/release_source_manifest.py",
    "scripts/design-gate-content-identity.py",
    "scripts/design-gate-evidence.sh",
    "scripts/native-window-probe.swift",
)


def files_for(root: Path, entries: tuple[str, ...]) -> list[Path]:
    paths: set[Path] = set()
    for entry in entries:
        path = root / entry
        if path.is_dir():
            paths.update(candidate for candidate in path.rglob("*") if candidate.is_file())
        elif path.is_file():
            paths.add(path)
        else:
            raise SystemExit(f"implementation identity input is missing: {entry}")
    return sorted(paths)


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("repository")
    args = parser.parse_args()
    root = Path(args.repository).resolve()
    entries: list[tuple[str, str]] = []
    for path in files_for(root, CURRENT_FILES):
        relative = path.relative_to(root).as_posix()
        digest = hashlib.sha256(path.read_bytes()).hexdigest()
        entries.append((relative, digest))
    manifest = "".join(f"{relative}\0{digest}\n" for relative, digest in entries)
    content_digest = hashlib.sha256(manifest.encode("utf-8")).hexdigest()
    print(f"sha256:{content_digest}")
    print("- Implementation identity scope: the iOS FleetNotifier implementation and tests, release wiring/docs, and the capture generator; generated evidence is excluded.")
    for relative, digest in entries:
        print(f"- `{relative}` — `{digest}`")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
