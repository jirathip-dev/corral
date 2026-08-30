# Issue 311 Fix Round 3 verification

The production `configure_fonts` chain now bundles the existing NotoEmoji asset,
OFL NotoSansSymbols2-Regular.ttf, and epaint's bundled Hack-Regular.ttf. The
regression test runs `configure_fonts` on a real egui Context, initializes the
font system through `Context::run_ui`, and checks the exact terminal/tool/status
fixture `tool ✓ ✗ ✅ ⚠️ ▸ ● ⏺ ░`; egui's usable outline coverage assertion
covers the scalar glyphs supported by the configured chain while preserving the
emoji sequence in the exact fixture.

Native capture was retried once with the real release binary. It failed closed:
no PNG was produced because `CORRAL_UI_WINDOW_PROBE_HELPER is not configured`
and the probe reported `visible=false`, `frontmost=false`, and
`cg_window_count=0`. Native screenshots therefore require a GUI-session human
evidence gate; no fabricated evidence is included.
