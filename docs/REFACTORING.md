# Refactoring Roadmap

This document defines the safe, incremental path for refactoring jcode while preserving behavior.

See also:

- [`docs/CODE_QUALITY_10_10_PLAN.md`](plans/CODE_QUALITY_10_10_PLAN.md) for the code-quality target, phased uplift program, and initial hotspot refactor list.
- [`docs/COMPILE_PERFORMANCE_PLAN.md`](plans/COMPILE_PERFORMANCE_PLAN.md) for compile-speed baselines, tactical build workflow, and the workspace/crate split roadmap.

## Goals

- Keep existing sessions and user workflows stable during refactors.
- Make regressions visible early with repeatable checks.
- Reduce architectural coupling in stages (not big-bang rewrites).

## Non-Negotiable Safety Rules

1. Use an isolated environment for refactor runs:

   - `scripts/refactor_shadow.sh serve`
   - `scripts/refactor_shadow.sh run`
   - `scripts/refactor_shadow.sh build --release`

2. Before each refactor merge, run the phase-1 verification suite:

   - `scripts/refactor_phase1_verify.sh`

3. Warning count may not increase above baseline:

   - `scripts/check_warning_budget.sh`

4. Run security preflight before merges:

   - `scripts/security_preflight.sh`

5. Prefer behavior-preserving moves first (extract/rename/split), then logic changes.

## Phase Plan

### Phase 1: Safety + Hygiene (current)

- Add isolated dev/run workflow for refactors.
- Add repeatable verification script.
- Add warning-budget guard to prevent warning drift.
- Clean low-risk warning debt without functional changes.

### Phase 2: CLI Decomposition

- Move `main.rs` subcommand handlers into focused `src/cli/*` modules.
- Keep top-level `main()` as parse + dispatch.

### Phase 3: Server Decomposition

- Split `server.rs` by responsibility (session lifecycle, debug API, swarm coordination, reload/update).
- Replace stringly states with typed enums where practical.

### Phase 4: Agent Turn-Loop Unification

- Consolidate duplicated turn-loop variants into one shared engine with pluggable event sink.

### Phase 5: TUI State/Reducer Split

- Separate app state, command parsing, remote-event reduction, and rendering control.

### Phase 6: Provider State Isolation

- Reduce global mutable state by moving caches into explicit state holders.

## Verification Matrix

- Compile: `cargo check -q`
- Compile timing: `scripts/bench_compile.sh check --runs 3 --touch <hot-file>` and `scripts/bench_compile.sh release-jcode --runs 3`
- Warnings: `scripts/check_warning_budget.sh`
- Security: `scripts/security_preflight.sh`
- Unit+integration tests: `cargo test -q`
- E2E tests: `cargo test --test e2e -q`
- Combined: `scripts/refactor_phase1_verify.sh`
