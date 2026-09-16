"""Read-only proof that the #533 observer edit needs an out-of-fence pin."""

import ast
import importlib.util
import io
from pathlib import Path
import subprocess
import tarfile
import tempfile

BASE = "05dafa0801c264de193e3f518914738c5ed1357d"
ROOT = Path(__file__).resolve().parents[3]
SOURCE = "ios/FleetNotifier/UI/AppTheme.swift"
MANIFEST = "ios/release_source_manifest.py"
CHECKER = "ios/check-release-demo.py"


def load_manifest(root):
    spec = importlib.util.spec_from_file_location("manifest", root / MANIFEST)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def release_pin(root):
    tree = ast.parse((root / CHECKER).read_text())
    return next(
        ast.literal_eval(node.value)
        for node in tree.body
        if isinstance(node, ast.Assign)
        and any(isinstance(t, ast.Name) and t.id == "APPROVED_RELEASE_SOURCE_DIGEST"
                for t in node.targets)
    )


head = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=ROOT, text=True, timeout=30).strip()
print(f"head={head}")
print(f"base={BASE}")
manifest = load_manifest(ROOT)
assert SOURCE in manifest.RELEASE_SOURCE_FILES
print(f"covered={SOURCE}")
actual = manifest.release_source_digest(ROOT)
print(f"head_digest={actual}")
print(f"head_pin={release_pin(ROOT)}")
assert actual == release_pin(ROOT)
archive = subprocess.check_output(
    ["git", "archive", BASE, MANIFEST, CHECKER, *manifest.RELEASE_SOURCE_FILES],
    cwd=ROOT, timeout=60,
)
with tempfile.TemporaryDirectory(prefix="g533-base-digest-") as directory:
    root = Path(directory)
    with tarfile.open(fileobj=io.BytesIO(archive)) as bundle:
        bundle.extractall(root, filter="data")
    old_manifest = load_manifest(root)
    old_digest = old_manifest.release_source_digest(root)
    old_pin = release_pin(root)
    print(f"archived_base_digest={old_digest}")
    print(f"archived_base_pin={old_pin}")
    assert old_digest == old_pin == actual
    assert (root / SOURCE).read_bytes() == (ROOT / SOURCE).read_bytes()
print("source_matches_base=True")
print("STOP: whole-file digest covers the observer; pin is outside the allowed file fence")
