#!/usr/bin/env python3
"""Prepare only a disposable diagnostic Core checkout; never the release tree."""
import hashlib
import json
from pathlib import Path
import subprocess
import sys

BASE = "c6772dbfef2395ff96b39bd4801945d92212dffb"
root = Path(sys.argv[1]).resolve()
output = Path(sys.argv[2]).resolve() if len(sys.argv) == 3 else None
head = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=root, text=True).strip()
source = root / "easytier/src/tunnel/udp.rs"
if output is None:
    assert head == BASE, (head, BASE)
    subprocess.run(["git", "diff", "--exit-code"], cwd=root, check=True)
    original = source.read_text()
else:
    # Syntax/preparation check only: read the exact object without mutating
    # the current worktree or manufacturing a fake Git identity.
    original = subprocess.check_output(["git", "show", BASE + ":easytier/src/tunnel/udp.rs"], cwd=root, text=True)
text = original


def replace(old, new, count):
    global text
    assert text.count(old) == count, (old, text.count(old), count)
    text = text.replace(old, new)


replace("let ring_for_send_udp = Arc::new(RingTunnel::new(128));",
        "let ring_for_send_udp = Arc::new(RingTunnel::new(issue4_flush_capacity()));", 2)
replace("Box::new(RingSink::new(ring_for_send_udp)),", "issue4_flush_sink(ring_for_send_udp),", 2)
begin = text.index("#[instrument]\nasync fn forward_from_ring_to_udp(")
end = text.index("\nstruct UdpConnection {", begin)
function = text[begin:end]
body = function.index(") -> Option<TunnelError> {") + len(") -> Option<TunnelError> {")
assert function.rstrip().endswith("}")
old_body = function[body:function.rfind("}")]
new_body = """
    #[cfg(target_os = "linux")]
    { issue4_flush::forward(ring_recv, socket, *addr, conn_id, stealth).await }
    #[cfg(not(target_os = "linux"))]
    {
    let mut ring_recv = ring_recv;
""" + old_body + "\n    }\n}"
# The helper takes an address reference, as does the original worker.
new_body = new_body.replace("socket, *addr, conn_id", "socket, addr, conn_id")
signature = function[:body].replace("mut ring_recv: RingStream", "ring_recv: RingStream")
text = text[:begin] + signature + new_body + "\n" + text[end:]
text += "\n" + Path(__file__).with_name("adapter.rs.in").read_text()
(output if output is not None else source).write_text(text)
print(json.dumps({"base": BASE, "original_sha256": hashlib.sha256(original.encode()).hexdigest(),
                  "generated_before_format_sha256": hashlib.sha256(text.encode()).hexdigest(),
                  "scope": "UDP-only outbound staging/kernel submission; receive and shared MPSC unchanged"}))
