#!/usr/bin/env python3
"""Generate a test-only adapted slicing candidate from an exact source snapshot."""
import difflib
import hashlib
import json
import subprocess
import sys
import tomllib
from pathlib import Path
FORK='c6772dbfef2395ff96b39bd4801945d92212dffb'
root=Path(sys.argv[1]).resolve();root.mkdir(parents=True,exist_ok=True)
def show(path):
    return subprocess.check_output(['git','show',f'{FORK}:{path}'],text=True)
original=show('easytier/src/tunnel/packet_def.rs')
patched='use bytes::Buf;\n'+original
helper='''    // Consuming extraction: keep legacy behavior for malformed len/cap combinations.
    // Valid packets advance without promoting unique storage to shared bookkeeping.
    fn into_inner_from_offset(mut self, offset: usize) -> BytesMut {
        if offset <= self.inner.len() {
            self.inner.advance(offset);
            self.inner
        } else {
            self.inner.split_off(offset)
        }
    }

'''
patched=patched.replace('impl ZCPacket {\n','impl ZCPacket {\n'+helper,1)
repls={
'''    pub fn payload_bytes(mut self) -> BytesMut {
        self.inner.split_off(self.payload_offset())
    }''':'''    pub fn payload_bytes(self) -> BytesMut {
        let offset = self.payload_offset();
        self.into_inner_from_offset(offset)
    }''',
'''    pub fn tunnel_payload_bytes(mut self) -> BytesMut {
        self.inner.split_off(
            self.packet_type
                .get_packet_offsets()
                .peer_manager_header_offset,
        )
    }''':'''    pub fn tunnel_payload_bytes(self) -> BytesMut {
        let offset = self.packet_type.get_packet_offsets().peer_manager_header_offset;
        self.into_inner_from_offset(offset)
    }''',
'''        Self::new_from_buf(self.inner.split_off(new_offset), target_packet_type)''':'''        Self::new_from_buf(self.into_inner_from_offset(new_offset), target_packet_type)''',
'''    pub fn foreign_network_packet(mut self) -> Self {
        let hdr = self.foreign_network_hdr().unwrap();
        let foreign_hdr_len = hdr.get_header_len();

        Self::new_from_buf(
            self.inner
                .split_off(foreign_hdr_len + self.payload_offset()),
            ZCPacketType::DummyTunnel,
        )
    }''':'''    pub fn foreign_network_packet(self) -> Self {
        let hdr = self.foreign_network_hdr().unwrap();
        let foreign_hdr_len = hdr.get_header_len();
        let offset = foreign_hdr_len + self.payload_offset();
        Self::new_from_buf(self.into_inner_from_offset(offset), ZCPacketType::DummyTunnel)
    }'''
}
for old,new in repls.items():
    if patched.count(old)!=1:raise RuntimeError('source guard mismatch: '+old[:80])
    patched=patched.replace(old,new,1)
lock=tomllib.loads(show('Cargo.lock'))
def version(name,prefix=''):
    values=[p['version'] for p in lock['package'] if p['name']==name and p['version'].startswith(prefix)]
    if len(values)!=1:raise RuntimeError(f'non-unique dependency {name}: {values}')
    return values[0]
versions={k:version(k,p) for k,p in [('bytes',''),('zerocopy','0.7.'),('tracing','')]}
(root/'src').mkdir(exist_ok=True)
(root/'src/original.rs').write_text(original)
(root/'src/patched.rs').write_text(patched)
(root/'packet_def.candidate.rs').write_text(patched)
(root/'candidate.patch').write_text(''.join(difflib.unified_diff(original.splitlines(True),patched.splitlines(True),fromfile='a/easytier/src/tunnel/packet_def.rs',tofile='b/easytier/src/tunnel/packet_def.rs')))
(root/'Cargo.toml').write_text(f'''[package]
name="issue4-packet-verification"
version="0.1.0"
edition="2024"
publish=false
[workspace]
[dependencies]
bytes="={versions['bytes']}"
zerocopy={{version="={versions['zerocopy']}",features=["derive","simd"]}}
tracing="={versions['tracing']}"
[features]
zstd=[]
''')
(root/'Cargo.lock').write_text(show('Cargo.lock'))
(root/'provenance.json').write_text(json.dumps({'source':FORK,'source_path':'easytier/src/tunnel/packet_def.rs','original_sha256':hashlib.sha256(original.encode()).hexdigest(),'candidate_sha256':hashlib.sha256(patched.encode()).hexdigest(),'dependency_versions':versions,'note':'Adaptation of upstream f24735a8, not a whole cherry-pick. Malformed capacity fallback retained. No repository production file changed.'},indent=2))
print('ETVERIFY_PACKET_SOURCE '+(root/'provenance.json').read_text().replace('\n',' '),flush=True)
