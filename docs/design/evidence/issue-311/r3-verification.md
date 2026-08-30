# Issue 311 Fix Round 3 verification

The production `configure_fonts` chain bundles the existing NotoEmoji asset,
OFL NotoSansSymbols2-Regular.ttf, and epaint's bundled Hack-Regular.ttf. The
regression test runs `configure_fonts` on a real egui Context, initializes the
font system through `Context::run_ui`, and lays out the exact fixture:
`tool ✓ ✗ ✅ ⚠️ ▸ ● ⏺ ░`. It asserts usable configured-chain coverage for every
required visible scalar (`✓`, `✗`, `✅`, `⚠`, `▸`, `●`, `⏺`, `░`) and checks the
configured font resolver for the non-emoji scalars. The `U+FE0F` variation
selector is intentionally excluded from scalar coverage: it modifies the
preceding visible `⚠` presentation and is not itself a visible glyph.

Focused RED/GREEN mutation evidence (clean Cargo target was already populated
by the baseline build; the mutation was temporary and production code was
restored before the GREEN run):

```text
# RED mutation: remove corral-transcript-symbols from both configured families
CARGO_BUILD_JOBS=1 cargo test -p corrald-ui --lib app::font_tests::transcript_fixture_has_glyph_coverage -- --exact
exit 101
thread 'app::font_tests::transcript_fixture_has_glyph_coverage' panicked at clients/egui/src/app.rs:2739:21:
missing configured glyph ✓
```

```text
# GREEN after restoring the corral-transcript-symbols registrations
CARGO_BUILD_JOBS=1 cargo test -p corrald-ui --lib app::font_tests::transcript_fixture_has_glyph_coverage -- --exact
test app::font_tests::transcript_fixture_has_glyph_coverage ... ok
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 164 filtered out
exit 0
```

Native capture was retried once with the real release binary. It failed closed:
no PNG was produced because `CORRAL_UI_WINDOW_PROBE_HELPER is not configured`
and the probe reported `visible=false`, `frontmost=false`, and
`cg_window_count=0`. Native screenshots therefore require a GUI-session human
evidence gate; no fabricated evidence is included.
