#!/usr/bin/env python3
"""Read-only #558 fence, archive, pin and evidence verification; run at repo root."""
import ast
import hashlib
import json
from pathlib import Path
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[3]
BASE = "74c96a1d57528083e67df7fd24de9764e21373f9"
RECEIPT = "43dc8ac64163367e7e31aaa131278bd9c303e25c"


def git(*args):
    return subprocess.check_output(["git", "-C", str(ROOT), *args])


def base_file(path):
    return git("show", f"{BASE}:{path}").decode()


def pins(source):
    return {node.targets[0].id: ast.literal_eval(node.value)
            for node in ast.parse(source).body
            if isinstance(node, ast.Assign) and isinstance(node.targets[0], ast.Name)
            and node.targets[0].id in {"APPROVED_RELEASE_SOURCE_DIGEST", "APPROVED_TEST_SOURCE_DIGEST"}}


allowed = {".report.md", "ios/FleetNotifier/UI/FleetViews.swift",
           "ios/FleetNotifier/Models/Models.swift", "ios/FleetNotifierTests/FleetNotifierTests.swift",
           "ios/check-release-demo.py"}
changed = git("diff", "--name-only", BASE).decode().splitlines()
assert all(p in allowed or p.startswith("docs/evidence/issue-558/") for p in changed), changed
view_path = "ios/FleetNotifier/UI/FleetViews.swift"
view = (ROOT / view_path).read_text()
start = view.index("/// #558: one row inventory")
end = view.index("/// Read-only recents: LIVE TAIL ONLY.", start)
call = "                RecentWorktreeBlock(details: RecentWorktreeDetails(workspace: agent.workspace))\n"
assert view.count(call) == 1
assert (view[:start] + view[end:]).replace(call, "") == base_file(view_path)
print("HEADER_DETENTS_MOTIONS_RECENTS_BOARD_UNCHANGED=true")
test_path = "ios/FleetNotifierTests/FleetNotifierTests.swift"
tests = (ROOT / test_path).read_text()
start = tests.index("// MARK: - Recent-output worktree facts (#558)")
end = tests.index("// MARK: - Optional host GitHub binding (#556)", start)
assert tests[:start] + tests[end:] == base_file(test_path)
print("EVERY_EXISTING_TEST_ASSERTION_UNCHANGED=true")
archive = git("show", f"{RECEIPT}:.report.md")
assert (ROOT / ".report.md").read_bytes().startswith(archive)
print(f"REPORT_APPEND_ONLY=true prefix_bytes={len(archive)} sha256={hashlib.sha256(archive).hexdigest()}")
sys.path.insert(0, str(ROOT / "ios"))
from release_source_manifest import release_source_digest
expected = pins((ROOT / "ios/check-release-demo.py").read_text())
actual = {"APPROVED_RELEASE_SOURCE_DIGEST": release_source_digest(ROOT),
          "APPROVED_TEST_SOURCE_DIGEST": hashlib.sha256((ROOT / test_path).read_bytes()).hexdigest()}
assert actual == expected, (actual, expected)
print("TREE_DIGEST_PINS=" + json.dumps(actual, sort_keys=True))
manifest = json.loads((ROOT / "docs/evidence/issue-558/captures.json").read_text())
assert len(manifest["captures"]) == 13
for entry in manifest["captures"]:
    data = (ROOT / "docs/evidence/issue-558" / entry["file"]).read_bytes()
    assert hashlib.sha256(data).hexdigest() == entry["sha256"], entry["file"]
print("CAPTURE_HASHES=13/13")
print("SCOPE_PASS")
