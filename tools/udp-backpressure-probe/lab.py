#!/usr/bin/env python3
"""Bounded isolated UDP EAGAIN mechanism check, not Core acceptance."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import select
import signal
import socket
import subprocess
import sys


def receiver(address, port):
    family = socket.AF_INET6 if ":" in address else socket.AF_INET
    with socket.socket(family, socket.SOCK_DGRAM) as sock:
        sock.bind((address, int(port)))
        sock.settimeout(12)
        print("ready", flush=True)
        count = 0
        while count <= 256:
            data, _ = sock.recvfrom(4096)
            if len(data) == 12 and data[:4] == b"DONE":
                assert int.from_bytes(data[4:], "big") == count
                print(json.dumps({"received": count, "ordered": True, "bytes_verified": True}))
                return
            assert len(data) == 1200
            assert int.from_bytes(data[:8], "big") == count
            assert data[8:] == b"\x5a" * 1192
            count += 1
        raise RuntimeError("receive budget exceeded")


def run(args):
    assert sys.platform == "linux" and os.geteuid() == 0
    output = Path(args.output).resolve()
    output.mkdir(parents=True, exist_ok=False)
    binary = Path(args.binary).resolve()
    names = [f"etbp{os.getpid()}a", f"etbp{os.getpid()}b"]
    created, children, cases = [], [], []

    def command(argv, ns=None, check=True):
        prefix = ["ip", "netns", "exec", ns] if ns else []
        return subprocess.run(prefix + list(map(str, argv)), check=check,
                              capture_output=True, text=True, timeout=20)

    def routes():
        return {family: json.loads(command(["ip", "-j", family, "route", "show", "table", "all"]).stdout)
                for family in ("-4", "-6")}

    def save(name, value):
        (output / name).write_text(json.dumps(value, indent=2))

    def interrupted(*_):
        raise RuntimeError("interrupted")

    signal.signal(signal.SIGTERM, interrupted)
    before = routes()
    save("routes-before.json", before)
    save("identity.json", {"binary_sha256": hashlib.sha256(binary.read_bytes()).hexdigest(),
                           "kernel": os.uname().release,
                           "scope": "actual experiment send functions; not full Core acceptance" if args.adapter else
                                    "kernel EAGAIN/readiness; no Core/Tokio/GSO acceptance"})
    try:
        for ns in names:
            command(["ip", "netns", "add", ns])
            created.append(ns)
            command(["ip", "-n", ns, "link", "set", "lo", "up"])
        command(["ip", "link", "add", "under0", "netns", names[0], "type", "veth",
                 "peer", "name", "under0", "netns", names[1]])
        for index, ns in enumerate(names, 1):
            command(["ip", "-n", ns, "addr", "add", f"192.0.2.{index}/30", "dev", "under0"])
            command(["ip", "-n", ns, "-6", "addr", "add", f"fd00:8888::{index}/64", "dev", "under0", "nodad"])
            command(["ip", "-n", ns, "link", "set", "under0", "up"])
        peer_mac = json.loads(command(["ip", "-j", "-n", names[1], "link", "show", "under0"]).stdout)[0]["address"]
        for family, destination in [("-4", "192.0.2.2"), ("-6", "fd00:8888::2")]:
            command(["ip", "-n", names[0], family, "neigh", "replace", destination,
                     "lladdr", peer_mac, "nud", "permanent", "dev", "under0"])
        command(["tc", "qdisc", "add", "dev", "under0", "root", "tbf", "rate", "32kbit",
                 "burst", "1600", "limit", "65536"], names[0])
        for family, source, destination in [("ipv4", "192.0.2.1", "192.0.2.2"),
                                            ("ipv6", "fd00:8888::1", "fd00:8888::2")]:
            modes = ["recover", "cancel", "shared", "reject-mtu"] if args.adapter else ["kernel"]
            if args.adapter and family == "ipv4":
                modes.append("reject-checksum")
            for mode in modes:
                mtu = 1280 if mode == "reject-mtu" else 1500
                for ns in names:
                    command(["ip", "-n", ns, "link", "set", "under0", "mtu", str(mtu)])
                label = f"{family}-{mode}"
                receivers = []
                for port in ([35906, 35907] if mode == "shared" else [35906]):
                    proc = subprocess.Popen(["ip", "netns", "exec", names[1], sys.executable,
                                             str(Path(__file__).resolve()), "receive", destination, str(port)],
                                            stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
                    children.append(proc)
                    receivers.append(proc)
                    ready, _, _ = select.select([proc.stdout], [], [], 3)
                    assert ready and proc.stdout.readline().strip() == "ready", "receiver startup failed"
                bind = f"[{source}]:0" if family == "ipv6" else f"{source}:0"
                remote = f"[{destination}]:35906" if family == "ipv6" else f"{destination}:35906"
                second = f"[{destination}]:35907" if family == "ipv6" else f"{destination}:35907"
                extra = [mode, second] if args.adapter else []
                sent = command(["env", "ET_UDP_BACKPRESSURE=ISOLATED_NETNS_ONLY", binary, bind, remote, *extra],
                               names[0], check=False)
                (output / f"{label}-sender.stdout").write_text(sent.stdout)
                (output / f"{label}-sender.stderr").write_text(sent.stderr)
                assert sent.returncode == 0, f"{label} sender: {sent.stderr}"
                tx, received = json.loads(sent.stdout), []
                for index, proc in enumerate(receivers):
                    stdout, stderr = proc.communicate(timeout=15)
                    (output / f"{label}-receiver-{index}.stderr").write_text(stderr)
                    assert proc.returncode == 0, f"{label} receiver: {stderr}"
                    rx = json.loads(stdout)
                    assert rx["received"] == (tx["sent_datagrams"] if index == 0 else 4)
                    received.append(rx)
                if mode.startswith("reject-"):
                    assert not tx["kernel_eagain"] and tx["gso_rejection_errno"] == 22
                    assert tx["ordinary_errno"] == (90 if mode == "reject-mtu" else 0)
                    assert tx["gso_after_error"] and tx["gso_calls"] >= 2
                elif args.adapter:
                    assert tx["kernel_eagain"]
                    assert tx["adapter_eagain"] > 0 and tx["gso_calls"] > 0 and tx["heartbeat_ticks"] > 0
                else:
                    assert tx["kernel_eagain"]
                    assert tx["writable_notifications"] > 0
                qdisc = json.loads(command(["tc", "-s", "-j", "qdisc", "show", "dev", "under0"], names[0]).stdout)
                save(f"{label}-qdisc.json", qdisc)
                assert qdisc and all(item.get("drops", 0) == 0 for item in qdisc), "qdisc dropped packets"
                cases.append({"family": family, "mode": mode, "sender": tx, "receivers": received})
                save("cases.json", cases)
    except BaseException as error:
        save("failure.json", {"error": repr(error)})
        raise
    finally:
        cleanup = []
        for child in children:
            forced = False
            if child.poll() is None:
                child.terminate()
                try:
                    child.wait(timeout=2)
                except subprocess.TimeoutExpired:
                    forced = True
                    child.kill()
                    child.wait(timeout=2)
            cleanup.append({"process_exit": child.returncode, "forced": forced})
        for ns in reversed(created):
            pids = command(["ip", "netns", "pids", ns], check=False).stdout.strip()
            result = command(["ip", "netns", "delete", ns], check=False)
            cleanup.append({"namespace": ns, "exit": result.returncode, "remaining": pids})
        save("cleanup.json", cleanup)
        after = routes()
        save("routes-after.json", after)
        assert before == after, "host routes changed"
    assert len(cases) == (9 if args.adapter else 2)
    assert all(not row.get("forced") and not row.get("remaining") and
               row.get("process_exit", row.get("exit")) == 0 for row in cleanup)
    print(json.dumps({"cases": cases, "cleanup_ok": True, "host_routes_unchanged": True}))


if __name__ == "__main__":
    if len(sys.argv) > 1 and sys.argv[1] == "receive":
        receiver(*sys.argv[2:])
    else:
        parser = argparse.ArgumentParser()
        parser.add_argument("--binary", required=True)
        parser.add_argument("--output", required=True)
        parser.add_argument("--adapter", action="store_true")
        run(parser.parse_args())
