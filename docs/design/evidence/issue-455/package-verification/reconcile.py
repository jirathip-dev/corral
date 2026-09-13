#!/usr/bin/env python3
"""Promotion-time reconciliation for the #455 approved-prototype package.

Fail-closed checks (all raw, no network):
 1. Approved-artifact identity: variant-a.html sha256 equals the owner-approved
    constant from corral#455 routing comment 5601951267; variant-b.html is
    recorded as the historical comparison artifact and asserted NOT approved.
 2. Source-manifest reconciliation: every entry of the designer run's
    manifest.sha256 (retained byte-identical at references/source-manifest.sha256)
    is either present byte-identical under its original or relocated package
    path, or explicitly excluded with a reason. 'missing' and 'changed' must be
    empty.
 3. Portability ledger: the only tools/ files whose bytes differ from the
    source manifest are the documented label/portability adaptations
    (see tools/PORTABILITY-NOTES.md); everything else must stay source-identical.
 4. Package manifest totals: every line of the package manifest.sha256 matches
    the actual file on disk; reports file counts and total bytes by extension.

Usage: python3 -B package-verification/reconcile.py [--output <path>]
Exit 0 only when every check passes.
"""
import argparse, hashlib, json, sys
from pathlib import Path
sys.dont_write_bytecode = True
ROOT = Path(__file__).resolve().parents[1]
APPROVED_VARIANT = 'variant-a.html'
APPROVED_SHA256 = 'ca0a1a0991aa66364598eaacdac4d328de6a4ac1e57e03707aa708851791bf61'
HISTORICAL_VARIANT = 'variant-b.html'
HISTORICAL_SHA256 = 'fa31421c9dcee0b5b1116627b404466f9009e78c3a970154326fca258d49b47c'
HISTORICAL_STATUS = 'historical comparison only; explicitly NOT the approved variant'
RELOCATED = {
    'README.md': 'README-designer-original.md',
    '.brief.md': 'references/designer-brief-455.md',
    'manifest.sha256': 'references/source-manifest.sha256',
}
EXCLUDED = {
    'designer-run.log': 'designer session runtime log; not a dependency of any claim; retained in the source bundle at /Users/jirathip/design-output/corral/455-immersive-herd/designer-run.log',
}
# Whitespace-only normalization applied for packaging hygiene: the source file
# ends with a blank line at EOF, which the docs-only diff gate
# (`git diff --check`) reports for an added file. Content is otherwise
# byte-identical; verified below via the content hash with trailing newlines
# stripped, plus a single-trailing-newline assertion.
NORMALIZED_FOR_WHITESPACE_GATE = {
    'evidence/run-log.txt': {
        'sourceSha256': '59a170a7159230c41e4b71a6b234d8ec6a1aa8d1f1d027d98c0fb8344a914cc2',
        'contentWithoutTrailingNewlinesSha256': '00a1151b919ad559f65094b76045d68acb4f268ac9d8ffd5e4965698faf9035a',
        'packageSha256': '92e67a605b9ca335afd9976246e347c50c1dba6fc44a153d3dfe6a46f87b7e43',
        'reason': 'trailing blank line at EOF normalized to a single newline so the docs-only diff passes git diff --check; content otherwise byte-identical',
    },
}
EXPECTED_ADAPTED_TOOLS = sorted([
    'tools/browser.py', 'tools/check-reachability.py', 'tools/gather.py',
    'tools/package.py', 'tools/record-motion.py', 'tools/verify-browser.py',
    'tools/verify-workflows.py',
])
def sha256(p):
    return hashlib.sha256(Path(p).read_bytes()).hexdigest()
def read_manifest(path):
    entries = {}
    for line in Path(path).read_text().splitlines():
        digest, rel = line.split('  ', 1)
        entries[rel] = digest
    return entries
def main():
    ap = argparse.ArgumentParser()
    ap.add_argument('--output', default=str(ROOT / 'package-verification' / 'reconcile.json'))
    args = ap.parse_args()
    failures = []
    report = {'checks': {}}
    # 1. approved artifact identity
    a_sha = sha256(ROOT / APPROVED_VARIANT)
    b_sha = sha256(ROOT / HISTORICAL_VARIANT)
    identity = {
        'approvedVariant': APPROVED_VARIANT, 'approvedSha256': a_sha,
        'approvedSha256Expected': APPROVED_SHA256, 'approvedSha256MatchesReceipt': a_sha == APPROVED_SHA256,
        'historicalVariant': HISTORICAL_VARIANT, 'historicalSha256': b_sha,
        'historicalSha256Recorded': HISTORICAL_SHA256, 'historicalArtifactIsDistinct': b_sha != APPROVED_SHA256,
        'historicalStatus': HISTORICAL_STATUS,
        'receipt': 'https://github.com/jirathip-dev/corral/issues/455#issuecomment-5601951267',
    }
    report['checks']['approvedArtifactIdentity'] = identity
    if not identity['approvedSha256MatchesReceipt']:
        failures.append('approved variant-a.html sha256 does not match the owner receipt constant')
    if not identity['historicalArtifactIsDistinct']:
        failures.append('historical variant-b.html unexpectedly equals the approved hash')
    # 2. source-manifest reconciliation
    source_manifest = read_manifest(ROOT / 'references' / 'source-manifest.sha256')
    verified, relocated, excluded, missing, changed, adapted_from_source, normalized = [], [], [], [], [], [], []
    expected_adapted = set(EXPECTED_ADAPTED_TOOLS)
    for src_rel, digest in sorted(source_manifest.items()):
        if src_rel in EXCLUDED:
            excluded.append({'sourcePath': src_rel, 'sha256': digest, 'reason': EXCLUDED[src_rel]})
            continue
        pkg_rel = RELOCATED.get(src_rel, src_rel)
        p = ROOT / pkg_rel
        if not p.is_file():
            missing.append({'sourcePath': src_rel, 'expectedPackagePath': pkg_rel})
            continue
        actual = sha256(p)
        row = {'sourcePath': src_rel, 'packagePath': pkg_rel, 'sha256': digest, 'matches': actual == digest}
        if actual == digest:
            verified.append(row)
        elif src_rel in NORMALIZED_FOR_WHITESPACE_GATE:
            spec = NORMALIZED_FOR_WHITESPACE_GATE[src_rel]
            raw = p.read_bytes()
            body_ok = hashlib.sha256(raw.rstrip(b'\n')).hexdigest() == spec['contentWithoutTrailingNewlinesSha256']
            ends_ok = raw.endswith(b'\n') and not raw.endswith(b'\n\n')
            row.update({'normalization': spec['reason'], 'sourceSha256': spec['sourceSha256'],
                        'contentWithoutTrailingNewlinesMatches': body_ok, 'singleTrailingNewline': ends_ok})
            normalized.append(row)
            if not (body_ok and ends_ok):
                changed.append(row)
        elif src_rel in expected_adapted:
            adapted_from_source.append(row)
        else:
            changed.append(row)
        if src_rel in RELOCATED:
            relocated.append({'sourcePath': src_rel, 'packagePath': pkg_rel, 'sha256': digest, 'matches': actual == digest})
    report['checks']['sourceManifest'] = {
        'sourceManifestPath': 'references/source-manifest.sha256',
        'sourceManifestEntries': len(source_manifest),
        'verifiedByteIdentical': len(verified), 'relocated': relocated,
        'adaptedFromSource': adapted_from_source,
        'normalizedForWhitespaceGate': normalized,
        'excluded': excluded, 'missing': missing, 'changed': changed,
    }
    if missing or changed:
        failures.append(f'source-manifest reconciliation failed: {len(missing)} missing, {len(changed)} changed')
    # 3. portability ledger
    adapted = sorted(rel for rel, digest in source_manifest.items()
                     if rel.startswith('tools/') and sha256(ROOT / rel) != digest)
    report['checks']['toolsPortabilityLedger'] = {
        'adaptedTools': adapted, 'expectedAdaptedTools': EXPECTED_ADAPTED_TOOLS,
        'matchesDocumentedSet': adapted == EXPECTED_ADAPTED_TOOLS,
    }
    if adapted != EXPECTED_ADAPTED_TOOLS:
        failures.append(f'adapted tools differ from the documented set: {adapted}')
    # 4. package manifest totals
    manifest_path = ROOT / 'manifest.sha256'
    manifest = read_manifest(manifest_path)
    self_out = Path(args.output).resolve()
    self_rel = str(self_out.relative_to(ROOT)) if str(self_out).startswith(str(ROOT) + '/') else None
    manifest_cmp = {k: v for k, v in manifest.items() if k != self_rel}
    actual_files = sorted(p for p in ROOT.rglob('*') if p.is_file() and p != manifest_path
                          and p.resolve() != self_out)
    mismatched = []
    by_ext, total_bytes = {}, 0
    for p in actual_files:
        rel = str(p.relative_to(ROOT))
        total_bytes += p.stat().st_size
        ext = p.suffix or '(none)'
        by_ext[ext] = by_ext.get(ext, 0) + 1
        if manifest_cmp.get(rel) != sha256(p):
            mismatched.append(rel)
    missing_from_manifest = sorted(set(str(p.relative_to(ROOT)) for p in actual_files) - set(manifest_cmp))
    extra_in_manifest = sorted(set(manifest_cmp) - set(str(p.relative_to(ROOT)) for p in actual_files))
    report['checks']['packageManifest'] = {
        'manifestPath': 'manifest.sha256', 'manifestEntries': len(manifest),
        'filesOnDiskExcludingManifest': len(actual_files), 'totalBytesExcludingManifest': total_bytes,
        'countsByExtension': dict(sorted(by_ext.items())),
        'mismatched': mismatched, 'missingFromManifest': missing_from_manifest,
        'extraInManifest': extra_in_manifest, 'manifestIncludesItself': 'manifest.sha256' in manifest,
        'selfOutput': self_rel or str(self_out), 'selfOutputExcludedFromAccounting': True,
        'note': 'Counts cover the manifest state read at this run. When this JSON lives inside the package, '
                'the committed manifest.sha256 is sealed after this file exists and additionally covers it; '
                'tools/package.py --verify validates that full final set.',
    }
    if mismatched or missing_from_manifest or extra_in_manifest or 'manifest.sha256' in manifest:
        failures.append('package manifest does not match the file set on disk')
    report['status'] = 'PASS' if not failures else 'FAIL'
    report['failures'] = failures
    out = Path(args.output)
    out.parent.mkdir(parents=True, exist_ok=True)
    out.write_text(json.dumps(report, indent=2) + '\n')
    print(json.dumps({
        'status': report['status'],
        'approvedSha256MatchesReceipt': identity['approvedSha256MatchesReceipt'],
        'sourceManifest': {'entries': len(source_manifest), 'verified': len(verified), 'relocated': len(relocated), 'excluded': len(excluded), 'missing': len(missing), 'changed': len(changed)},
        'adaptedTools': adapted,
        'packageManifest': {'entries': len(manifest), 'files': len(actual_files), 'bytes': total_bytes, 'mismatched': len(mismatched)},
        'failures': failures,
    }, indent=2))
    return 0 if not failures else 1
if __name__ == '__main__':
    sys.exit(main())
