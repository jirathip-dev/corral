#!/usr/bin/env python3
"""Fail-closed privacy gate for bundled and published demo artifacts."""
from pathlib import Path
import json
import re
import shutil
import subprocess
import sys
import tempfile
from urllib.parse import urlparse

URL_RE = re.compile(r"https?://[^\\s\\\"'<>]+")

FORBIDDEN = (
    "jirathip", "github.com/jirathip", "/Users/", "~/.herdr", "sendmeter",
    "morsel", "plush-meadow", "synergy-apps", "synergy-costing",
    "fleet-operations", "hermes-brain", "herdr-board", "project-hearthwild",
)
SYNTHETIC_REPOS = frozenset({
    "atlas-board", "route-lab", "pixel-garden", "orbit-console",
})
APPROVED_TITLES = frozenset({
    "Demo sample issue: web-board",
    "Demo sample issue: device-grants",
    "Demo sample issue: wasm-gate",
    "Demo sample issue: routing-slices",
    "Demo sample issue: partner-collaboration",
    "Demo sample issue: reconnect-loop",
    "Demo sample issue: ios-board",
})
APPROVED_IDENTITIES = frozenset({
    "demo:p01:impl", "demo:p02:impl", "demo:p03:rev", "demo:p04:fleet",
    "demo:p05:orch", "demo:p06:rs", "demo:p07:impl",
    "demo:p01:impl:sha256:d8c352324ee1db57df692113c2337e19ebcee80df9c517a0968bde65dcc48150",
    "demo:p02:impl:sha256:cf704d14dae066f74aeedc7fbf804bc23a44858b87eb3046ce1114ae6528b9e8",
    "demo:p04:fleet:sha256:1f32ae77f3d73e246350da19a055006c1be3e97c70411337d375e6b70a11acbf",
})
IDENTITY_FIELDS = frozenset({"agent_id", "parent_id", "ref", "del", "approval_id"})
APPROVED_FIELDS = frozenset({
    "agent_id", "agents", "ahead", "approval_id", "atlas-board", "attachment",
    "behind", "body", "branch", "capabilities", "choices", "ci_status", "color",
    "del", "deltas", "dirty", "display_name", "generated_at", "host", "issues",
    "kind", "labels", "name", "number", "orbit-console", "parent_id", "pixel-garden",
    "pr_number", "prompt", "prompt_hash", "reason", "recent_output", "ref", "repo",
    "rev", "route-lab", "schema_version", "seq", "snapshot", "source", "state",
    "title", "tool", "ts", "upd", "url", "waiting_on", "workspace", "worktree_path",
})


def validate_fixture(path: Path) -> int:
    try:
        value = json.loads(path.read_text())
    except (OSError, json.JSONDecodeError) as exc:
        print(f"privacy scan: invalid fixture {path}: {exc}", file=sys.stderr)
        return 2

    violations = 0

    def check_urls(text: str) -> None:
        nonlocal violations
        for candidate in URL_RE.findall(text):
            parsed = urlparse(candidate.rstrip(".,;:!?)]}"))
            if parsed.scheme != "https" or parsed.netloc != "demo.example.invalid":
                print(f"{path}: fixture URL has invalid origin: {candidate}")
                violations += 1

    def visit(node: object, key: str = "") -> None:
        nonlocal violations
        if isinstance(node, dict):
            for child_key in node:
                if child_key not in APPROVED_FIELDS and not (
                    key == "agents" and child_key in APPROVED_IDENTITIES
                ):
                    print(f"{path}: fixture key is not approved: {child_key}")
                    violations += 1
                check_urls(child_key)
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
                if title not in APPROVED_TITLES:
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
            if key in IDENTITY_FIELDS and node and node not in APPROVED_IDENTITIES:
                print(f"{path}: fixture identity is not approved: {node}")
                violations += 1
            check_urls(node)
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

    def rename_agent(d: dict) -> None:
        agents = d["snapshot"]["agents"]
        agents["herdr:real-prod-agent"] = agents.pop("demo:p01:impl")

    mutations = (
        ("customer-finance-prod", lambda d: d["snapshot"]["agents"]["demo:p01:impl"]["workspace"].__setitem__("repo", "customer-finance-prod")),
        ("github.com/acme/private", lambda d: d["issues"]["atlas-board"][0].__setitem__("repo", "github.com/acme/private")),
        ("sentinel-prefixed live title", lambda d: d["issues"]["atlas-board"][0].__setitem__("title", "Demo sample issue: iOS showcase: automatically refresh GitHub Pages after successful TestFlight upload")),
        ("string issue number", lambda d: d["issues"]["atlas-board"][0].__setitem__("number", "215")),
        ("non-demo attachment ref", lambda d: d["snapshot"]["agents"]["demo:p01:impl"]["attachment"].__setitem__("ref", "herdr:real-prod-agent")),
        ("non-demo delta deletion", lambda d: d["deltas"][0].__setitem__("del", ["herdr:real-prod-agent"])),
        ("non-demo parent id", lambda d: d["snapshot"]["agents"]["demo:p01:impl"].__setitem__("parent_id", "herdr:real-prod-agent")),
        ("non-demo parent id without colon", lambda d: d["snapshot"]["agents"]["demo:p01:impl"].__setitem__("parent_id", "real-prod-agent")),
        ("non-demo agent map key", rename_agent),
        ("non-demo agent map key with slash", lambda d: d["snapshot"]["agents"].__setitem__("herdr:real/prod-agent", d["snapshot"]["agents"].pop("demo:p01:impl"))),
        ("non-demo novel field", lambda d: d.__setitem__("novel_identity", "herdr:real-prod-agent")),
        ("non-demo URL under unknown key", lambda d: d.__setitem__("novel_url", "https://github.com/acme/private")),
        ("HTTP demo URL", lambda d: d["issues"]["atlas-board"][0].__setitem__("url", "http://demo.example.invalid/atlas-board/issues/9001")),
        ("embedded GitHub URL", lambda d: d["issues"]["atlas-board"][0].__setitem__("body", "See https://github.com/acme/private for details")),
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
    for name, mutate in (
        ("fixture agent id", lambda d: None),
        ("fixture URL in approved field", lambda d: d["issues"]["atlas-board"][0].__setitem__("url", "https://demo.example.invalid/atlas-board/issues/9001")),
        ("ordinary prose", lambda d: d["issues"]["atlas-board"][0].__setitem__("body", "This is ordinary fixture prose without a URL.")),
    ):
        candidate = json.loads(json.dumps(data))
        mutate(candidate)
        with tempfile.TemporaryDirectory() as directory:
            valid = Path(directory) / "demo-fixture.json"
            valid.write_text(json.dumps(candidate))
            if validate_fixture(valid) != 0:
                print(f"privacy self-test unexpectedly rejected positive control: {name}", file=sys.stderr)
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
    print("privacy self-test: all identity mutations and live titles rejected")
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
