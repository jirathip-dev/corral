#!/usr/bin/env python3
"""Regression tests for TestFlight build metadata extraction."""
from __future__ import annotations

import plistlib
import subprocess
import tempfile
import unittest
import zipfile
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
WORKFLOW = ROOT / ".github/workflows/ios-testflight.yml"
INFO_PLIST = "Payload/FleetNotifier.app/Info.plist"


class IOSBuildNumberTest(unittest.TestCase):
    def setUp(self) -> None:
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.workspace = Path(temporary.name)
        (self.workspace / "build/ios").mkdir(parents=True)
        (self.workspace / "scripts").symlink_to(ROOT / "scripts", target_is_directory=True)

    def workflow_build_number(self) -> subprocess.CompletedProcess[str]:
        lines = [line.strip() for line in WORKFLOW.read_text().splitlines()]
        assignments = [line for line in lines if line.startswith('build_number="$(')]
        fallbacks = [line for line in lines if line == '[ -n "$build_number" ] || build_number="unavailable"']
        self.assertEqual(len(assignments), 1, "workflow must have one build-number extractor")
        self.assertEqual(len(fallbacks), 1, "workflow must retain the unavailable fallback")
        script = "\n".join((
            "set -euo pipefail",
            assignments[0],
            fallbacks[0],
            "printf '%s\\n' \"$build_number\"",
        ))
        return subprocess.run(
            ["/bin/bash", "-c", script],
            cwd=self.workspace,
            text=True,
            capture_output=True,
            check=False,
        )

    def write_ipa(self, plist: bytes | None, *, member: str = INFO_PLIST) -> Path:
        path = self.workspace / "build/ios/FleetNotifier.ipa"
        with zipfile.ZipFile(path, "w") as archive:
            archive.writestr(member, plist if plist is not None else b"not a plist")
        return path

    def assert_build_number(self, expected: str) -> None:
        result = self.workflow_build_number()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(result.stdout, f"{expected}\n")

    def test_workflow_uses_exported_ipa_helper(self) -> None:
        workflow = WORKFLOW.read_text()
        self.assertIn(
            'build_number="$(python3 scripts/ios-build-number.py build/ios/FleetNotifier.ipa)"',
            workflow,
        )
        self.assertNotIn("build/ios/Payload/FleetNotifier.app/Info.plist", workflow)

    def test_valid_plist_yields_exact_build_number(self) -> None:
        for value in ("22", "0", "1.2.3"):
            with self.subTest(value=value):
                self.write_ipa(
                    plistlib.dumps({"CFBundleVersion": value}, fmt=plistlib.FMT_BINARY)
                )
                self.assert_build_number(value)

    def test_missing_archive_is_unavailable(self) -> None:
        self.assert_build_number("unavailable")

    def test_missing_plist_is_unavailable(self) -> None:
        self.write_ipa(b"fixture", member="Payload/FleetNotifier.app/not-Info.plist")
        self.assert_build_number("unavailable")

    def test_unreadable_archive_is_unavailable(self) -> None:
        (self.workspace / "build/ios/FleetNotifier.ipa").write_bytes(b"not a zip archive")
        self.assert_build_number("unavailable")

    def test_plistbuddy_diagnostic_is_unavailable(self) -> None:
        self.write_ipa(
            b"File Doesn't Exist, Will Create: "
            b"build/ios/Payload/FleetNotifier.app/Info.plist\n"
        )
        self.assert_build_number("unavailable")

    def test_malformed_values_are_unavailable(self) -> None:
        values: tuple[object, ...] = (
            "",
            "1.2.3.4",
            "22beta",
            "File Doesn't Exist, Will Create: build/ios/Payload/FleetNotifier.app/Info.plist",
            22,
        )
        for value in values:
            with self.subTest(value=value):
                self.write_ipa(plistlib.dumps({"CFBundleVersion": value}))
                self.assert_build_number("unavailable")

    def test_whitespace_values_are_unavailable(self) -> None:
        for value in (" ", " 22", "22 ", "22\n", "1. 2"):
            with self.subTest(value=value):
                self.write_ipa(plistlib.dumps({"CFBundleVersion": value}))
                self.assert_build_number("unavailable")


if __name__ == "__main__":
    unittest.main()
