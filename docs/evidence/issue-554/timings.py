#!/usr/bin/env python3
"""Summarize #554 raw stage marks and compare them against the #551 baseline.

Reads the G551_SAMPLE lines the (unchanged) #551 measurement method emits, then
compares each scenario's medians with the #551 lane's medians transcribed in
``docs/evidence/issue-554/baseline-551-medians.json``. Never substitutes fixture
data for measured data and never claims more than the simulator shows.
"""
import collections
import json
from pathlib import Path
import re
import statistics
import sys

ROOT = Path(__file__).resolve().parents[3]
log = Path(sys.argv[1])
baseline_path = ROOT / 'docs/evidence/issue-554/baseline-551-medians.json'
samples = [json.loads(value)
           for value in re.findall(r'^G551_SAMPLE (\{.*\})$', log.read_text(), re.MULTILINE)]
counts = collections.Counter(row['scenario'] for row in samples)
metrics = [
    ('dispatch', 'path_ready', 'key_request'),
    ('key_rtt', 'key_request', 'key_response'),
    ('path_to_sse_200', 'path_ready', 'sse_200'),
    ('sse_200_to_frame', 'sse_200', 'frame_received'),
    ('frame_to_apply', 'frame_received', 'frame_applied'),
    ('key_response_to_apply', 'key_response', 'frame_applied'),
    ('apply_to_row', 'frame_applied', 'row_visible'),
    ('path_to_apply', 'path_ready', 'frame_applied'),
    ('seam_entry_to_apply', 'seam_entry', 'frame_applied'),
]
result = {
    'source_log': str(log),
    'environment': 'iPhone 16 / iOS 26.5 Simulator; URLProtocol fixture',
    'sample_counts': dict(counts),
    'metrics': {},
}
for scenario in sorted(counts):
    selected = [sample for sample in samples if sample['scenario'] == scenario]
    values = {}
    for name, start, end in metrics:
        intervals = [(sample['stamps'][end] - (sample['start_uptime'] if start == 'seam_entry'
                     else sample['stamps'][start])) * 1000 for sample in selected]
        values[name] = {'n': len(intervals), 'milliseconds': intervals,
                        'median_ms': statistics.median(intervals)}
    result['metrics'][scenario] = values

baseline = json.loads(baseline_path.read_text())
result['baseline_source'] = str(baseline_path)
result['baseline_provenance'] = baseline.get('provenance')
baseline_metrics = baseline.get('metrics', {})
comparison = {}
for scenario, values in result['metrics'].items():
    comparison[scenario] = {
        metric: {'baseline_median_ms': baseline_metrics[metric][scenario],
                 'head_median_ms': values[metric]['median_ms'],
                 'head_minus_baseline_ms': values[metric]['median_ms'] - baseline_metrics[metric][scenario]}
        for metric in values if metric in baseline_metrics and scenario in baseline_metrics[metric]
    }
result['baseline_comparison'] = comparison
result['cold_launch_non_regression'] = {
    metric: comparison['cold_model'][metric]
    for metric in ('seam_entry_to_apply', 'path_to_apply', 'key_rtt', 'apply_to_row')
    if 'cold_model' in comparison and metric in comparison['cold_model']
}
result['non_claims'] = [
    'Fresh XCTest process and fresh AppModel per repetition; not normal app force-quit/reopen.',
    'Warm paths use the production scene seam and real elapsed 30/300s waits, not OS suspension.',
    'No real daemon/network, physical iPhone, loaded Bazzite, or Mac-server timing.',
    'path_ready is an asynchronous monitor observation; negative dispatch intervals are retained.',
    'No injected transport delay; local URLProtocol does not reproduce URLSession socket contention,',
    'so this harness cannot itself reproduce or falsify the stale-pool warm return.',
    'The device gate in docs/evidence/issue-554/device-protocol.md remains UNEXECUTED.',
]
print(json.dumps(result, indent=2))
