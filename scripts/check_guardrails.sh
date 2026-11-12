#!/usr/bin/env bash
# Run every gate in CI's "Quality Guardrails" + "Format" jobs, locally.
#
# Why this exists: the guardrail steps live only in .github/workflows/ci.yml, so
# the usual way to discover one is to push and watch master go red. That is slow
# and, with several agents pushing in parallel, it means whoever pushes next
# inherits someone else's red build. Run this before pushing instead.
#
# Usage:
#   scripts/check_guardrails.sh              # check only, non-zero on failure
#   scripts/check_guardrails.sh --fix        # rustfmt + rebaseline ratchets
#   scripts/check_guardrails.sh --skip-slow  # skip cargo check/clippy/machete
#
# Note: CI tracks the `stable` toolchain. If your local stable is behind, clippy
# can pass here and fail in CI on a newly added lint, so this warns when the two
# are likely to disagree. Run `rustup update stable` to align them.

set -uo pipefail
cd "$(dirname "$0")/.."

FIX=false
SKIP_SLOW=false
for arg in "$@"; do
    case "$arg" in
        --fix) FIX=true ;;
        --skip-slow) SKIP_SLOW=true ;;
        -h|--help) sed -n '2,17p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
        *) echo "unknown flag: $arg (try --help)" >&2; exit 2 ;;
    esac
done

FAILED=()
JOBS="${CARGO_BUILD_JOBS:-2}"

run_gate() {
    local label=$1
    shift
    printf '▸ %s' "$label"
    local output
    if output=$("$@" 2>&1); then
        printf '\r✅ %s\n' "$label"
        return 0
    fi
    printf '\r❌ %s\n' "$label"
    printf '%s\n' "$output" | tail -20 | sed 's/^/    /'
    FAILED+=("$label")
    return 1
}

# Ratchet scripts share a --update flag to accept intentional growth.
run_ratchet() {
    local label=$1 script=$2
    if $FIX; then
        python3 "scripts/$script" --update >/dev/null 2>&1
    fi
    run_gate "$label" python3 "scripts/$script"
}

echo "=== Format ==="
# Before rustfmt: a `mod x;` with no file makes rustfmt fail with "Error writing
# files: failed to resolve mod", which reads like a formatting problem and hides
# every gate behind it. Naming the real cause first turns a confusing Format
# failure into an obvious one (221159294).
run_gate "module declarations resolve" python3 scripts/check_module_files.py
if $FIX; then
    cargo fmt --all
fi
run_gate "cargo fmt --all --check" cargo fmt --all --check

echo ""
echo "=== Quality Guardrails ==="
if $SKIP_SLOW; then
    echo "⏭  cargo check / clippy / machete (--skip-slow)"
else
    run_gate "cargo check --all-targets --all-features" \
        cargo check --all-targets --all-features -j "$JOBS"
    run_gate "cargo clippy -- -D warnings" \
        cargo clippy --all-targets --all-features -j "$JOBS" -- -D warnings
fi

# Only the Windows CI jobs pass --locked, so a stale lockfile otherwise passes
# 8 of 9 jobs and fails Windows at "Build release binary".
run_gate "Cargo.lock is up to date" cargo metadata --locked --format-version 1
run_gate "warning budget" bash scripts/check_warning_budget.sh
run_ratchet "oversized-file ratchet" check_code_size_budget.py
run_ratchet "oversized-test ratchet" check_test_size_budget.py
run_ratchet "panic-prone usage ratchet" check_panic_budget.py
run_ratchet "swallowed-error usage ratchet" check_swallowed_error_budget.py
run_gate "crate dependency boundaries" python3 scripts/check_dependency_boundaries.py
run_gate "wildcard re-export ratchet" python3 scripts/check_wildcard_reexport_budget.py

# Onboarding state-space invariants. The onboarding flow is a graph, and the
# properties that keep users unstuck (no dead ends, every failure has a recovery
# edge, an escape hatch everywhere, bounded keystrokes to a settled state) are
# checkable in microseconds. Every onboarding bug we have shipped was a violated
# invariant that nobody could see by reading one screen's code, so this gate is
# cheap insurance against the whole class.
run_gate "onboarding state-space invariants" \
    cargo test --profile selfdev -p jcode-tui -j "$JOBS" onboarding_graph::

if $SKIP_SLOW; then
    :
elif command -v cargo-machete >/dev/null 2>&1; then
    run_gate "unused dependencies (cargo machete)" cargo machete
else
    echo "⏭  cargo machete (not installed: cargo install cargo-machete --locked)"
fi

echo ""
# CI installs the current `stable`; a stale local toolchain hides new lints.
if command -v rustup >/dev/null 2>&1; then
    installed="$(rustup run stable rustc --version 2>/dev/null | awk '{print $2}')"
    if [[ -n "$installed" ]]; then
        echo "toolchain: stable = $installed (CI uses whatever \`stable\` is today)"
    fi
fi

if (( ${#FAILED[@]} )); then
    echo ""
    echo "❌ ${#FAILED[@]} gate(s) failed:"
    for f in "${FAILED[@]}"; do
        echo "   - $f"
    done
    if ! $FIX; then
        echo ""
        echo "For formatting and intentional ratchet growth: scripts/check_guardrails.sh --fix"
    fi
    exit 1
fi

echo "✅ All guardrail gates pass."
