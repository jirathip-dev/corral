"""Recompute the authorized #533 release pin and unchanged test pin."""

import ast
import hashlib
import io
from pathlib import Path
import runpy
import subprocess
import tarfile
import tempfile

ROOT = Path(__file__).resolve().parents[3]
BASE = "05dafa0801c264de193e3f518914738c5ed1357d"
MANIFEST = "ios/release_source_manifest.py"
CHECKER = "ios/check-release-demo.py"
TEST = "ios/FleetNotifierTests/FleetNotifierTests.swift"
SOURCE = "ios/FleetNotifier/UI/AppTheme.swift"
OLD_RELEASE = "d58dd8adf1b70f72049678b0da464d65ab7fe3c064c93d57a7f38d5c4cb4027d"


def pins(root):
    return {
        node.targets[0].id: ast.literal_eval(node.value)
        for node in ast.parse((root / CHECKER).read_text()).body
        if isinstance(node, ast.Assign) and isinstance(node.targets[0], ast.Name)
        and node.targets[0].id in {"APPROVED_RELEASE_SOURCE_DIGEST", "APPROVED_TEST_SOURCE_DIGEST"}
    }


def verify(root, label):
    manifest = runpy.run_path(str(root / MANIFEST))
    values = pins(root)
    actual = manifest["release_source_digest"](root)
    assert actual == values["APPROVED_RELEASE_SOURCE_DIGEST"]
    test = hashlib.sha256((root / TEST).read_bytes()).hexdigest()
    assert test == values["APPROVED_TEST_SOURCE_DIGEST"]
    print(f"{label}_release_digest={actual}")
    print(f"{label}_release_pin={values['APPROVED_RELEASE_SOURCE_DIGEST']}")
    print(f"{label}_test_digest={test}")
    print(f"{label}_test_pin={values['APPROVED_TEST_SOURCE_DIGEST']}")
    return manifest, values


print("head=" + subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=ROOT, text=True, timeout=30).strip())
print("base=" + BASE)
manifest, new = verify(ROOT, "head")
archive = subprocess.check_output(
    ["git", "archive", BASE, MANIFEST, CHECKER, TEST, *manifest["RELEASE_SOURCE_FILES"]],
    cwd=ROOT, timeout=60,
)
with tempfile.TemporaryDirectory(prefix="g533-amend1-base-") as directory:
    root = Path(directory)
    with tarfile.open(fileobj=io.BytesIO(archive)) as bundle:
        bundle.extractall(root, filter="data")
    old_manifest, old = verify(root, "archived_base")
    assert old["APPROVED_RELEASE_SOURCE_DIGEST"] == OLD_RELEASE
    assert old["APPROVED_RELEASE_SOURCE_DIGEST"] != new["APPROVED_RELEASE_SOURCE_DIGEST"]
    assert (root / MANIFEST).read_bytes() == (ROOT / MANIFEST).read_bytes()
    changed = [p for p in manifest["RELEASE_SOURCE_FILES"] if (root / p).read_bytes() != (ROOT / p).read_bytes()]
    assert changed == [SOURCE], changed
    assert (root / TEST).read_bytes() == (ROOT / TEST).read_bytes()
    assert old["APPROVED_TEST_SOURCE_DIGEST"] == new["APPROVED_TEST_SOURCE_DIGEST"]
    print("changed_manifest_sources=" + repr(changed))
    print("manifest_byte_identical=True")
    print("pinned_test_file_byte_identical=True")
    print("test_pin_unchanged=True")
