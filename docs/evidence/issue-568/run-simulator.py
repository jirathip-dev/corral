#!/usr/bin/env python3
"""Run under flock /tmp/n.lock; owns only the devices in its receipt."""
import argparse
import json
import os
from pathlib import Path
import re
import signal
import subprocess
import threading
import time

ROOT = Path(__file__).resolve().parents[3]
STATE = Path('/tmp/g568-a2-devices.json')
RUNTIME = 'com.apple.CoreSimulator.SimRuntime.iOS-26-5'
TYPES = {'short': 'com.apple.CoreSimulator.SimDeviceType.iPhone-SE-3rd-generation',
         'tall': 'com.apple.CoreSimulator.SimDeviceType.iPhone-16-Pro-Max'}


def run(command, log, timeout=120, on_line=None):
    print('COMMAND ' + json.dumps(command), flush=True)
    with log.open('wb') as output:
        process = subprocess.Popen(command, cwd=ROOT, stdout=subprocess.PIPE if on_line else output, stderr=subprocess.STDOUT,
                                   env={**os.environ, 'HERDR_XCODEBUILD_DIRECT': '1', 'HERMES_SIM_TASK_ACTIVE': '1'},
                                   start_new_session=True)
        errors = []
        def observe():
            try:
                for line in iter(process.stdout.readline, b''):
                    output.write(line)
                    output.flush()
                    on_line(line.decode('utf-8', errors='replace'))
            except Exception as error:
                errors.append(error)
        observer = threading.Thread(target=observe, daemon=True) if on_line else None
        if observer:
            observer.start()
        try:
            code = process.wait(timeout=timeout)
        except subprocess.TimeoutExpired:
            os.killpg(process.pid, signal.SIGTERM)
            try:
                process.wait(timeout=20)
            except subprocess.TimeoutExpired:
                os.killpg(process.pid, signal.SIGKILL)
                process.wait(timeout=10)
            code = 124
        if observer:
            observer.join(timeout=40)
            if observer.is_alive() or errors:
                raise RuntimeError(f'recording observer failed: {errors}')
    print(f'RAW_EXIT={code} LOG={log}', flush=True)
    return code


class CaseRecorder:
    """Record individual real UI cases; wall/media clocks need not be aligned."""
    def __init__(self, device, output):
        self.device, self.output = device, output
        self.process = self.log = self.current = None
        self.receipts = []

    def stop(self, complete=False):
        if self.process is None:
            return
        self.process.send_signal(signal.SIGINT)
        try:
            code = self.process.wait(timeout=30)
        except subprocess.TimeoutExpired:
            self.process.kill()
            code = self.process.wait(timeout=10)
        self.log.close()
        self.current.update(exit=code, complete=complete, stop_epoch=time.time())
        self.receipts.append(self.current)
        self.process = self.log = self.current = None

    def observe(self, line):
        begin = re.search(r'G568_CASE_BEGIN (\S+) epoch=([\d.]+)', line)
        end = re.search(r'G568_CASE_END (\S+) epoch=([\d.]+)', line)
        if begin:
            self.stop()
            name = begin[1]
            assert re.fullmatch(r'[a-z0-9-]+', name)
            video = self.output / (name + '.mov')
            command = ['xcrun', 'simctl', 'io', self.device, 'recordVideo', '--codec', 'h264', '--force', str(video)]
            self.log = (self.output / (name + '-record.log')).open('wb')
            self.current = {'name': name, 'command': command, 'start_epoch': time.time(),
                            'case_epoch': float(begin[2]), 'path': str(video)}
            self.process = subprocess.Popen(command, stdout=self.log, stderr=subprocess.STDOUT)
        elif end and self.current and end[1] == self.current['name']:
            self.stop(complete=True)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--device', choices=TYPES, default='short')
    parser.add_argument('--label', required=True)
    parser.add_argument('--only', action='append', default=[])
    parser.add_argument('--video', action='store_true')
    args = parser.parse_args()
    output = Path('/tmp') / ('g568-a2-' + args.label)
    output.mkdir(exist_ok=False)
    devices = json.loads(STATE.read_text()) if STATE.exists() else {}
    available = json.loads(subprocess.check_output(['xcrun', 'simctl', 'list', 'devices', '--json'], timeout=30))
    known = {d['udid'] for group in available['devices'].values() for d in group}
    if args.device not in devices or devices[args.device] not in known:
        devices[args.device] = subprocess.check_output(['xcrun', 'simctl', 'create', 'g568-a2-' + args.device,
                                                       TYPES[args.device], RUNTIME], timeout=60, text=True).strip()
        STATE.write_text(json.dumps(devices, indent=2) + '\n')
    device = devices[args.device]
    receipt = {'device': device, 'device_type': TYPES[args.device], 'runtime': RUNTIME,
               'label': args.label, 'head': subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=ROOT, text=True).strip(),
               'status': subprocess.check_output(['git', 'status', '--porcelain'], cwd=ROOT, text=True),
               'source_blobs': {path: subprocess.check_output(['git', 'hash-object', path], cwd=ROOT, text=True).strip()
                                for path in ['ios/FleetNotifier/UI/Herd/HerdView.swift',
                                             'ios/FleetNotifierTests/HerdTests.swift',
                                             'ios/FleetNotifierUITests/HerdEdgeGestureTests.swift']}}
    recorder = CaseRecorder(device, output) if args.video else None
    try:
        boot = run(['xcrun', 'simctl', 'boot', device], output / 'boot.log')
        if boot not in (0, 149):
            return boot
        status = run(['xcrun', 'simctl', 'bootstatus', device, '-b'], output / 'bootstatus.log', 180)
        if status:
            return status
        command = ['xcodebuild', '-project', 'ios/FleetNotifier.xcodeproj', '-scheme', 'FleetNotifier',
                   '-configuration', 'Debug', '-destination', 'platform=iOS Simulator,id=' + device,
                   '-derivedDataPath', '/tmp/g568-dd', 'CODE_SIGNING_ALLOWED=NO', '-parallel-testing-enabled', 'NO',
                   '-resultBundlePath', str(output / 'result.xcresult')]
        command += ['-only-testing:' + name for name in (args.only or ['FleetNotifierTests/HerdTests'])]
        command += ['test']
        receipt['command'] = command
        receipt['start_epoch'] = time.time()
        receipt['exit'] = run(command, output / 'test.log', 1200, recorder.observe if recorder else None)
        receipt['end_epoch'] = time.time()
        return receipt['exit']
    finally:
        if recorder is not None:
            recorder.stop()
            receipt['recordings'] = recorder.receipts
        receipt['shutdown_exit'] = run(['xcrun', 'simctl', 'shutdown', device], output / 'shutdown.log')
        (output / 'receipt.json').write_text(json.dumps(receipt, indent=2) + '\n')
        print('RECEIPT ' + str(output / 'receipt.json'), flush=True)


if __name__ == '__main__':
    raise SystemExit(main())
