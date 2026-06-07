#!/usr/bin/env bash
set -euo pipefail

# Thin wrapper used by the test/refactor helper scripts (test_fast.sh,
# test_e2e.sh, refactor_phase1_verify.sh, agent_trace.sh, real_provider_smoke.sh).
#
# Historically this exec'd bare `cargo "$@"`, which meant the test/check
# workflows ran with NONE of the adaptive build controls the selfdev path gets
# via dev_cargo.sh: no memory-aware job sizing, no fast linker (clang+lld/mold),
# and no remote-cargo offload beyond a manual env check. On a memory-constrained
# host that let a `cargo test`/`cargo check` here assume the full core count and
# trip earlyoom/OOM while a concurrent selfdev build was running -- the two
# wrappers fragmenting the build instead of cooperating.
#
# Delegating to dev_cargo.sh unifies both paths: test/check builds now get the
# same MemAvailable-based CARGO_BUILD_JOBS throttle, the same fast linker, and
# the same remote-cargo handling (dev_cargo.sh performs its own JCODE_REMOTE_CARGO
# preflight, so we no longer duplicate it here). dev_cargo.sh passes every arg
# straight through to cargo, only layering env/setup, so `test`/`check`/`build`
# semantics are unchanged. The low-memory *selfdev profile* overrides remain
# gated to --profile selfdev inside dev_cargo.sh, so plain `cargo test` (test
# profile, debug-assertions on) is untouched aside from the job/linker tuning.
repo_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)

exec "$repo_root/scripts/dev_cargo.sh" "$@"
