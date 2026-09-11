#!/usr/bin/env python3
"""Read a closed-contract build number from Fastlane's exported IPA."""
from __future__ import annotations

import plistlib
import re
import sys
import zipfile
from pathlib import Path

INFO_PLIST = "Payload/FleetNotifier.app/Info.plist"
BUILD_NUMBER = re.compile(r"[0-9]+(?:\.[0-9]+){0,2}")
UNAVAILABLE = "unavailable"


def read_build_number(ipa: Path) -> str:
    try:
        with zipfile.ZipFile(ipa) as archive:
            metadata = plistlib.loads(archive.read(INFO_PLIST))
    except (
        KeyError,
        OSError,
        plistlib.InvalidFileException,
        RuntimeError,
        zipfile.BadZipFile,
    ):
        return UNAVAILABLE

    value = metadata.get("CFBundleVersion") if isinstance(metadata, dict) else None
    if not isinstance(value, str) or BUILD_NUMBER.fullmatch(value) is None:
        return UNAVAILABLE
    return value


def main(argv: list[str]) -> int:
    if len(argv) != 2:
        print(f"usage: {Path(argv[0]).name} IPA", file=sys.stderr)
        return 2
    print(read_build_number(Path(argv[1])))
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv))
