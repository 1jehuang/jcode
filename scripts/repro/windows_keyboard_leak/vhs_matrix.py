"""Second, independent driver: run the same A/B through VHS and assert on logs.

The point of a second driver is that the first one might be wrong in a way that
happens to look green. VHS reaches the app down a different path (ttyd + a real
terminal served to a headless browser), so agreement between the two is
meaningful evidence, while agreement of one driver with itself is not.

Assertions here are text, taken from the isolated home the tape ran in: VHS
reports success as soon as the recording is written, so a green tape alone says
nothing about the product.
"""

from __future__ import annotations

import argparse
import glob
import json
import subprocess
import sys
from collections import Counter
from pathlib import Path

HERE = Path(__file__).resolve().parent
ROOT = HERE.parent
ESC = "\x1b"
REPORT = f"{ESC}[119;1:3u"
SCRUB = 'Stripped a keyboard-protocol report that arrived as text: "[119;1:3u"'
SKIP = "kitty keyboard reporting is incompatible with the Windows console input decoder"


def build_tape(name: str, binary: Path, home: Path, out_dir: Path, settle: float) -> str:
    world = f'{HERE / "resume_world.cmd"} {home} {binary}'
    return "\n".join([
        f'Output "{(out_dir / (name + ".gif")).as_posix()}"',
        'Set Shell "cmd"',
        "Set FontSize 12",
        "Set Width 1000",
        "Set Height 700",
        "Set Padding 0",
        "Set Margin 0",
        "Set TypingSpeed 40ms",
        "Set Framerate 5",
        "Hide",
        f'Type "{world}"',
        "Enter",
        "Show",
        f"Sleep {settle:g}s",
        f'Type "{REPORT}"',
        "Sleep 3s",
        'Type "zzqx"',
        "Sleep 2s",
        "",
    ])


def read_log(home: Path) -> str:
    parts = []
    for name in sorted(glob.glob(str(home / "logs" / "**" / "*"), recursive=True)):
        path = Path(name)
        if path.is_file():
            parts.append(path.read_text(encoding="utf-8", errors="replace"))
    return "\n".join(parts)


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--runs", type=int, default=5)
    ap.add_argument("--binary", default=str(ROOT / "bin" / "jcode-fixed-5f70a539d.exe"))
    ap.add_argument("--out", required=True)
    ap.add_argument("--settle", type=float, default=11.0)
    ap.add_argument("--vhs", default=str(ROOT / "bin" / "vhs.exe"))
    args = ap.parse_args()

    out = Path(args.out).resolve()
    out.mkdir(parents=True, exist_ok=True)
    rows = []
    for index in range(1, args.runs + 1):
        name = f"vhs-{index:02d}"
        home = out / name / "home"
        tape_path = out / name / f"{name}.tape"
        tape_path.parent.mkdir(parents=True, exist_ok=True)
        tape_path.write_text(build_tape(name, Path(args.binary), home, out / name, args.settle),
                             encoding="utf-8")
        proc = subprocess.run([args.vhs, str(tape_path)], capture_output=True, text=True,
                              encoding="utf-8", errors="replace")
        log = read_log(home)
        checks = {
            "tape_recorded": proc.returncode == 0 and (out / name / f"{name}.gif").exists(),
            "scrub_logged_report_as_text": SCRUB in log,
            "log_skipped_kitty_reporting": SKIP in log,
            "session_present": any((home / "sessions").glob("session_mosquito_*.json")),
        }
        rows.append({"run": name, "checks": checks, "green": all(checks.values()),
                     "stderr_tail": (proc.stderr or "")[-400:] if proc.returncode != 0 else ""})

    greens = sum(1 for row in rows if row["green"])
    failures = Counter()
    for row in rows:
        for check, passed in row["checks"].items():
            if not passed:
                failures[check] += 1

    summary = {
        "driver": "vhs",
        "binary": args.binary,
        "runs": args.runs,
        "green": greens,
        "red": args.runs - greens,
        "failure_histogram": dict(failures),
        "passed": greens == args.runs,
        "rows": rows,
    }
    (out / "vhs-summary.json").write_text(json.dumps(summary, indent=2), encoding="utf-8")
    for row in rows:
        print(f"{row['run']} {'GREEN' if row['green'] else 'RED'} {row['checks']}")
    print(f"SUMMARY {'PASS' if summary['passed'] else 'FAIL'} {greens}/{args.runs}")
    return 0 if summary["passed"] else 1


if __name__ == "__main__":
    sys.exit(main())
