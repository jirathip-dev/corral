#!/usr/bin/env python3
"""Build a pinned comparison binary without changing this lane's sources/ref."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tarfile
import tempfile

ROOT = Path(__file__).resolve().parents[3]
TARGET = Path('/tmp/g555-daemon-target')
p = argparse.ArgumentParser()
p.add_argument('ref')
p.add_argument('label')
a = p.parse_args()
assert Path.cwd() == ROOT
assert (TARGET / '.g555-owned').read_text().strip() == str(ROOT)
assert a.label in ('base', 'reviewed', 'mutant', 'logging-red')
ref = subprocess.check_output(['git', 'rev-parse', a.ref], text=True).strip()
with tempfile.TemporaryDirectory(prefix=f'r2-{a.label}-', dir=TARGET) as directory:
    scratch = Path(directory)
    archive = scratch / 'source.tar'
    with archive.open('wb') as out:
        subprocess.run(['git', 'archive', ref], stdout=out, check=True, timeout=30)
    with tarfile.open(archive) as files:
        files.extractall(scratch, filter='data')
    archive.unlink()
    if a.label == 'logging-red':
        test = ROOT / 'tests/read_logging.rs'
        shutil.copyfile(test, scratch / 'tests/read_logging.rs')
        command = ['cargo', 'test', '--release', '--manifest-path', str(scratch/'Cargo.toml'),
                   '--test', 'read_logging', '--', '--nocapture']
        result = subprocess.run(command, env=dict(os.environ, CARGO_TARGET_DIR=str(TARGET)), timeout=500)
        print(json.dumps(dict(source_ref=ref, test_sha256=hashlib.sha256(test.read_bytes()).hexdigest(),
                              command=command, raw_exit=result.returncode)), flush=True)
        sys.exit(result.returncode)
    if a.label == 'mutant':
        path = scratch / 'src/main.rs'
        current = path.read_text()
        reviewed = subprocess.check_output(['git', 'show', '60851ed:src/main.rs'], text=True)
        start = reviewed.index('    // One dedicated HTTP executor,')
        end = reviewed.index('\n}', start)
        old = '    axum::serve(listener, app).await.expect("axum server");'
        assert current.count(old) == 1
        path.write_text(current.replace(old, reviewed[start:end]))
    command = ['cargo', 'build', '--release', '--manifest-path', str(scratch/'Cargo.toml'), '--bin', 'corrald']
    result = subprocess.run(command, env=dict(os.environ, CARGO_TARGET_DIR=str(TARGET)), timeout=500)
    print('BUILD_RAW_EXIT=', result.returncode, flush=True)
    if result.returncode:
        raise SystemExit(result.returncode)
    destination = TARGET / f'r2-{a.label}-corrald'
    shutil.copyfile(TARGET/'release/corrald', destination)
    destination.chmod(0o755)
    receipt = dict(source_ref=ref, mutation='restore only reviewed HTTP executor' if a.label == 'mutant' else None,
                   command=command, raw_exit=result.returncode, binary=str(destination),
                   binary_sha256=hashlib.sha256(destination.read_bytes()).hexdigest())
    Path(f'/tmp/g555-r2-build-{a.label}.json').write_text(json.dumps(receipt, indent=2)+'\n')
    print(json.dumps(receipt, indent=2))
