#!/usr/bin/env python3
import sys
sys.dont_write_bytecode=True
import argparse, hashlib, json, subprocess, tempfile, struct, re
from pathlib import Path
import capture, fixtures as F
ROOT=Path(__file__).resolve().parent
BASE='ecd3938a72cdfce256128c7d437dcd589141baf5'
PREFIX='docs/design/evidence/issue-442/r2-v1-premium/'
def sha(p):return hashlib.sha256(p.read_bytes()).hexdigest()
def run(cmd,cwd=ROOT):
    r=subprocess.run(cmd,cwd=cwd,capture_output=True,text=True);print('COMMAND',cmd,'raw_exit=',r.returncode);print(r.stdout);assert r.returncode==0,r.stderr;return r.stdout

def verify(mirror=None,repro=False):
    paths={p.relative_to(ROOT).as_posix() for p in ROOT.rglob('*') if p.is_file()}
    expected={v[0] for v in capture.SPECS.values()}
    png={p for p in paths if p.lower().endswith('.png')}
    assert png==expected,(png,expected);print('PASS raw canonical PNG count',len(png),sorted(png))
    for name,w,h in capture.SPECS.values():
        assert struct.unpack('>II',(ROOT/name).read_bytes()[16:24])==(w,h);print('PASS dimensions',name,w,h)
    for p in ROOT.rglob('*'):
        assert not any(x in p.name.lower() for x in ['__pycache__','.pyc','.pyo','.ds_store','.bak','.tmp','.swp','preview','scratch','~']),p
    allowed={'art.py','build.py','capture.py','probe.py','verify.py','run-gates.py','fixtures.py','control-input.html','control-sha256.json','protected-control-baseline.txt','day.html','night.html','study.html','README.md','comparison-note.md','PROVENANCE.md','visual-review.md','manifest.sha256'}|expected
    assert allowed-{'manifest.sha256'} <= paths, ('missing supporting artifact',allowed-paths)
    allowed_logs={'build.log','capture.log','browser.log','verify.log','manifest-check.log','exit-codes.json','delivery.log'}
    assert all(p in allowed or (p.startswith('logs/') and p.split('/')[1] in allowed_logs) for p in paths),paths-allowed
    repo=next((p for p in ROOT.parents if (p/'.git').exists()),None)
    assert repo and (repo/'docs/design/evidence/issue-442').exists(),'Run canonical verification from Corral worktree'
    baseline=json.loads((ROOT/'control-sha256.json').read_text())
    tracked=run(['git','ls-tree','-r','--name-only',BASE,'--','docs/design/evidence/issue-442/'],repo).splitlines()
    assert set(tracked)==set(baseline)
    for path,digest in baseline.items():
        original=subprocess.check_output(['git','show',BASE+':'+path],cwd=repo)
        assert hashlib.sha256(original).hexdigest()==digest==sha(repo/path),path
    assert (ROOT/'protected-control-baseline.txt').read_bytes()==subprocess.check_output(['git','ls-tree','-r',BASE,'--','docs/design/evidence/issue-442/'],cwd=repo)
    assert sha(ROOT/'control-input.html')==baseline['docs/design/evidence/issue-442/stage/herd-v1-mocha-blocked-heavy.html']
    assert sha(ROOT/'fixtures.py')==baseline['docs/design/evidence/issue-442/scripts/fixtures.py']
    print('PASS protected control SHA-256 plus Git blobs',len(baseline))
    changed=run(['git','diff','--name-only',BASE+'..HEAD'],repo).splitlines();assert all(p.startswith(PREFIX) for p in changed),changed
    run(['git','diff','--check',BASE+'..HEAD'],repo);print('PASS committed scope')
    for mode in ['day','night']:
        text=(ROOT/(mode+'.html')).read_text()
        for a in F.blocked_heavy().agents:
            assert a.name in text
        assert len(re.findall('data-identity=',text))==12
        import ast
        identities=[ast.literal_eval(v) for v in re.findall(r'data-identity="([^"]+)"',text)]
        for a in F.blocked_heavy().agents:
            assert F.horse_identity(a.name) in identities,a.name
            buttons=re.findall(r'<button class="horse-btn.*?</button>',text,re.S)
            matched=[b for b in buttons if f'>{a.name}</span>' in b]
            assert len(matched)==1,(a.name,len(matched))
            actual=ast.literal_eval(re.search(r'data-identity="([^"]+)"',matched[0]).group(1))
            assert actual==F.horse_identity(a.name),a.name
            assert f'class="state-flag {a.state}"' in matched[0],a.name
        assert not F.scan_forbidden(text)
    day=(ROOT/'day.html').read_text();night=(ROOT/'night.html').read_text()
    assert re.findall(r'<svg class="horse-svg".*?</svg>',day,re.S)==re.findall(r'<svg class="horse-svg".*?</svg>',night,re.S)
    study=(ROOT/'study.html').read_text()
    specimens=re.findall(r'<svg class="horse-svg".*?</svg>',study,re.S)
    assert len(specimens)==4
    assert specimens[0].replace('study0','study3')==specimens[3], 'RM must deliberately select calm standing'
    assert [re.search(r'data-pose="([^"]+)"',s).group(1) for s in specimens]==['stand','shift','graze','stand']
    print('PASS keyed identity/state, identical Day/Night horses, deliberate RM stand')
    # Secret patterns are scanned across all text payloads, not only HTML.
    patterns=[r'gh[pousr]_[A-Za-z0-9]{20,}',r'-----BEGIN [A-Z ]*PRIVATE KEY-----',r'AKIA[A-Z0-9]{16}',r'sk-[A-Za-z0-9]{30,}',r'synergyservices\.co\.th']
    for path in paths:
        if path.endswith('.png') or path in {'verify.py','fixtures.py'}:continue
        text=(ROOT/path).read_text()
        assert not any(re.search(p,text) for p in patterns),path
    print('PASS fictional fixture + private-data/secret-pattern scan; no credentials used')
    if repro:
        with tempfile.TemporaryDirectory(prefix='r2-repro-') as td:
            tmp=Path(td);run([sys.executable,'-B',str(ROOT/'build.py'),'--out',td]);run([sys.executable,'-B',str(ROOT/'capture.py'),'--src',td,'--out',td])
            for name in expected|{'day.html','night.html','study.html'}:
                assert sha(tmp/name)==sha(ROOT/name),name;print('PASS deterministic byte equality',name)
    if mirror:
        entries={}
        for line in (ROOT/'manifest.sha256').read_text().splitlines():
            digest,name=line.split('  ',1);assert name not in entries;entries[name]=digest
        assert set(entries)==paths-{'manifest.sha256','logs/manifest-check.log'}
        assert all(sha(ROOT/name)==digest for name,digest in entries.items())
        print('PASS manifest entries',len(entries),'manifest PNG entries',sum(n.endswith('.png') for n in entries))
        mirror=mirror.resolve();assert mirror!=ROOT.resolve()
        other={p.relative_to(mirror).as_posix() for p in mirror.rglob('*') if p.is_file()};assert other==paths,(paths-other,other-paths)
        assert all(sha(ROOT/p)==sha(mirror/p) for p in paths)
        print('PASS mirror raw path-set and hash equality',len(paths),'files')
    print('PASS hygiene; raw package files',len(paths),'raw PNG count',len(png))
if __name__=='__main__':
    ap=argparse.ArgumentParser();ap.add_argument('--mirror',type=Path);ap.add_argument('--repro',action='store_true');a=ap.parse_args();verify(a.mirror,a.repro)
