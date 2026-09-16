#!/usr/bin/env python3
"""Run under the lane's heavy-gate lock. Restore each production mutation."""
import hashlib
import json
from pathlib import Path
import subprocess
import time

root = Path(__file__).resolve().parents[3]
receipts = []
probes = [
    ("permissions", "src/adapters/gh_plane.rs", "meta.permissions().mode() & 0o077 == 0",
     "meta.permissions().mode() & 0o000 == 0", "g556_token_file_permissions_and_rotation"),
    ("publication", "src/core/store.rs", "if !fresh || (ws.head_sha.is_none() && ws.branch.is_none()) {",
     "if false && (!fresh || (ws.head_sha.is_none() && ws.branch.is_none())) {", "g556_binding_freshness_snapshot_and_sse"),
    ("response-redaction", "src/adapters/gh_plane.rs", "        redact_response(&mut response, token);",
     "        // Mutation: omit response redaction.", "g556_source_processes"),
]
for label, relative, old, new, test in probes:
    path = root / relative
    original = path.read_bytes()
    source = original.decode()
    assert source.count(old) == 1, (label, "anchor drift")
    log = Path("/tmp/g556-mutation-" + label + ".log")
    cmd = ["cargo", "test", "--lib", test, "--", "--nocapture"]
    started = time.monotonic()
    try:
        path.write_text(source.replace(old, new))
        with log.open("w") as out:
            rc = subprocess.run(cmd, cwd=root, stdout=out, stderr=subprocess.STDOUT, timeout=300).returncode
    finally:
        path.write_bytes(original)
        assert path.read_bytes() == original
    receipt = dict(probe=label, file=relative, old=old, new=new, command=cmd, exit=rc,
                   duration_s=round(time.monotonic()-started, 3), log=str(log),
                   restored_sha256=hashlib.sha256(path.read_bytes()).hexdigest())
    receipts.append(receipt)
    (root / "docs/evidence/issue-556/mutations.json").write_text(json.dumps(receipts, indent=2)+"\n")
    print(json.dumps(receipt), flush=True)
    output = log.read_text()
    assert rc == 101 and "panicked at" in output and "test result: FAILED" in output, label
cmd = ["cargo", "test", "--lib", "g556_", "--", "--nocapture"]
log = Path("/tmp/g556-restored.log")
with log.open("w") as out:
    rc = subprocess.run(cmd, cwd=root, stdout=out, stderr=subprocess.STDOUT, timeout=300).returncode
receipts.append(dict(probe="restored", command=cmd, exit=rc, log=str(log)))
(root / "docs/evidence/issue-556/mutations.json").write_text(json.dumps(receipts, indent=2)+"\n")
assert rc == 0
