#!/usr/bin/env python3
"""Generate the Release source marker consumed by the linker."""

from __future__ import annotations

import argparse
from pathlib import Path

from release_source_manifest import attestation_marker, release_source_digest


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--source-root", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()

    digest = release_source_digest(args.source_root)
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(attestation_marker(digest) + "\n", encoding="utf-8")
    print(f"embedded Release source digest {digest} into {args.output}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
