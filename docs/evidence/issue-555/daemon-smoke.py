#!/usr/bin/env python3
"""Exercise the built daemon's dedicated HTTP executor, with no live inputs."""
import http.client
import json
import re
from pathlib import Path
import socket
import subprocess
import tempfile
import time

binary = Path('/tmp/g555-daemon-target/release/corrald')
assert binary.is_file()
log_path = Path('/tmp/g555-daemon-smoke-runtime.log')
with tempfile.TemporaryDirectory(prefix='g555-smoke-') as directory:
    root = Path(directory)
    for name in ['repos', 'worktrees', 'bin']:
        (root / name).mkdir()
    with socket.socket() as reservation:
        reservation.bind(('127.0.0.1', 0))
        port = reservation.getsockname()[1]
    # Explicit fixture HOME/config/PATH: no herdr, gh, git CLI or credentials.
    env = {'HOME': directory, 'PATH': str(root / 'bin'), 'RUST_LOG': 'info',
           'CORRAL_CONFIG_DIR': str(root / 'config'),
           'CORRAL_REPO_ROOT': str(root / 'repos'),
           'CORRAL_WORKTREES_ROOT': str(root / 'worktrees')}
    with log_path.open('w') as log:
        child = subprocess.Popen([str(binary), '--port', str(port), '--socket', str(root / 'absent.sock')],
                                 env=env, stdout=log, stderr=subprocess.STDOUT, start_new_session=True)
        try:
            deadline = time.monotonic() + 10
            while True:
                assert child.poll() is None, 'fixture daemon exited'
                try:
                    conn = http.client.HTTPConnection('127.0.0.1', port, timeout=1)
                    conn.request('GET', '/healthz')
                    response = conn.getresponse()
                    assert response.status == 200 and response.read() == b'ok\n'
                    conn.close()
                    break
                except (ConnectionError, TimeoutError, OSError):
                    if time.monotonic() >= deadline:
                        raise
                    time.sleep(0.02)
            results = []
            for route in ['/snapshot', '/host-key', '/events']:
                conn = http.client.HTTPConnection('127.0.0.1', port, timeout=2)
                start = time.monotonic()
                conn.request('GET', route, headers={'Corral-Epoch': 'unknown', 'Last-Event-ID': '0'})
                response = conn.getresponse()
                assert response.status == 200
                if route == '/events':
                    body = bytearray()
                    while not body.endswith(b'\n\n'):
                        part = response.read(1)
                        assert part and len(body) < 65536, 'bounded initial SSE frame'
                        body.extend(part)
                    assert body.startswith(b'event: snapshot\nid: 0\ndata: ')
                    payload = json.loads(body.split(b'data: ', 1)[1])
                    assert payload['epoch'] == snapshot['epoch']
                    assert payload['agents'] == {}
                else:
                    body = response.read()
                    payload = json.loads(body)
                    if route == '/snapshot':
                        snapshot = payload
                        assert snapshot['rev'] == 0 and snapshot['agents'] == {}
                    else:
                        assert payload['algorithm'] == 'X25519'
                results.append({'route': route, 'status': response.status, 'bytes': len(body),
                                'client_ms': (time.monotonic()-start)*1000})
                conn.close()
            print(json.dumps({'fixture_daemon_pid': child.pid, 'routes': results}, indent=2))
        finally:
            child.terminate()
            child.wait(timeout=10)
# The daemon's formatter colors field names even when stdout is redirected.
# Preserve the raw log; remove ANSI sequences only for field assertions.
text = re.sub(r'\x1b\[[0-?]*[ -/]*[@-~]', '', log_path.read_text())
for message in ['snapshot served', 'SSE first frame served', 'serve_ms=', 'buffer_age_ms=']:
    assert message in text, message
print('G555_DAEMON_SMOKE_PASS info timing+age observed; fixture daemon reaped; fixture files removed')
