#!/usr/bin/env python3
"""Resolve the assertion ledger against actual Git sources and the tested head."""
import json
from pathlib import Path
import subprocess

root = Path(__file__).resolve().parents[3]
p = root / 'docs/evidence/issue-547/assertion-ledger.json'
entries = json.loads(p.read_text())
if isinstance(entries, dict):
    entries = entries['entries']
refs = ['01ada7d3dc887ca2bb3ec3fd762868cb989753c3',
        '50ba6e02e004bd1f0c962bf2f0044c4b74e809a1']
cache = {}

def locations(text, needle):
    starts = []
    offset = text.find(needle)
    while offset != -1:
        starts.append(text.count('\n', 0, offset) + 1)
        offset = text.find(needle, offset + 1)
    return starts

for index, item in enumerate(entries, 1):
    item['entry'] = index
    old = item.pop('old', item.get('old_assertion'))
    new = item.pop('new', item.get('new_assertion'))
    item['old_assertion'], item['new_assertion'] = old, new
    item['observed_pre_edit_line'] = item.pop('base_line', item.get('observed_pre_edit_line'))
    item['old_committed_locations'] = []
    for ref in refs:
        key = (ref, item['file'])
        if key not in cache:
            cache[key] = subprocess.check_output(['git', '-C', str(root), 'show', ref+':'+item['file']], text=True)
        found = locations(cache[key], old)
        if found:
            item['old_committed_locations'].append({'commit': ref, 'lines': found})
    item['new_lines_at_tested_head'] = locations((root/item['file']).read_text(), new)
    assert item['new_lines_at_tested_head'], ('Replacement not in final source', index)
    if not item['old_committed_locations']:
        item['old_location_note'] = 'Exact fragment captured between incremental edits; pre-edit line is an observed worktree coordinate, not a Git-base claim.'
print(json.dumps({'entry_count': len(entries), 'entries': entries}, indent=2))
