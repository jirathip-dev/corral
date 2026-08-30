#!/usr/bin/env python3
"""Fail-closed privacy gate for bundled and published demo artifacts."""
from pathlib import Path
import json
import shutil
import subprocess
import sys
import tempfile

FORBIDDEN = (
    "jirathip", "github.com/jirathip", "/Users/", "~/.herdr", "sendmeter",
    "morsel", "plush-meadow", "synergy-apps", "synergy-costing",
    "fleet-operations", "hermes-brain", "herdr-board", "project-hearthwild",
)
SYNTHETIC_REPOS = frozenset({
    "atlas-board", "route-lab", "pixel-garden", "orbit-console",
})
SYNTHETIC_ISSUE_TITLE_PREFIXES = ("Demo sample issue: ",)


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
            repo = node.get("repo")
            if isinstance(repo, str):
                if "/" in repo:
                    owner, _, name = repo.partition("/")
                    valid = owner == "demo.example.invalid" and name in SYNTHETIC_REPOS
                else:
                    valid = repo in SYNTHETIC_REPOS
                if not valid:
                    print(f"{path}: fixture repo is not synthetic: {repo}")
                    violations += 1
            elif "repo" in node:
                print(f"{path}: fixture repo has invalid type")
                violations += 1
            for number_key in ("number", "pr_number"):
                number = node.get(number_key)
                if number_key in node and (not isinstance(number, int) or isinstance(number, bool)):
                    print(f"{path}: fixture {number_key} has invalid type")
                    violations += 1
                elif isinstance(number, int) and number < 9000:
                    print(f"{path}: fixture {number_key} is not synthetic: {number}")
                    violations += 1
            for identity_key in ("agent_id", "ref"):
                identity = node.get(identity_key)
                if identity_key in node and (not isinstance(identity, str) or not identity.startswith("demo:")):
                    print(f"{path}: fixture {identity_key} is not synthetic: {identity}")
                    violations += 1
            title = node.get("title")
            if isinstance(node.get("number"), int) and isinstance(title, str):
                if not title.startswith(SYNTHETIC_ISSUE_TITLE_PREFIXES):
                    print(f"{path}: fixture issue title is not synthetic: {title}")
                    violations += 1
            elif "number" in node and "title" in node:
                print(f"{path}: fixture issue title has invalid type")
                violations += 1
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
            if key in {"agent_id", "ref", "del"} and (
                not isinstance(node, str) or not node.startswith("demo:")
            ):
                print(f"{path}: fixture {key} is not synthetic: {node}")
                violations += 1

    visit(value)
    return 1 if violations else 0


def self_test(path: Path) -> int:
    data = json.loads(path.read_text())
    mutations = (
        ("customer-finance-prod", lambda d: d["snapshot"]["agents"]["demo:p01:impl"]["workspace"].__setitem__("repo", "customer-finance-prod")),
        ("github.com/acme/private", lambda d: d["issues"]["atlas-board"][0].__setitem__("repo", "github.com/acme/private")),
        ("live #282 title", lambda d: d["issues"]["atlas-board"][0].__setitem__("title", "Devices/Grants surface — names + grant toggles on board and iOS")),
        ("string issue number", lambda d: d["issues"]["atlas-board"][0].__setitem__("number", "215")),
        ("non-demo attachment ref", lambda d: d["snapshot"]["agents"]["demo:p01:impl"]["attachment"].__setitem__("ref", "herdr:real-prod-agent")),
        ("non-demo delta deletion", lambda d: d["deltas"][0].__setitem__("del", ["herdr:real-prod-agent"])),
    )
    for name, mutate in mutations:
        candidate = json.loads(json.dumps(data))
        mutate(candidate)
        with tempfile.TemporaryDirectory() as directory:
            mutated = Path(directory) / "demo-fixture.json"
            mutated.write_text(json.dumps(candidate))
            if validate_fixture(mutated) == 0:
                print(f"privacy self-test unexpectedly accepted: {name}", file=sys.stderr)
                return 1
    gh = subprocess.run(
        ["gh", "issue", "list", "--repo", "jirathip-dev/corral", "--state", "all",
         "--limit", "200", "--json", "title", "--jq", ".[].title"],
        capture_output=True, text=True, check=False,
    )
    if gh.returncode != 0 or not gh.stdout.strip():
        print("privacy self-test could not fetch live issue titles", file=sys.stderr)
        return 2
    for live_title in gh.stdout.splitlines():
        candidate = json.loads(json.dumps(data))
        candidate["issues"]["atlas-board"][0]["title"] = live_title
        with tempfile.TemporaryDirectory() as directory:
            mutated = Path(directory) / "demo-fixture.json"
            mutated.write_text(json.dumps(candidate))
            if validate_fixture(mutated) == 0:
                print(f"privacy self-test unexpectedly accepted live title: {live_title}", file=sys.stderr)
                return 1
    print("privacy self-test: all six identity mutations and live titles rejected")
    return 0


def main(argv: list[str]) -> int:
    if argv[:1] == ["--self-test"]:
        if len(argv) != 2:
            print("privacy self-test requires a fixture path", file=sys.stderr)
            return 2
        return self_test(Path(argv[1]))
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
        if path.is_file() and path.suffix.lower() == ".json":
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
