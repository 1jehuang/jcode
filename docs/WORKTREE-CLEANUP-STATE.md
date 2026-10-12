# Worktree cleanup state — `~/wt-jcode-fix1`

Generated: 2026-10-12
Source of truth: `git status --porcelain` on master at `f6acfb1cb`.

## Headline

152 untracked artifacts in the worktree (was 154; 2 Emacs lock files removed at 2026-10-12 22:59). Cleanup beyond the 2 removed files is **blocked** by operator policy (the `git clean -fd` family requires explicit operator approval per the destructive-command blocklist). This file records the complete category breakdown so the operator can make the decision.

## Category breakdown

| # | Category | Count | Recommendation |
|---|---|---|---|
| 1 | `crates/kcode-*` (decollision rename leftover) | 94 | remove (BLOCKED, operator-approval needed) |
| 2 | `crates/jcode-desktop*` (rename-skipped variants) | 2 | remove (BLOCKED, operator-approval needed) |
| 3 | `docs/` (real new docs + session artifacts) | 15 | review-by-operator |
| 4 | `scripts/` (browser handoff, bench) | 14 | review-by-operator |
| 5 | `changelog/v*.json` (v0.85.0 → v0.90.0) | 10 | review-by-operator |
| 6 | `sdk/` | 6 | review-by-operator |
| 7 | `src/` | 3 | review-by-operator |
| 8 | `.github/` (FUNDING.yml + 2 workflows) | 3 | review-by-operator |
| 9 | `tests/` | 2 | review-by-operator |
| 10 | `telemetry-worker/` | 2 | review-by-operator |
| 11 | ~~`assets/app-icons/.!NNNN!Jcode.icns` (Emacs lock files)~~ | ~~2~~ | **DONE 2026-10-12 22:59** (removed) |
| 12 | `lefthook.yml` | 1 | review-by-operator |

Total: 152.

## Decollision debris (96 artifacts)

These are the 96 directories that exist because PR #33 renamed jcode → kcode, and the follow-up PR #36 reverted the rename. After the revert, the original jcode names came back as tracked files, but the new (kcode-*) names remain as untracked.

- 94 of `crates/kcode-*/` directories
- 2 of `crates/jcode-desktop*/` (these names did not exist before the rename; leftover from intermediate rename state)

All 96 are safe to remove — they have no `git ls-files` counterpart and no other branch references them.

## Clearly junk — REMOVED 2026-10-12 22:59

- ~~`assets/app-icons/.!81348!Jcode.icns`~~ — removed
- ~~`assets/app-icons/.!82102!Jcode.icns`~~ — removed

These were Emacs auto-save lock files (the `.!NNNN!` prefix is the macOS auto-save marker). They were 0-byte lock files left behind when an editor closed without releasing. Now deleted. The directory still contains the real `Jcode.icns` (1.9 MB) which is tracked.

## Needs operator judgment (56 artifacts)

The remaining 56 are non-crates untracked files and directories that may be intentional new work but were never `git add`ed. Examples:

- `docs/PANELS.md`, `docs/CONIFER_PROVIDER.md`, `docs/HERDR_VS_ACP.md`, `docs/REMOTE_COMPILE*.md`, `docs/SESSION_EDIT_STATS.md`, `docs/BROWSER_*.md` — look like real new docs
- `docs/sessions/20261001-herdr-crash-persistence/`, `docs/sessions/pr8-review/` — session artifacts
- `docs/dev/GUY_ISSUE_ACCEPTANCE.md`, `docs/dev/JEV_MEMORY_ACCEPTANCE.md` — dev workflow docs
- `scripts/bench_headless_memory.py`, `scripts/benchmark_browser_handoff.py`, `scripts/browser_handoff_fixture.py` — benchmarks and handoff scripts
- `changelog/v0.85.0.json` … `changelog/v0.90.0.json` — 10 version release notes
- `lefthook.yml` — git hooks config
- `.github/FUNDING.yml`, `.github/workflows/label-pr.yml`, `.github/workflows/pr8-review.yml` — GitHub config

Each of these needs the operator to decide: commit, ignore, or remove.

## What the operator can do

Two options remain (lowest-risk already done):

1. ~~**Lowest risk** — remove the 2 Emacs lock files~~ — **DONE 2026-10-12 22:59**
2. **Medium risk** — remove the 96 decollision debris via a targeted `git clean` invocation scoped to `crates/jcode-desktop*` and `crates/kcode-*` (requires operator approval).
3. **High risk** — remove all 152 via the un-scoped form of the clean command (BLOCKED, requires explicit operator approval and a per-file review).

The full list of paths is in the `git status --porcelain` output; this file records the categorization so the operator does not have to re-derive it.

## Regeneration

To re-derive the category breakdown:
```bash
cd ~/wt-jcode-fix1
git status --porcelain | grep '^??' | awk '{print $2}' | awk -F'/' '{print $1}' | sort | uniq -c | sort -rn
```

Last regenerated: 2026-10-12
