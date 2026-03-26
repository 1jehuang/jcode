#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
cd "$repo_root"

usage() {
  cat <<'USAGE'
Usage:
  scripts/bench_compile.sh <target> [options] [-- <extra cargo args>]

Targets:
  check            Run cargo check --quiet
  build            Run cargo build --quiet
  release-jcode    Run scripts/dev_cargo.sh build --release -p jcode --bin jcode --quiet

Options:
  --cold                 Run cargo clean before timing the first run
  --touch <path>         Touch a source file before each timed run to simulate an edit
  --edit <path>          Toggle a harmless text edit before each run (restored afterward)
  --runs <n>             Number of timed runs to execute (default: 1)
  --json                 Print per-run + summary data as JSON
  -h, --help             Show this help

Examples:
  scripts/bench_compile.sh check
  scripts/bench_compile.sh check --runs 3 --touch src/server.rs
  scripts/bench_compile.sh check --runs 3 --edit src/server.rs
  scripts/bench_compile.sh build -- --package jcode --bin test_api
  scripts/bench_compile.sh release-jcode --json
USAGE
}

if [[ $# -gt 0 ]] && [[ "$1" == "-h" || "$1" == "--help" ]]; then
  usage
  exit 0
fi

target="${1:-}"
shift || true

if [[ -z "$target" ]]; then
  usage
  exit 1
fi

cold=0
touch_path=""
edit_path=""
runs=1
json_output=0
extra_args=()

while [[ $# -gt 0 ]]; do
  case "$1" in
    --cold)
      cold=1
      ;;
    --touch)
      if [[ $# -lt 2 ]]; then
        printf 'error: --touch requires a path\n' >&2
        exit 1
      fi
      touch_path="$2"
      shift
      ;;
    --edit)
      if [[ $# -lt 2 ]]; then
        printf 'error: --edit requires a path\n' >&2
        exit 1
      fi
      edit_path="$2"
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
    --)
      shift
      extra_args=("$@")
      break
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

if [[ -n "$touch_path" && -n "$edit_path" ]]; then
  printf 'error: --touch and --edit are mutually exclusive\n' >&2
  exit 1
fi

case "$target" in
  check)
    cmd=(cargo check --quiet)
    ;;
  build)
    cmd=(cargo build --quiet)
    ;;
  release-jcode)
    cmd=(scripts/dev_cargo.sh build --release -p jcode --bin jcode --quiet)
    ;;
  *)
    printf 'error: unsupported target: %s\n' "$target" >&2
    usage
    exit 1
    ;;
esac

if [[ ${#extra_args[@]} -gt 0 ]]; then
  cmd+=("${extra_args[@]}")
fi

if [[ -n "$touch_path" ]] && [[ ! -e "$touch_path" ]]; then
  printf 'error: touch path does not exist: %s\n' "$touch_path" >&2
  exit 1
fi

if [[ -n "$edit_path" ]] && [[ ! -f "$edit_path" ]]; then
  printf 'error: edit path must be an existing file: %s\n' "$edit_path" >&2
  exit 1
fi

edit_backup=""
cleanup() {
  if [[ -n "$edit_backup" && -n "$edit_path" && -f "$edit_backup" ]]; then
    cp "$edit_backup" "$edit_path"
    rm -f "$edit_backup"
  fi
}
trap cleanup EXIT

if [[ -n "$edit_path" ]]; then
  edit_backup=$(mktemp)
  cp "$edit_path" "$edit_backup"
fi

if [[ $cold -eq 1 ]]; then
  echo 'bench_compile: running cargo clean' >&2
  cargo clean
fi

printf 'bench_compile: target=%s cold=%s runs=%s\n' "$target" "$cold" "$runs" >&2
printf 'bench_compile: touch=%s\n' "${touch_path:-<none>}" >&2
printf 'bench_compile: edit=%s\n' "${edit_path:-<none>}" >&2
printf 'bench_compile: command=%s\n' "${cmd[*]}" >&2

run_times=()

run_once() {
  local run_index="$1"
  if [[ -n "$touch_path" ]]; then
    echo "bench_compile: touching $touch_path (run $run_index/$runs)" >&2
    touch "$touch_path"
  elif [[ -n "$edit_path" ]]; then
    echo "bench_compile: editing $edit_path (run $run_index/$runs)" >&2
    python3 - "$edit_backup" "$edit_path" "$run_index" <<'PY'
from pathlib import Path
import sys

backup = Path(sys.argv[1]).read_bytes()
target = Path(sys.argv[2])
run_index = int(sys.argv[3])

if run_index % 2 == 1:
    target.write_bytes(backup + b"\n")
else:
    target.write_bytes(backup)
PY
  fi

  local start_ns end_ns elapsed_ns elapsed_secs
  start_ns=$(python3 - <<'PY'
import time
print(time.perf_counter_ns())
PY
)

  "${cmd[@]}"

  end_ns=$(python3 - <<'PY'
import time
print(time.perf_counter_ns())
PY
)
  elapsed_ns=$((end_ns - start_ns))
  elapsed_secs=$(python3 - "$elapsed_ns" <<'PY'
import sys
print(f"{int(sys.argv[1]) / 1_000_000_000:.3f}")
PY
)

  run_times+=("$elapsed_secs")

  if [[ $json_output -eq 0 ]]; then
    printf 'bench_compile: run %s/%s real %ss\n' "$run_index" "$runs" "$elapsed_secs" >&2
  fi
}

for ((i = 1; i <= runs; i++)); do
  run_once "$i"
done

summary_json=$(python3 - "$target" "$cold" "$touch_path" "$edit_path" "$runs" "${cmd[*]}" "${run_times[@]}" <<'PY'
import json
import statistics
import sys

target = sys.argv[1]
cold = sys.argv[2] == "1"
touch = sys.argv[3]
edit = sys.argv[4]
runs = int(sys.argv[5])
command = sys.argv[6]
times = [float(v) for v in sys.argv[7:]]
summary = {
    "target": target,
    "cold": cold,
    "touch": touch or None,
    "edit": edit or None,
    "runs": runs,
    "command": command,
    "times_seconds": times,
    "min_seconds": min(times),
    "max_seconds": max(times),
    "avg_seconds": sum(times) / len(times),
    "median_seconds": statistics.median(times),
}
print(json.dumps(summary))
PY
)

if [[ $json_output -eq 1 ]]; then
  printf '%s\n' "$summary_json"
else
  python3 - "$summary_json" <<'PY' >&2
import json
import sys

summary = json.loads(sys.argv[1])
print(
    "bench_compile: summary "
    f"min={summary['min_seconds']:.3f}s "
    f"median={summary['median_seconds']:.3f}s "
    f"avg={summary['avg_seconds']:.3f}s "
    f"max={summary['max_seconds']:.3f}s"
)
PY
fi
