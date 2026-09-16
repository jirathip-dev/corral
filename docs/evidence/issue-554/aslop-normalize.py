#!/usr/bin/env python3
"""Identity compare of the advisory anti-slop log: BASE tree vs lane HEAD.

Keys on (repo-relative file, rule) only — line numbers and message text are
ignored — and compares per-key counts, so an added violation in one file cannot
hide behind a removed one elsewhere. Reads the two logs produced by
`aslop-compare.sh` (base) and `run-gates.sh` G6 (head).

Same reducer as the #551 lane used (docs/evidence/issue-551/aslop-normalize.py),
re-pointed at this lane's logs so the #554 delta claim is computed the same way.
"""
import collections
import re
import sys

PAT = re.compile(r'^(?:\S*/)?(ios/\S+?\.swift):\d+:\d+: error: \[([^\]]+)\]')
LOGS = {'base': '/tmp/g554-aslop-base.log', 'head': '/tmp/g554-aslop-head.log'}
MODE = sys.argv[1] if len(sys.argv) > 1 else 'compare'

if MODE == 'render':
    # Normalizes a log into sorted (file, rule, count) lines for committing.
    counter = collections.Counter()
    for line in open(LOGS[sys.argv[2]], errors='replace'):
        match = PAT.match(line.strip())
        if match:
            counter[(match.group(1), match.group(2))] += 1
    for (path, rule), count in sorted(counter.items()):
        print('%s\t%s\t%d' % (path, rule, count))
    sys.exit(0)

sets = {}
for name, path in LOGS.items():
    counter = collections.Counter()
    for line in open(path, errors='replace'):
        match = PAT.match(line.strip())
        if match:
            counter[(match.group(1), match.group(2))] += 1
    sets[name] = counter
print('BASE_TOTAL=%d HEAD_TOTAL=%d' % (sum(sets['base'].values()), sum(sets['head'].values())))
print('ADDED=%s' % dict(sets['head'] - sets['base']))
print('GONE=%s' % dict(sets['base'] - sets['head']))
print('COUNT_CHANGED=%s' % {key: (sets['base'][key], sets['head'][key])
                            for key in set(sets['base']) & set(sets['head'])
                            if sets['base'][key] != sets['head'][key]})
