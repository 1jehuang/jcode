"""Run the keyboard-leak probe N times and demand that every run is green.

The rule this encodes: a single green proves nothing, so the matrix refuses to
report success unless all N identical runs pass, and it brackets those runs with
negative controls that MUST reproduce the original leak. If a control ever comes
back clean, the probe has gone blind and the matrix fails regardless of how the
fixed runs behaved.

Every run gets its own directory with its own stream, screens, log and result,
so a red run can be diagnosed from artifacts instead of being re-run.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import shutil
import subprocess
import sys
from collections import Counter
from pathlib import Path

HERE = Path(__file__).resolve().parent
PROBE = HERE / "pty_probe.py"
# The fixture is a local session transcript, so it is never committed. Point
# JCODE_REPRO_FIXTURE at your own frozen copy.
DEFAULT_FIXTURE = Path(os.environ.get(
    "JCODE_REPRO_FIXTURE",
    HERE.parent / "fixture" / "master" / "session_mosquito_1794671874260_10867a3abffb9231.json"))
DEFAULT_CONTROL = Path(os.environ.get(
    "JCODE_REPRO_CONTROL_BIN", HERE.parent / "bin" / "prefix-20261115-182121.exe"))


def run_one(name: str, binary: str, fixture: Path, out: Path, expect_bug: bool,
            gap_ms: float, inject_mode: str) -> dict:
    run_dir = out / name
    if run_dir.exists():
        shutil.rmtree(run_dir)
    argv = [
        sys.executable, str(PROBE),
        "--label", name,
        "--binary", binary,
        "--run-dir", str(run_dir),
        "--fixture", str(fixture),
        "--gap-ms", str(gap_ms),
        "--inject-mode", inject_mode,
    ]
    if expect_bug:
        argv.append("--expect-bug")
    proc = subprocess.run(argv, capture_output=True, text=True, encoding="utf-8", errors="replace")
    result_path = run_dir / "result.json"
    payload = json.loads(result_path.read_text(encoding="utf-8")) if result_path.exists() else {}
    payload["exit_code"] = proc.returncode
    payload["run_dir"] = str(run_dir)
    if not payload:
        payload["label"] = name
        payload["verdict"] = "NO_RESULT"
        payload["stderr_tail"] = (proc.stderr or "")[-800:]
    return payload


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--binary", default=r"C:\Users\koosh\AppData\Local\jcode\bin\jcode.exe")
    ap.add_argument("--control-binary", default=str(DEFAULT_CONTROL))
    ap.add_argument("--fixture", default=str(DEFAULT_FIXTURE))
    ap.add_argument("--out", required=True)
    ap.add_argument("--runs", type=int, default=10)
    ap.add_argument("--gap-ms", type=float, default=15.0)
    ap.add_argument("--inject-mode", choices=["atomic", "chars"], default="atomic")
    ap.add_argument("--no-controls", action="store_true")
    args = ap.parse_args()

    out = Path(args.out).resolve()
    out.mkdir(parents=True, exist_ok=True)
    fixture = Path(args.fixture).resolve()
    fixture_master_sha = hashlib.sha256(fixture.read_bytes()).hexdigest()

    rows: list[dict] = []

    if not args.no_controls and Path(args.control_binary).exists():
        rows.append(run_one("control-pre", args.control_binary, fixture, out, True, args.gap_ms,
                            args.inject_mode))

    for index in range(1, args.runs + 1):
        rows.append(run_one(f"fixed-{index:02d}", args.binary, fixture, out, False, args.gap_ms,
                            args.inject_mode))

    if not args.no_controls and Path(args.control_binary).exists():
        rows.append(run_one("control-post", args.control_binary, fixture, out, True, args.gap_ms,
                            args.inject_mode))

    fixed = [r for r in rows if r["label"].startswith("fixed-")]
    controls = [r for r in rows if r["label"].startswith("control-")]

    greens = [r for r in fixed if r.get("verdict") == "GREEN"]
    reproduced = [r for r in controls if r.get("verdict") == "BUG_REPRODUCED"]
    fixture_hashes = Counter(r.get("fixture_sha256") for r in fixed)

    failures: Counter = Counter()
    for row in fixed:
        for check, passed in (row.get("checks") or {}).items():
            if not passed:
                failures[check] += 1

    summary = {
        "binary": args.binary,
        "control_binary": args.control_binary,
        "fixture": str(fixture),
        "fixture_master_sha256": fixture_master_sha,
        "runs_requested": args.runs,
        "runs_green": len(greens),
        "runs_red": len(fixed) - len(greens),
        "controls_expected": 0 if args.no_controls else 2,
        "controls_reproduced": len(reproduced),
        "distinct_fixture_hashes_across_runs": sorted(fixture_hashes),
        "failure_histogram": dict(failures),
        "gap_max_ms_per_run": [r.get("gap_max_ms") for r in fixed],
        "inject_mode": args.inject_mode,
        "verdicts": {r["label"]: r.get("verdict") for r in rows},
        "passed": (
            len(greens) == args.runs
            and (args.no_controls or len(reproduced) == 2)
            and len(fixture_hashes) == 1
        ),
    }
    (out / "summary.json").write_text(json.dumps(summary, indent=2), encoding="utf-8")

    print(f"{'run':<14} {'verdict':<16} gap_max_ms")
    for row in rows:
        print(f"{row['label']:<14} {str(row.get('verdict')):<16} {row.get('gap_max_ms')}")
    print()
    print(f"green {len(greens)}/{args.runs}   controls reproduced {len(reproduced)}"
          f"/{0 if args.no_controls else 2}")
    print(f"fixture hashes seen: {sorted(fixture_hashes)}")
    if failures:
        print(f"failing checks: {dict(failures)}")
    print(f"SUMMARY {'PASS' if summary['passed'] else 'FAIL'}  -> {out / 'summary.json'}")
    return 0 if summary["passed"] else 1


if __name__ == "__main__":
    sys.exit(main())
