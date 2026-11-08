#!/usr/bin/env python3
"""Summarize content-free cloud timing metrics from Jcode log files."""
import argparse
from collections import defaultdict
from datetime import datetime
import glob
import math
import os
from pathlib import Path
import re
import sys

FIELD = re.compile(r"\b([a-z_]+)=([^\s,]+)")
NUMBER = re.compile(r"^\d+(?:\.\d+)?$")
SAFE_LABEL = re.compile(r"^[a-zA-Z0-9_.:-]{1,96}$")
TIMESTAMP = re.compile(r"^\[(\d{4}-\d{2}-\d{2} \d{2}:\d{2}:\d{2}(?:\.\d+)?)\]")
FAILURE_OUTCOMES = {
    "error", "failed", "dropped", "timeout", "cancelled", "expired",
    "error_or_cancelled", "http_error", "load_error", "read_error",
    "size_limit", "oversized", "identity_mismatch", "exit_failure",
    "rejected", "undelivered",
}


def local_timestamp(value):
    """Parse a wall-clock timestamp in the log's local timezone, not UTC."""
    try:
        parsed = datetime.fromisoformat(value)
    except ValueError as error:
        raise argparse.ArgumentTypeError("expected local YYYY-MM-DDTHH:MM:SS") from error
    if parsed.tzinfo is not None:
        raise argparse.ArgumentTypeError("use the log's local time without a timezone offset")
    return parsed


def in_window(lines, since=None, until=None):
    for line in lines:
        if since is not None or until is not None:
            match = TIMESTAMP.match(line)
            if match is None:
                continue
            try:
                timestamp = datetime.fromisoformat(match[1])
            except ValueError:
                continue
            if since is not None and timestamp < since:
                continue
            if until is not None and timestamp >= until:
                continue
        yield line


def collect(lines):
    groups = defaultdict(list)
    failures = defaultdict(int)
    for line in lines:
        if not any(prefix in line for prefix in ("[CLOUD_TIMING]", "mobile_sync stage=", "cloud_preview stage=", "cloud_inventory stage=")):
            continue
        fields = dict(FIELD.findall(line))
        stage = fields.get("stage", "unknown")
        operation = fields.get("operation", "")
        if not SAFE_LABEL.fullmatch(stage) or (operation and not SAFE_LABEL.fullmatch(operation)):
            continue
        kind = fields.get("kind", "")
        if kind and SAFE_LABEL.fullmatch(kind):
            operation = operation + "." + kind if operation else kind
        name = stage + (":" + operation if operation else "")
        outcome = fields.get("outcome", "unknown")
        for metric, value in fields.items():
            if not metric.endswith("_ms") or not NUMBER.fullmatch(value):
                continue
            number = float(value)
            if not math.isfinite(number):
                continue
            key = (name, metric)
            groups[key].append(number)
            if outcome in FAILURE_OUTCOMES:
                failures[key] += 1
    return groups, failures


def percentile(values, fraction):
    values = sorted(values)
    return values[max(0, math.ceil(len(values) * fraction) - 1)]


def report(groups, failures, threshold_ms=0):
    rows = [
        (max(values), name, metric, values)
        for (name, metric), values in groups.items()
        if max(values) >= threshold_ms
    ]
    print(f"{'stage:operation':52} {'metric':20} {'count':>7} {'errors':>7} {'p50 ms':>10} {'p95 ms':>10} {'max ms':>10}")
    for maximum, name, metric, values in sorted(rows, reverse=True):
        print(f"{name:52} {metric:20} {len(values):7} {failures[(name, metric)]:7} {percentile(values, .5):10.2f} {percentile(values, .95):10.2f} {maximum:10.2f}")
    if not rows:
        print("No matching timing samples. Run an instrumented cloud operation first, or select other logs.")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("files", nargs="*", help="Log files or glob patterns. '-' reads stdin. Default: all retained Jcode logs.")
    parser.add_argument("--slow-ms", type=float, default=0, help="Only show stages whose maximum reaches this duration.")
    parser.add_argument("--since", type=local_timestamp, help="Inclusive local log time, YYYY-MM-DDTHH:MM:SS. Undated records are excluded.")
    parser.add_argument("--until", type=local_timestamp, help="Exclusive local log time, YYYY-MM-DDTHH:MM:SS.")
    args = parser.parse_args()
    if args.since is not None and args.until is not None and args.since >= args.until:
        parser.error("--since must be earlier than --until")
    files = args.files or [str(Path(os.environ.get("JCODE_HOME", Path.home() / ".jcode")) / "logs" / "jcode-*.log")]

    def lines():
        seen = set()
        for pattern in files:
            if pattern == "-":
                yield from sys.stdin
                continue
            for filename in sorted(glob.glob(pattern)):
                if filename in seen:
                    continue
                seen.add(filename)
                try:
                    with open(filename, encoding="utf-8", errors="replace") as handle:
                        yield from handle
                except OSError as error:
                    print(f"Could not read log file: {error.strerror}", file=sys.stderr)
    groups, failures = collect(in_window(lines(), args.since, args.until))
    report(groups, failures, args.slow_ms)


if __name__ == "__main__":
    main()
