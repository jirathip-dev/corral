#!/usr/bin/env python3
"""Compare scanner identities while ignoring moved line/column positions."""
from collections import Counter
import json
from pathlib import Path
import re


def findings(path):
    matches = re.findall(r'^(.+?):\d+:\d+: error: (\[([^]]+)\].+)$', path.read_text(), re.M)
    assert matches, path
    return Counter((file, message) for file, message, _ in matches), Counter(rule for _, _, rule in matches)


base, base_rules = findings(Path('/tmp/g548-aslop-base.log'))
head, head_rules = findings(Path('/tmp/g548-aslop-committed.log'))
report = dict(base=sum(base.values()), head=sum(head.values()),
              added=[dict(path=k[0], diagnostic=k[1], count=v) for k, v in (head-base).items()],
              removed=[dict(path=k[0], diagnostic=k[1], count=v) for k, v in (base-head).items()],
              per_rule=dict(base=base_rules, head=head_rules))
Path('/tmp/g548-aslop-delta-final.json').write_text(json.dumps(report, indent=2)+'\n')
print(json.dumps(report, indent=2))
assert not report['added'], 'new anti-slop findings'
print('PASS: zero added diagnostic identities')
