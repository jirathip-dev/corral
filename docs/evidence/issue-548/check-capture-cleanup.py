#!/usr/bin/env python3
"""Unit-test the capture driver's cleanup only; no simulator/build is run."""
import ast
import json
from pathlib import Path

source = Path(__file__).with_name('capture-device.py')
main = next(node for node in ast.parse(source.read_text()).body
            if isinstance(node, ast.FunctionDef) and node.name == 'main')
cleanup = next(node for node in main.body if isinstance(node, ast.Try)).finalbody
code = compile(ast.Module(body=cleanup, type_ignores=[]), str(source), 'exec')
for state in ('Booted', 'Shutdown'):
    calls = []

    def simulated_call(*args):
        calls.append(args)
        if args == ('list', 'devices', '--json'):
            return json.dumps({'devices': {'runtime': [
                {'udid': 'owned', 'state': state},
                {'udid': 'unrelated', 'state': 'Booted'}]}})
        return ''

    exec(code, {'call': simulated_call, 'json': json, 'udid': 'owned'})
    expected = [('list', 'devices', '--json')]
    if state == 'Booted':
        expected.append(('shutdown', 'owned'))
    expected.append(('delete', 'owned'))
    assert calls == expected, calls
    print(f'PASS: mocked {state} cleanup {calls}')
print('PASS: both cleanup branches; no real simulator operations')
