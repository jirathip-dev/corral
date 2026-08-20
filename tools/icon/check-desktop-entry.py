#!/usr/bin/env python3
"""Parse and validate the single executable in a rendered desktop entry."""

from __future__ import annotations

import argparse
import sys
from pathlib import Path


ESCAPES = {
    "\\": "\\",
    '"': '"',
    "s": " ",
    "n": "\n",
    "t": "\t",
    "r": "\r",
    "`": "`",
    "$": "$",
}


def fail(message: str) -> None:
    raise SystemExit(f"desktop entry check failed: {message}")


def decode_exec_argument(value: str) -> str:
    if len(value) < 2 or value[0] != '"' or value[-1] != '"':
        fail("Exec must contain one double-quoted argument")
    body = value[1:-1]
    output: list[str] = []
    index = 0
    while index < len(body):
        character = body[index]
        if character == "\\":
            index += 1
            if index == len(body) or body[index] not in ESCAPES:
                fail("Exec contains an invalid backslash escape")
            output.append(ESCAPES[body[index]])
        elif character == "%":
            if index + 1 == len(body) or body[index + 1] != "%":
                fail("Exec contains an unescaped field-code marker")
            output.append("%")
            index += 1
        else:
            output.append(character)
        index += 1
    return "".join(output)


def parse_desktop_entry(path: Path) -> tuple[str, dict[str, str]]:
    try:
        lines = path.read_text(encoding="utf-8").splitlines()
    except OSError as error:
        fail(f"cannot read {path}: {error}")
    if not lines or lines[0] != "[Desktop Entry]":
        fail("missing [Desktop Entry] group")

    fields: dict[str, str] = {}
    for line in lines[1:]:
        if not line or line.startswith("#"):
            continue
        if line.startswith("["):
            break
        if "=" not in line:
            fail(f"malformed line: {line!r}")
        key, field = line.split("=", 1)
        fields[key] = field
    if "Exec" not in fields:
        fail("missing Exec field")
    return decode_exec_argument(fields["Exec"]), fields


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("desktop_entry", type=Path)
    parser.add_argument("expected_executable")
    args = parser.parse_args()

    executable, fields = parse_desktop_entry(args.desktop_entry)
    if executable != args.expected_executable:
        fail(f"Exec decoded as {executable!r}, expected {args.expected_executable!r}")
    if fields.get("Icon") != "corral":
        fail("Icon is not corral")
    print("desktop entry Exec/Icon: ok")
    return 0


if __name__ == "__main__":
    sys.exit(main())
