"""Archive round-2 raw evidence, including failed/blocked attempts."""
import gzip, hashlib, json, re, shutil, subprocess
from pathlib import Path
root=Path('/Users/jirathip/.herdr/worktrees/corral/impl555-daemon')
assert Path.cwd()==root
out=root/'docs/evidence/issue-555/round2'
out.mkdir(exist_ok=True)
assert not list(out.iterdir()), 'never overwrite a previous package'
raw=[]
for source in sorted(Path('/tmp').glob('g555-r2-*')):
    if not source.is_file() or source.suffix not in ('.json','.log','.py'):
        continue
    data=source.read_bytes()
    destination=out/(source.name+'.gz' if source.suffix=='.log' else source.name)
    destination.write_bytes(gzip.compress(data,mtime=0) if source.suffix=='.log' else data)
    raw.append(dict(file=destination.name,raw_bytes=len(data),raw_sha256=hashlib.sha256(data).hexdigest()))
# The reused smoke driver writes this fixed temporary name. Archive the R2 run
# under an R2 name; leave all previously committed R1 evidence untouched.
p=Path('/tmp/g555-daemon-smoke-runtime.log')
data=p.read_bytes()
destination=out/'g555-r2-smoke-daemon.log.gz'
destination.write_bytes(gzip.compress(data,mtime=0))
raw.append(dict(file=destination.name,raw_bytes=len(data),raw_sha256=hashlib.sha256(data).hexdigest()))
receipts={}
for p in sorted(out.glob('*-receipt.json')):
    value=json.loads(p.read_text())
    receipts[value['label']]=value
measurements={}
for p in sorted(out.glob('g555-r2-http-*.json')):
    value=json.loads(p.read_text())
    if 'median_latency_ms' not in value:
        measurements[p.name]={'complete':False}
        continue
    counts={phase:[len(run['phases'][phase]['samples']) for run in value['runs']]
            for phase in ('idle','single','concurrent')}
    assert all(c==[128,128,128] for c in counts.values()),(p,counts)
    measurements[p.name]=dict(complete=True,source_ref=value['source_ref'],binary_sha256=value['binary_sha256'],
        harness_sha256=value['harness_sha256'],sample_counts=counts,median_latency_ms=value['median_latency_ms'],
        info_lines=[run['snapshot_info_lines'] for run in value['runs']],
        buffer_age_ms={phase:[run['phases'][phase]['buffer_age_ms'] for run in value['runs']] for phase in counts})
rows=[tuple(map(int,m)) for m in re.findall(r'test result: .*? (\d+) passed; (\d+) failed; (\d+) ignored',Path('/tmp/g555-r2-workspace.log').read_text())]
summary=dict(source_head=subprocess.check_output(['git','rev-parse','HEAD'],text=True).strip(),
             workspace=dict(zip(('passed','failed','ignored'),map(sum,zip(*rows)))),
             receipts=receipts,measurements=measurements)
(out/'summary.json').write_text(json.dumps(summary,indent=2)+'\n')
(out/'manifest.json').write_text(json.dumps(dict(count=len(raw),files=raw),indent=2)+'\n')
for entry in raw:
    p=out/entry['file']; data=gzip.decompress(p.read_bytes()) if p.suffix=='.gz' else p.read_bytes()
    assert len(data)==entry['raw_bytes'] and hashlib.sha256(data).hexdigest()==entry['raw_sha256']
print(json.dumps(dict(archived_files=len(raw),workspace=summary['workspace'],receipts=len(receipts),
                     complete_matrices=sum(m['complete'] for m in measurements.values()),raw_hash_verification='PASS'),indent=2))
