#!/usr/bin/env python3
"""#564: compare advisory anti-slop findings base-vs-head by (file, rule).

Line numbers are intentionally dropped (the lane's own edits shift them); a
finding family is compared as (relative path, rule) with counts, plus the
distinct message sets so an add-in-one-file / remove-in-another cannot cancel.
"""
import collections
import json
import re
import sys
from pathlib import Path

LINE = re.compile(r'^(?P<path>\S+?\.swift):(?P<line>\d+):(?P<col>\d+):\s+(?P<sev>error|warning):\s+\[(?P<rule>[^\]]+)\]\s+(?P<msg>.*)$')
BASE_PREFIX = "/tmp/g564/slop-base/"


def load(path):
    rows = []
    for raw in Path(path).read_text().splitlines():
        m = LINE.match(raw.strip())
        if not m:
            continue
        rel = m.group("path")
        if rel.startswith(BASE_PREFIX):
            rel = rel[len(BASE_PREFIX):]
        rows.append((rel, m.group("rule"), m.group("sev"), m.group("msg")))
    return rows


def summarize(rows):
    by_file_rule = collections.Counter((r[0], r[1]) for r in rows)
    messages = collections.Counter((r[0], r[1], r[3]) for r in rows)
    return by_file_rule, messages


base = load(sys.argv[1])
head = load(sys.argv[2])
base_fr, base_msg = summarize(base)
head_fr, head_msg = summarize(head)

result = {
    "base_rows": len(base),
    "head_rows": len(head),
    "base_by_file_rule": {f"{k[0]} :: {k[1]}": v for k, v in sorted(base_fr.items())},
    "head_by_file_rule": {f"{k[0]} :: {k[1]}": v for k, v in sorted(head_fr.items())},
    "added_file_rule": {f"{k[0]} :: {k[1]}": v for k, v in sorted((head_fr - base_fr).items())},
    "removed_file_rule": {f"{k[0]} :: {k[1]}": v for k, v in sorted((base_fr - head_fr).items())},
    "added_messages": [f"{k[0]} [{k[1]}] {k[2]}" for k, v in sorted((head_msg - base_msg).items())],
    "removed_messages": [f"{k[0]} [{k[1]}] {k[2]}" for k, v in sorted((base_msg - head_msg).items())],
    "identity": "relative path + rule (+ message text set); line/column dropped",
}
print(json.dumps(result, indent=2))
