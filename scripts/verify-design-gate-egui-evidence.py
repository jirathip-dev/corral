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


TABS = ("board", "issues", "settings")
REQUIRED = ("prototype.png", "live-after.png", "comparison.png", "conformance.md", "capture.log")
ARTIFACTS = ("prototype.png", "live-after.png", "comparison.png", "capture.log")
EXPECTED_DIMENSIONS = {
    "prototype.png": "1160x631",
    "comparison.png": "2400x960",
}
# The configured egui viewport is 1320x860 logical pixels. Native capture
# backends may emit either the logical pixels or one Retina backing-pixel
# multiple, but an arbitrary host/window size is not evidence for this gate.
NATIVE_LIVE_DIMENSIONS = {
    (1320, 860): "1x",
    (2640, 1720): "2x",
}
SHA256 = re.compile(r"[0-9a-f]{64}\Z")


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
            if length != 13:
                raise SystemExit(f"{path} has an invalid IHDR")
            width, height = struct.unpack(">II", payload[:8])
            if width == 0 or height == 0:
                raise SystemExit(f"{path} has empty dimensions")
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
    decoded = decoder.decompress(b"".join(idat))
    decoded += decoder.flush()
    if not decoder.eof or decoder.unused_data:
        raise SystemExit(f"{path} has an incomplete IDAT stream")
    if not decoded:
        raise SystemExit(f"{path} has an empty image payload")
    return width, height


def native_live_scale(width: int, height: int, tab: str) -> str:
    scale = NATIVE_LIVE_DIMENSIONS.get((width, height))
    if scale is None:
        allowed = ", ".join(
            f"{candidate_width}x{candidate_height} ({candidate_scale})"
            for (candidate_width, candidate_height), candidate_scale in NATIVE_LIVE_DIMENSIONS.items()
        )
        raise SystemExit(
            f"{tab} live-after.png has dimensions {width}x{height}; expected one of {allowed}"
        )
    return scale


def verify_consistent_native_scale(scales: dict[str, str]) -> str:
    if not scales:
        raise SystemExit("no native tab scales were verified")
    distinct = set(scales.values())
    if len(distinct) != 1:
        observed = ", ".join(f"{tab}={scales[tab]}" for tab in TABS if tab in scales)
        raise SystemExit(f"native live screenshot scale differs across tabs: {observed}")
    return next(iter(distinct))


def verify_artifact_manifest(bundle: Path, conformance: str, tab: str) -> str:
    rows: dict[str, tuple[str, str]] = {}
    for line in conformance.splitlines():
        match = re.fullmatch(r"\| `([^`]+)` \| `([^`]+)` \| `([^`]+)` \|", line.strip())
        if match:
            name, dimensions, digest = match.groups()
            rows[name] = (dimensions, digest)
    if set(rows) != set(ARTIFACTS):
        missing = sorted(set(ARTIFACTS) - set(rows))
        unexpected = sorted(set(rows) - set(ARTIFACTS))
        detail = []
        if missing:
            detail.append(f"missing {', '.join(missing)}")
        if unexpected:
            detail.append(f"unexpected {', '.join(unexpected)}")
        raise SystemExit(f"{tab} conformance artifact table mismatch: {'; '.join(detail)}")

    native_scale = ""
    for name in ARTIFACTS:
        path = bundle / name
        recorded_dimensions, recorded_digest = rows[name]
        if not SHA256.fullmatch(recorded_digest):
            raise SystemExit(f"{tab} conformance has an invalid SHA-256 for {name}")
        actual_digest = sha(path)
        if actual_digest != recorded_digest:
            raise SystemExit(f"{tab} conformance SHA-256 does not match {name}")
        if name in EXPECTED_DIMENSIONS or name == "live-after.png":
            width, height = complete_png(path)
            actual_dimensions = f"{width}x{height}"
            if recorded_dimensions != actual_dimensions:
                raise SystemExit(
                    f"{tab} conformance dimensions for {name} do not match the PNG"
                )
            if name == "live-after.png":
                native_scale = native_live_scale(width, height, tab)
            elif actual_dimensions != EXPECTED_DIMENSIONS[name]:
                raise SystemExit(
                    f"{tab} {name} has dimensions {actual_dimensions}; expected {EXPECTED_DIMENSIONS[name]}"
                )
        elif recorded_dimensions != "n/a":
            raise SystemExit(f"{tab} conformance dimensions for {name} must be n/a")
    return native_scale


def provenance_hash(conformance: str, label: str, tab: str) -> str:
    match = re.search(rf"- {re.escape(label)}: `([^`]+)`", conformance)
    if not match:
        raise SystemExit(f"{tab} conformance is missing {label}")
    value = match.group(1)
    if not (SHA256.fullmatch(value) or value.startswith("not applicable (")):
        raise SystemExit(f"{tab} conformance has an invalid {label}")
    return value


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
            "cg_owner_pid_match": True,
            "visible_gate": True,
            "frontmost_gate": True,
            "reason_code": "dispatch_ready",
        }
        if any(record.get(key) != value for key, value in required.items()):
            return False
        if record.get("frontmost_application_pid") != record.get("pid"):
            return False
        non_target_count = record.get("non_target_window_count")
        if not isinstance(non_target_count, int) or isinstance(non_target_count, bool):
            return False
        windows = record.get("cg_window_list")
        if not isinstance(windows, list) or not windows:
            return False
        allowed_window_keys = {
            "placement",
            "window_number",
            "layer",
            "onscreen",
            "bounds",
        }
        return all(
            isinstance(window, dict) and set(window).issubset(allowed_window_keys)
            for window in windows
        )

    if not any(ready(record) for record in records):
        raise SystemExit(
            f"{tab} capture log lacks an exact-PID visible/frontmost/key/main readiness probe"
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
    runtime_hashes = []
    native_scales = {}
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
        runtime = (
            provenance_hash(conformance, "Native UI binary SHA-256", tab),
            provenance_hash(conformance, "Daemon binary SHA-256", tab),
            provenance_hash(conformance, "Fixture registry SHA-256", tab),
        )
        if any(not SHA256.fullmatch(value) for value in runtime):
            raise SystemExit(f"{tab} native evidence must record all runtime SHA-256 values")
        runtime_hashes.append(runtime)
        if "requesting viewport screenshot" not in capture_log:
            raise SystemExit(f"{tab} capture log does not prove a completed native capture")
        if "screenshot event received" not in capture_log:
            raise SystemExit(f"{tab} capture log does not prove a Screenshot event")
        if "screenshot saved — exiting" not in capture_log:
            raise SystemExit(f"{tab} capture log does not prove a saved Screenshot PNG")
        verify_native_probe(capture_log, tab)
        native_scales[tab] = verify_artifact_manifest(bundle, conformance, tab)
        live_hashes.append(sha(bundle / "live-after.png"))
    native_scale = verify_consistent_native_scale(native_scales)
    if len(set(live_hashes)) != len(TABS):
        raise SystemExit("native tab screenshots are not distinct")
    if len(set(runtime_hashes)) != 1:
        raise SystemExit("runtime daemon/fixture provenance differs across native tabs")
    print(f"verified committed native evidence: {', '.join(TABS)}")
    print(f"implementation identity: {expected_identity}")
    print(f"native live scale: {native_scale}")
    print("verification is read-only; no evidence artifact was regenerated")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
