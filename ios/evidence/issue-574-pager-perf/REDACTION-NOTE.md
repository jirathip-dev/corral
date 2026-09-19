# Evidence redaction note (post-merge gate fix)

The hosted `Secret scan` workflow (gitleaks, full tree) failed on the first
integration head carrying this bundle: rule `generic-api-key`, entropy 4.90,
three findings, all the same value in three committed xcodebuild logs
(`gates-logs/g4-unit.log`, `g4b-unit.log`, `g4d-unit.log`).

The value is emitted by a pre-existing test diagnostic,
`ios/FleetNotifierTests/FleetNotifierTests.swift:1369`
(`print("FN-DIAG storage=\(storage) pub=\(signer.publicKeyB64)")`) — i.e. the
test signer's PUBLIC key (no private material, no credential that can
authenticate anything). It is nevertheless key material that the repo's own
gate rejects, so the value was replaced in those three lines with
`<redacted-by-orch-574-evidence-hygiene>` and the three `SHA256SUMS.txt`
entries were re-pinned over the redacted bytes.

Nothing else in the bundle changed; no source, test or gate config was
touched, and no allowlist entry was added for the scanner (the gate keeps
biting). The redaction carries no logic, so the reviewed verdict at
`d131d69` stands for the substantive change.
