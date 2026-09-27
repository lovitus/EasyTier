#!/usr/bin/env python3
"""Same-binary receive off/on, using unchanged packet/lifecycle lab assertions."""
import argparse
import json
from pathlib import Path
import re
import statistics
import subprocess
import sys

parser = argparse.ArgumentParser()
parser.add_argument('--binaries', type=Path, required=True)
parser.add_argument('--output', type=Path, required=True)
parser.add_argument('--phase', choices=['activation', 'fixed', 'saturation', 'mixed', 'pressure'], required=True)
args = parser.parse_args()
root = args.output.resolve()
root.mkdir(parents=True, exist_ok=False)
binaries = args.binaries.resolve()
lab = Path(__file__).with_name('lab.py').resolve()
order = ['off', 'on', 'on', 'off', 'off', 'on']
if args.phase == 'activation': order = ['off', 'on']
families = [(4, 4), (6, 6)] if args.phase in ('fixed', 'activation') else [(4, 6)]
stealth_modes = [False, True] if args.phase == 'fixed' else [False]
if args.phase == 'activation': stealth_modes = [True]
rows = []
pressure_samples = []
cases = []
if args.phase == 'pressure':
    # Same target total supply, not the different achieved saturation rates.
    # No Core changes, larger queues, catch-up bursts or rate-adaptive settings.
    for total_cap in (500, 1000):
        for mixed in (False, True):
            cases.extend((4, 6, False, mode, total_cap, mixed) for mode in order)
else:
    for outer, inner in families:
        for stealth in stealth_modes:
            cases.extend((outer, inner, stealth, mode, None, args.phase == 'mixed') for mode in order)
for outer, inner, stealth, mode, total_cap, mixed in cases:
            index = len(rows)
            suffix = f'-rate{total_cap}-mixed{int(mixed)}' if total_cap is not None else ''
            output = root / f'{index:02d}-{mode}-outer{outer}-inner{inner}-stealth{int(stealth)}{suffix}'
            command = [sys.executable, str(lab), '--stock', str(binaries), '--candidate', str(binaries),
                       '--order', 'stock', '--network-counters', '--output', str(output),
                       '--udp-gro-mode', mode]
            if outer == 6: command.append('--underlay-ipv6')
            if inner == 6: command.append('--inner-ipv6')
            if stealth: command.append('--stealth')
            if args.phase in ('saturation', 'mixed'):
                command += ['--unpaced-probe', str(binaries / 'probe'), '--transfer-bytes', '1073741824']
            if mixed: command.append('--mixed-flow')
            if args.phase == 'pressure':
                per_flow_cap = total_cap // (2 if mixed else 1)
                command += ['--paced-mbps', str(per_flow_cap), '--paced-transfer-bytes', '268435456',
                            '--full-transfer-control']
            if args.phase == 'activation':
                # Independent activation observation, never a performance sample.
                # Payloads are not printed; work is bounded by the unchanged lab.
                command += ['--paced-mbps', '50']
                command = ['strace', '-ff', '-qq', '-s', '0', '-e', 'trace=setsockopt,recvmsg',
                           '-o', str(root / f'{index:02d}-recv'), *command]
            with (root / f'{index:02d}.log').open('w') as log:
                result = subprocess.run(command, stdout=log, stderr=subprocess.STDOUT, timeout=360)
            row = {'index': index, 'mode': mode, 'outer': outer, 'inner': inner, 'stealth': stealth,
                   'phase': args.phase, 'exit': result.returncode, 'output': output.name,
                   'performance_evidence': args.phase != 'activation'}
            if args.phase == 'pressure':
                row.update(total_cap_mbps=total_cap, per_flow_cap_mbps=per_flow_cap, mixed=mixed)
            rows.append(row)
            (root / 'runs.json').write_text(json.dumps(rows, indent=2))
            assert result.returncode == 0, f'{output.name}: original lab assertion failed; no retry or assertion change'
            if args.phase == 'pressure':
                for event in map(json.loads, (output / 'results.jsonl').read_text().splitlines()):
                    if event['kind'] not in ('transfer', 'mixed_transfer'): continue
                    assert event['result']['rate_cap_mbps'] == per_flow_cap
                    pressure_samples.append({'mode': mode, 'total_cap_mbps': total_cap, 'mixed': mixed,
                                             'role': event['kind'], 'direction': event['direction'],
                                             'mbps': event['result']['bits_per_second'] / 1e6})
            if args.phase == 'activation':
                peers = [f'192.0.2.{i}' if outer == 4 else f'2001:db8:88::{i}' for i in (1, 2)]
                enabled = 0
                grouped = {peer: 0 for peer in peers}
                trace_bytes = 0
                for path in root.glob(f'{index:02d}-recv.*'):
                    trace_bytes += path.stat().st_size
                    for line in path.open(errors='replace'):
                        if ('setsockopt(' in line and 'UDP_GRO' in line and
                                re.search(r', \[1\], 4\)\s*=\s*0\s*$', line)):
                            enabled += 1
                        if (('recvmsg(' in line or 'recvmsg resumed>' in line) and
                                re.search(r'cmsg_type=(UDP_GRO|0x68|104)\b', line)):
                            length = re.search(r'=\s*([1-9][0-9]*)\s*$', line)
                            if length and int(length[1]) > 2000:
                                for peer in peers:
                                    if f'"{peer}"' in line: grouped[peer] += 1
                row['activation'] = {'successful_enable_calls': enabled,
                                     'gro_receives_larger_than_one_frame_by_source': grouped,
                                     'trace_bytes': trace_bytes}
                (root / 'runs.json').write_text(json.dumps(rows, indent=2))
                if mode == 'on':
                    assert enabled >= 3 and all(grouped.values()), 'GRO not proven on both endpoints'
                else:
                    assert enabled == 0 and not any(grouped.values()), 'off arm unexpectedly enabled GRO'
if args.phase == 'pressure':
    comparisons = []
    keys = sorted({(r['total_cap_mbps'], r['mixed'], r['role'], r['direction']) for r in pressure_samples})
    for cap, mixed, role, direction in keys:
        values = {mode: [r['mbps'] for r in pressure_samples if
                        (r['total_cap_mbps'], r['mixed'], r['role'], r['direction'], r['mode']) ==
                        (cap, mixed, role, direction, mode)] for mode in ('off', 'on')}
        assert all(len(v) == 3 for v in values.values()), 'missing pressure repetition'
        off, on = (statistics.median(values[mode]) for mode in ('off', 'on'))
        delta = abs(on / off - 1)
        comparisons.append({'total_cap_mbps': cap, 'mixed': mixed, 'role': role, 'direction': direction,
                            'samples_mbps': values, 'off_median_mbps': off, 'on_median_mbps': on,
                            'relative_rate_difference': delta, 'matched_within_two_percent': delta <= .02})
    (root / 'rate-matching.json').write_text(json.dumps(comparisons, indent=2))
    assert len(comparisons) == 12 and all(r['matched_within_two_percent'] for r in comparisons), 'off/on actual load not matched; preserve measurements, do not claim equal-load CPU benefit'
print(json.dumps({'complete': True, 'cases': len(rows), 'phase': args.phase,
                  'scope': 'same-runner namespaces, one experimental binary; no production or WAN claim'}))
