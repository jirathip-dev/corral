#!/usr/bin/env python3
"""Compute non-circular implementation identities for design-gate evidence.

Each issue has an explicit scope and excludes generated evidence paths and
unrelated workspace code. Runtime inputs such as the daemon, fixture
repositories, and built binaries are recorded separately in conformance, so
an unrelated merge cannot invalidate an otherwise identical capture. The
renderer lock fingerprint remains part of the #206 identity because eframe/
wgpu resolution affects that native surface; #205 records the same narrow
fingerprint for the shared workspace environment.
"""

from __future__ import annotations

import argparse
import hashlib
from pathlib import Path
import re


ISSUE_206_FILES = (
    "clients/egui/Cargo.toml",
    "clients/egui/src",
    "docs/design/corral-ux-egui-redesign-prototype.html",
    "scripts/design-gate-content-identity.py",
    "scripts/design-gate-evidence.sh",
    "scripts/native-window-probe.swift",
    "scripts/test-design-gate-egui-integration.sh",
    "scripts/verify-design-gate-egui-evidence.py",
)

# The #205 identity is intentionally source-facing: every changed iOS/egui
# implementation input, its regression coverage and release wiring, the
# native capture generator, and the exact approved transcript prototype are
# listed with individual hashes in conformance. Generated PNGs/logs and the
# issue-205 evidence directory are never inputs to their own identity.
ISSUE_205_FILES = (
    "clients/egui/src/theme.rs",
    "clients/egui/src/ui/board.rs",
    "ios/FleetNotifier/App/AppModel.swift",
    "ios/FleetNotifier/App/FleetNotifierApp.swift",
    "ios/FleetNotifier/Demo/DemoFleet.swift",
    "ios/FleetNotifier/UI/FleetViews.swift",
    "ios/FleetNotifier/UI/RecentOutputModel.swift",
    "ios/FleetNotifierTests/FleetNotifierTests.swift",
    "ios/FleetNotifier.xcodeproj/project.pbxproj",
    "ios/project.yml",
    "ios/README.md",
    "ios/check-release-demo.py",
    "ios/release_source_manifest.py",
    "scripts/design-gate-content-identity.py",
    "scripts/design-gate-evidence.sh",
    "docs/design/corral-ux-transcript-chat-prototype.html",
)

# These are the packages whose resolved versions/checksums or dependency
# edges can change the native egui/wgpu surface. The fingerprint intentionally
# excludes every other Cargo.lock package and is documented in the generated
# conformance manifest.
RENDERER_LOCK_PACKAGES = (
    "block2",
    "core-foundation",
    "core-foundation-sys",
    "core-graphics",
    "core-graphics-types",
    "dispatch2",
    "eframe",
    "egui",
    "egui-wgpu",
    "egui-winit",
    "egui_extras",
    "epaint",
    "epaint_default_fonts",
    "glutin",
    "glutin-winit",
    "libc",
    "naga",
    "naga-types",
    "objc2",
    "objc2-app-kit",
    "objc2-core-foundation",
    "objc2-core-graphics",
    "objc2-foundation",
    "objc2-metal",
    "objc2-quartz-core",
    "raw-window-handle",
    "raw-window-metal",
    "wgpu",
    "wgpu-core",
    "wgpu-core-deps-apple",
    "wgpu-hal",
    "wgpu-naga-bridge",
    "wgpu-types",
    "winit",
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


def renderer_dependency_records(lock_text: str) -> str:
    """Return canonical selected Cargo.lock records for the native renderer."""

    records: list[tuple[str, str, str]] = []
    for block in re.findall(
        r"(?ms)^\[\[package\]\]\n.*?(?=^\[\[package\]\]\n|\Z)",
        lock_text,
    ):
        name_match = re.search(r'^name = "([^"]+)"$', block, re.MULTILINE)
        version_match = re.search(r'^version = "([^"]+)"$', block, re.MULTILINE)
        if name_match and version_match and name_match.group(1) in RENDERER_LOCK_PACKAGES:
            records.append((name_match.group(1), version_match.group(1), block.strip()))
    found = {name for name, _, _ in records}
    missing = sorted(set(RENDERER_LOCK_PACKAGES) - found)
    if missing:
        raise ValueError(f"renderer Cargo.lock package records are missing: {', '.join(missing)}")
    return "\n".join(
        record
        for _, _, record in sorted(records, key=lambda item: (item[0], item[1], item[2]))
    ) + "\n"


def renderer_dependency_fingerprint(lock_text: str) -> str:
    records = renderer_dependency_records(lock_text)
    return hashlib.sha256(records.encode("utf-8")).hexdigest()


def files_for_issue(root: Path, issue: str) -> list[Path]:
    if issue == "205":
        return files_for(root, ISSUE_205_FILES)
    if issue == "206":
        return files_for(root, ISSUE_206_FILES)
    raise SystemExit(f"unsupported design-gate identity issue: {issue}")


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("repository")
    parser.add_argument("--issue", choices=("205", "206"), default="206")
    args = parser.parse_args()
    root = Path(args.repository).resolve()
    entries: list[tuple[str, str]] = []
    for path in files_for_issue(root, args.issue):
        relative = path.relative_to(root).as_posix()
        digest = hashlib.sha256(path.read_bytes()).hexdigest()
        entries.append((relative, digest))
    lock_path = root / "Cargo.lock"
    if not lock_path.is_file():
        raise SystemExit("implementation identity input is missing: Cargo.lock")
    try:
        dependency_digest = renderer_dependency_fingerprint(
            lock_path.read_text(encoding="utf-8")
        )
    except (OSError, ValueError) as error:
        raise SystemExit(f"could not fingerprint renderer Cargo.lock records: {error}") from error
    entries.append(("Cargo.lock[renderer dependency records]", dependency_digest))
    manifest = "".join(f"{relative}\0{digest}\n" for relative, digest in entries)
    content_digest = hashlib.sha256(manifest.encode("utf-8")).hexdigest()
    print(f"sha256:{content_digest}")
    print(f"- Identity issue: `#{args.issue}`")
    for relative, digest in entries:
        print(f"- `{relative}` — `{digest}`")
    print(
        "- Renderer dependency fingerprint packages: "
        + ", ".join(RENDERER_LOCK_PACKAGES)
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
