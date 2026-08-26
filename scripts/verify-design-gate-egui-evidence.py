#!/usr/bin/env python3
"""Read-only verification of the committed native #206 evidence bundles."""

from __future__ import annotations

import hashlib
import json
from pathlib import Path
import re
import struct
import subprocess
import sys
import zlib


TABS = ("board", "issues", "registry", "settings")
REQUIRED = ("prototype.png", "live-after.png", "comparison.png", "conformance.md", "capture.log")


def sha(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def complete_png(path: Path) -> tuple[int, int]:
    data = path.read_bytes()
    if data[:8] != b"\x89PNG\r\n\x1a\n":
        raise SystemExit(f"{path} is not a PNG")
    offset = 8
    seen_iend = False
    idat = []
    width = height = 0
    while offset < len(data):
        if len(data) - offset < 12:
            raise SystemExit(f"{path} has a truncated PNG chunk")
        length = struct.unpack(">I", data[offset : offset + 4])[0]
        kind = data[offset + 4 : offset + 8]
        end = offset + 8 + length
        if end + 4 > len(data):
            raise SystemExit(f"{path} has a truncated PNG payload")
        payload = data[offset + 8 : end]
        crc = struct.unpack(">I", data[end : end + 4])[0]
        if zlib.crc32(kind + payload) & 0xFFFFFFFF != crc:
            raise SystemExit(f"{path} has a bad PNG CRC")
        if kind == b"IHDR":
            width, height = struct.unpack(">II", payload[:8])
        elif kind == b"IDAT":
            idat.append(payload)
        elif kind == b"IEND":
            if length != 0:
                raise SystemExit(f"{path} has a non-empty IEND")
            seen_iend = True
            offset = end + 4
            break
        offset = end + 4
    if not seen_iend or offset != len(data):
        raise SystemExit(f"{path} is incomplete or has trailing bytes")
    decoder = zlib.decompressobj()
    decoder.decompress(b"".join(idat))
    decoder.flush()
    if not decoder.eof or decoder.unused_data:
        raise SystemExit(f"{path} has an incomplete IDAT stream")
    return width, height


def verify_native_probe(capture_log: str, tab: str) -> None:
    records = []
    for line in capture_log.splitlines():
        try:
            record = json.loads(line)
        except json.JSONDecodeError:
            continue
        if isinstance(record, dict) and "action" in record:
            records.append(record)

    def ready(record: dict) -> bool:
        if record.get("action") != "dispatch_evaluation":
            return False
        required = {
            "probe_ok": True,
            "exact_pid_match": True,
            "process_visible": True,
            "window_visible": True,
            "frontmost": True,
            "key_window": True,
            "main_window": True,
            "frontmost_application_matches_target": True,
            "on_active_space": True,
            "cg_owner_pid_match": True,
            "visible_gate": True,
            "frontmost_gate": True,
            "reason_code": "dispatch_ready",
        }
        if any(record.get(key) != value for key, value in required.items()):
            return False
        windows = record.get("cg_window_list")
        return isinstance(windows, list) and any(
            isinstance(window, dict) and window.get("owner_pid_exact_match") is True
            for window in windows
        )

    if not any(ready(record) for record in records):
        raise SystemExit(
            f"{tab} capture log lacks an exact-PID visible/frontmost/key/space readiness probe"
        )


def main() -> int:
    if len(sys.argv) != 3:
        raise SystemExit(f"usage: {Path(sys.argv[0]).name} REPOSITORY IDENTITY_HELPER")
    repo = Path(sys.argv[1]).resolve()
    identity_helper = Path(sys.argv[2]).resolve()
    evidence_root = repo / "docs/design/evidence/issue-206"
    identity = subprocess.check_output(
        [sys.executable, str(identity_helper), str(repo)], text=True
    ).splitlines()
    if not identity or not identity[0].startswith("sha256:"):
        raise SystemExit("implementation identity helper returned no digest")
    expected_identity = identity[0]
    prototype = repo / "docs/design/corral-ux-egui-redesign-prototype.html"
    expected_prototype = sha(prototype)
    live_hashes = []
    for tab in TABS:
        bundle = evidence_root / tab
        if not bundle.is_dir():
            raise SystemExit(f"missing committed evidence directory: {bundle}")
        for name in REQUIRED:
            path = bundle / name
            if not path.is_file() or not path.stat().st_size:
                raise SystemExit(f"missing or empty committed artifact: {path}")
        conformance = (bundle / "conformance.md").read_text(encoding="utf-8")
        capture_log = (bundle / "capture.log").read_text(encoding="utf-8")
        if f"- Egui tab: `{tab}`" not in conformance:
            raise SystemExit(f"{tab} conformance is not tab-correct")
        digest_match = re.search(
            r"- Implementation content digest: `(sha256:[0-9a-f]+)`", conformance
        )
        if not digest_match or digest_match.group(1) != expected_identity:
            raise SystemExit(f"{tab} implementation content digest does not match the current tree")
        prototype_match = re.search(
            r"- Prototype source SHA-256: `([0-9a-f]+)`", conformance
        )
        if not prototype_match or prototype_match.group(1) != expected_prototype:
            raise SystemExit(f"{tab} prototype provenance does not match the checked-in source")
        if "requesting viewport screenshot" not in capture_log:
            raise SystemExit(f"{tab} capture log does not prove a completed native capture")
        if "screenshot event received" not in capture_log:
            raise SystemExit(f"{tab} capture log does not prove a Screenshot event")
        if "screenshot saved — exiting" not in capture_log:
            raise SystemExit(f"{tab} capture log does not prove a saved Screenshot PNG")
        verify_native_probe(capture_log, tab)
        for name in ("prototype.png", "live-after.png", "comparison.png"):
            complete_png(bundle / name)
        live_hashes.append(sha(bundle / "live-after.png"))
    if len(set(live_hashes)) != len(TABS):
        raise SystemExit("native tab screenshots are not distinct")
    print(f"verified committed native evidence: {', '.join(TABS)}")
    print(f"implementation identity: {expected_identity}")
    print("verification is read-only; no evidence artifact was regenerated")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
