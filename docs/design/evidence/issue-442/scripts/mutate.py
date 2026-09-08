#!/usr/bin/env python3
"""issue-442 mutation proof — prove verify.py probes BITE.

    PYTHONDONTWRITEBYTECODE=1 python3 scripts/mutate.py

Copies the evidence tree to a temp sandbox, applies ONE deliberate defect
per case, runs verify.py --no-manifest against the sandbox, and requires
the named probe to FAIL (RED). Then re-runs on the untouched tree and
requires VERIFY OK (GREEN). Exit non-zero if any mutant survives.
"""
from __future__ import annotations

import os
import re
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

HERE = Path(__file__).resolve().parent
EVID = HERE.parent
assert EVID.name == "issue-442", EVID


def run_verify(root: Path) -> tuple[int, str]:
    env = dict(os.environ, PYTHONDONTWRITEBYTECODE="1")
    r = subprocess.run([sys.executable, str(root / "scripts" / "verify.py"),
                        "--no-manifest"], capture_output=True, text=True,
                       env=env)
    return r.returncode, r.stdout + r.stderr


def sandbox() -> Path:
    tmp = Path(tempfile.mkdtemp(prefix="m442-"))
    dst = tmp / "issue-442"
    shutil.copytree(EVID, dst, ignore=shutil.ignore_patterns(
        "logs", "__pycache__", "*.pyc"))
    return dst


def sub_all(path: Path, pattern: str, repl: str, count_min: int = 1):
    txt = path.read_text(encoding="utf-8")
    new, n = re.subn(pattern, repl, txt)
    assert n >= count_min, f"mutation anchor missing in {path.name}: {pattern}"
    path.write_text(new, encoding="utf-8")


CASES = []


def case(name, probe):
    def deco(fn):
        CASES.append((name, probe, fn))
        return fn
    return deco


@case("console-error", "P01")
def m_console(sb: Path):
    p = sb / "stage" / "herd-v1-mocha-blocked-heavy.html"
    sub_all(p, r"</body>", "<script>throw new Error('mutant')</script></body>")


@case("horizontal-overflow", "P02")
def m_overflow(sb: Path):
    p = sb / "stage" / "herd-v2-mocha-blocked-heavy.html"
    sub_all(p, r"</body>", '<div style="width:900px;height:2px"></div></body>')


@case("small-tap-target", "P03")
def m_target(sb: Path):
    p = sb / "stage" / "herd-v1-latte-dense-fleet.html"
    sub_all(p, r"\.gear\{width:var\(--tap\);height:var\(--tap\);",
            ".gear{width:20px;height:20px;")


@case("switch-broken", "P04")
def m_switch(sb: Path):
    p = sb / "prototype-v1-pasture-panorama.html"
    sub_all(p, r"state\.mode = sw\.dataset\.switch;", "state.mode = 'herd';")


@case("filters-not-shared", "P05")
def m_filters(sb: Path):
    # Board ignores the repo scope: drop the agent filtering for board mode
    p = sb / "prototype-v2-ranch-map.html"
    # make every board-mode screen the unscoped one by rewriting the lookup
    sub_all(p, r"state\.repo \|\| 'all', sel\]\.join",
            "(state.mode === 'board' ? 'all' : (state.repo || 'all')), sel].join")


@case("detail-sheet-wrong-agent", "P06")
def m_detail(sb: Path):
    p = sb / "prototype-v1-pasture-panorama.html"
    sub_all(p, r"if \(opens\.dataset\.agent\) state\.selected = opens\.dataset\.agent;",
            "if (opens.dataset.agent) state.selected = 'willow-bend';")


@case("blocked-off-rail", "P08")
def m_rail(sb: Path):
    p = sb / "stage" / "herd-v2-mocha-blocked-heavy.html"
    # rename the rail wrapper so blocked horses are no longer inside a rail
    sub_all(p, r'class="front-rail"', 'class="front-rail-x"')


@case("blocked-color-only", "P08")
def m_cue(sb: Path):
    p = sb / "stage" / "herd-v1-mocha-blocked-heavy.html"
    sub_all(p, r'<span class="alert-mark">!</span>blocked</span>',
            '<span class="alert-mark"></span></span>')


@case("disconnected-fake-live", "P09")
def m_disc(sb: Path):
    p = sb / "stage" / "herd-v1-mocha-disconnected.html"
    sub_all(p, r'data-rm="true"', 'data-rm="false"')
    sub_all(p, r'class="phone v1 rm blocked-pulse"', 'class="phone v1 blocked-pulse"')
    sub_all(p, r'data-state="unknown"', 'data-state="working"', 1)


@case("identity-drift", "P10")
def m_ident(sb: Path):
    p = sb / "stage" / "herd-v2-latte-blocked-heavy.html"
    sub_all(p, r'data-coat="black"', 'data-coat="grey"', 1)


@case("png-wrong-size", "P11")
def m_png(sb: Path):
    p = sb / "herd-v1-mocha-blocked-heavy-390x844.png"
    subprocess.run(["sips", "-z", "800", "370", str(p), "--out", str(p)],
                   check=True, capture_output=True)


@case("real-token-leak", "P12")
def m_leak(sb: Path):
    p = sb / "index.html"
    # payload assembled at runtime so this file never carries the token
    leak = "ops" + "@" + "example" + "." + "com"
    sub_all(p, r"</body>", f"<!-- contact: {leak} --></body>")


@case("reduce-motion-still-animates", "P13")
def m_rm(sb: Path):
    # the stage still CLAIMS Reduce Motion (specs.json rm=true) but the
    # root loses its rm class, so the blocked pulse + heartbeat animate again
    p = sb / "stage" / "herd-v2-mocha-reduce-motion.html"
    sub_all(p, r'class="phone v2 rm blocked-pulse"', 'class="phone v2 blocked-pulse"')


@case("label-shrunk", "P15")
def m_label(sb: Path):
    p = sb / "stage" / "herd-v2-mocha-dense-fleet.html"
    sub_all(p, r"\.v2 \.grid \.nameplate\{max-width:84px;font-size:10px;",
            ".v2 .grid .nameplate{max-width:84px;font-size:7px;")


def main() -> int:
    print(f"mutation cases: {len(CASES)}")
    survivors = []
    for name, probe, fn in CASES:
        sb = sandbox()
        try:
            fn(sb)
            rc, out = run_verify(sb)
            bit = rc != 0 and re.search(rf"^FAIL {probe}\b", out, re.M) is not None
            print(f"{'RED  ' if bit else 'ALIVE'} {name:32s} expect {probe} "
                  f"rc={rc}")
            if not bit:
                survivors.append(name)
                print(out[-1200:])
        finally:
            shutil.rmtree(sb.parent, ignore_errors=True)
    rc, out = run_verify(EVID)
    green = rc == 0 and "VERIFY OK" in out
    print(f"{'GREEN' if green else 'NOT GREEN'} untouched tree rc={rc}")
    if survivors or not green:
        print(f"MUTATION FAIL: survivors={survivors} green={green}")
        return 1
    print(f"MUTATION OK: {len(CASES)}/{len(CASES)} mutants killed, "
          f"untouched tree GREEN")
    return 0


if __name__ == "__main__":
    sys.exit(main())
