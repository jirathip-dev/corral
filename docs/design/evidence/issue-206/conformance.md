# Issue #206 design-gate evidence

Generated: `2026-08-25T19:04:48Z`

## Capture

- Surface: `egui`
- Capture kind: explicit supplied PNG fixture
- Live description: caller-supplied file; this run did not capture a live surface
- Command: `cp /Users/jirathip/.herdr/worktrees/corral/g211-design-gate-evidence/clients/egui/evidence/fleet-live.png <issue-dir>/live-after.png`
- Host health URL: not checked for this supplied capture
- Selected live agent: none
- Operator/environment note: Fresh native capture was attempted against the healthy 127.0.0.1:8474 daemon but this shell session could not surface or wake the eframe window; live-after.png is the pre-existing real native egui evidence supplied explicitly, so this bundle makes no freshness claim.

## Sources

- Prototype source: `/Users/jirathip/.herdr/worktrees/corral/g211-design-gate-evidence/docs/design/corral-ux-prototype.html`
- Prototype source SHA-256: `60d9a2e2a9dc3fcdc57b2300d79e5ff92467f7187ba54a1bc02c75362b8c0e1f`
- Generator SHA-256: `ac04e34c1a3baf392e5ee00224b949cd45e02a4ac8ccc162c1e8c611bc3b37ca`
- Live input: `/Users/jirathip/.herdr/worktrees/corral/g211-design-gate-evidence/clients/egui/evidence/fleet-live.png`
- Live input SHA-256: `d8a51c4386ba8b4cd80c93bf2bb4d2b74e3bbcdf58d8c7ce7a889f072a3060c3`
- Repository HEAD: `f8c786d734b64b1990d25c7a7adf39ec0e1f2286`
- Reproducible invocation: `/Users/jirathip/.herdr/worktrees/corral/g211-design-gate-evidence/scripts/design-gate-evidence.sh --issue 206 --surface egui --prototype docs/design/corral-ux-prototype.html --live-png clients/egui/evidence/fleet-live.png --provenance-note Fresh\ native\ capture\ was\ attempted\ against\ the\ healthy\ 127.0.0.1:8474\ daemon\ but\ this\ shell\ session\ could\ not\ surface\ or\ wake\ the\ eframe\ window\;\ live-after.png\ is\ the\ pre-existing\ real\ native\ egui\ evidence\ supplied\ explicitly\,\ so\ this\ bundle\ makes\ no\ freshness\ claim. --chrome-timeout-seconds 20 `

## Artifacts

| File | Dimensions | SHA-256 |
| --- | --- | --- |
| `prototype.png` | `1160x631` | `f68deba0340591cab68217fa6ca35e702a299b91f20b61b6a27ebb7c1e733ce3` |
| `live-after.png` | `2640x1720` | `d8a51c4386ba8b4cd80c93bf2bb4d2b74e3bbcdf58d8c7ce7a889f072a3060c3` |
| `comparison.png` | `2400x960` | `c53b3639a4634af2696d5c9de8f489fdfe8f94c4b2fc840b09690fb12c5abb85` |

The comparison header is stamped with the target issue number. A supplied PNG or iOS Debug demo is explicitly labeled above and must not be read as proof of a live daemon session.
