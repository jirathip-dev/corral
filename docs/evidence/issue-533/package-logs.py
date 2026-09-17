"""Preserve native logs byte-for-byte without text whitespace normalization."""

import gzip
import hashlib
import json
from pathlib import Path

ROOT = Path(__file__).resolve().parent
names = ("red-harness-initial", "red", "exact-base-red", "focused", "full", "mutation", "restore")
manifest = {}
for name in names:
    path = ROOT / f"amend1-{name}.log"
    raw = path.read_bytes()
    compressed = gzip.compress(raw, mtime=0)
    assert gzip.decompress(compressed) == raw
    destination = path.with_suffix(".log.gz")
    destination.write_bytes(compressed)
    assert gzip.decompress(destination.read_bytes()) == raw
    lines = raw.decode().splitlines()
    manifest[destination.name] = {
        "raw_sha256": hashlib.sha256(raw).hexdigest(),
        "raw_bytes": len(raw),
        "raw_lines": len(lines),
        "final_executed_lines": [line.strip() for line in lines if "Executed " in line][-2:],
        "exit_lines": [line for line in lines if line.startswith(("red_exit=", "focused_exit=", "full_exit=", "mutation_exit=", "restore_exit="))],
        "restart_lines": [line for line in lines if "Restarting after unexpected exit" in line],
    }
    # Only these named, run-generated logs are replaced by lossless gzip.
    path.unlink()
(ROOT / "amend1-raw-log-manifest.json").write_text(json.dumps(manifest, indent=2) + "\n")
print(json.dumps(manifest, indent=2))
