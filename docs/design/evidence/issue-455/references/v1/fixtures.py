#!/usr/bin/env python3
"""issue-442 fixtures — ONE fictional Corral fleet dataset (self-checking).

Fictional-only rule: every repository, agent, host, branch, and output line
in this module is invented for the design prototype. Emails/domains, if any,
end in `.invalid`. FORBIDDEN_STRINGS lists real-world tokens that must never
appear in generated artifacts; verify.py re-uses this list.

The fixture self-checks its own invariants at import time so a broken edit
fails loudly at build time, not silently in a PNG.
"""
from __future__ import annotations

import fnmatch
import hashlib
import re
from dataclasses import dataclass, field

# ---------------------------------------------------------------- states ----
# Locked herdr raw vocabulary (StateStyle.swift): label + mark glyphs.
# Colors come from the Catppuccin state map in build.py (single shared map).
STATES = ("working", "blocked", "idle", "done", "unknown")

# ------------------------------------------------------------- palettes -----
# Verbatim from ios/FleetNotifier/UI/AppTheme.swift (locked #372 tables).
PALETTES = {
    "mocha": {
        "rosewater": "#f5e0dc", "flamingo": "#f2cdcd", "pink": "#f5c2e7",
        "mauve": "#cba6f7", "red": "#f38ba8", "maroon": "#eba0ac",
        "peach": "#fab387", "yellow": "#f9e2af", "green": "#a6e3a1",
        "teal": "#94e2d5", "sky": "#89dceb", "sapphire": "#74c7ec",
        "blue": "#89b4fa", "lavender": "#b4befe", "text": "#cdd6f4",
        "subtext1": "#bac2de", "subtext0": "#a6adc8", "overlay2": "#9399b2",
        "overlay1": "#7f849c", "overlay0": "#6c7086", "surface2": "#585b70",
        "surface1": "#45475a", "surface0": "#313244", "base": "#1e1e2e",
        "mantle": "#181825", "crust": "#11111b",
    },
    "latte": {
        "rosewater": "#dc8a78", "flamingo": "#dd7878", "pink": "#ea76cb",
        "mauve": "#8839ef", "red": "#d20f39", "maroon": "#e64553",
        "peach": "#fe640b", "yellow": "#df8e1d", "green": "#40a02b",
        "teal": "#179299", "sky": "#04a5e5", "sapphire": "#209fb5",
        "blue": "#1e66f5", "lavender": "#7287fd", "text": "#4c4f69",
        "subtext1": "#5c5f77", "subtext0": "#6c6f85", "overlay2": "#7c7f93",
        "overlay1": "#8c8fa1", "overlay0": "#9ca0b0", "surface2": "#acb0be",
        "surface1": "#bcc0cc", "surface0": "#ccd0da", "base": "#eff1f5",
        "mantle": "#e6e9ef", "crust": "#dce0e8",
    },
}

# Locked mapping (ThemeStore.stateToken) + StateStyle marks (verbatim glyphs).
STATE_TOKEN = {"working": "teal", "blocked": "red", "done": "green",
               "idle": "subtext0", "unknown": "surface2"}
STATE_MARK = {"working": "\u25cb", "blocked": "!", "idle": "\u25e6",
              "done": "\u2713", "unknown": "?"}

FLAVOR_FOR = {"mocha": "mocha", "latte": "latte"}

# ------------------------------------------------------------ repo hues -----
# Verbatim port of RepoHue (AppTheme.swift): FNV-1a 32 % 8 over the fixed
# ring, alphabetical repos, linear-probe collisions.


def fnv1a32(value: str) -> int:
    h = 0x811C9DC5
    for byte in value.encode("utf-8"):
        h ^= byte
        h = (h * 0x01000193) & 0xFFFFFFFF
    return h


REPO_RING = ["blue", "sapphire", "teal", "green", "yellow", "peach",
             "mauve", "pink"]


def repo_hues(repos) -> dict:
    taken: dict[int, str] = {}
    out: dict[str, str] = {}
    for repo in sorted(repos):
        start = fnv1a32(repo) % len(REPO_RING)
        assigned = REPO_RING[start]
        for step in range(len(REPO_RING)):
            idx = (start + step) % len(REPO_RING)
            if idx not in taken:
                taken[idx] = repo
                assigned = REPO_RING[idx]
                break
        out[repo] = assigned
    return out


# ------------------------------------------------------- chip mix math ------
# Verbatim port of ThemeStore.mixedHex: sRGB component lerp, half-boundary
# components round to EVEN (toNearestOrEven), matching CSS color-mix renders.


def _components(hexstr: str) -> tuple[float, float, float]:
    body = hexstr.lstrip("#")
    return (float(int(body[0:2], 16)), float(int(body[2:4], 16)),
            float(int(body[4:6], 16)))


def mix_hex(hue: str, t: float, surface: str) -> str:
    ar, ag, ab = _components(hue)
    br, bg, bb = _components(surface)
    t = min(max(t, 0.0), 1.0)

    # Python banker's rounding == Swift .toNearestOrEven at exact halves.
    def lerp(a: float, b: float) -> int:
        x = a * t + b * (1 - t)
        fl = int(x)
        frac = x - fl
        if frac > 0.5:
            return fl + 1
        if frac < 0.5:
            return fl
        return fl + 1 if fl % 2 == 1 else fl

    return "#{:02x}{:02x}{:02x}".format(
        lerp(ar, br), lerp(ag, bg), lerp(ab, bb))


def state_colors(pal: dict) -> dict:
    return {s: pal[tok] for s, tok in STATE_TOKEN.items()}


def state_chip(pal: dict, state: str) -> tuple[str, str]:
    return (mix_hex(pal[STATE_TOKEN[state]], 0.17, pal["base"]),
            mix_hex(pal[STATE_TOKEN[state]], 0.34, pal["base"]))


def repo_chip(pal: dict, hue: str) -> tuple[str, str]:
    return (mix_hex(pal[hue], 0.15, pal["base"]),
            mix_hex(pal[hue], 0.38, pal["base"]))


def repo_band(pal: dict, hue: str) -> str:
    return mix_hex(pal[hue], 0.09, pal["mantle"])


REPO_INK_RATIO = {"latte": 0.29, "mocha": 1.0}


def repo_ink(pal: dict, flavor: str, hue: str) -> str:
    if hue == "surface2":
        return pal["subtext1"]
    return mix_hex(pal[hue], REPO_INK_RATIO[flavor], pal["text"])


# --------------------------------------------------- horse identity axis ----
# Deterministic identity: sha256(agent_name) bits pick coat / mane / breed
# silhouette / tack / accessory. Same name => same horse in every state,
# palette, variant, and frame. State changes pose only, never identity bits.
COATS = ["bay", "chestnut", "black", "grey", "palomino", "dun", "roan",
         "buckskin"]
COAT_HEX = {  # original flat vector coat fills (horse-realistic, invented)
    "bay":      {"body": "#8a5a33", "shades": "#6f4527"},
    "chestnut": {"body": "#a05c2c", "shades": "#7d4520"},
    "black":    {"body": "#3b3b44", "shades": "#2a2a31"},
    "grey":     {"body": "#b9bcc6", "shades": "#989ca9"},
    "palomino": {"body": "#c89a56", "shades": "#a37a3c"},
    "dun":      {"body": "#b08d5e", "shades": "#8e6f45"},
    "roan":     {"body": "#96685a", "shades": "#77524a"},
    "buckskin": {"body": "#bd8f4e", "shades": "#96703a"},
}
MANES = ["flowing", "braided", "cropped"]
BREEDS = ["light", "stock", "draft"]          # silhouette families
TACKS = ["saddle", "pad", "none"]
ACCESSORIES = ["bandana", "hat", "none"]


def horse_identity(agent_name: str) -> dict:
    digest = hashlib.sha256(agent_name.encode("utf-8")).digest()
    bit = digest[0]
    return {
        "coat": COATS[bit % len(COATS)],
        "mane": MANES[(digest[1] >> 2) % len(MANES)],
        "breed": BREEDS[(digest[2] >> 4) % len(BREEDS)],
        "tack": TACKS[digest[3] % len(TACKS)],
        "accessory": ACCESSORIES[digest[4] % len(ACCESSORIES)],
    }


# ---------------------------------------------------------------- data ------

@dataclass
class Agent:
    name: str
    repo: str
    host: str
    state: str
    branch: str
    harness: str          # carried verbatim; never drives species/identity
    tool: str = ""


@dataclass
class Fixture:
    frame: str            # blocked-heavy | dense-fleet | disconnected
    hosts: list = field(default_factory=list)
    agents: list = field(default_factory=list)
    source_ok: bool = True
    note: str = ""


def _a(name, repo, host, state, branch, harness, tool=""):
    return Agent(name=name, repo=repo, host=host, state=state,
                 branch=branch, harness=harness, tool=tool)


# Long names (AC: long repository and agent names) + 4+ paddocks in every
# frame. Harness/tool strings are illustrative agnostic labels only.
REPO_LONG = "project-hearthwild-monorepo"
REPOS_ALL = [REPO_LONG, "atlas-vector", "fern-quarry", "orchard-lane"]


def blocked_heavy() -> Fixture:
    ag = [
        _a("oak-before-dark", REPO_LONG, "mac-mini", "blocked",
           "feat/ledger-replay-window", "claude-code"),
        _a("birch-clearing", "atlas-vector", "mac-mini", "blocked",
           "fix/tile-cache-miss", "codex"),
        _a("juniper-south", "fern-quarry", "bazzite", "working",
           "refactor/tailwind-lints", "claude-code"),
        _a("hazel-hollow", REPO_LONG, "mac-mini", "working",
           "chore/dep-bump-quarter", "opencode"),
        _a("cedar-ridge", "orchard-lane", "bazzite", "working",
           "feat/route-slices", "codex"),
        _a("maple-stand", "fern-quarry", "mac-mini", "idle",
           "main", "claude-code"),
        _a("willow-bend", "atlas-vector", "bazzite", "idle",
           "develop", "opencode"),
        _a("aspen-grove", REPO_LONG, "bazzite", "done",
           "docs/arch-refresh", "claude-code"),
        _a("sumac-row", "orchard-lane", "mac-mini", "unknown",
           "spike/eval-harness", "codex"),
        _a("elder-flats", "fern-quarry", "bazzite", "idle",
           "main", "opencode"),
        _a("hawthorn-fork", REPO_LONG, "mac-mini", "working",
           "feat/quota-readout", "opencode"),
        _a("spruce-hollow", "atlas-vector", "bazzite", "done",
           "fix/scene-pick", "claude-code"),
    ]
    return Fixture(frame="blocked-heavy",
                   hosts=[("mac-mini", 7, "ok"), ("bazzite", 5, "ok")],
                   agents=ag,
                   note="2+ blocked horses in different paddocks at the "
                        "front rail; 12 horses, 4 paddocks.")


def dense_fleet() -> Fixture:
    ag = [
        _a("oak-before-dark", REPO_LONG, "mac-mini", "working",
           "feat/ledger-replay-window", "claude-code"),
        _a("birch-clearing", "atlas-vector", "mac-mini", "working",
           "fix/tile-cache-miss", "codex"),
        _a("juniper-south", "fern-quarry", "bazzite", "working",
           "refactor/tailwind-lints", "claude-code"),
        _a("hazel-hollow", REPO_LONG, "mac-mini", "blocked",
           "chore/dep-bump-quarter", "opencode"),
        _a("cedar-ridge", "orchard-lane", "bazzite", "working",
           "feat/route-slices", "codex"),
        _a("maple-stand", "fern-quarry", "mac-mini", "idle", "main",
           "claude-code"),
        _a("willow-bend", "atlas-vector", "bazzite", "idle", "develop",
           "opencode"),
        _a("aspen-grove", REPO_LONG, "bazzite", "done", "docs/arch-refresh",
           "claude-code"),
        _a("sumac-row", "orchard-lane", "mac-mini", "unknown",
           "spike/eval-harness", "codex"),
        _a("elder-flats", "fern-quarry", "bazzite", "working",
           "perf/quarry-batch", "opencode"),
        _a("hawthorn-fork", REPO_LONG, "mac-mini", "working",
           "feat/quota-readout", "opencode"),
        _a("spruce-hollow", "atlas-vector", "bazzite", "done",
           "fix/scene-pick", "claude-code"),
        _a("rowan-copse", "fern-quarry", "mac-mini", "working",
           "test/quarrier-contract", "claude-code"),
        _a("bracken-moor", "orchard-lane", "bazzite", "idle", "main",
           "codex"),
        _a("sloe-bank", "atlas-vector", "mac-mini", "blocked",
           "fix/vector-serif-kern", "opencode"),
        _a("thistle-drum", REPO_LONG, "bazzite", "idle", "develop",
           "claude-code"),
        _a("vetch-meadow", "orchard-lane", "mac-mini", "done",
           "chore/prune-lane", "opencode"),
        _a("yarrow-flat", "fern-quarry", "bazzite", "working",
           "feat/quarry-cams", "codex"),
    ]
    return Fixture(frame="dense-fleet",
                   hosts=[("mac-mini", 10, "ok"), ("bazzite", 8, "ok")],
                   agents=ag,
                   note="18 horses / 4 paddocks: overflow + navigation, "
                        "labels never shrink below usable size.")


def disconnected() -> Fixture:
    # Last-known snapshot = the same 12 horses as blocked-heavy, now all
    # reported as unknown (the outage banner says nothing here is live).
    ag = [Agent(name=a.name, repo=a.repo, host=a.host, state="unknown",
                branch=a.branch, harness=a.harness, tool=a.tool)
          for a in blocked_heavy().agents]
    return Fixture(frame="disconnected",
                   hosts=[("mac-mini", 7, "source-failure"),
                          ("bazzite", 5, "source-failure")],
                   agents=ag, source_ok=False,
                   note="Source failure: the last-known 12 horses hold "
                        "static unknown poses under an explicit outage "
                        "banner. No animated demo data substitutes for "
                        "the truth.")


FRAMES = {"blocked-heavy": blocked_heavy,
          "dense-fleet": dense_fleet,
          "disconnected": disconnected}


# -------------------------------------------------------------- self-check --

def _self_check() -> None:
    for fname, fn in FRAMES.items():
        fx = fn()
        names = [a.name for a in fx.agents]
        assert len(names) == len(set(names)), f"{fname}: duplicate names"
        repos = {a.repo for a in fx.agents}
        assert len(repos) >= 4, f"{fname}: needs >=4 paddocks"
        assert len(fx.agents) >= 12 or fname == "disconnected", (
            f"{fname}: needs >=12 horses")
        for a in fx.agents:
            assert a.state in STATES, f"{fname}: bad state {a.state}"
            ident = horse_identity(a.name)
            assert all(k in ident for k in
                       ("coat", "mane", "breed", "tack", "accessory"))
    bh = blocked_heavy()
    blocked_repos = {a.repo for a in bh.agents if a.state == "blocked"}
    assert len(blocked_repos) >= 2, "blocked-heavy: need 2 repos blocked"
    df = dense_fleet()
    assert len(df.agents) >= 18, "dense-fleet should be dense (>=18)"
    assert sum(1 for a in df.agents if a.state == "working") >= 4
    disc = disconnected()
    assert disc.source_ok is False
    assert all(a.state == "unknown" for a in disc.agents)
    assert max(len(r) for r in REPOS_ALL) > 20, "needs a long repo name"
    # mix math sanity: mocha teal-17%-over-base pins to the exact locked
    # tint (documented formula, verified channel-by-channel: 50/63/74).
    assert state_chip(PALETTES["mocha"], "working")[0] == "#323f4a"


_self_check()

# Real-world tokens that must never appear in GENERATED artifacts (HTML,
# PNG-adjacent text, README). The scanner runs over outputs + docs, not over
# this pattern list itself (verify.py excludes fixtures.py/verify.py).
FORBIDDEN_STRINGS = [
    r"synergyservices\.co\.th",
    r"jirathip(?!-dev/corral)",       # the owner handle; the repo slug is allowed
    r"github\.com/(?!jirathip-dev/corral)[a-z0-9-]+/",
    r"@(?![a-z0-9-]*invalid\b)[a-z0-9.-]+\.(com|org|net|co|th)\b",
    r"\b(api[_-]?key|secret|password|token)\s*[:=]\s*['\"][^'\"]{6,}",
]


def scan_forbidden(text: str) -> list[str]:
    hits = []
    stripped = re.sub(r"data:[^;)]+", "data:", text)  # strip data URIs first
    for pat in FORBIDDEN_STRINGS:
        for m in re.finditer(pat, stripped, flags=re.IGNORECASE):
            hits.append(m.group(0))
    return hits


if __name__ == "__main__":
    for fname, fn in FRAMES.items():
        fx = fn()
        print(f"{fname}: {len(fx.agents)} horses, "
              f"{len({a.repo for a in fx.agents})} paddocks, "
              f"source_ok={fx.source_ok}")
    print("fixture self-check OK")
