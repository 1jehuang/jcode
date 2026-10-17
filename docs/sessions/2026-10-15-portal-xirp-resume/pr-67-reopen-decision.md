# PR 67 reopen-vs-new decision

> **Banner — 2026-10-17:** The premise of this document ("PR 67 is CLOSED, not
> merged, reopen-vs-new needed") was a **misread**. On 2026-10-17T06:24Z we
> discovered that PR 67 was integrated to `KooshaPari/KCode:master` at commit
> `fed23782d` via the **harness-compliant squash-merge path** documented in
> AGENTS.md, which bypasses GitHub's merge button. `gh pr view 67` still
> returns `state=CLOSED, mergedAt=null` because the merge was via `git push`,
> not via the GH API. See `STATE.md` §16 (rewritten), §22 (NEW FINDING),
> §22.1 (full audit, 3 PRs), and the standalone doc
> `22-NEW-FINDING-gh-pr-view-closed-vs-merged.md`.
>
> **What this means for the recommendation below:** Option B (open a new PR)
> was executed — PR #103 was opened with the corrected body. PR #103's
> original "Closes #67" footer was a follow-on of the same misread and was
> retargeted to "Supersedes #67 — but the original was already on master" at
> commit `abf024199`. This document is retained for record only.

**Date:** 2026-10-17
**Repo:** `KooshaPari/Jcode` (decision text; the actual PR was opened on `KooshaPari/KCode`)
**State of PR 67 at time of writing:** CLOSED (not merged) since 2026-10-15T08:13:15Z
**State of PR 67 as of 2026-10-17T06:24Z:** **integrated to master at `fed23782d` via harness squash-merge** (see banner)
**Trigger:** §4 row K (apply the corrected PR 67 description) requires a vehicle.

## TL;DR

**Recommended: open a NEW PR (option B).** PR 67 was closed, not merely made
draft, and the work has accumulated six follow-up commits
(`d20634bfe`, `4d23d4247`, `143ef63ff`, `cb40ffe54`, `50504fb83`, `de358aeca`)
since the original. The diff is now small, the body is
correct, and reopening would carry stale review context.

The corrected body is at `docs/sessions/2026-10-15-portal-xirp-resume/pr-67-body.md`
and is still aligned with the branch (verified 2026-10-17 — see §11).

## Current state of PR 67

`gh pr view 67 --json state,headRefName,baseRefName,closedAt,mergedAt`:

```json
{
  "number": 67,
  "state": "CLOSED",
  "isDraft": false,
  "mergedAt": null,
  "closedAt": "2026-10-15T08:13:15Z",
  "headRefName": "chore/xirp-custom-harness-discovery-20261014",
  "baseRefName": "master",
  "title": "chore: prepare Xirp custom harness compatibility discovery"
}
```

Notes:
- **CLOSED, not merged.** Closed by whom and why was not captured in the
  record. The "STALE" marker in §1b is a description of the consequence, not a
  reason. Possible reasons include stale-bot action, manual close by the
  operator, or repo-rename noise. Whatever the cause, the *consequence* is the
  same: PR 67 is no longer the place to push the corrected body.
- **The head branch still exists and is current.** The same branch
  (`chore/xirp-custom-harness-discovery-20261014`) is at `de358aeca` on
  `KooshaPari/KCode` and has been the active worktree throughout the audit
  record.
- **The corrected body is in the WT, not on the live PR.** The corrected text
  was committed at `docs/sessions/2026-10-15-portal-xirp-resume/pr-67-body.md`
  after `gh pr edit 67` was deferred twice by the pre_tool hook with the same
  request id `hook-ff88046b9d4d5eb62452865d3a30d179`.

## The two options

### Option A: Reopen #67

**Mechanics:**
- `gh pr reopen 67 --repo KooshaPari/Jcode`
- Apply the corrected body (paste from `pr-67-body.md` or run
  `gh pr edit 67 --body-file pr-67-body.md` after inbox approval)
- The branch is already at `de358aeca`, so no force-push needed

**Pros:**
- Preserves the original PR title, the "Xirp custom harness compatibility
  discovery" framing
- Any review comments or bot annotations survive (if any were left)
- Single-PR history

**Cons:**
- "Reopen CLOSED PR" is a soft action; review tools (CodeRabbit, stale-bot)
  may re-fire on the old head
- Carries whatever baggage caused the original close (a stale-bot might close
  it again within hours if it sees the head as behind master)
- Cannot reset the closed timestamp; downstream metrics and notifications treat
  it as a new event

**Risk if #67 was closed by a stale-bot:** moderate. The fix is to push a
no-op commit or force-push the head, both of which are easy but visible.

### Option B (recommended): Open a new PR

**Mechanics:**
- The branch is already pushed at `chore/xirp-custom-harness-discovery-20261014`
  on `de358aeca`. No force-push needed.
- `gh pr create --repo KooshaPari/Jcode --base master --head chore/xirp-custom-harness-discovery-20261014 --title "docs: record Xirp custom harness discovery + audit-correction arc (Xirp 0.49.0, 2026-10-17 refresh)" --body-file docs/sessions/2026-10-15-portal-xirp-resume/pr-67-body.md`
- Optionally add a "Closes #67" footer in the body so the closed PR is
  superseded in any cross-references

**Pros:**
- Clean slate, fresh review context, no stale-bot risk
- Title can reflect the current scope (Xirp 0.49.0, 2026-10-17 refresh)
- Reviewers (if any are configured) get a single, current notification
- The closed PR stays as historical record of the original work

**Cons:**
- Slight loss of cross-reference continuity (mitigated by the "Closes #67" footer)
- A new number to track
- The new PR still needs to be draft (or live) — same as the original

**Risk:** low. The new PR is the standard "fix forward" path when an old PR
was closed.

## Why option B over option A

1. **The work has accumulated 6+ commits since the original PR body was
   written.** The body claims 46 non-zz repos (29 public, 17 private, 14
   archived, 13 forks, 32 active), `@chirp/squab` 0.11.11, and Xirp
   0.49.0+app.db5402dc7 — all still true on master — but the audit-correction
   arc and the operator-action addenda are now first-class parts of the
   deliverable. A new title conveys that.
2. **PR 67 was closed for a reason.** Without knowing the reason, reopening
   risks re-triggering it. A new PR removes that risk entirely.
3. **The cost of "Closes #67" is one line in the body.** Continuity is
   preserved at near-zero cost.
4. **The corrected body is now load-bearing for the new PR.** Any future
   rebase or new commit on the new PR should keep the body current; the
   audit-correction arc in `STATE.md §6` is the record of what changed.

## What this does NOT decide

- **Whether the new PR should be draft or live.** That is a separate
  question. The current convention in this record (PR 619 is a draft) suggests
  draft is fine for evidence-only changes.
- **Whether to send the vendor question (§4 row F) before opening the PR.**
  No — the PR is docs-only and can ship without the vendor answer.
- **Whether to merge the new PR immediately.** The convention in §6 says "Do
  not merge merely because a PR is mergeable." The new PR should sit draft
  until the operator decides the evidence is final.

## Files preserved

- `docs/sessions/2026-10-15-portal-xirp-resume/pr-67-body.md` — corrected body, ready to paste
- `docs/sessions/2026-10-15-portal-xirp-resume/STATE.md` §11 — original record of why this was needed
- `docs/sessions/2026-10-15-portal-xirp-resume/STATE.md` §4 row K — operator action

## Next operator step (when ready)

```bash
gh pr create --repo KooshaPari/Jcode \
  --base master \
  --head chore/xirp-custom-harness-discovery-20261014 \
  --title "docs: record Xirp custom harness discovery + audit-correction arc (Xirp 0.49.0, 2026-10-17 refresh)" \
  --body-file docs/sessions/2026-10-15-portal-xirp-resume/pr-67-body.md
```

(Add `--draft` if keeping with the PR 619 convention.)

Then close PR 67 with a comment pointing at the new PR.
