#!/usr/bin/env python3
"""Summarize real #547 simulator fixture samples, never handset/network data."""
import json
from pathlib import Path
import re
import statistics
import sys

log = Path(sys.argv[1])
samples = [(int(number), json.loads(payload)) for number, payload in
           re.findall(r'^G547_SIM_SAMPLE (\d+) (\{.*\})$', log.read_text(), re.MULTILINE)]
assert len(samples) == 10 and sorted(number for number, _ in samples) == list(range(10))
metrics = [
    ('key_dispatch_minus_path_observed', 'path_ready', 'key_request'),
    ('key_round_trip', 'key_request', 'key_response'),
    ('path_observed_to_sse_200', 'path_ready', 'sse_200'),
    ('sse_200_to_frame_received', 'sse_200', 'frame_received'),
    ('frame_received_to_applied', 'frame_received', 'frame_applied'),
    ('key_response_to_applied', 'key_response', 'frame_applied'),
    ('applied_to_row_visible', 'frame_applied', 'row_visible'),
    ('path_observed_to_retained_row_visible', 'path_ready', 'retained_row_visible'),
]
result = {'source_log': str(log), 'environment': 'iPhone 16 iOS Simulator; local URLProtocol; controlled key gates',
          'sample_count': len(samples), 'samples': [{'sample': number, 'uptime_seconds': stamps} for number, stamps in samples],
          'metrics': [],
          'non_claims': ['Not physical-device, real-network, Mac-server or loaded-Bazzite timings.',
                         'No instrumented serial-control performance comparison was run.',
                         'Path-ready observation may arrive AFTER key dispatch; negative differences are retained.',
                         'Sample 0 includes attachment capture before key release. All 10 samples are included.',
                         'Row marks are SwiftUI appearance/update callbacks, not GPU scan-out.']}
for name, start, end in metrics:
    values = [(stamps[end]-stamps[start])*1000 for _, stamps in samples]
    result['metrics'].append({'metric': name, 'start': start, 'end': end, 'n': len(values),
                              'milliseconds': values, 'median_ms': statistics.median(values)})
print(json.dumps(result, indent=2))
