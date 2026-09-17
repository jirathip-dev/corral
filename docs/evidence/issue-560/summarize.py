#!/usr/bin/env python3
"""Aggregate complete raw receipts; do not infer a gate from a filtered log."""
import hashlib
import json
from pathlib import Path
import re
import subprocess

ROOT = Path(__file__).resolve().parents[3]
OUT = ROOT/'docs/evidence/issue-560'
RUNS = OUT/'runs'
BASE = 'a42b83ab3e50aa9553076971f39176711f20cb76'
RECEIPT = '4e6d185a257ad137c857244d96ce5194cd8c2a34'
CODE = '7696af2a83f64df7ae9841284a9a7eae20df1d45'
labels = ['red-prefixed','green-regression','green-focused','oracle','fmt','clippy',
          'workspace','deny','audit','release','measurement','coverage-clean','coverage','client-coverage']
results = {}
for label in labels:
    receipt = json.loads((RUNS/f'{label}.json').read_text())
    raw = (RUNS/f'{label}.log').read_text()
    groups = [tuple(map(int, m)) for m in re.findall(
        r'^test result: (?:ok|FAILED)\. (\d+) passed; (\d+) failed; (\d+) ignored; (\d+) measured; (\d+) filtered out;', raw, re.M)]
    results[label] = dict(command=receipt['command'], raw_exit=receipt['raw_exit'],
                          timed_out=receipt['timed_out'], head=receipt['head'],
                          result_groups=len(groups),
                          totals=dict(zip(('passed','failed','ignored','measured','filtered'),
                                          (sum(g[i] for g in groups) for i in range(5)))),
                          observations=[line for line in raw.splitlines() if 'G560_' in line or 'oracle:' in line or line.startswith('TOTAL')])
    if label != 'red-prefixed':
        for path, digest in receipt['source_sha256'].items():
            assert hashlib.sha256((ROOT/path).read_bytes()).hexdigest() == digest, (label,path)
assert results['red-prefixed']['raw_exit'] == 101
assert results['red-prefixed']['totals']['failed'] == 1
assert results['green-regression']['totals']['passed'] == 1
assert results['green-focused']['totals']['passed'] == 6
assert results['oracle']['totals']['passed'] == 1
old_report = subprocess.check_output(['git','show',f'{RECEIPT}:.report.md'],cwd=ROOT)
assert (ROOT/'.report.md').read_bytes().startswith(old_report)
old_tests = subprocess.check_output(['git','show',f'{BASE}:src/core/store/publication_tests.rs'],cwd=ROOT)
assert (ROOT/'src/core/store/publication_tests.rs').read_bytes().startswith(old_tests)
assert subprocess.run(['git','diff','--exit-code',CODE,'HEAD','--','src'],cwd=ROOT).returncode == 0
measured = json.loads((OUT/'measurements.json').read_text())
assert len(measured) == 2
assert all(r['agent_count_min'] >= 38 and r['requested_seconds'] == 120 for r in measured)
summary = dict(base=BASE, receipt_commit=RECEIPT, code_head=CODE, gates=results,
               report_prefix_bytes=len(old_report), report_prefix_sha256=hashlib.sha256(old_report).hexdigest(),
               independent_oracle_original_prefix_byte_identical=True,
               all_gates_green=all(r['raw_exit']==0 and not r['timed_out'] for k,r in results.items() if k!='red-prefixed'),
               measurement={r['label']:{k:v for k,v in r.items() if k not in ('samples','command','private_directory')} for r in measured})
(OUT/'summary.json').write_text(json.dumps(summary,indent=2)+'\n')
print(json.dumps(summary,indent=2))
