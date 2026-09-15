#!/usr/bin/env python3
"""Summarize #551 raw stage marks; never substitute fixture data for device data."""
import collections
import json
from pathlib import Path
import re
import statistics
import sys

log = Path(sys.argv[1])
samples = [json.loads(value) for value in re.findall(r'^G551_SAMPLE (\{.*\})$', log.read_text(), re.MULTILINE)]
counts = collections.Counter(row['scenario'] for row in samples)
assert counts == {'cold_model': 5, 'warm_30': 5, 'warm_300': 5}, counts
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
result = {'source_log': str(log), 'environment': 'iPhone 16 / iOS 26.5 Simulator; URLProtocol fixture',
          'production_base': 'b6b69f377883d6b2507f8418b6b4dc34f7452145',
          'sample_counts': dict(counts), 'samples': samples, 'metrics': {},
          'non_claims': [
              'Fresh XCTest process and fresh AppModel per repetition; not normal app force-quit/reopen.',
              'Warm paths use production scene seam and real elapsed 30/300s waits, not OS suspension.',
              'No real daemon/network, physical iPhone, loaded Bazzite, or Mac-server timing.',
              'path_ready is an asynchronous monitor observation; negative dispatch intervals are retained.',
              'row_visible is a SwiftUI lifecycle/update hook, not GPU scan-out.',
              'No injected transport delay; local URLProtocol does not reproduce URLSession socket contention.',
              'Retained-row marks may be absent on an already mounted row; absence is not proof of blanking.',
          ]}
for scenario in counts:
    selected = [sample for sample in samples if sample['scenario'] == scenario]
    values = {}
    for name, start, end in metrics:
        intervals = [(sample['stamps'][end] - (sample['start_uptime'] if start == 'seam_entry'
                     else sample['stamps'][start])) * 1000 for sample in selected]
        values[name] = {'n': len(intervals), 'milliseconds': intervals,
                        'median_ms': statistics.median(intervals)}
    result['metrics'][scenario] = values
result['fixture_warm_30_to_cold_ratio'] = {
    metric: result['metrics']['warm_30'][metric]['median_ms'] / result['metrics']['cold_model'][metric]['median_ms']
    for metric in ['path_to_apply', 'seam_entry_to_apply']}
print(json.dumps(result, indent=2))
