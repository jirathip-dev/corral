#!/usr/bin/env python3
"""Discrimination probes; run under daemon-run.py's single heavy-gate lease."""
import hashlib
import json
from pathlib import Path
import subprocess
import sys

root = Path(__file__).resolve().parents[3]
assert Path.cwd() == root
assert not subprocess.check_output(['git', 'diff', 'HEAD', '--', 'src', 'tests'])
runner = [sys.executable, str(Path(__file__).with_name('daemon-run.py')), '--locked']
barrier = ['cargo', 'test', '--release', '--lib',
           'core::store::publication_tests::reads_and_initial_sse_do_not_wait_for_writer_or_publisher',
           '--', '--exact', '--nocapture']
bytes_test = ['cargo', 'test', '--release', '--lib',
              'core::store::publication_tests::randomized_publications_match_old_snapshot_and_sse_bytes',
              '--', '--exact', '--nocapture']
bound = ['cargo', 'test', '--release', '--test', 'http',
         'snapshot_returns_json_with_rev_and_agents', '--', '--exact', '--nocapture']
probes = [
    ('mutation-snapshot-flush', 'src/api/mod.rs',
     '    let publication = state.store.published();',
     '    state.store.snapshot().await;\n    let publication = state.store.published();',
     barrier, 'read queued behind writer/publisher'),
    ('mutation-sse-flush', 'src/api/events.rs',
     '    let publication = store.published();',
     '    store.snapshot().await;\n    let publication = store.published();',
     barrier, 'read queued behind writer/publisher'),
    ('mutation-wire-epoch', 'src/core/store/published.rs',
     'serde_json::json!(epoch)', 'serde_json::json!("mutated-epoch")',
     bytes_test, 'snapshot case 0'),
    ('mutation-no-publication', 'src/core/store.rs',
     'published.send_replace(Arc::new(next));', 'drop((published, next));',
     bound, 'snapshot must publish within 3s'),
]
selected = set(sys.argv[1:])
assert selected <= {probe[0] for probe in probes}, selected
if selected:
    probes = [probe for probe in probes if probe[0] in selected]
results = []
for label, name, before, after, command, message in probes:
    path = root / name
    original = path.read_bytes()
    digest = hashlib.sha256(original).hexdigest()
    text = original.decode()
    assert text.count(before) == 1, (name, before)
    try:
        path.write_text(text.replace(before, after))
        result = subprocess.run([*runner, label, *command], timeout=360)
        assert result.returncode == 101, (label, 'gate or admission exit', result.returncode)
        log = Path(f'/tmp/g555-{label}.log').read_text()
        assert result.returncode == 101 and 'panicked at' in log and message in log, label
        results.append(dict(label=label, raw_exit=result.returncode, assertion=message, restored_sha256=digest))
    finally:
        path.write_bytes(original) # fresh mtime forces restoration to rebuild
        assert hashlib.sha256(path.read_bytes()).hexdigest() == digest
    assert not subprocess.check_output(['git', 'diff', 'HEAD', '--', 'src', 'tests'])
for label, command in [
    ('restored-publication', ['cargo', 'test', '--release', '--lib', 'core::store::publication_tests', '--', '--nocapture']),
    ('restored-http-bound', bound),
]:
    result = subprocess.run([*runner, label, *command], timeout=360)
    assert result.returncode == 0, label
selection = '-'.join(sorted(selected)) if selected else 'all'
Path(f'/tmp/g555-mutations-{selection}.json').write_text(json.dumps(results, indent=2)+'\n')
print(json.dumps(results, indent=2))
