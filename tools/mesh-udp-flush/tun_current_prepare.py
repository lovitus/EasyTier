#!/usr/bin/env python3
"""Runner-only capacity/observation overlay on the retained integrated Core."""
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys


BASE = '3166ab672d347cdcc5a6768bc77056cd8ec38323'
assert os.environ.get('GITHUB_ACTIONS') == 'true', 'overlay is CI-only'
root = Path(sys.argv[1]).resolve()
relative = 'easytier/src/instance/linux_tun_offload.rs'
assert subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=root, text=True).strip() == BASE
original = subprocess.check_output(['git', 'show', BASE + ':' + relative], cwd=root, text=True)
source = root / relative
assert source.read_text() == original, 'refuse to overwrite modified source'
text = original


def replace_once(old, new):
    global text
    assert text.count(old) == 1, ('source anchor changed', old)
    text = text.replace(old, new, 1)


# Keep the parent's eligibility checks and exact reclaim/flush ownership. Only
# the allocated head's capacity varies; no ACK selector or receive overlay.
replace_once('    let index = packets.iter().position(|frame| {\n',
             '    let head_capacity = head.as_ref()?.capacity();\n'
             '    let index = packets.iter().position(|frame| {\n')
replace_once('frame.len() >= GRO_HEAD_CAPACITY || frame.capacity() >= GRO_HEAD_CAPACITY',
             'frame.len() >= head_capacity || frame.capacity() >= head_capacity')
replace_once('    gro_head_identity: Option<usize>,\n',
             '    gro_head_identity: Option<usize>,\n'
             '    capacity_observation: Option<TunCapacityObservation>,\n')
replace_once('impl LinuxTunOffloadSink {\n    pub(crate) fn new(device: Arc<AsyncDevice>) -> Self {\n        Self {',
             'impl LinuxTunOffloadSink {\n    pub(crate) fn new(device: Arc<AsyncDevice>) -> Self {\n'
             '        let (capacity, capacity_observation) = TunCapacityObservation::configure();\n'
             '        Self {')
replace_once('            gro_head: Some(BytesMut::with_capacity(GRO_HEAD_CAPACITY)),\n',
             '            gro_head: Some(BytesMut::with_capacity(capacity)),\n')
replace_once('            gro_head_identity: None,\n',
             '            gro_head_identity: None,\n            capacity_observation,\n')
replace_once('            self.gro_head_identity = promote_gro_head(&mut packets, &mut self.gro_head);\n',
             '            self.gro_head_identity = promote_gro_head(&mut packets, &mut self.gro_head);\n'
             '            if let Some(observation) = &mut self.capacity_observation {\n'
             '                observation.cohorts[packets.len()] += 1;\n'
             '                observation.input_packets += packets.len() as u64;\n'
             '                observation.promoted += u64::from(self.gro_head_identity.is_some());\n'
             '                observation.head_before_bytes = self.gro_head_identity.and_then(|identity| {\n'
             '                    packets.iter().find(|packet| packet.as_ptr() as usize == identity)\n'
             '                        .map(BytesMut::len)\n'
             '                }).unwrap_or(0);\n'
             '            }\n')
replace_once('        self.flush_future = None;\n',
             '        self.flush_future = None;\n'
             '        if let Some(observation) = &mut self.capacity_observation {\n'
             '            observation.completed += 1;\n'
             '            observation.failed_flushes += u64::from(result.is_err());\n'
             '        }\n')
replace_once('            self.gro_head = reclaim_gro_head(&mut packets, identity);\n',
             '            if let Some(observation) = &mut self.capacity_observation {\n'
             '                if let Some(head) = packets.iter().find(|packet| packet.as_ptr() as usize == identity) {\n'
             '                    observation.head_grew += u64::from(head.len() > observation.head_before_bytes);\n'
             '                    observation.max_head_bytes = observation.max_head_bytes.max(head.len());\n'
             '                    let bucket = (head.len() / 1024).min(observation.head_length_kib.len() - 1);\n'
             '                    observation.head_length_kib[bucket] += 1;\n'
             '                }\n'
             '            }\n'
             '            self.gro_head = reclaim_gro_head(&mut packets, identity);\n')
replace_once('            if self.gro_head.is_none() {\n',
             '            if self.gro_head.is_none() {\n'
             '                if let Some(observation) = &mut self.capacity_observation {\n'
             '                    observation.scratch_lost += 1;\n'
             '                }\n')
text += '\n' + Path(__file__).with_name('tun_capacity_observation.rs.in').read_text()
source.write_text(text)
print(json.dumps({'base': BASE, 'file': relative,
                  'original_sha256': hashlib.sha256(original.encode()).hexdigest(),
                  'generated_sha256': hashlib.sha256(text.encode()).hexdigest(),
                  'scope': 'same-binary 8/16/32 KiB sink head; counters in all arms; no queue, selector, receive, routing or protocol change'}))
