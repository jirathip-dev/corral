#!/usr/bin/env python3
"""Reproduce the bounded #539 installer proof; all installs are fixtures."""
import gzip
import hashlib
import json
import os
from pathlib import Path
import shlex
import shutil
import subprocess
import tarfile
import tempfile

ROOT = Path(__file__).resolve().parents[3]
OUT = Path(__file__).resolve().parent
BASE = "05dafa0801c264de193e3f518914738c5ed1357d"
SCRIPTS = ["install-corral.sh", "test-install-corral-linux.sh", "setup-corrald.sh",
           "setup-corrald-linux.sh", "update-corral.sh", "lib-corral-update-path.sh",
           "rotate-corral-logs.sh"]
results = []


def run(label, argv, *, cwd=ROOT, env=None, expected=0, timeout=240):
    result = subprocess.run(argv, cwd=cwd, env=env, stdout=subprocess.PIPE,
                            stderr=subprocess.STDOUT, timeout=timeout)
    with gzip.open(OUT / f"{label}.log.gz", "wb") as log:
        log.write(result.stdout)
    record = {"label": label, "command": shlex.join(argv), "cwd": str(cwd),
              "raw_exit": result.returncode, "expected_exit": expected}
    if env is not None:
        record["environment"] = env.copy()
    results.append(record)
    (OUT / "commands.json").write_text(json.dumps(results, indent=2) + "\n")
    print(f"{label}: RAW_EXIT={result.returncode}", flush=True)
    assert result.returncode == expected, result.stdout.decode(errors="replace")[-4000:]
    return result.stdout


def bundle(destination, installer=None):
    with tarfile.open(destination, "w:gz") as tar:
        for name in SCRIPTS:
            tar.add(ROOT / "scripts" / name, arcname="scripts/" + name)
        tar.add(OUT / "bazzite-proof.py", arcname="bazzite-proof.py")


run("syntax", ["/bin/bash", "-c", 'for f in scripts/*.sh; do bash -n "$f" || exit; done; printf "bash -n: all scripts OK\\n"'])
run("shellcheck-installer", ["shellcheck", "scripts/install-corral.sh"])
run("shellcheck-suite", ["shellcheck", "-e", "SC2016", "scripts/test-install-corral-linux.sh"])
with tempfile.TemporaryDirectory(prefix="corral539-local-") as temp:
    work = Path(temp)
    (work / "home").mkdir()
    (work / "tmp").mkdir()
    env = {"PATH": "/usr/bin:/bin", "HOME": str(work / "home"), "TMPDIR": str(work / "tmp"),
           "CORRAL_TEST_EVIDENCE_DIR": str(work / "macos-logs")}
    run("macos-path", ["/bin/bash", "--noprofile", "--norc", "-c", "command -v gh"], env=env, expected=1)
    run("macos", ["/bin/bash", "scripts/test-install-corral-linux.sh"], env=env)
    shutil.copyfile(work / "macos-logs/forbidden.log", OUT / "macos-forbidden.log")
    shutil.copyfile(work / "macos-logs/exits.tsv", OUT / "macos-exits.tsv")
    assert (OUT / "macos-forbidden.log").stat().st_size == 0
    assert not any((work / "tmp").iterdir())

    # The same extended suite must reject the actual base installer, not a fake.
    source = run("base-source", ["git", "show", BASE + ":scripts/install-corral.sh"])
    (work / "base/scripts").mkdir(parents=True)
    for name in SCRIPTS:
        shutil.copyfile(ROOT / "scripts" / name, work / "base/scripts" / name)
    (work / "base/scripts/install-corral.sh").write_bytes(source)
    env["CORRAL_TEST_EVIDENCE_DIR"] = str(work / "base-logs")
    run("base-red", ["/bin/bash", "scripts/test-install-corral-linux.sh"], cwd=work / "base", env=env, expected=1)
    shutil.copyfile(work / "base-logs/forbidden.log", OUT / "base-forbidden.log")
    assert "gh api repos/jirathip-dev/corral/releases/latest" in (OUT / "base-forbidden.log").read_text()
    # No worktree sources were swapped; hash identity stays explicit.
    hashes = {name: hashlib.sha256((ROOT / "scripts" / name).read_bytes()).hexdigest() for name in SCRIPTS}
    (OUT / "tested-scripts.json").write_text(json.dumps(hashes, indent=2) + "\n")

    archive = work / "scripts.tar.gz"
    bundle(archive)
    ssh = ["ssh", "-o", "BatchMode=yes", "-o", "ConnectTimeout=10", "bazzite"]
    remote = run("remote-create", ssh + ["mktemp -d /tmp/corral539-proof-XXXXXXXX"]).decode().strip()
    assert remote.startswith("/tmp/corral539-proof-") and "/" not in remote[len("/tmp/"):]
    remote_q = shlex.quote(remote)
    try:
        run("remote-marker", ssh + [f"touch {remote_q}/.created-for-539"])
        run("remote-copy", ["scp", "-o", "BatchMode=yes", "-o", "ConnectTimeout=10", str(archive), f"bazzite:{remote}/scripts.tar.gz"])
        run("remote-extract", ssh + [f"tar -xzf {remote_q}/scripts.tar.gz -C {remote_q}"])
        # Non-login shell, explicit PATH; live service probes remain read-only.
        proof = subprocess.run(ssh + [f"env PATH=/usr/bin:/bin /usr/bin/python3 {remote_q}/bazzite-proof.py {remote_q}"],
                               cwd=ROOT, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, timeout=300)
        with gzip.open(OUT / "bazzite-driver.log.gz", "wb") as log:
            log.write(proof.stdout)
        results.append({"label": "bazzite-driver", "command": shlex.join(ssh + [f"env PATH=/usr/bin:/bin /usr/bin/python3 {remote_q}/bazzite-proof.py {remote_q}"]), "raw_exit": proof.returncode})
        run("remote-receipt", ["scp", "-o", "BatchMode=yes", "-o", "ConnectTimeout=10", f"bazzite:{remote}/results/isolation.json", str(OUT / "bazzite-isolation.json")])
        for relative, name in [("suite.log", "suite.log"), ("scenario-logs/exits.tsv", "bazzite-exits.tsv"), ("scenario-logs/forbidden.log", "bazzite-forbidden.log")]:
            destination = work / name if name == "suite.log" else OUT / name
            run("remote-fetch-" + name, ["scp", "-o", "BatchMode=yes", "-o", "ConnectTimeout=10", f"bazzite:{remote}/results/{relative}", str(destination)])
        with gzip.open(OUT / "bazzite.log.gz", "wb") as log:
            log.write((work / "suite.log").read_bytes())
        with gzip.open(OUT / "bazzite-isolation.json.gz", "wb") as log:
            log.write((OUT / "bazzite-isolation.json").read_bytes())
        (OUT / "bazzite-isolation.json").unlink()
        assert proof.returncode == 0, proof.stdout.decode(errors="replace")
    finally:
        # The marker is owned by this invocation, and only that exact root goes.
        cleanup = f"import pathlib,shutil; p=pathlib.Path({remote!r}); assert (p/'.created-for-539').is_file(); items=sorted(str(x.relative_to(p)) for x in p.rglob('*')); print('REMOVED_ROOT', p); print('CREATED_AND_REMOVED', items); shutil.rmtree(p); assert not p.exists(); print('CLEANUP_VERIFIED absent')"
        run("remote-cleanup", ssh + ["/usr/bin/python3 -c " + shlex.quote(cleanup)])
        run("remote-cleanup-readback", ssh + [f"test ! -e {remote_q} && systemctl --user is-active corrald"])

run("diff-check", ["git", "diff", "--check"])
print("#539 proof completed; no push/merge/release or live service mutation.")
