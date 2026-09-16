#!/usr/bin/env python3
"""Three bounded one-defect RED/GREEN probes; invoke under the heavy lock.
Requires pristine committed source files. Every restore writes the saved bytes
(new mtime), executes shasum, and compares the working file to Git HEAD.
"""
import json
import os
from pathlib import Path
import subprocess
import sys

runner = str(Path(__file__).with_name('run-command.py'))
acquire = '''    let _permit = budget
        .acquire_owned()
        .await
        .map_err(|_| ProbeError::Git("git command budget closed".into()))?;
    match tokio::time::timeout(EVENT_BUDGET, probe).await {'''
mutations = [
    ('poison', 'src/adapters/git_plane.rs',
     'self.state.lock().unwrap_or_else(|error| error.into_inner())',
     'self.state.lock().unwrap()', 'g492_', 'PoisonError'),
    ('permit', 'src/adapters/git_plane.rs', acquire,
     '''    match tokio::time::timeout(EVENT_BUDGET, async {
        let _permit = budget.acquire_owned().await
            .map_err(|_| ProbeError::Git("git command budget closed".into()))?;
        probe.await
    }).await {''', 'g492_permit_queue_is_not_execution_budget_and_sweep_is_paced',
     'permit wait was incorrectly charged to the execution budget'),
    ('staleness', 'src/core/store.rs',
     'stale: !alive || fact_age_ms.is_none_or(|age| age >= 120_000),',
     'stale: !alive,', 'g492_snapshot_marks_old_absent_and_stopped_facts',
     'old git facts must never be marked current'),
]
results = []
for name, filename, old, new, test, signature in mutations:
    path = Path(filename)
    subprocess.run(['git', 'diff', '--exit-code', 'HEAD', '--', filename], check=True)
    pristine = path.read_bytes()
    assert pristine.count(old.encode()) == 1, f'non-unique mutation: {name}'
    before = subprocess.check_output(['shasum', '-a', '256', filename], text=True).strip()
    command = ['cargo', 'test', '-p', 'corrald', '--lib', test, '--', '--nocapture']
    try:
        path.write_bytes(pristine.replace(old.encode(), new.encode(), 1))
        red = subprocess.call([sys.executable, runner, f'mutation-{name}-red', '1800', *command])
        text = Path(f'/tmp/g492-mutation-{name}-red.log').read_text()
        assert red == 101 and 'test result: FAILED' in text and signature in text, f'not a behavioral RED: {name}'
    finally:
        path.write_bytes(pristine)
        os.utime(path, None)
        after = subprocess.check_output(['shasum', '-a', '256', filename], text=True).strip()
        assert before == after, f'byte restore failed: {name}'
        diff = subprocess.call([sys.executable, runner, f'mutation-{name}-restore', '60',
                                'git', 'diff', '--exit-code', 'HEAD', '--', filename])
        assert diff == 0
        print(f'{name} BEFORE {before}\n{name} AFTER  {after}', flush=True)
    green = subprocess.call([sys.executable, runner, f'mutation-{name}-green', '1800', *command])
    assert green == 0, f'restored source is not GREEN: {name}'
    results.append(dict(mutation=name, path=filename, test=test, red=red, green=green,
                        expected_failure=signature, before=before, after=after, restore_diff=diff))
    Path('/tmp/g492-mutations.json').write_text(json.dumps(results,indent=2)+'\n')
print('G492_MUTATION_BATTERY_COMPLETE',flush=True)
