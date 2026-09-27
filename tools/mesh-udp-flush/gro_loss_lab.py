#!/usr/bin/env python3
"""Bounded ring-loss/ICMP diagnosis, never a throughput acceptance run."""
import argparse
import json
from pathlib import Path
import re
import subprocess
import sys

parser = argparse.ArgumentParser()
parser.add_argument('--binaries', type=Path, required=True)
parser.add_argument('--output', type=Path, required=True)
args = parser.parse_args()
binaries = args.binaries.resolve()
root = args.output.resolve()
root.mkdir(parents=True, exist_ok=False)
lab = Path(__file__).with_name('lab.py').resolve()
rows = []
for index, mode in enumerate(('off', 'on', 'on', 'off', 'off', 'on')):
    output = root / f'{index:02d}-{mode}-mixed'
    command = [sys.executable, str(lab), '--stock', str(binaries), '--candidate', str(binaries),
               '--order', 'stock', '--udp-gro-mode', mode, '--network-counters', '--inner-ipv6',
               '--mixed-flow', '--unpaced-probe', str(binaries / 'probe'), '--transfer-bytes',
               '1073741824', '--packet-trace', '--full-transfer-control', '--output', str(output)]
    with (root / f'{index:02d}.log').open('w') as log:
        result = subprocess.run(command, stdout=log, stderr=subprocess.STDOUT, timeout=360)
    row = {'index': index, 'mode': mode, 'exit': result.returncode, 'output': output.name,
           'performance_evidence': False, 'scope': 'two Core namespaces, UDP4/inner IPv6, mixed load',
           'endpoints': []}
    rows.append(row)
    # Preserve the original functional failure before interpreting diagnostics.
    (root / 'runs.json').write_text(json.dumps(rows, indent=2))
    for endpoint in (0, 1):
        log = output / f'r0-stock-core-{endpoint}.log'
        if not log.exists():
            row['endpoints'].append({'endpoint': endpoint, 'missing_log': True})
            continue
        text = log.read_text()
        counts = {}
        for kind in ('lossy', 'nonlossy'):
            values = [int(v) for v in re.findall(rf'ISSUE4_RING_CENSUS kind={kind} count=(\d+)', text)]
            lower = max(values, default=0)
            counts[kind] = {'lower_bound': lower, 'upper_bound_exclusive': lower * 2 if lower else 1,
                            'recorded_bounds': values}
        active = all(f'ISSUE4_PACKET stage={stage} ' in text
                     for stage in ('encrypted_tx', 'udp_rx', 'peer_rx', 'nic_enqueue'))
        row['endpoints'].append({
            'endpoint': endpoint, 'stage_observer_active': active,
            'complete_scope': active and result.returncode == 0,
            'ring_rejections': counts,
            'small_ciphertext_ring_rejection_records': text.count('ISSUE4_PACKET stage=ring_reject '),
            'log_bytes': log.stat().st_size,
        })
    (root / 'runs.json').write_text(json.dumps(rows, indent=2))
    assert result.returncode == 0, f'{output.name}: original lab failed; preserve, do not retry or weaken'
    assert all(e.get('complete_scope') for e in row['endpoints']), 'incomplete observation is not zero loss'
    for endpoint in row['endpoints']:
        for count in endpoint['ring_rejections'].values():
            values = count['recorded_bounds']
            expected = [1 << i for i in range(count['lower_bound'].bit_length())]
            assert sorted(values) == expected, 'census log is incomplete, duplicated or malformed'
print(json.dumps({'complete': True, 'cases': len(rows), 'performance_evidence': False,
                  'original_combined_loss': 'OPEN; this observation cannot erase the earlier failure'}))
