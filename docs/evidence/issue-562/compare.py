#!/usr/bin/env python3
"""Compare actual base/fixed canonical wire bytes, masking only two fields.

Usage: python3 docs/evidence/issue-562/compare.py BASE_LOG FIXED_LOG
The logs come from: cargo test -p corrald g562_ -- --nocapture
"""
import hashlib
import json
from pathlib import Path
import re
import sys


def collect(path):
    cases = {}
    for case, body in re.findall(rb"^G562_CANONICAL (\S+) (.*)$", Path(path).read_bytes(), re.M):
        assert case not in cases, f"duplicate case {case!r}"
        body = re.sub(rb'"epoch":"[^"]+"', b'"epoch":"normalized"', body)
        body = re.sub(rb'"generated_at":\d+', b'"generated_at":0', body)
        cases[case] = body
    assert set(cases) == {b"fresh", b"stopped", b"unobserved", b"absent"}
    return cases


if __name__ == "__main__":
    before, after = map(collect, sys.argv[1:])
    for case in sorted(before):
        assert before[case] == after[case], f"wire bytes changed: {case!r}"
        print(json.dumps({"case": case.decode(), "bytes": len(before[case]),
                          "sha256": hashlib.sha256(before[case]).hexdigest(),
                          "byte_identical": True}))
    print("PASS: 4 canonical snapshots; only epoch/generated_at masked")
