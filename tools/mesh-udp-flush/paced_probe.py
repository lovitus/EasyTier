#!/usr/bin/env python3
"""Separate fixed-load diagnostic; never replaces the unpaced acceptance run.

Wire-compatible with tools/easytier-perf-probe.rs. The sender caps its rate
without catch-up bursts. Actual elapsed time/rate, not the configured cap,
is reported. Forwarder CPU excludes this Python load generator.
"""
import argparse
import json
import os
import socket
import struct
import time

MAGIC = b"ETPERF01"
BUFFER = b"\xa5" * 65536
RATE = float(os.environ.get("ET_PACED_MBPS", "300"))
assert 0 < RATE <= 1000


def small_read(sock, size):
    data = bytearray()
    while len(data) < size:
        part = sock.recv(size - len(data))
        if not part:
            raise RuntimeError("unexpected control EOF")
        data.extend(part)
    return bytes(data)


def receive(sock, size):
    while size:
        part = sock.recv(min(size, len(BUFFER)))
        if not part:
            raise RuntimeError("unexpected data EOF")
        size -= len(part)


def send(sock, size):
    deadline = time.monotonic()
    while size:
        n = min(size, len(BUFFER))
        deadline = max(deadline, time.monotonic()) + n * 8 / (RATE * 1e6)
        delay = deadline - time.monotonic()
        if delay > 0:
            time.sleep(delay)
        sock.sendall(BUFFER[:n])
        size -= n


def address(value):
    host, port = value.rsplit(":", 1)
    return host, int(port)


def configure(sock, timeout):
    sock.settimeout(timeout)
    sock.setsockopt(socket.IPPROTO_TCP, socket.TCP_NODELAY, 1)


def server(args):
    with socket.socket() as listener:
        listener.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
        listener.settimeout(args.timeout_seconds)
        listener.bind(address(args.listen))
        listener.listen(1)
        for _ in range(args.sessions):
            stream, _ = listener.accept()
            with stream:
                configure(stream, args.timeout_seconds)
                header = small_read(stream, 24)
                assert header[:8] == MAGIC and header[9:16] == b"\0" * 7
                direction, size = header[8], struct.unpack("!Q", header[16:])[0]
                assert direction in [1, 2] and 0 < size <= 16 * 1024**3
                stream.sendall(b"\xa5")
                assert small_read(stream, 1) == b"\x5a"
                start = time.monotonic_ns()
                (receive if direction == 1 else send)(stream, size)
                elapsed = max(1, time.monotonic_ns() - start)
                stream.sendall(struct.pack("!QQ", size, elapsed))


def client(args):
    assert 0 < args.bytes <= 16 * 1024**3
    code = 1 if args.direction == "upload" else 2
    with socket.create_connection(address(args.target), timeout=args.timeout_seconds) as stream:
        configure(stream, args.timeout_seconds)
        stream.sendall(MAGIC + bytes([code]) + b"\0" * 7 + struct.pack("!Q", args.bytes))
        assert small_read(stream, 1) == b"\xa5"
        stream.sendall(b"\x5a")
        start = time.monotonic_ns()
        (send if code == 1 else receive)(stream, args.bytes)
        elapsed_data = max(1, time.monotonic_ns() - start)
        count, elapsed_server = struct.unpack("!QQ", small_read(stream, 16))
        assert count == args.bytes and elapsed_server > 0
        elapsed_client = max(1, time.monotonic_ns() - start)
        elapsed = elapsed_server if code == 1 else elapsed_data
        print(json.dumps({"schema_version": 1, "ok": True, "direction": args.direction,
                          "bytes": count, "elapsed_ns": elapsed,
                          "client_elapsed_ns": elapsed_client, "server_elapsed_ns": elapsed_server,
                          "bits_per_second": count * 8e9 / elapsed,
                          "diagnostic": "paced-python-no-catchup", "rate_cap_mbps": RATE}))


if __name__ == "__main__":
    p = argparse.ArgumentParser()
    sub = p.add_subparsers(dest="role", required=True)
    s = sub.add_parser("server")
    s.add_argument("--listen", required=True)
    s.add_argument("--sessions", type=int, default=1)
    s.add_argument("--timeout-seconds", type=float, default=20)
    c = sub.add_parser("client")
    c.add_argument("--target", required=True)
    c.add_argument("--direction", choices=["upload", "download"], required=True)
    c.add_argument("--bytes", type=int, required=True)
    c.add_argument("--timeout-seconds", type=float, default=20)
    a = p.parse_args()
    (server if a.role == "server" else client)(a)
