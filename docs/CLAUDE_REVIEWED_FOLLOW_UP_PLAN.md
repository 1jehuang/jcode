# Claude-Reviewed Follow-up Plan: Selfdev Reload and Native PR Watch Reliability

> Status: Claude-reviewed and revised
> Date: 2026-05-13
> Review: Claude Sonnet 4.6 subagent review completed. This revision incorporates the review's must-fix corrections around reload root cause, no-checks repro, monitor scope, gitignore policy, and first-slice sequencing.

## Goals

1. Restore the self-development loop so `selfdev build` followed by `selfdev reload` works from a Jcode repo checkout.
2. Make PR feedback watching reliable without ad-hoc `/tmp` scripts.
3. Preserve safety boundaries: read-only polling by default, no accidental push/comment/thread resolve/merge.
4. Leave a dogfooded workflow that can be reused by future Jcode PRs.

## Non-goals

- Do not redesign the whole scheduler in this pass.
- Do not change GitHub mutation policy defaults.
- Do not clean unrelated user artifacts in the workspace.
- Do not require release/LTO builds for validation.
- Do not bundle every improvement into one large PR.

## Current Findings

### What works

- PR #1 merged successfully with the new `jcode-pr-watch-core` crate and `pr_watch` tool.
- The core model covers the important review surfaces:
  - PR metadata
  - check runs/status
  - top-level issue comments
  - review comments
  - reviews
  - review threads and unresolved state
- Readiness logic is conservative around transient failures, merge state, review decisions, checks, unresolved threads, and quiet cycles.
- Direct manual polling with `gh` proved the workflow can handle repeated automated reviews until quiet-cycle success.
- Post-merge validation passed:
  - `cargo test -p jcode-overnight-core`
  - `cargo test -p jcode-pr-watch-core`
  - `cargo test -p jcode pr_watch --lib`
  - `cargo check -p jcode --bin jcode`
- `selfdev build` passed after merge.

### What does not work well

- `selfdev reload` failed with `Could not find jcode repository directory` despite running inside `/home/shawn/business-projects/jcode`.
- Scheduled PR watch polls/watchdogs repeatedly missed due cycles or timed out, forcing direct bounded bash/Python polling.
- Watch runtime state/logs are untracked under `.jcode/pr-feedback-watch`; this is a dirty-repo hazard and should be gitignored immediately.
- There is a likely edge case in native `pr_watch`: `gh pr checks` can return exit code 1 for `no checks reported`, while manual polling allowed exit codes 1 and 8. Confirm and fix separately from reload.

## Claude Review Summary

### Overall verdict

Claude judged the plan conditionally actionable but required two corrections before implementation:

1. The original reload hypothesis was too simplistic. `working_dir` is likely passed into `do_reload`; the actual problem is more likely a mismatch between compile-time repo paths, session working directory, and the actual repo used by the selfdev build/enter flow.
2. The no-checks fix needs a concrete repro or mock strategy, not a vague live-PR dependency.

### Additional accepted recommendations

- Tighten the first implementation slice.
- Gitignore live PR watch state before any broad coding to prevent accidental commits.
- Note and consolidate/remove the dead `SelfDevTool::resolve_repo_dir` duplication.
- Do not implement a 90-minute sleeping tool call. Any monitor mode must be bounded to the harness/background-task timeout model.
- Dogfooding should not block on Phase 3 if native monitor mode is not ready.

## Phase 0: Immediate repo hygiene

### Implementation steps

1. Add this exact ignore rule:

```gitignore
.jcode/pr-feedback-watch/
```

2. Do not delete existing local state/logs unless the user explicitly asks.
3. Keep curated fixtures outside the live runtime path, for example under a test fixture directory.

### Acceptance criteria

- Future PR watch runs do not dirty the repo with live state/log files.
- Existing local artifacts are left untouched.

## Phase 1: Fix `selfdev reload` repo discovery

### Revised hypothesis

`src/tool/selfdev/reload.rs` resolves the repo via:

```rust
resolve_selfdev_reload_repo_dir_from(build::get_repo_dir(), working_dir)
```

The problem is probably not simply that `working_dir` is unpropagated. Claude's review found that `do_reload` is called with `ctx.working_dir.as_deref()`, and `build::get_repo_dir()` already has multiple fallback strategies.

The more likely failure mode is:

- `get_repo_dir()` uses a compile-time `CARGO_MANIFEST_DIR` path that may not correspond to the source tree from which the running selfdev session should reload.
- The session `working_dir` may reflect the parent/session working directory rather than the actual Jcode repo used by `selfdev enter` or `selfdev build`.
- A duplicated/dead helper, `SelfDevTool::resolve_repo_dir`, may confuse future changes and should be consolidated or removed.

### Implementation steps

1. Inspect the `selfdev enter`, `selfdev build`, and `selfdev reload` call path to identify the actual repo directory selected for the selfdev build.
2. Persist or pass the actual resolved repo directory used by selfdev build/enter into reload context, and prefer it over weaker fallbacks.
3. Consolidate or remove the dead/duplicated `SelfDevTool::resolve_repo_dir` helper so there is one authoritative repo-resolution path.
4. Add focused tests around reload repo resolution:
   - explicit repo dir from selfdev context wins
   - primary repo path wins when valid
   - working directory ancestor fallback works
   - current process directory fallback works if intentionally supported
   - non-repo paths return a diagnostic error
5. Improve the reload error message to list paths considered:
   - compile-time/build-support candidate
   - selfdev context repo dir, if any
   - supplied working directory
   - process current directory
   - suggested build/reload command
6. Validate with:
   - focused unit tests
   - `cargo test -p jcode selfdev --lib` or the nearest narrower target
   - `cargo check -p jcode --bin jcode`
   - `selfdev build`
   - `selfdev reload`

### Acceptance criteria

- Running `selfdev reload` from `/home/shawn/business-projects/jcode` no longer fails with repo discovery error.
- If reload cannot find a repo, the error clearly says which paths were tried.
- There is only one authoritative selfdev repo-resolution path, or duplicated code is clearly removed.
- Existing selfdev tests continue to pass.

## Phase 2: Harden native PR watch collection edge cases

### Concrete repro/test strategy

Do not rely on the already-merged PR #1 for this. Use one of:

1. Create a temporary/draft PR in a repo with no CI and record exact `gh pr checks` exit code/output.
2. Preferably add unit tests that inject mocked `gh pr checks` results:
   - exit code 1 with `no checks reported` or empty no-checks output
   - exit code 8 for pending checks
   - non-JSON/non-empty real failure

### Implementation steps

1. Confirm how the current native collector handles no-checks output.
2. Treat `gh pr checks` exit code 1 only as success when the output clearly means no checks are reported.
3. Keep pending checks blocking quiet cycles.
4. Keep real collection failures as transient failures that block readiness.
5. Re-check top-level comments and thread-reply fixtures while in this code.

### Acceptance criteria

- A PR with no checks does not block quiet cycles.
- Pending checks still block quiet cycles.
- Real collection failures still block readiness.
- Behavior is covered by tests or deterministic fixtures.

## Phase 3: Replace ad-hoc watch scripts with native bounded monitor mode

### Problem

The skill says to create a detached watchdog, but the merged tool mostly exposes stateful poll/schedule actions. Dogfooding showed that ambient scheduled tasks can miss due polls, and background shell loops timed out when they did not self-exit correctly.

### Recommended design

Add a native `pr_watch` action or mode for bounded monitor execution, but do not make it a single long 90-minute blocking tool call.

Example interface:

```text
pr_watch action="monitor" owner="..." repo="..." pr=N \
  poll_interval_seconds=300 quiet_cycles_required=3 max_runtime_minutes=10
```

Behavior:

1. Acquires a per-watch lock.
2. Polls immediately or acknowledges baseline if needed.
3. Performs only as many poll/sleep cycles as fit safely inside `max_runtime_minutes`.
4. Emits progress lines in `JCODE_PROGRESS {json}` format.
5. Writes state after every poll before scheduling, sleeping, or returning.
6. Returns before the harness/background task timeout.
7. Records `next_poll_at` and a clear next recommended action if quiet cycles are not complete.
8. Stops with success after quiet cycles are satisfied.
9. Stops with blocked/failure if actionable feedback appears and automation is not authorized to mutate.

### Watchdog design

Add either:

- `pr_watch action="watchdog"` for independent health checks, or
- monitor-owned watchdog state plus a scheduled one-shot fallback.

The watchdog should alert if:

- `updated_at` is older than two poll intervals
- `next_poll_at` is overdue and no poll is running
- transient failures are present
- monitor exits before quiet-cycle success or user stop

### Acceptance criteria

- Starting a monitor produces a background task that naturally exits before the harness timeout.
- It reports progress/checkpoints parseably.
- It does not rely on `/tmp/prwatch_*.py` scripts.
- It watches top-level comments, review comments, reviews, and threads.
- It can be run read-only by default.

## Phase 4: Dogfood on a follow-up PR

1. Create a branch for these fixes.
2. Implement Phase 0 and commit.
3. Implement Phase 1 and commit.
4. Implement Phase 2 only after the no-checks behavior is reproduced or mocked.
5. Implement Phase 3 in a separate PR if scope grows.
6. Open a PR against `ShawnSantiago/jcode`.
7. Use native `pr_watch` to monitor that PR if Phase 3 is available. If not, use the existing safe direct bounded polling workflow and document the gap.
8. Merge only after quiet-cycle success and explicit user confirmation.

## Risks and mitigations

| Risk | Mitigation |
| --- | --- |
| Reload changes affect normal sessions | Keep changes focused to repo discovery and diagnostics; test normal fallback paths |
| Compile-time `CARGO_MANIFEST_DIR` mismatches runtime source path | Prefer explicit selfdev context repo dir and add diagnostics |
| Monitor loop becomes another long-running fragile task | Bound max runtime, emit progress, return before timeout, persist state every cycle |
| GitHub CLI behavior varies by version | Add parser tests for observed outputs and fallback handling |
| Runtime state leaks into commits | Add `.jcode/pr-feedback-watch/` to `.gitignore` before broad edits and untrack existing live state |
| Accidental GitHub mutations | Keep native monitor read-only unless explicit authorization is stored in policy |
| Repo-resolution helpers diverge | Audit `SelfDevTool::resolve_repo_dir` usage in Slice 1A and consolidate during Slice 1B when reload behavior is changed |

## Final First Slice

Claude recommended splitting the first work into two small slices.

### Slice 1A: Zero-risk hygiene

1. Add `.jcode/pr-feedback-watch/` to `.gitignore`.
2. Untrack any already tracked live PR watch state with `git rm --cached`, leaving local artifacts untouched.
3. Audit `SelfDevTool::resolve_repo_dir` references. If it is still used by build/enter paths, defer consolidation to Slice 1B instead of removing it in a hygiene-only PR.
4. Validate with a narrow test or at least `cargo check -p jcode --bin jcode` if code changed.

### Slice 1B: Reload fix

1. Record or pass the actual resolved repo dir from selfdev enter/build context into reload.
2. Prefer that explicit repo dir in reload resolution.
3. Improve diagnostics for repo discovery failure.
4. Add focused tests for resolution and diagnostics.
5. Validate with:
   - `cargo test -p jcode selfdev --lib`
   - `cargo check -p jcode --bin jcode`
   - `selfdev build`
   - `selfdev reload`

### Explicitly deferred

- Do not bundle Phase 2 no-checks handling into Slice 1B unless it is already reproduced/mocked and trivial.
- Do not bundle Phase 3 monitor/watchdog into the reload-fix PR.
