#!/usr/bin/env python3
"""One retained-parent binary, interleaved capacities, unchanged lab gates."""
import argparse
import json
from pathlib import Path
import subprocess
import sys


parser = argparse.ArgumentParser()
parser.add_argument('phase', choices=['fixed', 'trace', 'saturation'])
parser.add_argument('--binaries', type=Path, required=True)
parser.add_argument('--output', type=Path, required=True)
parser.add_argument('--probe', type=Path)
args = parser.parse_args()
binary_dir = args.binaries.resolve()
root = args.output.resolve()
root.mkdir(parents=True, exist_ok=False)
lab = Path(__file__).with_name('lab.py').resolve()
assert (binary_dir / 'easytier-core').is_file()
assert (binary_dir / 'easytier-cli').is_file()
order = [8192, 16384, 32768, 32768, 16384, 8192, 16384, 8192, 32768]
if args.phase == 'fixed':
    cases = [(family, rate, capacity) for rate in (250, 500)
             for family in (4, 6) for capacity in order]
elif args.phase == 'trace':
    cases = [(family, 100, capacity) for family in (4, 6)
             for capacity in (8192, 16384, 32768)]
else:
    assert args.probe and args.probe.is_file()
    cases = [(6, None, capacity) for capacity in
             (8192, 16384, 32768, 32768, 16384, 8192)]
rows = []
for index, (family, per_flow_mbps, capacity) in enumerate(cases):
    output = root / f'{index:02d}-v{family}-rate{per_flow_mbps}-head{capacity}'
    command = [sys.executable, str(lab), '--stock', str(binary_dir),
               '--candidate', str(binary_dir), '--order', 'stock',
               '--output', str(output), '--tun-head-capacity', str(capacity),
               '--mixed-flow', '--network-counters', '--full-transfer-control']
    if family == 6:
        command.append('--inner-ipv6')
    if args.phase == 'saturation':
        command += ['--unpaced-probe', str(args.probe.resolve())]
    else:
        size = 268435456 if args.phase == 'fixed' else 67108864
        command += ['--paced-mbps', str(per_flow_mbps), '--paced-transfer-bytes', str(size)]
    if args.phase == 'trace':
        command.append('--tun-trace')
    with (root / f'{index:02d}.log').open('w') as log:
        result = subprocess.run(command, stdout=log, stderr=subprocess.STDOUT, timeout=360)
    rows.append({'index': index, 'inner_ip_family': family, 'underlay_ip_family': 4,
                 'capacity': capacity, 'per_flow_mbps': per_flow_mbps,
                 'mixed_flow': True, 'traced': args.phase == 'trace',
                 'exit_code': result.returncode, 'output': output.name})
    (root / 'runs.json').write_text(json.dumps(rows, indent=2))
    assert result.returncode == 0, 'original lab failed; stop and retain its errors/cleanup'
    records = [json.loads(line) for line in (output / 'results.jsonl').read_text().splitlines()]
    stats = [row['stats'] for row in records if row['kind'] == 'tun_capacity']
    assert len(stats) == 2, 'both endpoints must report the selected capacity'
    assert all(row['failed_flushes'] == 0 and row['capacity'] == capacity for row in stats)
    assert all(row['promoted'] > 0 and row['head_grew'] > 0 for row in stats), 'head never grew; no capacity comparison claim'
