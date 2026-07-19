#!/usr/bin/env python3
"""Evaluate repeated legacy/Leaf-owned-TUN dataplane evidence without dependencies."""

from __future__ import annotations

import argparse
import csv
import json
import statistics
import sys
from pathlib import Path


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--output-root", required=True, type=Path)
    parser.add_argument("--candidate-sha", required=True)
    parser.add_argument("--minimum-runs", type=int, default=3)
    parser.add_argument("--minimum-throughput-ratio", type=float, default=0.95)
    parser.add_argument("--maximum-rss-growth-kib", type=int, default=65536)
    parser.add_argument("--maximum-syscalls-per-byte-ratio", type=float, default=0.50)
    parser.add_argument("--require-strace", action="store_true")
    parser.add_argument("--output", type=Path)
    return parser.parse_args()


def read_pairs(path: Path) -> dict[str, str]:
    values: dict[str, str] = {}
    with path.open(encoding="utf-8") as stream:
        for line in stream:
            key, separator, value = line.rstrip("\n").partition("\t")
            if separator:
                values[key] = value
    return values


def combined_rss(path: Path) -> int:
    with path.open(encoding="utf-8", newline="") as stream:
        rows = csv.DictReader(stream, delimiter="\t")
        return sum(int(row["max_rss_kib"]) for row in rows)


def load_mode(root: Path, mode: str, candidate_sha: str) -> list[dict[str, float]]:
    records: list[dict[str, float]] = []
    for directory in sorted(root.glob(f"{mode}-*")):
        summary_path = directory / "summary.tsv"
        resources_path = directory / "resources-summary.tsv"
        if not summary_path.is_file() or not resources_path.is_file():
            continue
        summary = read_pairs(summary_path)
        if summary.get("candidate_sha") != candidate_sha:
            raise ValueError(f"{directory}: candidate SHA mismatch")
        if summary.get("mode") != mode:
            raise ValueError(f"{directory}: mode mismatch")
        transferred = int(summary["upload_bytes"]) + int(summary["download_bytes"])
        core_syscalls = summary.get("core_syscalls", "")
        worker_syscalls = summary.get("worker_syscalls", "")
        syscall_rate = None
        if core_syscalls and worker_syscalls:
            syscall_rate = (int(core_syscalls) + int(worker_syscalls)) / transferred
        records.append(
            {
                "upload_bps": float(summary["upload_bps"]),
                "download_bps": float(summary["download_bps"]),
                "rss_kib": float(combined_rss(resources_path)),
                "core_idle_cpu": float(summary["core_idle_cpu_percent"]),
                "worker_idle_cpu": float(summary["worker_idle_cpu_percent"]),
                "syscalls_per_byte": syscall_rate,
            }
        )
    return records


def median(records: list[dict[str, float]], key: str) -> float | None:
    values = [record[key] for record in records if record[key] is not None]
    return statistics.median(values) if values else None


def main() -> int:
    args = parse_args()
    if args.minimum_runs < 1:
        raise ValueError("minimum-runs must be positive")
    modes = {
        mode: load_mode(args.output_root, mode, args.candidate_sha)
        for mode in ("legacy", "leaf-owned-tun")
    }
    failures: list[str] = []
    for mode, records in modes.items():
        if len(records) < args.minimum_runs:
            failures.append(
                f"{mode}: found {len(records)} complete runs, require {args.minimum_runs}"
            )

    metrics = {
        mode: {
            key: median(records, key)
            for key in (
                "upload_bps",
                "download_bps",
                "rss_kib",
                "core_idle_cpu",
                "worker_idle_cpu",
                "syscalls_per_byte",
            )
        }
        for mode, records in modes.items()
    }
    if all(modes.values()):
        legacy = metrics["legacy"]
        fast = metrics["leaf-owned-tun"]
        for direction in ("upload_bps", "download_bps"):
            ratio = fast[direction] / legacy[direction]
            if ratio < args.minimum_throughput_ratio:
                failures.append(
                    f"{direction}: ratio {ratio:.4f} is below {args.minimum_throughput_ratio:.4f}"
                )
        rss_growth = fast["rss_kib"] - legacy["rss_kib"]
        if rss_growth > args.maximum_rss_growth_kib:
            failures.append(
                f"rss: growth {rss_growth:.0f} KiB exceeds {args.maximum_rss_growth_kib} KiB"
            )
        syscall_values = (
            legacy["syscalls_per_byte"],
            fast["syscalls_per_byte"],
        )
        if all(value is not None for value in syscall_values):
            syscall_ratio = syscall_values[1] / syscall_values[0]
            if syscall_ratio > args.maximum_syscalls_per_byte_ratio:
                failures.append(
                    "syscalls_per_byte: ratio "
                    f"{syscall_ratio:.4f} exceeds {args.maximum_syscalls_per_byte_ratio:.4f}"
                )
        elif args.require_strace:
            failures.append("strace: complete syscall totals are required for both modes")

    report = {
        "candidate_sha": args.candidate_sha,
        "run_counts": {mode: len(records) for mode, records in modes.items()},
        "medians": metrics,
        "gates": {
            "minimum_throughput_ratio": args.minimum_throughput_ratio,
            "maximum_rss_growth_kib": args.maximum_rss_growth_kib,
            "maximum_syscalls_per_byte_ratio": args.maximum_syscalls_per_byte_ratio,
            "require_strace": args.require_strace,
        },
        "passed": not failures,
        "failures": failures,
    }
    rendered = json.dumps(report, indent=2, sort_keys=True) + "\n"
    if args.output:
        args.output.write_text(rendered, encoding="utf-8")
    sys.stdout.write(rendered)
    return 0 if not failures else 1


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (KeyError, OSError, ValueError, ZeroDivisionError) as error:
        print(f"evidence evaluation failed: {error}", file=sys.stderr)
        raise SystemExit(2)
