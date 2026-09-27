#!/usr/bin/env python3
"""Layer bounded loss observation over the disposable, frozen GRO experiment."""
from pathlib import Path
import hashlib
import json
import subprocess
import sys

base = '3166ab672d347cdcc5a6768bc77056cd8ec38323'
root = Path(sys.argv[1]).resolve()
assert subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=root, text=True).strip() == base
path = root / 'easytier/src/tunnel/udp.rs'
before = path.read_text()
assert 'mod udp_gro_experiment;' in before, 'apply the unchanged GRO overlay first'
receiver = path.with_name('udp_gro_experiment.rs')
receiver_hash = hashlib.sha256(receiver.read_bytes()).hexdigest()
packet_trace = subprocess.run(
    [sys.executable, str(Path(__file__).with_name('packet_trace_prepare.py')), str(root), base],
    check=True, capture_output=True, text=True,
)
text = path.read_text()
replacements = [
    ('                issue4_packet_trace("ring_reject", &e, None);',
     '                issue4_ring_rejection_census(true);\n                issue4_packet_trace("ring_reject", &e, None);'),
    ('        } else if self.ring_sender.force_send(zc_packet).is_err() {',
     '        } else if self.ring_sender.force_send(zc_packet).is_err() {\n            issue4_ring_rejection_census(false);'),
]
for old, new in replacements:
    assert text.count(old) == 1, ('immutable rejection anchor mismatch', old, text.count(old))
    text = text.replace(old, new, 1)
text += '''
// Observation only: count after rejection, never retry, yield or retain a packet.
// At most usize::BITS lines per class/process. A final power-of-two report gives
// [count, 2*count), not an exact total. No report means zero only with complete
// logs, active packet-stage diagnostics and a clean process exit.
fn issue4_ring_rejection_census(lossy: bool) {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static LOSSY: AtomicUsize = AtomicUsize::new(0);
    static NONLOSSY: AtomicUsize = AtomicUsize::new(0);
    if !issue4_packet_trace_enabled() {
        return;
    }
    let (counter, kind) = if lossy { (&LOSSY, "lossy") } else { (&NONLOSSY, "nonlossy") };
    let count = counter.fetch_add(1, Ordering::Relaxed) + 1;
    if count.is_power_of_two() {
        eprintln!("ISSUE4_RING_CENSUS kind={} count={}", kind, count);
    }
}
'''
path.write_text(text)
assert hashlib.sha256(receiver.read_bytes()).hexdigest() == receiver_hash
print(json.dumps({
    'base': base,
    'packet_stage_overlay': json.loads(packet_trace.stdout),
    'udp_before_observation_sha256': hashlib.sha256(before.encode()).hexdigest(),
    'udp_after_observation_sha256': hashlib.sha256(text.encode()).hexdigest(),
    'receiver_sha256': receiver_hash,
    'scope': 'observation only; unchanged receiver, queue, scheduling, crypto and wire behavior',
    'census': 'power-of-two lower bound and exclusive upper bound; no per-packet logging',
}))
