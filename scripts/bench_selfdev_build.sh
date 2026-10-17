#!/usr/bin/env bash
# Repeatable self-dev build benchmark (issue #392, part C).
#
# Measures the wall-clock cost of the selfdev build paths that matter day to
# day, so build-pipeline changes are hill-climbable:
#
#   1. warm no-op        - rebuild with zero changes on the same commit
#   2. leaf touch        - 1-line change in the root `jcode` bin crate
#   3. tui touch         - 1-line change in jcode-tui (largest UI crate)
#   4. core touch        - 1-line change in jcode-app-core (mid-stack crate)
#   5. commit blast      - simulate HEAD moving (KCODE_BUILD_GIT_HASH change),
#                          which reruns jcode-build-meta's build script and
#                          recompiles every crate that depends on it
#                          (base, app-core, tui, setup-hints, telemetry-core, root)
#
# Touches are reverted after each run. Requires a clean-enough tree that
# `scripts/dev_cargo.sh build --profile selfdev -p jcode --bin jcode` succeeds.
#
# Usage: scripts/bench_selfdev_build.sh [--skip-warmup]

set -euo pipefail

repo_root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"

build_cmd=(scripts/dev_cargo.sh build --profile selfdev -p jcode --bin jcode)

run_build() {
    # Per-phase logs survive later phases, so a mid-benchmark failure (often a
    # concurrent agent breaking the shared tree) stays diagnosable.
    local label="$1"
    local log="/tmp/bench_selfdev_build_${label}.log"
    local start end elapsed
    start=$(date +%s.%N)
    if ! "${build_cmd[@]}" >"$log" 2>&1; then
        echo "  ${label}: BUILD FAILED (see ${log})"
        return 1
    fi
    end=$(date +%s.%N)
    elapsed=$(echo "$end $start" | awk '{printf "%.1f", $1 - $2}')
    echo "  ${label}: ${elapsed}s"
}

touch_file() {
    # Append and immediately strip a trailing comment marker so the file
    # content changes (mtime + hash) without altering semantics.
    local file="$1"
    printf '\n// bench_selfdev_build touch\n' >> "$file"
}

revert_file() {
    local file="$1"
    # Remove the exact marker we appended (plus its preceding blank line).
    python3 - "$file" << 'EOF'
import sys
path = sys.argv[1]
with open(path) as fh:
    content = fh.read()
marker = "\n// bench_selfdev_build touch\n"
if content.endswith(marker):
    content = content[: -len(marker)]
with open(path, "w") as fh:
    fh.write(content)
EOF
}

echo "selfdev build benchmark ($(git rev-parse --short HEAD), $(nproc) cpus)"
echo "command: ${build_cmd[*]}"
echo

if [[ "${1:-}" != "--skip-warmup" ]]; then
    echo "warmup (populate incremental caches)..."
    run_build "warmup" || exit 1
    echo
fi

echo "1. warm no-op (same commit, no changes):"
run_build "no-op"

leaf_file="src/main.rs"
echo "2. leaf touch (${leaf_file}):"
touch_file "$leaf_file"
run_build "leaf" || true
revert_file "$leaf_file"

tui_file="crates/jcode-tui/src/lib.rs"
echo "3. tui touch (${tui_file}):"
touch_file "$tui_file"
run_build "tui" || true
revert_file "$tui_file"

core_file="crates/jcode-app-core/src/lib.rs"
echo "4. core touch (${core_file}):"
touch_file "$core_file"
run_build "core" || true
revert_file "$core_file"

echo "5. commit blast radius (simulated HEAD move via KCODE_BUILD_GIT_HASH):"
fake_hash="bench$(date +%s | tail -c 4)"
start=$(date +%s.%N)
if KCODE_BUILD_GIT_HASH="$fake_hash" "${build_cmd[@]}" >/tmp/bench_selfdev_build_commit-blast.log 2>&1; then
    end=$(date +%s.%N)
    echo "$end $start" | awk '{printf "  commit-blast: %.1fs\n", $1 - $2}'
else
    echo "  commit-blast: BUILD FAILED (see /tmp/bench_selfdev_build_commit-blast.log)"
fi
echo "  (restoring real-hash build so the next selfdev build is warm)"
run_build "restore" || true

echo
echo "done. Interpretation:"
echo "  - no-op should be a few seconds (cargo fingerprinting + link check)."
echo "  - leaf/tui/core touches show per-crate recompile + link cost."
echo "  - commit-blast shows the cost every commit pays because"
echo "    jcode-build-meta embeds GIT_HASH via env! and jcode-base/app-core/"
echo "    tui/setup-hints/telemetry-core all depend on it."
