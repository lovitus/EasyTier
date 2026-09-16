"""Isolated kernel mechanism evidence. Not a Core/crypto compatibility test."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import signal
import socket
import statistics
import subprocess
import sys
import time


def integrity(role, direction):
    count = 1048595
    chunk = bytes(range(256)) * 256
    expected = hashlib.sha256((chunk * (count // len(chunk) + 1))[:count]).hexdigest()
    if role == "server":
        listener = socket.socket()
        listener.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
        listener.settimeout(5)
        listener.bind(("10.88.0.2", 35802))
        listener.listen(1)
        sock, _ = listener.accept()
        listener.close()
    else:
        sock = socket.create_connection(("10.88.0.2", 35802), timeout=5)
    sock.settimeout(5)
    sending = (role == "client") == (direction == "upload")
    if sending:
        remaining = count
        while remaining:
            part = chunk[:min(remaining, len(chunk))]
            sock.sendall(part)
            remaining -= len(part)
        sock.shutdown(socket.SHUT_WR)
        response = b""
        while True:
            part = sock.recv(128)
            if not part:
                break
            response += part
        assert response.decode() == expected
    else:
        digest = hashlib.sha256()
        received = 0
        while True:
            part = sock.recv(65536)
            if not part:
                break
            received += len(part)
            digest.update(part)
        assert received == count and digest.hexdigest() == expected
        sock.sendall(digest.hexdigest().encode())
        sock.shutdown(socket.SHUT_WR)
    sock.close()
    print(json.dumps({"ok": True, "bytes": count, "sha256": expected}))


def lab(args):
    root = Path(args.output).resolve()
    root.mkdir(parents=True, exist_ok=False)
    binary, probe = Path(args.binary).resolve(), Path(args.probe).resolve()
    a, b = "etnca" + str(os.getpid()), "etncb" + str(os.getpid())
    names, children, created, rows = [a, b], [], [], []

    def record(row):
        rows.append(row)
        with (root / "results.jsonl").open("a") as out:
            out.write(json.dumps(row) + "\n")

    def command(command, ns=None, check=True, timeout=20):
        p = subprocess.run((["ip", "netns", "exec", ns] if ns else []) + list(map(str, command)),
                           capture_output=True, text=True, timeout=timeout)
        if check and p.returncode:
            raise RuntimeError(f"{command}: exit={p.returncode}: {p.stderr}")
        return p

    def spawn(command, name, ns):
        with (root / (name + ".log")).open("w") as log:
            p = subprocess.Popen(["ip", "netns", "exec", ns] + list(map(str, command)),
                                 stdout=log, stderr=log, start_new_session=True)
        children.append(p)
        return p

    def stop(p):
        killed = False
        if p.poll() is None:
            os.killpg(p.pid, signal.SIGTERM)
            try:
                p.wait(timeout=4)
            except subprocess.TimeoutExpired:
                killed = True
                os.killpg(p.pid, signal.SIGKILL)
                p.wait(timeout=2)
        return {"pid": p.pid, "exit": p.returncode, "killed": killed}

    def snapshot(processes):
        values = []
        for p in processes:
            stat = Path(f"/proc/{p.pid}/stat").read_text().rsplit(")", 1)[1].split()
            values.append({"pid": p.pid, "start": int(stat[19]),
                           "cpu": (int(stat[11]) + int(stat[12])) / os.sysconf("SC_CLK_TCK"),
                           "rss": int(stat[21]) * os.sysconf("SC_PAGESIZE")})
        return values

    def interrupted(sig, _frame):
        raise RuntimeError(f"interrupted by {sig}")

    signal.signal(signal.SIGTERM, interrupted)
    signal.signal(signal.SIGINT, interrupted)
    before_routes = {family: command(["ip", "-j", family, "route", "show", "table", "all"]).stdout for family in ["-4", "-6"]}
    try:
        record({"kind": "binary", "sha256": hashlib.sha256(binary.read_bytes()).hexdigest(),
                "warning": "No Core, no crypto, no policy; kernel mechanism only"})
        for ns in names:
            command(["ip", "netns", "add", ns])
            created.append(ns)
            command(["ip", "-n", ns, "link", "set", "lo", "up"])
            command(["sysctl", "-qw", "net.ipv6.conf.all.disable_ipv6=1", "net.ipv6.conf.default.disable_ipv6=1"], ns)
        va, vb = "nca" + str(os.getpid()), "ncb" + str(os.getpid())
        command(["ip", "link", "add", va, "type", "veth", "peer", "name", vb])
        for i, (ns, interface) in enumerate(zip(names, [va, vb])):
            command(["ip", "link", "set", interface, "netns", ns])
            command(["ip", "-n", ns, "link", "set", interface, "name", "under0"])
            command(["ip", "-n", ns, "addr", "add", f"192.0.2.{i+1}/30", "dev", "under0"])
            command(["ip", "-n", ns, "link", "set", "under0", "mtu", "1500", "up"])
        order = ["single", "mmsg", "gso", "gso", "mmsg", "single", "mmsg", "single", "gso"]
        for number, mode in enumerate(order):
            processes = [spawn(["env", "ET_KERNEL_COHORT_LAB=1", binary, mode,
                                f"192.0.2.{i+1}:35804", f"192.0.2.{2-i}:35804"],
                               f"r{number}-{mode}-{i}", ns) for i, ns in enumerate(names)]
            time.sleep(1)
            for i, ns in enumerate(names):
                assert processes[i].poll() is None, "probe exited during startup"
                command(["ip", "-n", ns, "addr", "add", f"10.88.0.{i+1}/24", "dev", "cohort0"])
                command(["ip", "-n", ns, "link", "set", "cohort0", "up"])
                record({"kind": "tun", "round": number, "endpoint": i,
                        "state": json.loads(command(["ip", "-j", "-d", "link", "show", "cohort0"], ns).stdout)})
            for direction in (["upload", "download"] if number % 2 == 0 else ["download", "upload"]):
                label = f"r{number}-{mode}-{direction}"
                server = spawn([sys.executable, __file__, "integrity", "server", direction], label + "-integrity-server", b)
                time.sleep(.2)
                result = command([sys.executable, __file__, "integrity", "client", direction], a)
                server.wait(timeout=3)
                assert server.returncode == 0
                record({"kind": "integrity", "round": number, "mode": mode, "direction": direction, "result": json.loads(result.stdout)})
                server = spawn([probe, "server", "--listen", "10.88.0.2:35803", "--sessions", "1", "--timeout-seconds", "20"], label + "-server", b)
                time.sleep(.2)
                ping = spawn(["ping", "-c", "20", "-i", "0.1", "-W", "1", "10.88.0.2"], label + "-ping", a)
                before = snapshot(processes)
                result = command([probe, "client", "--target", "10.88.0.2:35803", "--direction", direction, "--bytes", str(536870912), "--timeout-seconds", "20"], a, check=False, timeout=25)
                after = snapshot(processes)
                (root / (label + "-client.json")).write_text(result.stdout)
                (root / (label + "-client.stderr")).write_text(result.stderr)
                server.wait(timeout=3)
                assert result.returncode == 0 and server.returncode == 0, "transfer failed"
                data = json.loads(result.stdout)
                assert data["ok"] and data["bytes"] == 536870912
                for first, last in zip(before, after):
                    assert (first["pid"], first["start"]) == (last["pid"], last["start"])
                ping.wait(timeout=5)
                text = (root / (label + "-ping.log")).read_text()
                loss = re.search(r"([0-9.]+)% packet loss", text)
                assert ping.returncode == 0 and loss and float(loss[1]) == 0, "ICMP progress failed"
                record({"kind": "transfer", "round": number, "mode": mode, "direction": direction,
                        "before": before, "after": after, "result": data,
                        "cpu_s_GiB": sum(last["cpu"]-first["cpu"] for first, last in zip(before, after))/.5})
            exits = [stop(p) for p in processes]
            record({"kind": "stop", "round": number, "values": exits})
            assert all(r["exit"] == 0 and not r["killed"] for r in exits), "unclean probe stop"
            for i in range(2):
                lines = (root / f"r{number}-{mode}-{i}.log").read_text().splitlines()
                stats = json.loads(lines[-1])
                record({"kind": "cohorts", "round": number, "mode": mode, "endpoint": i, "stats": stats})
                assert stats["tx_packets"] > 0 and stats["rx_packets"] > 0
                if mode != "single":
                    assert sum(stats["histogram"][2:]) > 0, "no natural multi-packet cohorts observed"
                    assert stats["mmsg_calls" if mode == "mmsg" else "gso_calls"] > 0
    except Exception as e:
        record({"kind": "failure", "error": repr(e)})
        raise
    finally:
        cleanup = [stop(p) for p in reversed(children)]
        for ns in reversed(created):
            residual = command(["ip", "netns", "pids", ns], check=False).stdout.strip()
            p = command(["ip", "netns", "delete", ns], check=False)
            cleanup.append({"namespace": ns, "exit": p.returncode, "residual": residual})
        after_routes = {family: command(["ip", "-j", family, "route", "show", "table", "all"]).stdout for family in ["-4", "-6"]}
        record({"kind": "cleanup", "values": cleanup, "root_routes_unchanged": before_routes == after_routes})
    assert all(r["exit"] == 0 and not r.get("killed") and not r.get("residual") for r in cleanup)
    assert before_routes == after_routes
    summary = []
    for mode in ["single", "mmsg", "gso"]:
        for direction in ["upload", "download"]:
            selected = [r for r in rows if r["kind"] == "transfer" and r["mode"] == mode and r["direction"] == direction]
            summary.append({"mode": mode, "direction": direction, "samples": len(selected),
                            "Mbps": statistics.median(r["result"]["bits_per_second"]/1e6 for r in selected),
                            "cpu_s_GiB": statistics.median(r["cpu_s_GiB"] for r in selected)})
    (root / "summary.json").write_text(json.dumps(summary, indent=2))
    print(json.dumps({"result": "MECHANISM_ONLY", "summary": summary}))


if __name__ == "__main__":
    if len(sys.argv) > 1 and sys.argv[1] == "integrity":
        integrity(sys.argv[2], sys.argv[3])
    else:
        parser = argparse.ArgumentParser()
        parser.add_argument("--binary", required=True)
        parser.add_argument("--probe", required=True)
        parser.add_argument("--output", required=True)
        lab(parser.parse_args())
