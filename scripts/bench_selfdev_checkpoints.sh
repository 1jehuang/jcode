#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
cd "$repo_root"

usage() {
  cat <<'USAGE'
Usage:
  scripts/bench_selfdev_checkpoints.sh [options]

Runs the standard compile checkpoints for the self-dev loop using scripts/bench_compile.sh.

Options:
  --touch <path>   Source file to touch for warm edit-loop runs (default: src/server.rs)
  --runs <n>       Number of warm runs per checkpoint (default: 3)
  --json           Print a single JSON object with all checkpoint summaries
  -h, --help       Show this help

Checkpoints:
  cold_check           cargo check after cargo clean
  warm_check_edit      touched-file cargo check loop
  cold_selfdev_build   selfdev jcode build after cargo clean
  warm_selfdev_edit    touched-file selfdev jcode build loop
USAGE
}

runs=3
touch_path="src/server.rs"
json_output=0

while [[ $# -gt 0 ]]; do
  case "$1" in
    --touch)
      if [[ $# -lt 2 ]]; then
        printf 'error: --touch requires a path\n' >&2
        exit 1
      fi
      touch_path="$2"
      shift
      ;;
    --runs)
      if [[ $# -lt 2 ]]; then
        printf 'error: --runs requires a positive integer\n' >&2
        exit 1
      fi
      runs="$2"
      shift
      ;;
    --json)
      json_output=1
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      printf 'error: unknown argument: %s\n' "$1" >&2
      exit 1
      ;;
  esac
  shift
done

if ! [[ "$runs" =~ ^[1-9][0-9]*$ ]]; then
  printf 'error: --runs must be a positive integer (got %s)\n' "$runs" >&2
  exit 1
fi

if [[ ! -e "$touch_path" ]]; then
  printf 'error: touch path does not exist: %s\n' "$touch_path" >&2
  exit 1
fi

run_bench() {
  local name="$1"
  shift

  local stdout_file stderr_file status
  stdout_file=$(mktemp)
  stderr_file=$(mktemp)
  if scripts/bench_compile.sh "$@" --json >"$stdout_file" 2>"$stderr_file"; then
    python3 - "$name" "$stdout_file" <<'PY'
import json
import pathlib
import sys

name = sys.argv[1]
payload = json.loads(pathlib.Path(sys.argv[2]).read_text())
payload["checkpoint"] = name
payload["ok"] = True
print(json.dumps(payload))
PY
  else
    status=$?
    python3 - "$name" "$status" "$stderr_file" <<'PY'
import json
import pathlib
import sys

name = sys.argv[1]
status = int(sys.argv[2])
stderr = pathlib.Path(sys.argv[3]).read_text().strip()
print(json.dumps({
    "checkpoint": name,
    "ok": False,
    "exit_code": status,
    "error": stderr,
}))
PY
  fi
  rm -f "$stdout_file" "$stderr_file"
}

cold_check_json=$(run_bench cold_check check --cold)
warm_check_json=$(run_bench warm_check_edit check --runs "$runs" --touch "$touch_path")
cold_selfdev_json=$(run_bench cold_selfdev_build selfdev-jcode --cold)
warm_selfdev_json=$(run_bench warm_selfdev_edit selfdev-jcode --runs "$runs" --touch "$touch_path")

summary_json=$(python3 - <<'PY' "$touch_path" "$runs" "$cold_check_json" "$warm_check_json" "$cold_selfdev_json" "$warm_selfdev_json"
import json
import sys

touch_path = sys.argv[1]
runs = int(sys.argv[2])
cold_check = json.loads(sys.argv[3])
warm_check = json.loads(sys.argv[4])
cold_selfdev = json.loads(sys.argv[5])
warm_selfdev = json.loads(sys.argv[6])

summary = {
    "touch_path": touch_path,
    "warm_runs": runs,
    "checkpoints": {
        "cold_check": cold_check,
        "warm_check_edit": warm_check,
        "cold_selfdev_build": cold_selfdev,
        "warm_selfdev_edit": warm_selfdev,
    },
    "failed_checkpoints": [
        name for name, payload in {
            "cold_check": cold_check,
            "warm_check_edit": warm_check,
            "cold_selfdev_build": cold_selfdev,
            "warm_selfdev_edit": warm_selfdev,
        }.items()
        if not payload.get("ok", False)
    ],
}
print(json.dumps(summary))
PY
)

if [[ $json_output -eq 1 ]]; then
  printf '%s\n' "$summary_json"
else
  python3 - <<'PY' "$summary_json"
import json
import sys

summary = json.loads(sys.argv[1])
print("selfdev compile checkpoints")
print(f"  touch_path: {summary['touch_path']}")
print(f"  warm_runs:  {summary['warm_runs']}")
for name, payload in summary["checkpoints"].items():
    if payload.get("ok", False):
        print(
            f"  {name}: min={payload['min_seconds']:.3f}s "
            f"median={payload['median_seconds']:.3f}s avg={payload['avg_seconds']:.3f}s "
            f"max={payload['max_seconds']:.3f}s"
        )
    else:
        print(
            f"  {name}: FAILED exit={payload.get('exit_code')} error={payload.get('error', '')[:160]}"
        )
if summary["failed_checkpoints"]:
    print(f"  failed_checkpoints: {', '.join(summary['failed_checkpoints'])}")
PY
fi
