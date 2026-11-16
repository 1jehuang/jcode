"""Print every per-run record from a matrix directory, one block per run."""

import glob
import json
import sys
from pathlib import Path

root = Path(sys.argv[1])
names = ["session_loaded", "injection_within_burst_window", "leak_absent_after_injection",
         "leak_absent_at_end", "positive_control_present", "scrub_logged_report_as_text",
         "log_skipped_kitty_reporting", "stream_emits_stack_neutral_clear",
         "stream_decoded_losslessly"]


def composer(run_dir: Path, stage: str) -> str:
    path = run_dir / "screen" / stage
    if not path.exists():
        return "<missing>"
    for line in path.read_text(encoding="utf-8", errors="replace").splitlines():
        if line.startswith("2>"):
            return line.rstrip()
    return "<no composer row>"


print(f"{'run':<12} {'verdict':<16} {'gap':>5}  {'leak':<5} {'pos':<4} {'scrub':<6} "
      f"{'skip':<5} {'clear':<6} {'lossless':<8}  composer after report -> after zzqx")
print("-" * 150)
for path in sorted(glob.glob(str(root / "*" / "result.json"))):
    data = json.loads(Path(path).read_text(encoding="utf-8"))
    run_dir = Path(path).parent
    checks = data["checks"]

    def mark(name):
        return "ok" if checks.get(name) else "NO"

    print(f"{data['label']:<12} {data['verdict']:<16} {str(data.get('gap_max_ms')):>5}  "
          f"{mark('leak_absent_after_injection'):<5} {mark('positive_control_present'):<4} "
          f"{mark('scrub_logged_report_as_text'):<6} {mark('log_skipped_kitty_reporting'):<5} "
          f"{mark('stream_emits_stack_neutral_clear'):<6} {mark('stream_decoded_losslessly'):<8}  "
          f"{composer(run_dir, '02-after-leak.txt')!r:22} -> {composer(run_dir, '03-after-positive.txt')!r}")

print()
print("per-run artifact paths:")
for path in sorted(glob.glob(str(root / "*" / "result.json"))):
    run_dir = Path(path).parent
    print(f"  {run_dir.name:<12} {run_dir}")
