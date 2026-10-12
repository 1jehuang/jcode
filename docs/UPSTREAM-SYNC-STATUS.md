# Upstream Sync Status — 2026-10-12

## Headline

**The fork cannot mechanically absorb upstream changes via merge.** Of 152 untracked worktree artifacts, 47 occupy paths that exist in `upstream/master`. Git refuses to merge because the untracked files would be overwritten. Until those 47 paths are resolved, the monthly upstream-sync ritual is blocked.

This is the precondition the long-term plan assumed. The plan is not blocked by strategy; it is blocked by the worktree.

## What was attempted (2026-10-12 23:08)

1. `git fetch upstream` — succeeded. Upstream HEAD: `b3d4e2143 docs: update weekly stars chart`.
2. `git checkout -b sync/upstream-2026-10` from current master `e2f849ae6`.
3. `git merge upstream/master --no-edit` — **ABORTED**.

```
error: The following untracked working tree files would be overwritten by merge:
  .github/FUNDING.yml
  .github/workflows/label-pr.yml
  changelog/v0.85.0.json
  ... (47 files total)
Please move or remove them before you merge.
Aborting
Merge with strategy ort failed.
```

4. `git status` confirmed clean abort. Branch `sync/upstream-2026-10` left in place (identical to master; merge left no commits).

## The 47 conflict paths

These untracked paths also exist in `upstream/master`. The merge would overwrite them with upstream's version. The fork's untracked copies would be lost unless saved.

| Group | Count | Examples | Likely origin |
|---|---|---|---|
| `changelog/v0.85.0.json` … `changelog/v0.90.0.json` | 10 | release changelog entries | Manually copied from upstream at release time, never committed |
| `scripts/test_*.{sh,py}`, `scripts/bench_*.{py,md}`, `scripts/browser_handoff_*`, `scripts/fake_openai_server.py`, `scripts/rustc-parallel-frontend` | 12 | test/bench scripts | Fork-local scripts that overlap upstream test paths; need per-file diff |
| `docs/BROWSER_*.md`, `docs/CONIFER_PROVIDER.md`, `docs/PANELS.md`, `docs/REMOTE_COMPILE*.md`, `docs/SESSION_EDIT_STATS.md`, `docs/browser-*.md`, `docs/dev/GUY_ISSUE_ACCEPTANCE.md`, `docs/dev/JEV_MEMORY_ACCEPTANCE.md` | 12 | doc files | Fork-local docs that overlap upstream doc paths; need per-file diff |
| `sdk/typescript/src/tools.ts`, `sdk/typescript/test/{live-text-framing.mjs,live-tools.mjs,system-prompt.test.ts,text-framing.test.ts,tools.test.ts}` | 6 | TypeScript SDK | Fork TypeScript SDK files that overlap upstream |
| `src/cli/cloud_move.rs` | 1 | CLI command | Fork cloud_move command that overlaps upstream CLI |
| `tests/e2e/text_framing.rs`, `tests/support/` | 2 | tests | Fork test files that overlap upstream |
| `telemetry-worker/migrations/0027_daily_model_usage.sql`, `telemetry-worker/model-usage.sql` | 2 | telemetry | Fork telemetry files that overlap upstream |
| `.github/FUNDING.yml`, `.github/workflows/label-pr.yml` | 2 | CI config | Fork CI config that overlaps upstream |

**Total: 47** — exact list recoverable by running:

```bash
cd ~/wt-jcode-fix1
git status --porcelain | grep '^??' | awk '{print $2}' | while read p; do
  git cat-file -e "upstream/master:$p" 2>/dev/null && echo "$p"
done
```

## The 105 non-conflict paths (pure debris)

The remaining 152 − 47 = 105 untracked paths do NOT exist in `upstream/master`. They are pure fork debris — safe to delete without merge concerns because no upstream version would replace them. Per `WORKTREE-CLEANUP-STATE.md`, these break down as:

- 96 decollision rename leftovers (`crates/jcode-desktop*` + `crates/kcode-*`)
- ~9 misc (changelog entries not in upstream, fork-only docs, fork-only scripts that don't overlap upstream paths)

The 96 decollision debris are entirely fork-side; they are safe to delete immediately. The 9 misc need a quick scan, but they do not block the merge.

## The path forward

There are three sequential actions, each gated by the previous:

### Step 1 — Free the merge (LOW risk, operator-approval still required by policy)

Resolve the 47 conflict paths. Two strategies:

**Strategy A (lossy, fast):** delete all 47 untracked files. The merge will then succeed and bring in upstream's versions. Lose any fork-specific changes in those 47 untracked copies.
- Best for: changelogs (10), CI config (2), telemetry migrations (2) — these are likely identical or upstream-better.
- Risky for: scripts (12), docs (12), TypeScript SDK (6), CLI (1), tests (2) — these may contain fork-specific improvements.

**Strategy B (lossless, slow):** for each of the 47, diff `upstream/master:<path>` against the untracked local copy. Decide per-file:
- Identical: delete untracked, let merge bring it in.
- Fork-better: commit the untracked version to a `sync/upstream-2026-10` branch first, then merge (manual conflict resolution).
- Upstream-better: delete untracked, let merge bring it in.

**Recommended hybrid:**
- 10 changelogs → Strategy A (delete untracked, let merge bring upstream in)
- 2 .github + 2 telemetry + 1 tests/support + 1 tests/e2e → Strategy A (likely fork copies of upstream, low fork-specific value)
- 12 scripts + 12 docs + 6 sdk/typescript + 1 src/cli → Strategy B (per-file diff, keep fork-better)

**Why this is policy-gated:** the destructive form `git clean -fd` is on the blocklist. The per-file `rm` of the 47 paths is also mass-deletion. Per the destructive-command blocklist, mass deletion of uncommitted files requires explicit operator approval, even when individually targeted. Recommend the operator approve the 47-path resolution as a single decision.

### Step 2 — Clear the 96 decollision debris (MEDIUM risk, operator-approval required)

After Step 1, the 96 `crates/jcode-desktop*` + `crates/kcode-*` directories remain. They are pure fork debris. Delete via:

```bash
git clean -fd crates/jcode-desktop crates/jcode-desktop2 crates/kcode-*
```

This is `git clean` with explicit scope. It is on the blocklist per the destructive-command policy. Requires operator approval.

### Step 3 — Re-attempt the sync

After Steps 1 and 2:

```bash
git checkout -b sync/upstream-2026-10
git merge upstream/master --no-edit
```

Expected outcome: clean merge (or a small number of expected conflicts in fork-only paths like `crates/herdr-*`, `crates/jcode-new-crates/*`, decollision-rename leftovers in tracked files).

## What this changes in the long-term plan

The long-term plan assumed a monthly sync ritual. That ritual is now blocked until Steps 1 and 2 are done. The 30-day horizon is no longer "3 of 9 PRs filed" — it is:

1. Operator approves Step 1 (47-path resolution)
2. Operator approves Step 2 (96 decollision debris)
3. Sync is mechanically possible
4. THEN: 3 of 9 PRs filed, with demos, on a clean fork

Without operator approval, the fork remains unsyncable and the 2,649-behind number compounds.

## Cockpit-relevant summary

- Sync attempt: 2026-10-12 23:08
- Outcome: ABORTED
- Reason: 47 untracked files would be overwritten by upstream merge
- Recovery: 2 sequential operator-approved cleanups (Step 1 = 47 paths, Step 2 = 96 debris)
- After cleanup: re-attempt sync
- Branch state: `sync/upstream-2026-10` exists, identical to `master`, no merge commit; safe to `git branch -d sync/upstream-2026-10` after the cleanups complete
