#!/usr/bin/env python3
"""Preserve raw logs, then strip display-only trailing spaces for Git's gate."""
import hashlib
import json
from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]
OUT = ROOT/'docs/evidence/issue-560'
assert (OUT/'runs/full-battery.json').exists(), 'wait for the complete battery'
backup = Path('/tmp/g560-raw-logs')
backup.mkdir(mode=0o700, exist_ok=True)
manifest = OUT/'log-hygiene.json'
records = json.loads(manifest.read_text()) if manifest.exists() else []
for record in records:
    assert hashlib.sha256(Path(record['raw_path']).read_bytes()).hexdigest() == record['raw_sha256']
for path in sorted((OUT/'runs').glob('*.log')):
    raw = path.read_bytes()
    normalized = b''.join(line.rstrip(b' \t\r\n')+b'\n' for line in raw.splitlines(keepends=True))
    normalized = normalized.rstrip(b'\n') + b'\n' if normalized else b''
    if raw == normalized:
        continue
    saved = backup/path.name
    assert not saved.exists(), 'never replace original raw evidence'
    saved.write_bytes(raw)
    assert saved.read_bytes() == raw
    path.write_bytes(normalized)
    records.append(dict(path=str(path.relative_to(ROOT)), raw_path=str(saved),
                        raw_sha256=hashlib.sha256(raw).hexdigest(),
                        committed_sha256=hashlib.sha256(normalized).hexdigest(),
                        normalization='only line-ending/trailing whitespace; exit receipts unmodified'))
(OUT/'log-hygiene.json').write_text(json.dumps(records,indent=2)+'\n')
print(json.dumps(records,indent=2))
