# Contributing to jcode

Thanks for contributing.

This repo moves quickly, so quality expectations must be explicit and enforceable.

## Baseline workflow

Before opening a PR, run the relevant checks for your change. At minimum:

```bash
cargo fmt --all -- --check
cargo check -q
scripts/check_warning_budget.sh
scripts/check_code_size_budget.py
scripts/check_panic_budget.py
scripts/check_swallowed_error_budget.py
```

When you touch core orchestration, provider, server, or TUI code, run the stricter set too:

```bash
cargo check --all-targets --all-features
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-targets --all-features
cargo test --test e2e
```

For larger refactors, use:

```bash
scripts/refactor_phase1_verify.sh
```

## Code quality guardrails

### File size targets

These are targets, not excuses to stop refactoring.

- No production Rust file should exceed **1200 LOC** without a documented reason.
- Most production Rust files should stay below **800 LOC**.
- Existing oversized files are under a ratcheting budget via `scripts/check_code_size_budget.py`.
- If you intentionally shrink oversized files, update the baseline with:

```bash
scripts/check_code_size_budget.py --update
```

Do **not** update the baseline to permit growth.

### Function size targets

- Most functions should stay below **100 LOC**.
- Functions larger than that should usually be split into helpers, reducers, or service methods.
- If a function must stay large temporarily, leave it clearer than you found it and avoid making it larger.

### Warning policy

- Do not introduce new warnings.
- The warning baseline is ratcheted downward via `scripts/check_warning_budget.sh`.
- If your change removes warnings, update the baseline deliberately:

```bash
scripts/check_warning_budget.sh --update
```

Do **not** update the baseline to permit new warnings.

### Lint policy

- `cargo clippy --all-targets --all-features -- -D warnings` should stay green.
- Prefer fixing lint findings over suppressing them.
- Broad module-level suppressions are strongly discouraged.
- If a suppression is truly necessary, make it narrow and document why.

### Error handling policy

- Production panic-prone usage must stay at zero via `scripts/check_panic_budget.py`.
- Suspicious swallowed-error patterns are ratcheted via `scripts/check_swallowed_error_budget.py`.
- Do not add new `let _ = ...`, `.ok()`, or `.unwrap_or_default()` in production code unless the error is intentionally best-effort and there is no clearer helper available.
- Prefer propagating errors with context, or logging failures with enough context to debug the affected session/provider/path.

### Refactor rules

- Prefer behavior-preserving extraction before logic changes.
- Avoid mixing unrelated cleanup with feature work unless the cleanup is required to make the change safe.
- When you touch a large file, leave it smaller, clearer, or better-tested.
- Delete dead code instead of carrying it forward.
- Prefer typed enums/structs over new stringly-typed states.
- Avoid silently ignoring important errors on persistence, protocol, or lifecycle paths.

## Repository docs

If you are working on quality or refactors, read:

- `docs/REFACTORING.md`
- `docs/CODE_QUALITY_10_10_PLAN.md`
- `docs/CODE_QUALITY_TODO.md`
- `docs/COMPILE_PERFORMANCE_PLAN.md`

## CI expectations

CI is expected to catch:

- formatting regressions
- all-target/all-feature compile regressions
- clippy regressions
- warning-budget regressions
- oversized-file regressions

If your change weakens a guardrail, treat that as a design change and document the reason clearly.
