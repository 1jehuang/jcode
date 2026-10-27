#!/usr/bin/env python3
"""Compare real picker calls on private files/ and sqlite/ corpus copies."""
import argparse
import json
import os
from pathlib import Path
import statistics
import subprocess
import sys
import time


def stats(values):
    ordered = sorted(values)
    return {
        "median_ms": statistics.median(ordered),
        "min_ms": ordered[0],
        "max_ms": ordered[-1],
        "p95_ms": ordered[min(len(ordered) - 1, (95 * len(ordered) + 99) // 100 - 1)],
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("binary", type=Path, help="compiled session_picker_bench example")
    parser.add_argument("root", type=Path, help="private root containing files/ and sqlite/")
    parser.add_argument("--repeats", type=int, default=7)
    args = parser.parse_args()
    if args.repeats < 3:
        parser.error("use at least three repetitions")
    binary, root = args.binary.resolve(), args.root.resolve()
    samples = {"files": [], "sqlite": []}
    for iteration in range(args.repeats):
        order = ("files", "sqlite") if iteration % 2 == 0 else ("sqlite", "files")
        for backend in order:
            env = os.environ.copy()
            env.update(JCODE_HOME=str(root / backend), JCODE_SESSION_BACKEND=backend,
                       JCODE_EXTERNAL_SESSIONS="0", JCODE_SESSION_PICKER_MAX_SESSIONS="400",
                       JCODE_SESSION_PICKER_INCLUDE_OLD_SAVED="0")
            start = time.perf_counter()
            child = subprocess.run([str(binary), "1"], env=env, capture_output=True,
                                   text=True, timeout=180)
            elapsed = (time.perf_counter() - start) * 1000
            if child.returncode:
                raise SystemExit(f"picker {backend} exited {child.returncode}; private stderr withheld")
            result = json.loads(child.stdout)
            samples[backend].append({"wall_ms": elapsed, "load_ms": result["first_ms"],
                                     "rows": result["rows"], "digest": result["metadata_sha256"]})
            print(f"completed picker iteration {iteration + 1}/{args.repeats}: {backend}", file=sys.stderr)
    report = {"repetitions": args.repeats, "order": "alternating fresh processes",
              "external_sessions": False, "scan_limit": 400,
              "filesystem_cache": "warm; no global eviction",
              "backends": {}}
    for backend, items in samples.items():
        report["backends"][backend] = {
            "wall": stats([item["wall_ms"] for item in items]),
            "load_sessions": stats([item["load_ms"] for item in items]),
            "row_counts": sorted({item["rows"] for item in items}),
            "metadata_consistent": len({item["digest"] for item in items}) == 1,
        }
    report["metadata_equal"] = len({item["digest"] for items in samples.values() for item in items}) == 1
    report["rows_equal"] = len({item["rows"] for items in samples.values() for item in items}) == 1
    print(json.dumps(report, indent=2))
    if not report["metadata_equal"] or not report["rows_equal"]:
        raise SystemExit("picker metadata differs; no speedup claim permitted")


if __name__ == "__main__":
    main()
