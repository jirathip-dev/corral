#!/usr/bin/env python3
"""Fail-closed privacy gate for bundled and published demo artifacts."""
from pathlib import Path
import json
import shutil
import subprocess
import sys

FORBIDDEN = (
    "jirathip", "github.com/jirathip", "/Users/", "~/.herdr", "sendmeter",
    "morsel", "plush-meadow", "synergy-apps", "synergy-costing",
    "fleet-operations", "hermes-brain", "herdr-board", "project-hearthwild",
)


def validate_fixture(path: Path) -> int:
    try:
        value = json.loads(path.read_text())
    except (OSError, json.JSONDecodeError) as exc:
        print(f"privacy scan: invalid fixture {path}: {exc}", file=sys.stderr)
        return 2

    violations = 0

    def visit(node: object, key: str = "") -> None:
        nonlocal violations
        if isinstance(node, dict):
            for child_key, child in node.items():
                visit(child, child_key.lower())
        elif isinstance(node, list):
            for child in node:
                visit(child, key)
        elif isinstance(node, str):
            if key == "url" and not node.startswith("https://demo.example.invalid/"):
                print(f"{path}: fixture URL is not reserved: {node}")
                violations += 1
            if key in {"worktree_path", "path"} and not node.startswith("/demo/"):
                print(f"{path}: fixture path is not synthetic: {node}")
                violations += 1
            if key == "agent_id" and not node.startswith("demo:"):
                print(f"{path}: fixture agent id is not synthetic: {node}")
                violations += 1

    visit(value)
    return 1 if violations else 0


def main(argv: list[str]) -> int:
    if not argv:
        print("privacy scan: no inputs", file=sys.stderr)
        return 2
    hits = 0
    for name in argv:
        path = Path(name)
        if not path.exists() or path.is_symlink():
            print(f"privacy scan: missing or symlink input: {path}", file=sys.stderr)
            return 2
        files = [path] if path.is_file() else [p for p in path.rglob("*") if p.is_file()]
        if not files:
            print(f"privacy scan: empty input: {path}", file=sys.stderr)
            return 2
        if path.is_file() and path.name == "demo-fixture.json":
            result = validate_fixture(path)
            if result:
                return result
        for file in files:
            try:
                text = file.read_bytes().decode("utf-8", errors="ignore").lower()
            except OSError as exc:
                print(f"privacy scan: cannot read {file}: {exc}", file=sys.stderr)
                return 2
            for needle in FORBIDDEN:
                count = text.count(needle.lower())
                if count:
                    print(f"{file}: {needle}: {count}")
                    hits += count
            if file.suffix.lower() == ".png":
                tesseract = shutil.which("tesseract")
                if tesseract is None:
                    print(f"privacy scan: OCR unavailable for {file}", file=sys.stderr)
                    return 2
                result = subprocess.run(
                    [tesseract, str(file), "stdout", "--psm", "11"],
                    stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, text=True, check=False,
                )
                if result.returncode != 0:
                    print(f"privacy scan: OCR failed for {file}", file=sys.stderr)
                    return 2
                rendered = result.stdout.lower()
                for needle in FORBIDDEN:
                    count = rendered.count(needle.lower())
                    if count:
                        print(f"{file} (rendered): {needle}: {count}")
                        hits += count
    print(f"privacy scan: {hits} forbidden matches")
    return 1 if hits else 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
