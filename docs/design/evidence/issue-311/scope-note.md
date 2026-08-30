# Issue 311 scope verification

The prior implementation changed `clients/egui/src/main.rs`:

```diff
-        .with_min_inner_size([900.0, 600.0])
+        .with_min_inner_size([1280.0, 800.0])
```

This was unrelated to the glyph fallback, glyph fixture, and UI decoration
changes. It was reverted to the base value `[900.0, 600.0]`. The final change
set therefore contains no viewport/window-policy change; the native evidence
requirement is handled by the existing evidence launch procedure rather than
by changing production viewport policy.
