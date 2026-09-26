#!/usr/bin/env python3
"""Apply a reversible experiment only to immutable, disposable Core source."""
from pathlib import Path
import hashlib
import json
import subprocess
import sys

base = '3166ab672d347cdcc5a6768bc77056cd8ec38323'
root = Path(sys.argv[1]).resolve()
assert subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=root, text=True).strip() == base
assert not subprocess.check_output(['git', 'status', '--porcelain'], cwd=root, text=True).strip()
path = root / 'easytier/src/tunnel/udp.rs'
text = path.read_text()
original = text
replacements = [
    ('pub const UDP_DATA_MTU: usize = 2000;', '''#[cfg(target_os = "linux")]
#[path = "udp_gro_experiment.rs"]
mod udp_gro_experiment;

pub const UDP_DATA_MTU: usize = 2000;'''),
    ('        let socket = self.socket.as_ref().unwrap().clone();\n        let mut buf = BytesMut::new();',
     '''        let socket = self.socket.as_ref().unwrap().clone();
        #[cfg(target_os = "linux")]
        let mut receiver = udp_gro_experiment::Receiver::new(socket.clone());
        let mut buf = BytesMut::new();'''),
    ('            let addr = match socket.recv_buf_from(&mut buf).await {',
     '''            #[cfg(target_os = "linux")]
            let received = receiver.recv(&mut buf).await;
            #[cfg(not(target_os = "linux"))]
            let received = socket.recv_buf_from(&mut buf).await;
            let addr = match received {'''),
    ('        let recv_loop = async move {\n            let mut buf = BytesMut::new();',
     '''        let recv_loop = async move {
            #[cfg(target_os = "linux")]
            let mut receiver = udp_gro_experiment::Receiver::new(socket_clone.clone());
            let mut buf = BytesMut::new();'''),
    ('                let addr = match socket_clone.recv_buf_from(&mut buf).await {',
     '''                #[cfg(target_os = "linux")]
                let received = receiver.recv(&mut buf).await;
                #[cfg(not(target_os = "linux"))]
                let received = socket_clone.recv_buf_from(&mut buf).await;
                let addr = match received {'''),
]
for old, new in replacements:
    assert text.count(old) == 1, ('immutable source anchor mismatch', old, text.count(old))
    text = text.replace(old, new, 1)
path.write_text(text)
module = Path(__file__).with_name('udp_gro_receiver.rs').read_bytes()
destination = path.with_name('udp_gro_experiment.rs')
with destination.open('xb') as stream:
    stream.write(module)
print(json.dumps({'base': base, 'udp_before': hashlib.sha256(original.encode()).hexdigest(),
                  'udp_after': hashlib.sha256(text.encode()).hexdigest(),
                  'receiver_sha256': hashlib.sha256(module).hexdigest(),
                  'scope': 'Linux receive adapter only; same binary off/on; production source unchanged'}))
