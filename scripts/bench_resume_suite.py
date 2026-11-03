#!/usr/bin/env python3
"""Run the offline resume UX matrix and retain raw output plus structured results.

Uses justrust, serial ignored tests, and the selfdev profile by default. The
optional PTY startup measurement only opens/cancels the picker, never resumes a
session. No terminal-emulator paint or cold OS page-cache claims are made.
"""
import argparse
import datetime
import json
import os
from pathlib import Path
import subprocess
import sys


def benchmark_rows(text):
    """Accept Rust matrix marker lines and standalone disk-suite JSON records."""
    rows = []
    decoder = json.JSONDecoder()
    for line in text.splitlines():
        if "resume_bench " in line:
            candidate = line.split("resume_bench ", 1)[1].lstrip()
        else:
            start = line.find('{"resume_bench":')
            if start < 0:
                # serde_json object ordering may put the marker after other keys.
                if '"resume_bench":1' not in line:
                    continue
                start = line.find("{")
            candidate = line[start:]
        try:
            row, _ = decoder.raw_decode(candidate)
        except (json.JSONDecodeError, ValueError):
            continue
        if isinstance(row, dict) and "scenario" in row:
            rows.append(row)
    return rows


def validate_sample_counts(rows, requested):
    """Reject hosted runs that silently lost sample configuration."""
    for row in rows:
        if "metrics" in row:
            counts = [metric["samples"] for metric in row["metrics"].values()
                      if isinstance(metric, dict) and "samples" in metric]
            if not counts or any(count != requested for count in counts):
                raise RuntimeError(f"{row['scenario']}: expected {requested} samples, got {counts}")
        elif "attempts" in row and row["attempts"] != requested:
            raise RuntimeError(f"{row['scenario']}: expected {requested} attempts, got {row['attempts']}")


def execute(command, root, env, log_path):
    print("+ " + " ".join(command), flush=True)
    output = []
    with log_path.open("w") as log:
        with subprocess.Popen(command, cwd=root, env=env, stdout=subprocess.PIPE,
                              stderr=subprocess.STDOUT, text=True, bufsize=1) as process:
            for line in process.stdout:
                sys.stdout.write(line)
                sys.stdout.flush()
                log.write(line)
                log.flush()
                output.append(line)
            code = process.wait()
    if code:
        raise RuntimeError(f"command exited {code}, full output retained in {log_path}")
    return "".join(output)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--samples", type=int, default=20,
                        help="5..200 per scenario; use >=100 for more useful tail estimates")
    parser.add_argument("--profile", default="selfdev", help="Cargo profile, default selfdev")
    parser.add_argument("--output", type=Path)
    parser.add_argument("--skip-check", action="store_true")
    parser.add_argument("--binary", type=Path,
                        help="also run existing real-PTY startup/cancel benchmarks on this binary")
    parser.add_argument("--pty-runs", type=int, default=5)
    args = parser.parse_args()
    if not 5 <= args.samples <= 200 or args.pty_runs < 1:
        parser.error("samples must be 5..200 and pty-runs must be positive")
    root = Path(__file__).resolve().parent.parent
    revision = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=root, text=True).strip()
    status = subprocess.check_output(["git", "status", "--porcelain"], cwd=root, text=True)
    stamp = datetime.datetime.now(datetime.timezone.utc).strftime("%Y%m%dT%H%M%SZ")
    output_dir = (args.output or root / "target" / "resume-latency" / f"{stamp}-{revision[:9]}").resolve()
    output_dir.mkdir(parents=True, exist_ok=True)
    env = dict(os.environ, JCODE_RESUME_BENCH_SAMPLES=str(args.samples))
    metadata = {"schema_version": 1, "timestamp_utc": stamp, "revision": revision,
                "working_tree_dirty": bool(status), "profile": args.profile,
                "samples_per_scenario": args.samples, "platform": sys.platform,
                "measurement_scope": "in-process/offline fixtures; no cold OS cache or emulator paint",
                "environment": {name: os.environ.get(name) for name in
                                ["JUSTRUST_REMOTE", "JCODE_PERF_TIER", "JCODE_CONFIG"]}}
    (output_dir / "metadata.json").write_text(json.dumps(metadata, indent=2) + "\n")
    rows = []
    # An interrupted rerun must not leave an older completed result looking fresh.
    (output_dir / "results.json").write_text(json.dumps(
        {"metadata": metadata, "rows": rows, "status": "running"}, indent=2) + "\n")
    try:
        if not args.skip_check:
            execute(["justrust", "check", "-p", "jcode-tui", "--tests", "--profile", args.profile],
                    root, env, output_dir / "check.log")
        for module in ["benchmark_matrix::", "benchmark_io::"]:
            name = module.rstrip(":")
            text = execute(["justrust", "test", "-p", "jcode-tui", "--lib", "--profile", args.profile,
                            module, "--config", f'env.JCODE_RESUME_BENCH_SAMPLES="{args.samples}"',
                            "--", "--ignored", "--nocapture", "--test-threads=1"],
                           root, env, output_dir / f"{name}.log")
            found = benchmark_rows(text)
            if not found:
                raise RuntimeError(f"{name} produced no benchmark rows, refusing an empty success")
            rows.extend(found)
            validate_sample_counts(found, args.samples)
        if args.binary:
            binary = str(args.binary.resolve())
            for cancel in [False, True]:
                command = [sys.executable, str(root / "scripts/bench_resume_picker.py"), binary,
                           "--runs", str(args.pty_runs)]
                if cancel:
                    command.append("--cancel-loading")
                execute(command, root, env, output_dir / ("pty_cancel.log" if cancel else "pty_startup.log"))
        result = {"metadata": metadata, "rows": rows, "status": "completed"}
    except (OSError, RuntimeError) as error:
        result = {"metadata": metadata, "rows": rows, "status": "failed", "error": str(error)}
        (output_dir / "results.json").write_text(json.dumps(result, indent=2) + "\n")
        print(str(error), file=sys.stderr)
        return 1
    (output_dir / "results.json").write_text(json.dumps(result, indent=2) + "\n")
    print(f"Resume benchmark results: {output_dir / 'results.json'}", flush=True)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
