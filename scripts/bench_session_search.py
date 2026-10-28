#!/usr/bin/env python3
"""Compare public session search calls without publishing private results."""
import argparse
import json
import os
from pathlib import Path
import statistics
import subprocess
import sys
import time


def stats(values):
    values = sorted(values)
    return {"median_ms": statistics.median(values), "min_ms": values[0],
            "max_ms": values[-1]}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("before", type=Path)
    parser.add_argument("after", type=Path)
    parser.add_argument("before_home", type=Path)
    parser.add_argument("after_home", type=Path)
    parser.add_argument("--repeats", type=int, default=5)
    parser.add_argument("--queries", type=Path,
                        help="private JSON object mapping public labels to query strings")
    parser.add_argument("--picker", action="store_true", help="compare no-query picker binaries")
    parser.add_argument("--exhaustive", action="store_true", help="compare exhaustive search without candidate indexes")
    parser.add_argument("--labels", nargs="+", help="run only these public query labels")
    args = parser.parse_args()
    if args.repeats < 3:
        parser.error("use at least three repetitions")
    if not args.picker and args.queries is None:
        parser.error("--queries is required unless --picker is set")
    queries = {"picker": "1"} if args.picker else json.loads(args.queries.read_text())
    if args.labels:
        queries = {label: queries[label] for label in args.labels}
    report = {"repetitions": args.repeats, "order": "alternating fresh processes",
              "filesystem_cache": "warm; no global eviction", "exhaustive": args.exhaustive,
              "queries": {}}
    equal = True
    for label, query in queries.items():
        samples = {"before": [], "after": []}
        for iteration in range(args.repeats):
            order = ("before", "after") if iteration % 2 == 0 else ("after", "before")
            for version in order:
                env = os.environ.copy()
                env.update(JCODE_HOME=str(getattr(args, version + "_home").resolve()),
                           JCODE_SESSION_BACKEND="sqlite", JCODE_EXTERNAL_SESSIONS="0",
                           JCODE_SESSION_PICKER_MAX_SESSIONS="400",
                           JCODE_SESSION_PICKER_INCLUDE_OLD_SAVED="0")
                started = time.perf_counter()
                command = [str(getattr(args, version).resolve()), query]
                if args.exhaustive and not args.picker:
                    command.append("exhaustive")
                result = subprocess.run(command,
                                        env=env, capture_output=True, text=True, timeout=180)
                if result.returncode:
                    raise SystemExit(f"{label}/{version}: failed; private stderr withheld")
                sample = json.loads(result.stdout)
                if args.picker:
                    sample = {"elapsed_ms": sample["first_ms"], "results": sample["rows"],
                              "result_sha256": sample["metadata_sha256"]}
                sample["wall_ms"] = (time.perf_counter() - started) * 1000
                samples[version].append(sample)
            print(f"completed {label} {iteration + 1}/{args.repeats}", file=sys.stderr)
        matching = len({sample["result_sha256"] for group in samples.values()
                        for sample in group}) == 1
        equal &= matching
        report["queries"][label] = {"results_equal": matching}
        for version, group in samples.items():
            report["queries"][label][version] = {
                "load_sessions" if args.picker else "search": stats([sample["elapsed_ms"] for sample in group]),
                "wall": stats([sample["wall_ms"] for sample in group]),
                "result_counts": sorted({sample["results"] for sample in group})}
    report["all_results_equal"] = equal
    print(json.dumps(report, indent=2))
    if not equal:
        raise SystemExit("result mismatch; no speedup claim permitted")


if __name__ == "__main__":
    main()
