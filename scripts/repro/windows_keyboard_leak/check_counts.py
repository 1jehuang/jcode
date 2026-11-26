"""Summarise per-check evidence across a matrix run directory."""

import collections
import glob
import json
import sys
from pathlib import Path

root = Path(sys.argv[1])
files = sorted(glob.glob(str(root / "fixed-*" / "result.json")))
counts = collections.Counter()
runs = 0
for path in files:
    data = json.loads(Path(path).read_text(encoding="utf-8"))
    runs += 1
    for check, passed in data["checks"].items():
        counts[check] += 1 if passed else 0
print(f"runs={runs}")
for check, passed in sorted(counts.items()):
    print(f"{check:38} {passed}/{runs}")
