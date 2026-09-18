#!/usr/bin/env python3
"""Losslessly package the lane's raw logs and record evidence identities."""
import gzip
import hashlib
import json
from pathlib import Path
import subprocess

root = Path(__file__).resolve().parents[3]
out = Path(__file__).resolve().parent
sha = lambda data: hashlib.sha256(data).hexdigest()
base = '4cc316599b0ca1d69039aceb7dbb44193be48e52'
old = subprocess.check_output(['git', 'show', f'{base}:.report.md'], cwd=root)
assert (root / '.report.md').read_bytes().startswith(old)
manifest = {
    'source_head': subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=root, text=True).strip(),
    'base': base,
    'archive_prefix_bytes': len(old),
    'archive_prefix_sha256': sha(old),
    'sources': {},
    'logs': {},
}
for name in ['src/adapters/git_plane.rs', 'src/adapters/git_plane_issue561_tests.rs']:
    data = (root / name).read_bytes()
    manifest['sources'][name] = {'sha256': sha(data), 'lines': len(data.splitlines())}
for path in sorted(out.glob('*.log')):
    data = path.read_bytes()
    packed = gzip.compress(data, mtime=0)
    compressed = path.with_suffix('.log.gz')
    compressed.write_bytes(packed)
    assert gzip.decompress(packed) == data
    manifest['logs'][path.name] = {
        'raw_bytes': len(data), 'raw_lines': len(data.splitlines()),
        'raw_sha256': sha(data), 'compressed': compressed.name,
        'compressed_sha256': sha(packed),
    }
(out / 'artifact-manifest.json').write_text(json.dumps(manifest, indent=2) + '\n')
print(json.dumps({'source_head': manifest['source_head'], 'raw_logs': len(manifest['logs']),
                  'archive_prefix_bytes': len(old), 'sources': manifest['sources']}, indent=2))
