# CI status — `KooshaPari/KCode` master

Generated: 2026-10-12
Source of truth: `.github/workflows/` (tracked) + `gh run list` recent activity.

## Headline

12 workflows configured on master. Recent activity (last 30 days) is dominated by `label-pr.yml` (the "Semantic PR labels" workflow), which is the auto-merge labeler — not a CI test. The actual CI workflows (ci.yml, freebsd-smoke, windows-smoke, ios-testflight) are configured but have not been triggered on master recently enough to appear in the last 30 runs.

## Configured workflows (12)

| Workflow | File | Tracked? | Recent activity |
|---|---|---|---|
| ci.yml | 28 KB (main test suite) | yes | not in last 30 master runs |
| freebsd-smoke.yml | 2.9 KB | yes | not in last 30 master runs |
| ios-testflight.yml | 4.5 KB | yes | not in last 30 master runs |
| windows-smoke.yml | 6.8 KB | yes | not in last 30 master runs |
| release.yml | 36 KB | yes | not in last 30 master runs |
| publish-github-packages.yml | 14 KB | yes | not in last 30 master runs |
| publish-typescript-sdk.yml | 2.4 KB | yes | not in last 30 master runs |
| discord-release.yml | 1.1 KB | yes | not in last 30 master runs |
| require-issue.yml | 4.5 KB | yes | not in last 30 master runs |
| update-star-history.yml | 959 B | yes | not in last 30 master runs |
| label-pr.yml | 1.7 KB | NO (untracked) | 30/30 recent master runs |
| pr8-review.yml | 2.0 KB | NO (untracked) | none observed |

## Recent run distribution (last 30, master)

- `success`: 18
- `action_required`: 10
- (no conclusion): 2

All 30 runs are from `label-pr.yml` (Semantic PR labels). The 10 `action_required` results are the auto-close behavior of label-pr when a PR merges or closes, not actual CI failures. The 2 with no conclusion are mid-flight.

## What this means

The "pre-existing CI failures on master" referenced in earlier session notes (ci.yml, windows-smoke, ios-testflight) cannot be reproduced from the current `gh run list` output — those workflows simply have not run on master in the last 30 days. They are configured and ready, but the recent push activity to master has not triggered them. This could mean:
- Recent master pushes came from squash-merges (which do not trigger downstream workflows on the merge commit)
- Or the workflows have a path/branch filter that excludes the merge commits
- Or the workflows were paused

## Recommendation

Without a concrete failing run to look at, there is nothing actionable to do here. The next time a PR or push to master triggers one of these workflows, the failure will be visible in `gh run list --branch master --workflow ci.yml` (or equivalent). Until then, this is a "no signal" state, not a "failing" state.

## What the operator can do

If the operator wants to actually exercise the CI, the minimal action is to push a small change to a non-master branch and open a PR. The `ci.yml` workflow should trigger on the PR. Any failure will then be visible in `gh run list --branch <branch>`.

## Regeneration

```bash
cd ~/wt-jcode-fix1
gh run list --branch master --limit 30 --json name,conclusion,status,headSha
ls .github/workflows/
```

Last regenerated: 2026-10-12
