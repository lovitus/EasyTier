#!/usr/bin/env python3
"""Same-binary receive off/on, using unchanged packet/lifecycle lab assertions."""
import argparse
import json
import os
from pathlib import Path
import subprocess
import sys

parser = argparse.ArgumentParser()
parser.add_argument('--binaries', type=Path, required=True)
parser.add_argument('--output', type=Path, required=True)
parser.add_argument('--phase', choices=['fixed', 'saturation', 'mixed'], required=True)
args = parser.parse_args()
root = args.output.resolve()
root.mkdir(parents=True, exist_ok=False)
binaries = args.binaries.resolve()
lab = Path(__file__).with_name('lab.py').resolve()
order = ['off', 'on', 'on', 'off', 'off', 'on']
families = [(4, 4), (6, 6)] if args.phase == 'fixed' else [(4, 6)]
stealth_modes = [False, True] if args.phase == 'fixed' else [False]
rows = []
for outer, inner in families:
    for stealth in stealth_modes:
        for mode in order:
            index = len(rows)
            output = root / f'{index:02d}-{mode}-outer{outer}-inner{inner}-stealth{int(stealth)}'
            metrics = root / f'{index:02d}-metrics'
            metrics.mkdir()
            env = {**os.environ, 'ET_ISSUE4_UDP_GRO': mode, 'ET_ISSUE4_GRO_METRICS': str(metrics)}
            command = [sys.executable, str(lab), '--stock', str(binaries), '--candidate', str(binaries),
                       '--order', 'stock', '--network-counters', '--output', str(output)]
            if outer == 6: command.append('--underlay-ipv6')
            if inner == 6: command.append('--inner-ipv6')
            if stealth: command.append('--stealth')
            if args.phase != 'fixed':
                command += ['--unpaced-probe', str(binaries / 'probe'), '--transfer-bytes', '1073741824']
            if args.phase == 'mixed': command.append('--mixed-flow')
            with (root / f'{index:02d}.log').open('w') as log:
                result = subprocess.run(command, env=env, stdout=log, stderr=subprocess.STDOUT, timeout=360)
            stats = [json.loads(path.read_text()) for path in sorted(metrics.glob('*.json'))]
            row = {'index': index, 'mode': mode, 'outer': outer, 'inner': inner, 'stealth': stealth,
                   'phase': args.phase, 'exit': result.returncode, 'stats': stats, 'output': output.name}
            rows.append(row)
            (root / 'runs.json').write_text(json.dumps(rows, indent=2))
            assert result.returncode == 0, f'{output.name}: original lab assertion failed; no retry or assertion change'
            assert len(stats) >= 3, 'missing listener/connector shutdown evidence'
            assert all(s['enabled'] == (mode == 'on') for s in stats), 'GRO activation/fallback mismatch'
            assert all(s['option_error'] is None and s['rejected_batches'] == 0 for s in stats)
            if mode == 'on':
                assert all(s['scratch_bytes'] == 65536 for s in stats)
                assert sum(s['gro_batches'] for s in stats) > 0, 'no actual Core GRO receive batch'
                assert max(s['max_batch'] for s in stats) > 1
            else:
                assert all(s['scratch_bytes'] == 0 and s['gro_batches'] == 0 for s in stats)
print(json.dumps({'complete': True, 'cases': len(rows), 'phase': args.phase,
                  'scope': 'same-runner namespaces, one experimental binary; no production or WAN claim'}))
