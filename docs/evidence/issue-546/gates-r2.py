import json
import os
from pathlib import Path
import shlex
import signal
import subprocess
import time

root = Path(__file__).resolve().parents[3]
os.chdir(root)
results = []
head = subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=root, text=True).strip()

def run(name, args, log, timeout=600, cwd=root):
    print(name, shlex.join(args), '>', log, '2>&1', flush=True)
    start = time.monotonic()
    with open(log, 'w') as output:
        process = subprocess.Popen(args, cwd=cwd, stdout=output, stderr=subprocess.STDOUT, start_new_session=True)
        try:
            code = process.wait(timeout=timeout)
            timed_out = False
        except subprocess.TimeoutExpired:
            os.killpg(process.pid, signal.SIGTERM)
            try:
                process.wait(timeout=10)
            except subprocess.TimeoutExpired:
                os.killpg(process.pid, signal.SIGKILL)
                process.wait()
            code = process.returncode
            timed_out = True
    item = dict(head=head, name=name, command=shlex.join(args), cwd=str(cwd), log=log, exit=code,
                timed_out=timed_out, seconds=round(time.monotonic()-start, 2))
    results.append(item)
    Path('/tmp/g546-r2-gates.json').write_text(json.dumps(results, indent=2)+'\n')
    print(f'{name}_EXIT={code} TIMED_OUT={timed_out}', flush=True)
    return code

common = ['flock', '/tmp/n.lock', 'env', 'HERDR_XCODEBUILD_DIRECT=1', 'HERMES_SIM_TASK_ACTIVE=1', 'xcodebuild']
project = ['-project', 'ios/FleetNotifier.xcodeproj', '-scheme', 'FleetNotifier']
dest = ['-destination', 'platform=iOS Simulator,id=59DDC0C5-891E-4EC0-91AF-4F50DF68D793', '-derivedDataPath', '/tmp/g546-dd']
classes = ['PushPayloadTests', 'BackgroundHintTests', 'HostAwarePushModelTests', 'NotificationOptInRuntimeTests', 'NotificationTapDeferredLifecycleTests', 'NotificationEnableModelTests']
if run('FOCUSED', common+['test']+project+dest+['-only-testing:FleetNotifierTests/'+c for c in classes], '/tmp/g546-r2-focused.log'):
    raise SystemExit(1)
run('FULL', common+['test']+project+dest+['-only-testing:FleetNotifierTests'], '/tmp/g546-r2-full.log')
run('CHECK_RELEASE', ['python3', 'ios/check-release-demo.py'], '/tmp/g546-r2-check-release.log')
run('SELF_TEST', ['python3', 'ios/check-release-demo.py', '--self-test'], '/tmp/g546-r2-self-test.log')
run('DEBUG', common+['build']+project+['-configuration', 'Debug', '-destination', 'generic/platform=iOS Simulator', '-derivedDataPath', '/tmp/g546-debug-dd'], '/tmp/g546-r2-debug.log')
run('RELEASE', common+['build']+project+['-configuration', 'Release', '-sdk', 'iphonesimulator', '-destination', 'generic/platform=iOS Simulator', '-derivedDataPath', '/tmp/g546-release-dd', 'CODE_SIGNING_ALLOWED=NO'], '/tmp/g546-r2-release.log')
run('BINARY', ['python3', 'ios/check-release-demo.py', '--binary', '/tmp/g546-release-dd/Build/Products/Release-iphonesimulator/FleetNotifier.app/FleetNotifier'], '/tmp/g546-r2-binary.log')
run('XCODEGEN', ['xcodegen', 'generate', '--spec', 'project.yml'], '/tmp/g546-r2-xcodegen.log', cwd=root/'ios')
run('XCODEGEN_DIFF', ['git', 'diff', '--exit-code', '--', 'ios/'], '/tmp/g546-r2-xcodegen-diff.log')
run('ASLOP', ['swift', 'run', '--package-path', 'ios/tools/anti-slop-swift', 'anti-slop', 'ios/FleetNotifier', 'ios/FleetNotifierTests'], '/tmp/g546-r2-aslop-head.log')
run('DIFFCHECK', ['git', 'diff', '--check'], '/tmp/g546-r2-diffcheck.log')
raise SystemExit(any(r['exit'] != 0 for r in results if r['name'] != 'ASLOP'))
