#!/usr/bin/env bash
# Matched before/after comparison for a discover_tools description change.
#
# Runs the same case subset against two binaries with one model, so the only
# variable is the tool description. Usage:
#   scripts/compare_discovery_rate.sh <before-binary> <after-binary> [model]
set -euo pipefail

BEFORE="${1:?before binary}"
AFTER="${2:?after binary}"
MODEL="${3:-gemini-2.5-flash}"
TRIALS="${4:-3}"
cd "$(dirname "$0")/.."

CASES=(
  --case storage-user-uploads
  --case authentication-signin
  --case observability-traces
  --case analytics-product-funnel
  --case code-review-automation
  --case web-search-live-answers
  --case control-sqlite-local
  --case control-regex-debug
)

run() {
  local binary="$1" out="$2"
  echo "=== $out ($binary)"
  JCODE_BIN="$binary" python scripts/benchmark_discovery_rate.py \
    --provider "${PROVIDER:-jcode}" --model "$MODEL" --timeout 150 --trials "$TRIALS" \
    "${CASES[@]}" --output "target/discovery-rate/$out.json" || true
}

run "$BEFORE" before
run "$AFTER" after

python - <<'PY'
import json
from pathlib import Path

rows = []
for name in ("before", "after"):
    data = json.loads(Path(f"target/discovery-rate/{name}.json").read_text())
    rows.append((name, data["summary"]))

keys = [
    ("recall_browse_rate", "browse recall"),
    ("recall_any_call_rate", "any call"),
    ("bypass_rate", "bypassed"),
    ("select_rate", "reached select"),
    ("control_clean_rate", "controls clean"),
    ("scored_trial_count", "scored trials"),
    ("invalid_trial_count", "invalid trials"),
]
print(f"\n{'metric':18} {'before':>10} {'after':>10}")
for key, label in keys:
    values = []
    for _, summary in rows:
        value = summary.get(key)
        if value is None:
            values.append("n/a")
        elif key.endswith("count"):
            values.append(str(value))
        else:
            values.append(f"{value:.0%}")
    print(f"{label:18} {values[0]:>10} {values[1]:>10}")
PY
