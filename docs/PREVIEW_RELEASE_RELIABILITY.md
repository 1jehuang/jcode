# Automatic preview publishing and recovery

## Contract

A push to `master` starts **V1 preview**. After 15 minutes without another
push, it requires the same commit's push CI under the preview policy, builds
all six platform archives, verifies Linux ARM on a native runner, and publishes
only while that commit is still master. Compilation, Clippy, tests, lockfiles,
security checks and production safety ratchets remain blocking. Named
maintenance checks are advisory only for previews, not stable releases.

New pushes intentionally cancel obsolete previews. The quiet period is not a
promise to publish in 15 minutes: CI and all platform builds must finish too.
Manual/recovery dispatches queue instead of cancelling an active build, even
if the Actions run-list API has not caught up yet.

**Preview Recovery** now reconciles current master when push CI completes,
and at minutes 17 and 47 each hour. Scheduled Actions can be delayed by GitHub.
It dispatches the existing preview pipeline only when:

- Exact-commit source CI currently satisfies the same preview policy.
- There is no queued/running preview for that master commit.
- No published preview already points to that commit.
- Fewer than three preview workflow runs exist for that commit.
- The `v1.0.0` stable tag does not exist and preview publishing is enabled.

This allows a green CI rerun to recover an earlier rejection, and a transient
platform/build/publish failure to get a fresh preview number without another
commit. Each recovery still repeats the quiet period and all release gates.
Drafts and orphaned tags do not count as published previews. Successful releases
are deduplicated by the actual tag SHA, not mutable `target_commitish` names.
Retries do not reset the attempt budget with time or automatically rerun source
CI. Persistent failures stop consuming build minutes after three runs. Inspect
the failed job, fix it, then push, or explicitly dispatch **V1 preview** for a
manual attempt. Preview numbers can legitimately have gaps.

Recovery has no contents-write permission. On CI completion it only accepts
canonical-repository master push events and checks out trusted master, never
PR code or workflow artifacts. It cannot publish directly or waive failed CI.
`JCODE_V1_PREVIEW_ENABLED=false` disables both workflows.

## November 6, 2026 investigation

The last published preview when this investigation began was
`v1.0.0-preview.136` on November 3. Quiet-period runs `42026568210`,
`42133252764`, `42170324328`, `42186736286` and `42205421836` all rejected
**Quality Guardrails / Check all targets and all features** before building
release artifacts. This was not simply the expected cancellation during a
burst of commits.

Source CI logs exposed test constructors missing `Request::Subscribe` and
`Request::ResumeSession`'s `client_name` field. The same issue also broke
integration-test compilation. Clippy additionally rejected a dead-code mobile
push helper and identical branches in session compaction. Independent safety
gates exposed new panic-prone SSH retry handling and swallowed-error patterns
in cloud/server/usage paths. Fix the underlying code rather than increasing
safety baselines or making these gates advisory.

The old preview workflow was push/manual-only and rejected failed CI
immediately. A later successful CI rerun could not restart publishing, and a
transient release build failure had no automatic retry. The recovery workflow
addresses that separate orchestration gap without weakening authorization.

An additional hosted-build prerequisite failed during validation:
`POST /internal/builds/charge` returned a missing-route 404. Local justrust
reported a local fallback, which could not validate hosted execution. Linux
preview builds deliberately prohibit that fallback.
The live account API had drifted from its checked-in billing routes. A
separately reviewed, SHA-pinned additive backend restoration preserved all
existing live modules, 41 bindings/settings, mobile handlers, machine-key
restrictions, transactional billing and tenant isolation. No allowance or
billing bypass was introduced. The backend's 536 tests passed, and a real
forced-hosted Rust test passed with `justrust show` confirming
`where remote on hosted` (run `20261106-032554574-1606144`). The source-pinned
preview client did not need changing.

## Verification

```sh
python3 scripts/test_reconcile_preview_release.py
python3 scripts/test_preview_release.py
python3 tests/test_release_ci.py
python3 tests/test_ci_workflow_guards.py
actionlint .github/workflows/preview-recovery.yml .github/workflows/preview.yml .github/workflows/ci.yml
python3 scripts/check_panic_budget.py
python3 scripts/check_swallowed_error_budget.py
```

For a blocked preview, inspect **CI**, not just the preview's gate error.
**Preview Recovery** logs whether it skipped for pending/failed CI, an active
run, an existing release, an exhausted attempt budget, or the stable cutoff.
