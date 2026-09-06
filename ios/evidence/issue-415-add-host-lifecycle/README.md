# 415 evidence — Add Host draft/error lifecycle (iOS)

Frames for acceptance criteria 6a/6b/6c (390x844 px, iPhone 14 @3x
1179x2556 downscaled — same device-class standard as the #401 set; 0.0 %
aspect distortion).

- phase-a-415-bg-return-390x844.png — (a) background-return with
  populated fields: after a REAL app-switch (system Settings app) and
  return, the Add Host sheet still shows the entered host name "Bazzite"
  and URL (demo-host-d.tail0123.ts.net). The draft is model-owned and
  scene-scoped, so sheet view-identity churn from the scene lifecycle
  cannot clear it.
- phase-a-415-bg-returned-390x844.png — same state, later settle frame.
- phase-b-415-failed-sheet-open-390x844.png — (b) a failed submission with
  the sheet STILL OPEN and an actionable, phase-identifying error
  ("Could not verify this host's key — Could not connect to the server.")
  with every draft value intact and the Verify button available for retry.
  Real transport failure (connection-refused loopback URL; no daemon on
  the evidence simulator).
- phase-b-415-done-390x844.png — settled/dismissed after the driver ends.
- phase-c-415-confirm-before-submit-390x844.png — fingerprint-confirmation
  phase with the masked registration token, before the submit.
- phase-c-415-committed-390x844.png — (c) successful new-host commit: the
  Settings Hosts list shows the ORIGINAL Mac host (dev_evidence_mac) STILL
  PRESENT plus exactly ONE new host (Bazzite, dev_evidence_add, Active).
  The real prepare/register/commit flow ran end-to-end against a DEBUG
  fixture transport (AddHostCommitEvidenceURLProtocol) — the commit
  cleared the draft only after success and the sheet dismissed once.
- phase-c-415-done-390x844.png — settled state after the capture hold.

Seeding and driver mechanics are documented in capture.log (fresh
Corral415 iPhone 14 sim, erased before the runs; each driver launch
uninstalls the app so containers/markers start clean; the DEBUG seed
creates one original "Mac" profile; the successful-commit instance uses a
fixture URLSession — no daemon, no push permission prompt).
