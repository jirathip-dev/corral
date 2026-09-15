#!/usr/bin/env python3
"""Compare advisory findings by source identity, preserving multiplicities."""
import collections
import json
from pathlib import Path
import re
import subprocess
import sys

base, head = map(Path, sys.argv[1:3])
pattern = re.compile(r'^(ios/[^:]+):(\d+):(\d+): (error|warning): \[([^]]+)\] (.*)$')

def findings(root, log):
    rows = []
    for line in Path(log).read_text().splitlines():
        match = pattern.match(line)
        if not match:
            continue
        path, number, column, severity, rule, message = match.groups()
        source = (root / path).read_text().splitlines()[int(number) - 1].strip()
        rows.append((path, rule, severity, column, source, message))
    assert rows, f'No findings parsed from {log}'
    return collections.Counter(rows)

before = findings(base, '/tmp/g547-aslop-base.log')
after = findings(head, '/tmp/g547-aslop-head.log')

def totals(items):
    counts = collections.Counter()
    for identity, count in items.items():
        counts[identity[1]] += count
    return dict(sorted(counts.items()))

def delta(items):
    return [{'path': key[0], 'rule': key[1], 'column': key[3], 'source': key[4], 'count': count}
            for key, count in sorted(items.items())]

result = {'base': subprocess.check_output(['git', '-C', str(base), 'rev-parse', 'HEAD'], text=True).strip(),
          'head': subprocess.check_output(['git', '-C', str(head), 'rev-parse', 'HEAD'], text=True).strip(),
          'identity': 'file, rule, severity, column, source line (trimmed), diagnostic message; multiset',
          'base_total': before.total(), 'head_total': after.total(),
          'base_per_rule': totals(before), 'head_per_rule': totals(after),
          'added': delta(after-before), 'removed': delta(before-after)}
print(json.dumps(result, indent=2))
sys.exit(bool(result['added']))
