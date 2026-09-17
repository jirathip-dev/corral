#!/usr/bin/env python3
"""Read-only #568 pin, reserved-zone, and report-prefix provenance."""
import ast
import hashlib
import io
import json
from pathlib import Path
import subprocess
import sys
import tarfile
import tempfile

ROOT = Path(__file__).resolve().parents[3]
BASE = "4cc316599b0ca1d69039aceb7dbb44193be48e52"
RECEIPT = "644db78c5a6a2e6c4436372f6708b8cb1a7400c4"
VIEW = "ios/FleetNotifier/UI/Herd/HerdView.swift"
CHECKER = "ios/check-release-demo.py"
TEST = "ios/FleetNotifierTests/FleetNotifierTests.swift"


def git(*args):
    return subprocess.check_output(["git", *args], cwd=ROOT, timeout=60)


def sha(data):
    return hashlib.sha256(data).hexdigest()


def pins(data):
    tree = ast.parse(data)
    return {node.targets[0].id: ast.literal_eval(node.value) for node in tree.body
            if isinstance(node, ast.Assign) and isinstance(node.targets[0], ast.Name)
            and node.targets[0].id in ("APPROVED_RELEASE_SOURCE_DIGEST", "APPROVED_TEST_SOURCE_DIGEST")}


def digest(root):
    command = [sys.executable, "-B", "-c", "from pathlib import Path; from ios.release_source_manifest import release_source_digest; print(release_source_digest(Path('.')))"]
    return subprocess.check_output(command, cwd=root, timeout=60, text=True).strip()


def main():
    result = {"base": BASE, "preserved_receipt": RECEIPT}
    result["old_pins"] = pins(git("show", f"{BASE}:{CHECKER}"))
    result["current_pins"] = pins((ROOT / CHECKER).read_bytes())
    result["current_release_digest"] = digest(ROOT)
    assert result["current_release_digest"] == result["current_pins"]["APPROVED_RELEASE_SOURCE_DIGEST"]
    with tempfile.TemporaryDirectory(prefix="g568-a1-base-") as directory:
        archive = git("archive", BASE, "ios/FleetNotifier", "ios/release_source_manifest.py", CHECKER, TEST)
        with tarfile.open(fileobj=io.BytesIO(archive)) as bundle:
            bundle.extractall(directory, filter="data")
        result["archived_base_release_digest"] = digest(Path(directory))
        assert result["archived_base_release_digest"] == result["old_pins"]["APPROVED_RELEASE_SOURCE_DIGEST"]
    assert (ROOT / TEST).read_bytes() == git("show", f"{BASE}:{TEST}")
    result["test_source_sha256"] = sha((ROOT / TEST).read_bytes())
    assert result["test_source_sha256"] == result["old_pins"]["APPROVED_TEST_SOURCE_DIGEST"] == result["current_pins"]["APPROVED_TEST_SOURCE_DIGEST"]
    old = git("show", f"{BASE}:{VIEW}").decode()
    new = (ROOT / VIEW).read_text()
    result["preserved_sections"] = {}
    for name, start, end in (
        ("railZone", "    private var railZone:", "    private var repositoryChip:"),
        ("repositoryChip", "    private var repositoryChip:", "    @ViewBuilder private func herdColumn(")):
        previous = old[old.index(start):old.index(end)].encode()
        current = new[new.index(start):new.index(end)].encode()
        assert current == previous, f"changed #548 section: {name}"
        result["preserved_sections"][name] = {"bytes": len(current), "sha256": sha(current)}
    changes = git("diff", "--name-only", BASE, "--", "ios/FleetNotifier").decode().splitlines()
    assert changes == [VIEW]
    result["changed_app_sources"] = changes
    prefix = git("show", f"{RECEIPT}:.report.md")
    report = (ROOT / ".report.md").read_bytes()
    assert report.startswith(prefix)
    result["report_prefix"] = {"bytes": len(prefix), "lines": len(prefix.splitlines()),
                               "sha256": sha(prefix), "current_prefix_sha256": sha(report[:len(prefix)])}
    result["candidate_view_sha256"] = sha((ROOT / VIEW).read_bytes())
    result["candidate_herd_tests_sha256"] = sha((ROOT / "ios/FleetNotifierTests/HerdTests.swift").read_bytes())
    print(json.dumps(result, indent=2))


if __name__ == "__main__":
    main()
