#!/usr/bin/env python3
"""Add bounded packet-correlation diagnostics only to the disposable Core tree."""
from pathlib import Path
import hashlib
import json
import subprocess
import sys

root = Path(sys.argv[1]).resolve()
base = 'c6772dbfef2395ff96b39bd4801945d92212dffb'
assert subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=root, text=True).strip() == base
changes = {}

def patch(relative, replacements, suffix=''):
    path = root / relative
    text = path.read_text()
    original = text
    for old, new in replacements:
        assert text.count(old) == 1, (relative, old, text.count(old))
        text = text.replace(old, new, 1)
    text += suffix
    path.write_text(text)
    changes[relative] = {'before': hashlib.sha256(original.encode()).hexdigest(),
                         'after': hashlib.sha256(text.encode()).hexdigest()}

prefix = 'crate::tunnel::udp::'
patch('easytier/src/peers/peer_manager.rs', [
    ('        let compressor = DefaultCompressor {};\n        compressor\n',
     f'        let issue4_identity = {prefix}issue4_icmp_identity(msg);\n        let compressor = DefaultCompressor {{}};\n        compressor\n'),
    ('            encryptor.encrypt(msg).with_context(|| "encrypt failed")?;\n        }\n        Ok(())',
     f'            encryptor.encrypt(msg).with_context(|| "encrypt failed")?;\n        }}\n        if issue4_identity.is_some() {{ {prefix}issue4_packet_trace("encrypted_tx", msg, issue4_identity); }}\n        Ok(())'),
    ('                    if !secure_mode_enabled {\n                        if let Err(e) = encryptor.decrypt(&mut ret)',
     f'                    {prefix}issue4_packet_trace("peer_rx", &ret, None);\n                    if !secure_mode_enabled {{\n                        if let Err(e) = encryptor.decrypt(&mut ret)'),
    ('                    tracing::trace!(?packet, "send packet to nic channel");',
     f'                    if let Some(identity) = {prefix}issue4_icmp_identity(&packet) {{ {prefix}issue4_packet_trace("nic_enqueue", &packet, Some(identity)); }}\n                    tracing::trace!(?packet, "send packet to nic channel");'),
])
patch('easytier/src/tunnel/udp.rs', [
    ('        if zc_packet.is_lossy() {\n            if let Err(e) = self.ring_sender.try_send(zc_packet) {',
     '        issue4_packet_trace("udp_rx", &zc_packet, None);\n        if zc_packet.is_lossy() {\n            if let Err(e) = self.ring_sender.try_send(zc_packet) {\n                issue4_packet_trace("ring_reject", &e, None);'),
], '\n' + Path(__file__).with_name('packet_trace.rs.in').read_text())
print(json.dumps({'base': base, 'files': changes, 'scope': 'bounded isolated tracing; queue policy unchanged'}))
