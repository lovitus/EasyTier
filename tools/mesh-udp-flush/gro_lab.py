#!/usr/bin/env python3
"""Same-binary receive off/on, using unchanged packet/lifecycle lab assertions."""
import argparse
import json
from pathlib import Path
import re
import subprocess
import sys

parser = argparse.ArgumentParser()
parser.add_argument('--binaries', type=Path, required=True)
parser.add_argument('--output', type=Path, required=True)
parser.add_argument('--phase', choices=['activation', 'fixed', 'saturation', 'mixed'], required=True)
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
for outer, inner in families:
    for stealth in stealth_modes:
        for mode in order:
            index = len(rows)
            output = root / f'{index:02d}-{mode}-outer{outer}-inner{inner}-stealth{int(stealth)}'
            command = [sys.executable, str(lab), '--stock', str(binaries), '--candidate', str(binaries),
                       '--order', 'stock', '--network-counters', '--output', str(output),
                       '--udp-gro-mode', mode]
            if outer == 6: command.append('--underlay-ipv6')
            if inner == 6: command.append('--inner-ipv6')
            if stealth: command.append('--stealth')
            if args.phase in ('saturation', 'mixed'):
                command += ['--unpaced-probe', str(binaries / 'probe'), '--transfer-bytes', '1073741824']
            if args.phase == 'mixed': command.append('--mixed-flow')
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
            rows.append(row)
            (root / 'runs.json').write_text(json.dumps(rows, indent=2))
            assert result.returncode == 0, f'{output.name}: original lab assertion failed; no retry or assertion change'
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
print(json.dumps({'complete': True, 'cases': len(rows), 'phase': args.phase,
                  'scope': 'same-runner namespaces, one experimental binary; no production or WAN claim'}))
