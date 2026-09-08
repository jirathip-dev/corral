#!/usr/bin/env python3
"""issue-442 verify — THE GATE. DOM/interaction probes + static probes +
SHA-256 manifest. Exit non-zero on any finding.

    PYTHONDONTWRITEBYTECODE=1 python3 scripts/verify.py           # full
    PYTHONDONTWRITEBYTECODE=1 python3 scripts/verify.py --no-manifest

Interaction probes run in chrome-headless-shell over the raw DevTools
protocol (stdlib websocket-free: --remote-debugging-pipe is not exposed by
this build, so we use --remote-debugging-port + a tiny WebSocket client).
Static probes read the HTML/PNG bytes directly.

Probes (each must BITE — see verify-mutation.log for the RED/GREEN proof):
  P01 console: zero console errors / uncaught exceptions on every page
  P02 overflow: document.scrollWidth <= 390 on every 390x844 stage
  P03 targets: every interactive control >= 44x44 CSS px (buttons, options)
  P04 switch: Board<->Herd segmented control flips the rendered mode
  P05 filters: applying a repo scope in Herd is reflected in Board (shared)
  P06 detail: tapping a horse opens the sheet that names the SAME agent,
              and the Board row for that agent opens the same sheet
  P07 states: working/blocked/idle/done/unknown all present in blocked-heavy
  P08 rail: blocked horses are inside the front-rail region AND carry a
              text/glyph cue (alert mark + 'blocked' label), never color-only
  P09 disconnected: outage frame shows explicit banner + card, all horses
              unknown, no running animations (RM forced), no live counts
  P10 identity: horse identity attributes are identical across states,
              palettes, and variants for the same agent name
  P11 dimensions: every PNG is exactly 390x844 and count >= 12 required set
  P12 fictional: no forbidden strings in any artifact (data URIs stripped)
  P13 reduce-motion: with rm the working glyph is the static dot and no
              element reports a running animation
  P14 self-contained: no http(s) URLs loaded by the pages (file:// only)
  P15 labels: every .nameplate has non-empty text and computed font-size
              >= 10px (never shrunk below the documented floor)
"""
from __future__ import annotations

import base64
import glob
import hashlib
import json
import os
import re
import socket
import struct
import subprocess
import sys
import tempfile
import time
import urllib.request
from pathlib import Path

HERE = Path(__file__).resolve().parent
EVID = HERE.parent
assert EVID.name == "issue-442", EVID
sys.path.insert(0, str(HERE))
import fixtures as F  # noqa: E402

W, H = 390, 844
FINDINGS: list[str] = []
PASSES: list[str] = []


def ok(msg):
    PASSES.append(msg)
    print("PASS", msg)


def fail(msg):
    FINDINGS.append(msg)
    print("FAIL", msg)


# ------------------------------------------------------------ CDP client ---

class WS:
    """Minimal RFC6455 client for localhost DevTools (text frames only)."""

    def __init__(self, url: str):
        m = re.match(r"ws://([^:/]+):(\d+)(/.*)", url)
        host, port, path = m.group(1), int(m.group(2)), m.group(3)
        self.s = socket.create_connection((host, port), timeout=30)
        key = base64.b64encode(os.urandom(16)).decode()
        req = (f"GET {path} HTTP/1.1\r\nHost: {host}:{port}\r\n"
               f"Upgrade: websocket\r\nConnection: Upgrade\r\n"
               f"Sec-WebSocket-Key: {key}\r\nSec-WebSocket-Version: 13\r\n\r\n")
        self.s.sendall(req.encode())
        buf = b""
        while b"\r\n\r\n" not in buf:
            buf += self.s.recv(4096)
        assert b" 101 " in buf.split(b"\r\n")[0], buf[:200]
        self.buf = buf.split(b"\r\n\r\n", 1)[1]
        self.id = 0

    def send(self, obj):
        data = json.dumps(obj).encode()
        hdr = bytearray([0x81])
        n = len(data)
        if n < 126:
            hdr.append(0x80 | n)
        elif n < 65536:
            hdr.append(0x80 | 126)
            hdr += struct.pack(">H", n)
        else:
            hdr.append(0x80 | 127)
            hdr += struct.pack(">Q", n)
        mask = os.urandom(4)
        hdr += mask
        masked = bytes(b ^ mask[i % 4] for i, b in enumerate(data))
        self.s.sendall(bytes(hdr) + masked)

    def _read_exact(self, n):
        while len(self.buf) < n:
            chunk = self.s.recv(65536)
            if not chunk:
                raise ConnectionError("ws closed")
            self.buf += chunk
        out, self.buf = self.buf[:n], self.buf[n:]
        return out

    def recv(self):
        while True:
            b1, b2 = self._read_exact(2)
            op = b1 & 0x0F
            n = b2 & 0x7F
            if n == 126:
                n = struct.unpack(">H", self._read_exact(2))[0]
            elif n == 127:
                n = struct.unpack(">Q", self._read_exact(8))[0]
            if b2 & 0x80:
                self._read_exact(4)
            payload = self._read_exact(n)
            if op == 1:
                return json.loads(payload.decode())
            if op == 8:
                raise ConnectionError("ws close")
            # ping/pong/binary: ignore

    def call(self, method, **params):
        self.id += 1
        mid = self.id
        self.send({"id": mid, "method": method, "params": params})
        events = []
        while True:
            msg = self.recv()
            if msg.get("id") == mid:
                if "error" in msg:
                    raise RuntimeError(f"{method}: {msg['error']}")
                return msg.get("result", {}), events
            events.append(msg)

    def drain(self, t=0.25):
        """Collect events for t seconds (console messages etc.)."""
        out = []
        self.s.settimeout(t)
        try:
            while True:
                out.append(self.recv())
        except (socket.timeout, TimeoutError):
            pass
        finally:
            self.s.settimeout(30)
        return out


class Browser:
    def __init__(self):
        self.shell = self._find_shell()
        self.port = self._free_port()
        self.udd = tempfile.mkdtemp(prefix="v442-")
        self.proc = subprocess.Popen(
            [self.shell, "--headless", "--disable-gpu", "--no-first-run",
             "--hide-scrollbars", f"--user-data-dir={self.udd}",
             f"--remote-debugging-port={self.port}",
             f"--window-size={W},{H}", "about:blank"],
            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        for _ in range(100):
            try:
                with urllib.request.urlopen(
                        f"http://127.0.0.1:{self.port}/json/version",
                        timeout=1) as r:
                    json.load(r)
                break
            except Exception:
                time.sleep(0.1)
        else:
            raise RuntimeError("devtools did not come up")
        with urllib.request.urlopen(
                f"http://127.0.0.1:{self.port}/json", timeout=5) as r:
            targets = json.load(r)
        page = next(t for t in targets if t["type"] == "page")
        self.ws = WS(page["webSocketDebuggerUrl"])
        self.ws.call("Runtime.enable")
        self.ws.call("Log.enable")
        self.ws.call("Page.enable")
        self.ws.call("Emulation.setDeviceMetricsOverride", width=W, height=H,
                     deviceScaleFactor=2, mobile=True)
        self.console: list[dict] = []

    @staticmethod
    def _find_shell():
        env = os.environ.get("CHROME_HEADLESS_SHELL")
        if env and Path(env).exists():
            return env
        pats = sorted(glob.glob(os.path.expanduser(
            "~/Library/Caches/ms-playwright/chromium_headless_shell-*/"
            "chrome-headless-shell-mac-arm64/chrome-headless-shell")),
            key=lambda p: os.stat(p).st_mtime, reverse=True)
        if not pats:
            sys.exit("chrome-headless-shell not found")
        return pats[0]

    @staticmethod
    def _free_port():
        s = socket.socket()
        s.bind(("127.0.0.1", 0))
        p = s.getsockname()[1]
        s.close()
        return p

    def _absorb(self, events):
        for e in events:
            m = e.get("method")
            if m == "Runtime.consoleAPICalled" and e["params"]["type"] in ("error", "assert"):
                self.console.append(e["params"])
            elif m == "Runtime.exceptionThrown":
                self.console.append(e["params"])
            elif m == "Log.entryAdded" and e["params"]["entry"]["level"] == "error":
                self.console.append(e["params"]["entry"])

    def goto(self, path: Path):
        _, ev = self.ws.call("Page.navigate", url=f"file://{path}")
        self._absorb(ev)
        # wait for load
        deadline = time.time() + 10
        while time.time() < deadline:
            ev = self.ws.drain(0.2)
            self._absorb(ev)
            if any(e.get("method") == "Page.loadEventFired" for e in ev):
                break
        self._absorb(self.ws.drain(0.3))

    def js(self, expr: str):
        r, ev = self.ws.call("Runtime.evaluate", expression=expr,
                             returnByValue=True, awaitPromise=True)
        self._absorb(ev)
        self._absorb(self.ws.drain(0.05))
        if "exceptionDetails" in r:
            raise RuntimeError(r["exceptionDetails"].get("text"))
        return r.get("result", {}).get("value")

    def click(self, selector: str):
        return self.js(f"""(() => {{
            const el = document.querySelector({json.dumps(selector)});
            if (!el) return 'MISSING';
            el.click(); return 'clicked';
        }})()""")

    def close(self):
        try:
            self.proc.terminate()
            self.proc.wait(timeout=5)
        except Exception:
            self.proc.kill()


# ------------------------------------------------------------- helpers ----

JS_TARGETS = """(() => {
  const sel = 'button, [role="button"], a[href], .opt, .horse-btn';
  const bad = [];
  document.querySelectorAll(sel).forEach(el => {
    if (!el.offsetParent && getComputedStyle(el).position !== 'fixed') return;
    const r = el.getBoundingClientRect();
    if (r.width === 0 && r.height === 0) return;
    if (r.width < 44 - 0.5 || r.height < 44 - 0.5)
      bad.push(`${el.className || el.tagName} ${Math.round(r.width)}x${Math.round(r.height)} '${(el.textContent||'').trim().slice(0,24)}'`);
  });
  return bad;
})()"""

JS_ANIMS = """(() => {
  const running = document.getAnimations().filter(a => a.playState === 'running');
  return running.length;
})()"""

JS_NAMEPLATES = """(() => {
  const out = [];
  document.querySelectorAll('.nameplate').forEach(n => {
    const fs = parseFloat(getComputedStyle(n).fontSize);
    out.push({text: n.textContent.trim(), fs});
  });
  return out;
})()"""


def sha256(p: Path) -> str:
    h = hashlib.sha256()
    with open(p, "rb") as fh:
        for chunk in iter(lambda: fh.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


def png_size(path: Path):
    with open(path, "rb") as fh:
        head = fh.read(24)
    if head[:8] != b"\x89PNG\r\n\x1a\n":
        return None
    return struct.unpack(">II", head[16:24])


# -------------------------------------------------------------- probes ----

def probe_stages(b: Browser):
    specs = json.loads((EVID / "stage" / "specs.json").read_text())
    seen_states: set[str] = set()
    identity: dict[str, str] = {}
    for spec in specs:
        name = spec["name"]
        b.console.clear()
        b.goto(EVID / "stage" / f"{name}.html")
        # P02 overflow
        sw = b.js("Math.max(document.documentElement.scrollWidth, document.body.scrollWidth)")
        if sw > W:
            fail(f"P02 overflow {name}: scrollWidth {sw} > {W}")
        # P03 targets
        bad = b.js(JS_TARGETS)
        if bad:
            fail(f"P03 targets {name}: {len(bad)} < 44px: {bad[:4]}")
        # P15 nameplates
        nps = b.js(JS_NAMEPLATES)
        small = [n for n in nps if n["fs"] < 10 or not n["text"]]
        if small:
            fail(f"P15 labels {name}: {small[:3]}")
        # P10 identity (attributes per agent must be constant everywhere)
        ids = b.js("""(() => { const m = {};
          document.querySelectorAll('.horse-btn').forEach(bt => {
            const g = bt.querySelector('g.horse');
            if (!g) return;
            m[bt.dataset.agent] = [g.dataset.coat, g.dataset.breed, g.dataset.mane,
                                   g.dataset.tack, g.dataset.accessory].join('/');
          }); return m; })()""")
        for agent, sig in ids.items():
            if agent in identity and identity[agent] != sig:
                fail(f"P10 identity drift {agent}: {identity[agent]} vs {sig} in {name}")
            identity.setdefault(agent, sig)
        # P07 states
        for st in b.js("Array.from(document.querySelectorAll('.horse-btn')).map(b => b.dataset.state)"):
            seen_states.add(st)
        # P08 front rail: every blocked horse inside a rail region + text cue
        if spec["frame"] != "disconnected" and spec["mode"] == "herd":
            res = b.js("""(() => {
              const out = {blocked: 0, inRail: 0, cue: 0};
              document.querySelectorAll('.horse-btn[data-state="blocked"]').forEach(bt => {
                out.blocked++;
                if (bt.closest('.rail-row, .front-rail')) out.inRail++;
                const flag = bt.querySelector('.state-flag.blocked');
                if (flag && /blocked/.test(flag.textContent) && flag.querySelector('.alert-mark')) out.cue++;
              });
              return out; })()""")
            if res["blocked"] == 0 or res["inRail"] != res["blocked"] or res["cue"] != res["blocked"]:
                fail(f"P08 rail {name}: {res}")
        # P09 disconnected treatment
        if spec["frame"] == "disconnected":
            res = b.js("""(() => ({
              banner: !!document.querySelector('.stale-bar') && /Disconnected/.test(document.querySelector('.stale-bar').textContent),
              card: !!document.querySelector('.outage[role="alert"]') && /Source disconnected/.test(document.querySelector('.outage').textContent),
              allUnknown: Array.from(document.querySelectorAll('.horse-btn')).every(b => b.dataset.state === 'unknown'),
              horses: document.querySelectorAll('.horse-btn').length,
              rm: document.querySelector('.phone').dataset.rm,
              anims: document.getAnimations().filter(a => a.playState === 'running').length,
              liveCounts: !!document.querySelector('.rail-summary')
            }))()""")
            if not (res["banner"] and res["card"] and res["allUnknown"]
                    and res["horses"] >= 4 and res["anims"] == 0
                    and not res["liveCounts"]):
                fail(f"P09 disconnected {name}: {res}")
        # P13 reduce motion
        if spec["rm"]:
            res = b.js("""(() => ({
              anims: document.getAnimations().filter(a => a.playState === 'running').length,
              hbHidden: Array.from(document.querySelectorAll('.hb')).every(e => getComputedStyle(e).display === 'none'),
              hbStatic: Array.from(document.querySelectorAll('.hb-static')).some(e => getComputedStyle(e).display !== 'none'),
              rmHorses: document.querySelectorAll('g.horse.horse-rm').length
            }))()""")
            if res["anims"] != 0 or not res["hbHidden"] or not res["hbStatic"] or res["rmHorses"] == 0:
                fail(f"P13 reduce-motion {name}: {res}")
        else:
            if spec["frame"] != "disconnected":
                n = b.js(JS_ANIMS)
                if n == 0:
                    fail(f"P13 motion {name}: expected running animations, got 0")
        # P01 console
        if b.console:
            fail(f"P01 console {name}: {len(b.console)} error(s): "
                 f"{str(b.console[0])[:200]}")
    missing = set(F.STATES) - seen_states
    if missing:
        fail(f"P07 states missing across stages: {sorted(missing)}")
    else:
        ok(f"P07 states present across stages: {sorted(seen_states)}")
    ok(f"P10 identity stable for {len(identity)} agents across "
       f"{len(specs)} stages")
    ok(f"stages probed: {len(specs)} (P01/P02/P03/P08/P09/P13/P15)")


def probe_interactive(b: Browser, variant: str):
    page = EVID / f"prototype-{'v1-pasture-panorama' if variant == 'v1' else 'v2-ranch-map'}.html"
    b.console.clear()
    b.goto(page)
    # P14 self-contained (no external requests)
    ext = b.js("""performance.getEntriesByType('resource').filter(r => /^https?:/.test(r.name)).map(r => r.name)""")
    if ext:
        fail(f"P14 external {variant}: {ext[:3]}")
    else:
        ok(f"P14 self-contained {variant}: 0 external resources")
    mode = b.js("document.querySelector('#app .phone').dataset.mode")
    if mode != "herd":
        fail(f"P04 {variant}: initial mode {mode}")
    # P04 switch
    b.click('#app [data-switch="board"]')
    m2 = b.js("document.querySelector('#app .phone').dataset.mode")
    has_rows = b.js("document.querySelectorAll('#app .b-row').length")
    b.click('#app [data-switch="herd"]')
    m3 = b.js("document.querySelector('#app .phone').dataset.mode")
    horses = b.js("document.querySelectorAll('#app .horse-btn').length")
    if m2 == "board" and has_rows > 0 and m3 == "herd" and horses > 0:
        ok(f"P04 switch {variant}: herd->board({has_rows} rows)->herd({horses} horses)")
    else:
        fail(f"P04 switch {variant}: {m2}/{has_rows}/{m3}/{horses}")
    # P05 shared filters: open Filters in Herd, pick a repo, switch to Board
    b.click('#app [data-opens="filter-sheet"]')
    sheet = b.js("!!document.querySelector('#app [data-sheet=\"filter\"]')")
    if not sheet:
        fail(f"P05 {variant}: filter sheet did not open")
    picked = b.js("""(() => {
      const opts = Array.from(document.querySelectorAll('#app .opt'));
      const o = opts.find(o => o.querySelector('.chip') && /atlas-vector/.test(o.textContent));
      if (!o) return null; o.click(); return 'atlas-vector'; })()""")
    scope_h = b.js("document.querySelector('#app .scope b').textContent")
    herd_repos = b.js("Array.from(new Set(Array.from(document.querySelectorAll('#app .horse-btn')).map(b => b.dataset.repo)))")
    b.click('#app [data-switch="board"]')
    scope_b = b.js("document.querySelector('#app .scope b').textContent")
    board_repos = b.js("Array.from(new Set(Array.from(document.querySelectorAll('#app .b-sub')).map(b => b.textContent.trim())))")
    fbtn = b.js("document.querySelector('#app [data-opens=\"filter-sheet\"]').textContent.trim()")
    if (picked and "atlas-vector" in scope_h and scope_h == scope_b
            and herd_repos == ["atlas-vector"] and board_repos == ["atlas-vector"]
            and "1" in fbtn):
        ok(f"P05 shared filters {variant}: Herd scope '{scope_h}' == Board scope; "
           f"both show only atlas-vector; Filters button '{fbtn}'")
    else:
        fail(f"P05 shared filters {variant}: picked={picked} herd='{scope_h}' "
             f"board='{scope_b}' herd_repos={herd_repos} board_repos={board_repos} btn='{fbtn}'")
    # reset filter via All repositories
    b.click('#app [data-opens="filter-sheet"]')
    b.js("""(() => { const o = Array.from(document.querySelectorAll('#app .opt')).find(o => /^All repositories/.test(o.textContent.trim())); o && o.click(); })()""")
    b.click('#app [data-switch="herd"]')
    # P06 horse -> detail sheet == board row -> same sheet
    target = b.js("""(() => { const bt = document.querySelector('#app .horse-btn[data-state="blocked"]'); if (!bt) return null; const n = bt.dataset.agent; bt.click(); return n; })()""")
    sheet_agent = b.js("""(() => { const s = document.querySelector('#app [data-sheet="detail"]'); return s ? s.querySelector('h2').textContent.replace(/^.*?(blocked|working|idle|done|unknown)/, '').trim() : null; })()""")
    sheet_note = b.js("""(() => { const s = document.querySelector('#app [data-sheet="detail"] .same'); return s ? s.textContent : ''; })()""")
    b.click('#app [data-closes="detail-sheet"]')
    b.click('#app [data-switch="board"]')
    row_agent = b.js(f"""(() => {{ const r = document.querySelector('#app .b-row[data-agent="{target}"]'); if (!r) return null; r.click(); return r.dataset.agent; }})()""")
    sheet_agent2 = b.js("""(() => { const s = document.querySelector('#app [data-sheet="detail"]'); return s ? s.querySelector('h2').textContent.replace(/^.*?(blocked|working|idle|done|unknown)/, '').trim() : null; })()""")
    if target and sheet_agent == target and row_agent == target and sheet_agent2 == target and "Same sheet the Board row opens" in sheet_note:
        ok(f"P06 detail path {variant}: horse '{target}' -> sheet '{sheet_agent}'; Board row -> sheet '{sheet_agent2}'")
    else:
        fail(f"P06 detail path {variant}: target={target} sheet={sheet_agent} row={row_agent} sheet2={sheet_agent2}")
    b.click('#app [data-closes="detail-sheet"]')
    # V1 paging / V2 collapse+more (navigation instead of shrinking)
    b.click('#app [data-switch="herd"]')
    b.click('.ctl [data-set="frame=dense-fleet"]')
    if variant == "v1":
        pages = b.js("document.querySelectorAll('#app .paddock').length")
        dots = b.js("document.querySelectorAll('#app .dots i').length")
        if pages >= 4 and dots == pages:
            ok(f"P-nav v1: {pages} paddock pages, {dots} page dots (swipe navigation)")
        else:
            fail(f"P-nav v1: pages={pages} dots={dots}")
    else:
        more = b.js("Array.from(document.querySelectorAll('#app .more')).map(m => m.textContent.trim())")
        if more and all("Show all" in m for m in more):
            ok(f"P-nav v2: overflow rows {more}")
        else:
            fail(f"P-nav v2: no Show-all overflow rows in dense fleet: {more}")
    # P01
    if b.console:
        fail(f"P01 console {variant} interactive: {str(b.console[0])[:200]}")
    else:
        ok(f"P01 console {variant} interactive: 0 errors")


def probe_static():
    specs = json.loads((EVID / "stage" / "specs.json").read_text())
    pngs = sorted(EVID.glob("*-390x844.png"))
    required = {f"herd-{v}-{p}-{fr}-390x844.png"
                for v in ("v1", "v2") for p in ("mocha", "latte")
                for fr in ("blocked-heavy", "dense-fleet", "disconnected")}
    names = {p.name for p in pngs}
    miss = required - names
    if miss:
        fail(f"P11 required PNGs missing: {sorted(miss)}")
    bad = [(p.name, png_size(p)) for p in pngs if png_size(p) != (W, H)]
    if bad:
        fail(f"P11 dimensions: {bad}")
    if len(pngs) != len(specs):
        fail(f"P11 count: {len(pngs)} PNGs vs {len(specs)} stage specs")
    if not miss and not bad and len(pngs) == len(specs):
        ok(f"P11 dimensions: {len(pngs)} PNGs all exactly {W}x{H}; "
           f"12/12 required present")
    # P12 fictional / forbidden
    hits = []
    for p in list(EVID.glob("*.html")) + list((EVID / "stage").glob("*.html")) \
            + list(EVID.glob("*.md")) + list(HERE.glob("*.py")):
        if p.name in ("verify.py", "fixtures.py"):
            continue   # the scanner + its pattern source are not artifacts
        txt = p.read_text(encoding="utf-8", errors="replace")
        for h in F.scan_forbidden(txt):
            hits.append((p.name, h))
    if hits:
        fail(f"P12 forbidden strings: {hits[:6]}")
    else:
        ok("P12 fictional-only: 0 forbidden strings across HTML/MD/PY")
    # P14 static: no http(s) src/href in stage/prototype HTML
    ext = []
    for p in list(EVID.glob("prototype-*.html")) + list((EVID / "stage").glob("*.html")) + [EVID / "sprite-sheet.html"]:
        txt = p.read_text(encoding="utf-8")
        for m in re.finditer(r'(?:src|href)="(https?://[^"]+)"', txt):
            ext.append((p.name, m.group(1)))
    # index.html links to github-served README only relatively; allow none
    if ext:
        fail(f"P14 static external refs: {ext[:4]}")
    else:
        ok("P14 static: no http(s) src/href in prototypes/stages")


# ------------------------------------------------------------ manifest ----

def write_manifest():
    files = []
    for p in sorted(EVID.rglob("*")):
        if p.is_dir():
            continue
        rel = p.relative_to(EVID).as_posix()
        if rel.startswith("logs/") or rel == "manifest.sha256":
            continue
        if "__pycache__" in rel or rel.endswith((".pyc", ".DS_Store")):
            fail(f"transient artifact present: {rel}")
            continue
        files.append((sha256(p), rel))
    out = EVID / "manifest.sha256"
    out.write_text("".join(f"{h}  {rel}\n" for h, rel in files),
                   encoding="utf-8")
    ok(f"manifest: {len(files)} files -> manifest.sha256 "
       f"(manifest sha256 {sha256(out)})")
    return out


# ---------------------------------------------------------------- main -----

def main() -> int:
    no_manifest = "--no-manifest" in sys.argv
    probe_static()
    b = Browser()
    try:
        probe_stages(b)
        probe_interactive(b, "v1")
        probe_interactive(b, "v2")
    finally:
        b.close()
    if not no_manifest and not FINDINGS:
        write_manifest()
    print(f"\n{len(PASSES)} pass, {len(FINDINGS)} findings")
    if FINDINGS:
        print("VERIFY FAIL")
        for f_ in FINDINGS:
            print("  -", f_)
        return 1
    print("VERIFY OK")
    return 0


if __name__ == "__main__":
    sys.exit(main())
