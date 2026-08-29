# Issue #250 evidence — first-entry Devices & Grants discoverability

This bundle records the native egui regression capture for the reopened #250
acceptance. The Settings pane now names the existing approved V2 surface
`Devices & Grants` and expands it on a fresh `SettingsState` by default.

## Capture

- `capture.log` — exact focused native egui render-test command and output.
- Regression test: `ui::register::tests::first_settings_entry_exposes_devices_and_grants_expanded`
  in `clients/egui/src/ui/register.rs`.

The test renders the Settings pane with the default state and a registered
this-device fixture, then verifies that the first-entry render contains
`Devices & Grants`, `THIS DEVICE`, and `REMOTE DEVICES`. This proves the V2
surface is not behind an undiscoverable advanced-only gate. Existing V2
rendering and grant behavior remain unchanged.

## Before / after

Before this lane, the Settings entry was a collapsed `Advanced device access`
disclosure (`audit_open: false`), so the device/grant controls were absent from
the initial render.

After this lane, the entry is `Devices & Grants` and defaults expanded
(`audit_open: true`). Users can still collapse the egui disclosure, but the
first Settings entry exposes the V2 surface immediately.

## Boundary

This is a native egui render-test evidence capture, not an installed-app
screenshot. Installed-app re-verification is owned by the orchestrator after
merge, as specified by the lane brief.
