# 12-Hour Overnight Prompt: Selfdev Reload Slice 1B and PR Watch Hardening

You are an overnight Jcode self-development agent working in `/home/shawn/business-projects/jcode`.

## Mission

Use up to 12 hours to implement, validate, and prepare a PR for the next follow-up slice after PR #2. The primary goal is **Slice 1B: fix `selfdev reload` repo discovery** with strong diagnostics and tests. If completed with time remaining, proceed to the explicitly listed secondary hardening work.

## Context

Recent work:

- PR #1 merged native PR feedback watch support.
- PR #2 merged a Claude-reviewed follow-up plan and PR watch runtime-state hygiene.
- Current next recommended work is Slice 1B from `docs/CLAUDE_REVIEWED_FOLLOW_UP_PLAN.md`.
- Known failure: after `selfdev build` succeeded, `selfdev reload` failed with:

```text
Could not find jcode repository directory
```

Relevant files:

- `docs/CLAUDE_REVIEWED_FOLLOW_UP_PLAN.md`
- `src/tool/selfdev/mod.rs`
- `src/tool/selfdev/reload.rs`
- `src/tool/selfdev/launch.rs`
- `src/tool/selfdev/build_queue.rs`
- `src/tool/selfdev/tests.rs`
- `src/build.rs` or build-support path helpers if applicable
- `crates/jcode-pr-watch-core/src/lib.rs`
- `src/tool/pr_watch.rs`

## Hard Requirements

1. Preserve user artifacts. Do not delete unrelated untracked files such as screenshots, `memory-bank/`, or `mobile-section-metrics.json`.
2. Commit as you go.
3. Prefer focused, reviewable changes over a large mixed PR.
4. Validate before claiming completion.
5. Use `selfdev build` for coordinated builds when applicable.
6. If you create a PR, open it against `ShawnSantiago/jcode`, not upstream `1jehuang/jcode`.
7. Do not merge PRs without explicit user confirmation.
8. PR watching is read-only by default. Do not push/comment/resolve threads unless explicitly needed for your own PR workflow and authorized by the user/context.

## Primary Task: Slice 1B, Fix `selfdev reload` Repo Discovery

### Investigation

1. Read `docs/CLAUDE_REVIEWED_FOLLOW_UP_PLAN.md`, especially Phase 1 and Final First Slice.
2. Trace the runtime flow:
   - `selfdev enter`
   - `selfdev build`
   - `selfdev reload`
   - how `ToolContext.working_dir` is populated
   - how `ReloadContext` is saved/loaded
   - how the actual repo used for build is selected
3. Identify why reload can fail to find the repo even when the session is in `/home/shawn/business-projects/jcode`.
4. Confirm whether `build::get_repo_dir()` relies on compile-time `CARGO_MANIFEST_DIR` and whether session `working_dir` can be stale or non-repo.

### Implementation Goals

Implement a robust solution with the following properties:

1. Reload resolution should prefer an explicit selfdev repo dir if one exists in selfdev context.
2. The actual repo dir used by `selfdev enter` or `selfdev build` should be persisted or passed into reload context.
3. Existing fallback behavior should continue to work:
   - primary build-support repo candidate
   - `ToolContext.working_dir` ancestor search
   - current process directory ancestor search, if appropriate
4. Failure diagnostics should be actionable and include paths considered:
   - explicit selfdev repo dir, if any
   - build-support/compile-time candidate, if known
   - supplied `working_dir`
   - process current directory
   - suggested next command
5. Avoid duplicate divergent repo-resolution helpers. Consolidate or clearly centralize the resolver.

### Tests

Add focused tests for the resolver and diagnostics. Cover at least:

1. Explicit selfdev repo dir wins.
2. Working directory ancestor fallback works.
3. Current process directory fallback works if supported.
4. Non-repo paths fail with a diagnostic listing attempted paths.
5. Existing session-scoped reload context tests still pass.

Prefer unit tests in `src/tool/selfdev/tests.rs` or a nearby test module. If additional integration coverage is practical, add it, but do not overexpand scope.

### Validation

Run, at minimum:

```bash
cargo test -p jcode selfdev --lib
cargo check -p jcode --bin jcode
```

Then run coordinated selfdev validation:

```text
selfdev build
selfdev reload
```

If `selfdev reload` succeeds, continue automatically after reload and record the outcome.

If `selfdev reload` still fails, capture the new diagnostics and either fix the remaining issue or document the exact blocker in the PR body.

## Secondary Task A: Native PR Watch No-Checks Hardening

Only start this if Slice 1B is complete, validated, and committed.

Known issue:

- `gh pr checks` can exit `1` with text like `no checks reported on the '<branch>' branch`.
- Manual polling treated that as zero checks, not a transient failure.
- Native `pr_watch` should likely do the same only when the output clearly means no checks exist.

Implementation goals:

1. Add deterministic tests or fixtures for:
   - no checks reported, exit code 1
   - pending checks, exit code 8
   - real command failure
2. Treat no-checks as a successful zero-check state.
3. Keep pending checks blocking quiet cycles.
4. Keep real failures as transient failures.

Validation:

```bash
cargo test -p jcode-pr-watch-core
cargo test -p jcode pr_watch --lib
cargo check -p jcode --bin jcode
```

## Secondary Task B: Native Bounded Monitor Design Spike

Only start if Primary and Secondary A are complete.

Do not implement a huge monitor loop unless it is clearly scoped. Prefer a design doc or small skeleton. The monitor must not be a 90-minute blocking tool call. It should be bounded, emit parseable progress, persist state every cycle, and return before harness timeouts.

Potential interface:

```text
pr_watch action="monitor" owner="..." repo="..." pr=N \
  poll_interval_seconds=300 quiet_cycles_required=3 max_runtime_minutes=10
```

Deliver either:

- a small implementation with tests, or
- a detailed design note and issue/PR plan.

## PR Workflow

1. Create a new branch from `master`, for example:

```bash
git switch master
git pull --ff-only origin master
git switch -c fix/selfdev-reload-repo-discovery
```

2. Commit logical slices separately:
   - reload repo discovery fix
   - tests/diagnostics
   - optional PR watch no-checks hardening
3. Open a PR against `ShawnSantiago/jcode`.
4. Include in the PR body:
   - summary
   - validation commands and results
   - whether `selfdev reload` was successfully dogfooded
   - any deferred work
5. Start read-only PR watch after opening the PR.
6. Stop after 3 quiet cycles or when user intervention is needed.

## Success Criteria

Primary success:

- `selfdev reload` no longer fails with `Could not find jcode repository directory` in the Jcode repo checkout.
- New diagnostics make future failures actionable.
- Tests cover repo resolution.
- A PR is opened or ready with validation results.

Stretch success:

- `gh pr checks` no-checks behavior is hardened in native `pr_watch`.
- Bounded monitor/watchdog design is advanced without destabilizing the codebase.

## Final Report Format

At the end of the overnight run, report:

1. Branch and PR URL, if created.
2. Commits made.
3. Files changed.
4. Validation commands and pass/fail results.
5. Whether `selfdev reload` was dogfooded successfully.
6. PR watch status, if applicable.
7. Remaining blockers or next recommended step.
