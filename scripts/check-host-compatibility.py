#!/usr/bin/env python3
"""Fail a client promotion unless host compatibility is explicit."""

from __future__ import annotations

import json
import sys
from pathlib import Path

EXPECTED_PROTOCOL = 1
MINIMUM_SCHEMA = 5
DEFAULT_MANIFEST = Path(__file__).resolve().parents[1] / "ios" / "host-compatibility.json"


def fail(message: str) -> int:
    print(f"host compatibility gate: FAIL: {message}", file=sys.stderr)
    return 1


def main(argv: list[str]) -> int:
    path = Path(argv[1]) if len(argv) == 2 else DEFAULT_MANIFEST
    if len(argv) > 2:
        return fail("usage: check-host-compatibility.py [manifest.json]")
    try:
        manifest = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as exc:
        return fail(f"cannot read {path}: {exc}")
    if not isinstance(manifest, dict):
        return fail("manifest must be a JSON object")
    protocol = manifest.get("protocol_version")
    if not isinstance(protocol, int) or isinstance(protocol, bool) or protocol != EXPECTED_PROTOCOL:
        return fail(f"protocol_version must be {EXPECTED_PROTOCOL}")
    schema = manifest.get("schema_version")
    if not isinstance(schema, int) or isinstance(schema, bool) or schema < MINIMUM_SCHEMA:
        return fail(f"schema_version must be an integer >= {MINIMUM_SCHEMA}")

    artifact = manifest.get("host_artifact")
    declaration = manifest.get("compatibility_declaration")
    artifact_present = isinstance(artifact, dict) and bool(artifact)
    declaration_present = isinstance(declaration, str) and bool(declaration.strip())
    if not artifact_present and not declaration_present:
        return fail("provide a compatible host_artifact or compatibility_declaration")

    if artifact_present:
        if not str(artifact.get("name", "")).strip():
            return fail("host_artifact needs a non-empty name")
        artifact_protocol = artifact.get("protocol_version")
        if (
            not isinstance(artifact_protocol, int)
            or isinstance(artifact_protocol, bool)
            or artifact_protocol != EXPECTED_PROTOCOL
        ):
            return fail("host_artifact protocol_version is incompatible")
        artifact_schema = artifact.get("schema_version")
        if not isinstance(artifact_schema, int) or artifact_schema < MINIMUM_SCHEMA:
            return fail(f"host_artifact schema_version must be >= {MINIMUM_SCHEMA}")

    print(f"host compatibility gate: PASS ({path})")
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv))
