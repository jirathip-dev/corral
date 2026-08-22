#!/usr/bin/env python3
"""Parse and validate a rendered desktop entry's Exec value.

Exec has two escaping layers: desktop-entry string escapes are decoded before
the command-line quoting rules are applied. Keeping those passes separate
prevents a writer that only escaped the command layer from passing validation.
"""

from __future__ import annotations

import argparse
import sys
from pathlib import Path


GENERAL_ESCAPES = {
    "s": " ",
    "n": "\n",
    "t": "\t",
    "r": "\r",
    "\\": "\\",
}
DEFERRED_COMMAND_ESCAPES = {'"': '"', "$": "$", "`": "`"}
COMMAND_ESCAPES = {'"': '"', "$": "$", "`": "`", "\\": "\\"}
RESERVED_UNQUOTED = set(" \t\n\r\"'\\><~|&;$*?#()=`")


def fail(message: str) -> None:
    raise SystemExit(f"desktop entry check failed: {message}")


def decode_general_string(value: str) -> str:
    """Decode the desktop-entry string layer, before Exec tokenization."""

    output: list[str] = []
    index = 0
    while index < len(value):
        character = value[index]
        if character != "\\":
            output.append(character)
            index += 1
            continue
        if index + 1 == len(value):
            fail("Exec has a trailing general-string escape")
        escaped = value[index + 1]
        if escaped in GENERAL_ESCAPES:
            output.append(GENERAL_ESCAPES[escaped])
        elif escaped in DEFERRED_COMMAND_ESCAPES:
            # These are command-layer escapes that remain after the general
            # layer has consumed the preceding escaped backslash. Keeping the
            # command character here lets the second pass validate it.
            output.append(DEFERRED_COMMAND_ESCAPES[escaped])
        else:
            fail(f"Exec has an invalid general-string escape: \\{escaped}")
        index += 2
    return "".join(output)


def decode_field_code(value: str, index: int, token: list[str]) -> int:
    if index + 1 == len(value):
        fail("Exec has a trailing field-code marker")
    field = value[index + 1]
    if field == "%":
        token.append("%")
        return index + 2
    fail(f"Exec has an unexpanded or invalid field code: %{field}")


def tokenize_exec(value: str) -> list[str]:
    """Apply Exec command-line quoting to the already-decoded string value."""

    arguments: list[str] = []
    index = 0
    while index < len(value):
        if value[index] in " \t":
            index += 1
            continue
        token: list[str] = []
        if value[index] == '"':
            index += 1
            closed = False
            while index < len(value):
                character = value[index]
                if character == '"':
                    index += 1
                    closed = True
                    break
                if character in "\n\r":
                    fail("Exec quoted argument contains a newline")
                if character == "\\":
                    if index + 1 == len(value) or value[index + 1] not in COMMAND_ESCAPES:
                        fail("Exec has an invalid command-line escape")
                    token.append(COMMAND_ESCAPES[value[index + 1]])
                    index += 2
                    continue
                if character in "$`":
                    fail("Exec reserved command character is not escaped")
                if character == "=":
                    fail("Exec executable path contains Desktop Entry '='")
                if character == "%":
                    index = decode_field_code(value, index, token)
                    continue
                token.append(character)
                index += 1
            if not closed:
                fail("Exec has an unterminated quoted argument")
            if index < len(value) and value[index] not in " \t":
                fail("Exec quoted argument is adjacent to another token")
        else:
            while index < len(value) and value[index] not in " \t":
                character = value[index]
                if character in RESERVED_UNQUOTED:
                    fail(f"Exec reserved character is not quoted: {character!r}")
                if character == "%":
                    index = decode_field_code(value, index, token)
                    continue
                token.append(character)
                index += 1
        arguments.append("".join(token))
    return arguments


def parse_desktop_entry(path: Path) -> tuple[list[str], dict[str, str]]:
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
    general_decoded = decode_general_string(fields["Exec"])
    return tokenize_exec(general_decoded), fields


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("desktop_entry", type=Path)
    parser.add_argument("expected_executable")
    args = parser.parse_args()

    arguments, fields = parse_desktop_entry(args.desktop_entry)
    if arguments != [args.expected_executable]:
        fail(f"Exec decoded as {arguments!r}, expected {[args.expected_executable]!r}")
    if fields.get("Icon") != "corral":
        fail("Icon is not corral")
    print("desktop entry Exec/Icon: ok")
    return 0


if __name__ == "__main__":
    sys.exit(main())
