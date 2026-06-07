#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
cd "$repo_root"

usage() {
  cat <<'USAGE'
Usage:
  scripts/compile_time_probe.sh [options]

Runs a full-feature selfdev jcode build with Cargo timings enabled and summarizes
critical-path-ish rustc units from target/cargo-timings/cargo-timing.html.

Options:
  --skip-build              Parse the latest timing HTML without running cargo
  --timing-html <path>      Parse a specific cargo timing HTML file
  --touch <path>            Touch a file before building to simulate an edit
  --profile <name>          Cargo profile to build (default: selfdev)
  --package <name>          Cargo package to build (default: jcode)
  --bin <name>              Cargo binary to build (default: jcode)
  --feature-profile <name>  JCODE_DEV_FEATURE_PROFILE for dev_cargo.sh (default: default)
  --json <path>             Write the parsed summary JSON to this path
  --top <n>                 Number of slowest units to print (default: 12)
  -h, --help                Show this help

Examples:
  scripts/compile_time_probe.sh --skip-build
  scripts/compile_time_probe.sh --touch crates/jcode-tui/src/tui/app/input.rs
  scripts/compile_time_probe.sh --json target/compile-time-probe.json

Notes:
  - This intentionally defaults to the full/default feature set. It is for
    compile-time isolation work that keeps debug/selfdev behavior production-like.
  - The "jcode serial stack" summary is not a formal Cargo critical path. It is a
    focused view of the known long-pole crates: jcode-base, jcode-app-core,
    jcode-tui, root jcode lib, and jcode bin.
USAGE
}

skip_build=0
timing_html=""
touch_path=""
profile="selfdev"
package="jcode"
bin="jcode"
feature_profile="default"
json_path=""
top_n=12

while [[ $# -gt 0 ]]; do
  case "$1" in
    --skip-build)
      skip_build=1
      ;;
    --timing-html)
      [[ $# -ge 2 ]] || { echo 'error: --timing-html requires a path' >&2; exit 1; }
      timing_html="$2"
      shift
      ;;
    --touch)
      [[ $# -ge 2 ]] || { echo 'error: --touch requires a path' >&2; exit 1; }
      touch_path="$2"
      shift
      ;;
    --profile)
      [[ $# -ge 2 ]] || { echo 'error: --profile requires a value' >&2; exit 1; }
      profile="$2"
      shift
      ;;
    --package|-p)
      [[ $# -ge 2 ]] || { echo 'error: --package requires a value' >&2; exit 1; }
      package="$2"
      shift
      ;;
    --bin)
      [[ $# -ge 2 ]] || { echo 'error: --bin requires a value' >&2; exit 1; }
      bin="$2"
      shift
      ;;
    --feature-profile)
      [[ $# -ge 2 ]] || { echo 'error: --feature-profile requires a value' >&2; exit 1; }
      feature_profile="$2"
      shift
      ;;
    --json)
      [[ $# -ge 2 ]] || { echo 'error: --json requires a path' >&2; exit 1; }
      json_path="$2"
      shift
      ;;
    --top)
      [[ $# -ge 2 ]] || { echo 'error: --top requires a positive integer' >&2; exit 1; }
      top_n="$2"
      shift
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      printf 'error: unknown argument: %s\n' "$1" >&2
      usage >&2
      exit 1
      ;;
  esac
  shift
done

if ! [[ "$top_n" =~ ^[1-9][0-9]*$ ]]; then
  printf 'error: --top must be a positive integer (got %s)\n' "$top_n" >&2
  exit 1
fi

if [[ -n "$touch_path" && ! -e "$touch_path" ]]; then
  printf 'error: touch path does not exist: %s\n' "$touch_path" >&2
  exit 1
fi

if [[ $skip_build -eq 0 ]]; then
  if [[ -n "$touch_path" ]]; then
    printf 'compile_time_probe: touching %s\n' "$touch_path" >&2
    touch "$touch_path"
  fi

  printf 'compile_time_probe: building %s/%s profile=%s feature_profile=%s with --timings\n' \
    "$package" "$bin" "$profile" "$feature_profile" >&2

  start_ns=$(python3 - <<'PY'
import time
print(time.perf_counter_ns())
PY
)

  JCODE_DEV_FEATURE_PROFILE="$feature_profile" \
    scripts/dev_cargo.sh build --profile "$profile" -p "$package" --bin "$bin" --timings

  end_ns=$(python3 - <<'PY'
import time
print(time.perf_counter_ns())
PY
)
  elapsed_seconds=$(python3 - "$start_ns" "$end_ns" <<'PY'
import sys
start = int(sys.argv[1])
end = int(sys.argv[2])
print(f"{(end - start) / 1_000_000_000:.3f}")
PY
)
else
  elapsed_seconds=""
fi

if [[ -z "$timing_html" ]]; then
  timing_html=$(find target/cargo-timings -maxdepth 1 -type f -name 'cargo-timing*.html' -printf '%T@ %p\n' 2>/dev/null | sort -n | tail -1 | cut -d' ' -f2- || true)
fi

if [[ -z "$timing_html" || ! -f "$timing_html" ]]; then
  printf 'error: no cargo timing HTML found; run without --skip-build or pass --timing-html\n' >&2
  exit 1
fi

python3 - "$timing_html" "$elapsed_seconds" "$json_path" "$top_n" "$profile" "$package" "$bin" "$feature_profile" <<'PY'
from __future__ import annotations

import json
import re
import sys
from pathlib import Path
from typing import Any

html_path = Path(sys.argv[1])
elapsed_arg = sys.argv[2]
json_path = Path(sys.argv[3]) if sys.argv[3] else None
top_n = int(sys.argv[4])
profile = sys.argv[5]
package = sys.argv[6]
bin_name = sys.argv[7]
feature_profile = sys.argv[8]

text = html_path.read_text(errors="replace")

def extract_duration() -> float | None:
    match = re.search(r"(?:const\s+)?DURATION\s*=\s*([0-9]+(?:\.[0-9]+)?)\s*;", text)
    return float(match.group(1)) if match else None


def extract_unit_data() -> list[dict[str, Any]]:
    marker = "const UNIT_DATA = "
    start = text.find(marker)
    if start < 0:
        raise SystemExit(f"error: {html_path} does not contain Cargo UNIT_DATA")
    idx = start + len(marker)
    while idx < len(text) and text[idx].isspace():
        idx += 1
    if idx >= len(text) or text[idx] != "[":
        raise SystemExit("error: Cargo UNIT_DATA did not start with '['")

    depth = 0
    in_string = False
    escape = False
    end = idx
    while end < len(text):
        ch = text[end]
        if in_string:
            if escape:
                escape = False
            elif ch == "\\":
                escape = True
            elif ch == '"':
                in_string = False
        else:
            if ch == '"':
                in_string = True
            elif ch == "[":
                depth += 1
            elif ch == "]":
                depth -= 1
                if depth == 0:
                    end += 1
                    break
        end += 1
    return json.loads(text[idx:end])


def section_duration(unit: dict[str, Any], section_name: str) -> float | None:
    sections = unit.get("sections")
    if not sections:
        return None
    for name, payload in sections:
        if name == section_name:
            return float(payload["end"]) - float(payload["start"])
    return None


def fmt_seconds(value: float | None) -> str:
    if value is None:
        return "n/a"
    return f"{value:.2f}s"

units = extract_unit_data()
for unit in units:
    unit["end"] = float(unit.get("start", 0.0)) + float(unit.get("duration", 0.0))
    unit["frontend_duration"] = section_duration(unit, "frontend")
    unit["codegen_duration"] = section_duration(unit, "codegen")

# Slowest rustc-ish units by duration. Keep build-script run units visible but low-noise.
top_units = sorted(units, key=lambda unit: float(unit.get("duration", 0.0)), reverse=True)[:top_n]

def is_jcode_stack_unit(unit: dict[str, Any]) -> bool:
    name = unit.get("name")
    target = unit.get("target") or ""
    if name in {"jcode-base", "jcode-app-core", "jcode-tui"} and "build script" not in target:
        return True
    if name == "jcode" and (target == "" or f'bin "{bin_name}"' in target):
        return True
    return False

jcode_stack = sorted([unit for unit in units if is_jcode_stack_unit(unit)], key=lambda unit: float(unit.get("start", 0.0)))
stack_span = None
if jcode_stack:
    stack_span = max(float(unit["end"]) for unit in jcode_stack) - min(float(unit.get("start", 0.0)) for unit in jcode_stack)
stack_sum = sum(float(unit.get("duration", 0.0)) for unit in jcode_stack)
stack_frontend_sum = sum(float(unit.get("frontend_duration") or 0.0) for unit in jcode_stack)
stack_codegen_sum = sum(float(unit.get("codegen_duration") or 0.0) for unit in jcode_stack)

summary = {
    "timing_html": str(html_path),
    "profile": profile,
    "package": package,
    "bin": bin_name,
    "feature_profile": feature_profile,
    "wall_seconds_from_cargo_timing": extract_duration(),
    "wall_seconds_measured_by_probe": float(elapsed_arg) if elapsed_arg else None,
    "unit_count": len(units),
    "top_units": [
        {
            "name": unit.get("name"),
            "version": unit.get("version"),
            "target": unit.get("target") or "",
            "features": unit.get("features") or [],
            "start_seconds": round(float(unit.get("start", 0.0)), 3),
            "duration_seconds": round(float(unit.get("duration", 0.0)), 3),
            "frontend_seconds": round(unit["frontend_duration"], 3) if unit.get("frontend_duration") is not None else None,
            "codegen_seconds": round(unit["codegen_duration"], 3) if unit.get("codegen_duration") is not None else None,
        }
        for unit in top_units
    ],
    "jcode_serial_stack": {
        "span_seconds": round(stack_span, 3) if stack_span is not None else None,
        "sum_unit_seconds": round(stack_sum, 3),
        "sum_frontend_seconds": round(stack_frontend_sum, 3),
        "sum_codegen_seconds": round(stack_codegen_sum, 3),
        "units": [
            {
                "name": unit.get("name"),
                "target": unit.get("target") or "",
                "start_seconds": round(float(unit.get("start", 0.0)), 3),
                "end_seconds": round(float(unit.get("end", 0.0)), 3),
                "duration_seconds": round(float(unit.get("duration", 0.0)), 3),
                "frontend_seconds": round(unit["frontend_duration"], 3) if unit.get("frontend_duration") is not None else None,
                "codegen_seconds": round(unit["codegen_duration"], 3) if unit.get("codegen_duration") is not None else None,
                "features": unit.get("features") or [],
            }
            for unit in jcode_stack
        ],
    },
}

if json_path:
    json_path.parent.mkdir(parents=True, exist_ok=True)
    json_path.write_text(json.dumps(summary, indent=2, sort_keys=True) + "\n")

print("compile_time_probe summary")
print(f"  timing html: {html_path}")
print(f"  cargo timing wall: {fmt_seconds(summary['wall_seconds_from_cargo_timing'])}")
if summary["wall_seconds_measured_by_probe"] is not None:
    print(f"  measured wall: {fmt_seconds(summary['wall_seconds_measured_by_probe'])}")
print(f"  units: {len(units)}")
print("  jcode serial stack:")
print(f"    span: {fmt_seconds(stack_span)}")
print(f"    sum: {stack_sum:.2f}s (frontend {stack_frontend_sum:.2f}s, codegen {stack_codegen_sum:.2f}s)")
for unit in jcode_stack:
    target = unit.get("target") or "lib"
    frontend = fmt_seconds(unit.get("frontend_duration"))
    codegen = fmt_seconds(unit.get("codegen_duration"))
    print(
        f"    - {unit.get('name')} {target}: "
        f"start {float(unit.get('start', 0.0)):.2f}s, "
        f"dur {float(unit.get('duration', 0.0)):.2f}s, "
        f"frontend {frontend}, codegen {codegen}"
    )
print(f"  top {top_n} units:")
for unit in top_units:
    target = unit.get("target") or "lib"
    frontend = fmt_seconds(unit.get("frontend_duration"))
    codegen = fmt_seconds(unit.get("codegen_duration"))
    print(
        f"    - {unit.get('name')} {target}: "
        f"{float(unit.get('duration', 0.0)):.2f}s "
        f"(frontend {frontend}, codegen {codegen})"
    )
if json_path:
    print(f"  wrote json: {json_path}")
PY
