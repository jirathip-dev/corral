#!/usr/bin/env python3
"""#568 additive-target proof and the repository's regenerate/compare drift check."""
import copy
import json
from pathlib import Path
import subprocess
import tempfile
import xml.etree.ElementTree as ET

ROOT = Path(__file__).resolve().parents[3]
BASE = "644db78c5a6a2e6c4436372f6708b8cb1a7400c4"
PROJECT = "ios/FleetNotifier.xcodeproj/project.pbxproj"
SCHEME = "ios/FleetNotifier.xcodeproj/xcshareddata/xcschemes/FleetNotifier.xcscheme"


def base(path):
    return subprocess.check_output(["git", "show", f"{BASE}:{path}"], cwd=ROOT, timeout=30)


def parse(data):
    with tempfile.NamedTemporaryFile(suffix=".pbxproj") as file:
        file.write(data)
        file.flush()
        return json.loads(subprocess.check_output(
            ["plutil", "-convert", "json", "-o", "-", file.name], timeout=30))


def main():
    before = {path: (ROOT / path).read_bytes() for path in (PROJECT, SCHEME)}
    result = subprocess.run(["xcodegen", "generate"], cwd=ROOT / "ios", timeout=60)
    print(f"xcodegen_exit={result.returncode}", flush=True)
    assert result.returncode == 0
    for path, data in before.items():
        assert (ROOT / path).read_bytes() == data, f"generated drift: {path}"
    old = parse(base(PROJECT))
    new = parse(before[PROJECT])
    added = new["objects"].keys() - old["objects"].keys()
    normalized = copy.deepcopy(new)
    normalized["objects"] = {key: value for key, value in normalized["objects"].items() if key not in added}
    for key, obj in normalized["objects"].items():
        if obj["isa"] == "PBXGroup":
            obj["children"] = [child for child in obj["children"] if child not in added]
        if obj["isa"] == "PBXProject":
            obj["targets"] = [target for target in obj["targets"] if target not in added]
            obj["attributes"]["TargetAttributes"] = {
                target: value for target, value in obj["attributes"]["TargetAttributes"].items()
                if target not in added}
    assert normalized == old, "existing project object changed"
    added_targets = [new["objects"][key] for key in added if new["objects"][key]["isa"] == "PBXNativeTarget"]
    assert len(added_targets) == 1
    assert added_targets[0]["name"] == "FleetNotifierUITests"
    assert added_targets[0]["productType"] == "com.apple.product-type.bundle.ui-testing"
    # Existing build/test entries, app launch/archive configuration remain identical.
    previous = ET.fromstring(base(SCHEME))
    current = ET.fromstring(before[SCHEME])
    for parent in current.iter():
        for child in list(parent):
            ref = child.find("BuildableReference")
            if ref is not None and ref.get("BlueprintName") == "FleetNotifierUITests":
                if child.tag == "BuildActionEntry":
                    assert child.get("buildForTesting") == "YES"
                    assert all(child.get(key) == "NO" for key in (
                        "buildForRunning", "buildForProfiling", "buildForArchiving", "buildForAnalyzing"))
                parent.remove(child)
    def signature(node):
        return node.tag, node.attrib, (node.text or "").strip(), [signature(child) for child in node]
    assert signature(previous) == signature(current), "existing scheme entry changed"
    # YAML target bodies and old settings stay byte-identical after removing the added lines.
    diff = subprocess.check_output(["git", "diff", BASE, "--", "ios/project.yml"], cwd=ROOT, text=True, timeout=30)
    assert not any(line.startswith("-") and not line.startswith("---") for line in diff.splitlines())
    print(f"PASS: {len(added)} added project objects; every pre-existing object unchanged except new group/target references")
    print("PASS: old scheme entries unchanged; UI target test-only, not archived or run as the product")
    print("PASS: project.yml additive; second xcodegen produced byte-identical project and scheme")


if __name__ == "__main__":
    main()
