#!/usr/bin/env python3
"""Overlay the isolated exact source; never edit a release checkout."""
import hashlib
import json
from pathlib import Path
import subprocess
import sys

BASE = 'c6772dbfef2395ff96b39bd4801945d92212dffb'
root = Path(sys.argv[1]).resolve()
relative = 'easytier/src/instance/linux_tun_offload.rs'
original = subprocess.check_output(['git', 'show', BASE + ':' + relative], cwd=root, text=True)
assert subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=root, text=True).strip() == BASE
source = root / relative
assert source.read_text() == original, 'refuse to replace modified TUN source'
begin = original.index('type FlushFuture =')
end = original.index('impl Sink<ZCPacket> for LinuxTunOffloadSink')
text = original[:begin] + Path(__file__).with_name('tun_capacity.rs.in').read_text() + '\n' + original[end:]
source.write_text(text)
print(json.dumps({'base': BASE, 'original_sha256': hashlib.sha256(original.encode()).hexdigest(),
                  'generated_sha256': hashlib.sha256(text.encode()).hexdigest(),
                  'scope': 'isolated Linux TUN head capacity; no batching waits or protocol changes'}))
