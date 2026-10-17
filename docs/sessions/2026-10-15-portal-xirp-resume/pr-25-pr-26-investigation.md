# Investigation: PR #25 "Closes #26" — alleged orphan

**Date:** 2026-10-17T22:34Z
**Investigator:** orchestrator (read-only)
**Source flag:** `docs/sessions/2026-10-15-portal-xirp-resume/STATE.md` row V / `22-NEW-FINDING-gh-pr-view-closed-vs-merged.md` lines 165–177

## TL;DR — the §22 audit was a false positive.

The audit's `gh pr view 26 --repo KooshaPari/KCode` failed with
"Could not resolve to a PullRequest with the number of 26" because
**#26 is an issue, not a PR**. The audit's subagent then concluded the
entity doesn't exist. It does: `gh issue view 26 --repo KooshaPari/KCode`
returns a live issue, and the REST API `/repos/KooshaPari/KCode/issues/26`
returns the same. The "Closes #26" reference is valid.

---

## Three interpretations, ranked by likelihood

### #1 (highest, ~95%): §22 audit used the wrong `gh` subcommand — false positive

**Hypothesis.** The audit subagent at 2026-10-17T07:06Z ran
`gh pr view 26 --repo KooshaPari/KCode` to check whether #26 exists.
That command fails for issues (GitHub separates issue and PR
namespaces; an issue #N is invisible to `gh pr view N`). The subagent
interpreted the GraphQL error "Could not resolve to a PullRequest" as
"#26 does not exist" and recorded the orphan finding
(`22-NEW-FINDING-...md` lines 167–177, `STATE.md` row V).

**Evidence supporting.**

- `gh pr view 26 --repo KooshaPari/KCode` today:
  > `GraphQL: Could not resolve to a PullRequest with the number of 26. (repository.pullRequest)`
- `gh issue view 26 --repo KooshaPari/KCode` today: returns
  `title: "Add /poke context recovery sessions", state: OPEN, author: KooshaPari, createdAt: 2026-10-02T19:10:12Z, labels: []`.
- `gh api repos/KooshaPari/KCode/issues/26` today:
  > `{"number":26,"state":"open","title":"Add /poke context recovery sessions","created_at":"2026-10-02T19:10:12Z","html_url":"https://github.com/KooshaPari/KCode/issues/26"}`
- `gh pr view 25 --repo KooshaPari/KCode --json closingIssuesReferences` today:
  > `[{"id":"I_kwDOUWlMzs8AAAABUq5TKw","number":26,"repository":"KooshaPari/KCode",...}]`
  This is GitHub's own parsed "Closing issues" graph node — the link
  is real.
- Issue #26's body is textually identical in subject, problem, and
  acceptance criteria to PR #25's body summary. Both describe the
  `/poke` context recovery feature.

**Evidence contradicting.**

- None found. The audit ran in the same `gh` environment.

**Action to confirm/refute.** None — already confirmed. **Fix the
record:** edit `STATE.md` row V and the §22 doc lines 165–177 to retract
the orphan finding and add a one-line methodology correction: "use
`gh issue view` AND `gh pr view` when checking for a referenced
number; `gh pr view` alone is insufficient".

**Risk if left unresolved.** Negligible for the PR itself (the
relationship is real and will close #26 on merge). The only risk is
that future agents trust the §22 audit's "flagged for human review"
note and waste time investigating it.

---

### #2 (medium, ~4%): PR-first / issue-later workflow

**Hypothesis.** The PR was intentionally opened before its tracking
issue, and "Closes #26" was added retroactively after #26 was created
83 seconds later. This is unusual but legitimate.

**Evidence supporting.**

- PR #25 `createdAt`: `2026-10-02T19:08:49Z`
- Issue #26 `createdAt`: `2026-10-02T19:10:12Z` (83 seconds after)
- PR #25's first commit (`2e93042f6`, `feat(tui): add /poke context
  recovery launcher`, `tx-agent: codex`) was authored at
  `2026-10-02T19:07:12Z` — 97 seconds before the issue. The author
  is `codex` (not KooshaPari), which suggests a Codex agent opened
  the PR without first filing an issue.
- The body text of #26 matches PR #25's summary verbatim, including
  the acceptance criteria list, suggesting the issue body was
  authored from the PR (or vice versa).

**Evidence contradicting.**

- None substantive. PR-first workflow is rare but permitted.

**Action to confirm/refute.** None — not actionable.

**Risk if left unresolved.** None. This is descriptive, not a defect.

---

### #3 (low, ~1%): The footer was meant for a different repo

**Hypothesis.** The author originally drafted the PR against
`1jehuang/jcode` or another fork where #26 is a real PR (not an
issue), copied the body to `KooshaPari/KCode`, and the footer kept
its original number.

**Evidence supporting.**

- `gh search issues "26" --owner KooshaPari --include-prs` returns 29
  hits, none on `KooshaPari/KCode` for #26 as a PR. (Issue #26 on
  KCode is not in the search output because the search returned
  matches across all repos, but none of them is KCode's PR #26.)
- `1jehuang/jcode` upstream #26 not verified in this investigation,
  but cross-fork footers are a known footgun.

**Evidence contradicting.**

- `closingIssuesReferences` in PR #25 points to
  `KooshaPari/KCode/issues/26`, not a different repo. GitHub's parser
  resolved the "Closes #26" footer against the KCode repo at the
  time of PR creation (or edit), and the link target is the local
  issue #26, which exists. If the original intent had been a
  different repo, the link would point there.

**Action to confirm/refute.** None — already refuted by
`closingIssuesReferences`.

**Risk if left unresolved.** None.

---

## Surprising finding

**`gh pr view <N>` is not equivalent to "does <N> exist on this repo."**
GitHub's `PullRequest` GraphQL type and `Issue` type share a per-repo
number sequence but are different objects. `gh pr view 26` correctly
fails with "Could not resolve to a PullRequest with the number of 26"
when #26 is an issue, and a naive agent will treat that as
"doesn't exist." The audit subagent did exactly that. The fix is
mechanical: a future audit must use
`gh issue view <N> --repo X` (or REST `/issues/<N>`) in addition to
`gh pr view <N>` whenever the reference type is ambiguous.

---

## Recommended next step

Add a one-line method-note to `22-NEW-FINDING-gh-pr-view-closed-vs-merged.md`
(after line 178) and update `STATE.md` row V to retract the
"flagged for human review" status. Both edits are in this session's
own files and do not touch the PR. No operator action needed.

---

## Sources cited

| Source | Lines / field |
|---|---|
| `gh pr view 25 --repo KooshaPari/KCode --json closingIssuesReferences` | `closingIssuesReferences[0].number=26, repository=KooshaPari/KCode` |
| `gh pr view 25 --repo KooshaPari/KCode --json createdAt` | `2026-10-02T19:08:49Z` |
| `gh issue view 26 --repo KooshaPari/KCode` | title, state, createdAt |
| `gh api repos/KooshaPari/KCode/issues/26` | `{number:26, state:open, created_at:2026-10-02T19:10:12Z}` |
| `gh pr view 26 --repo KooshaPari/KCode` | `Could not resolve to a PullRequest with the number of 26` |
| `gh pr view 25 --repo KooshaPari/KCode --json commits[0].oid` | `2e93042f6b094ca5c4d9c1f8027ba42f42c9598d`, authored `2026-10-02T19:07:12Z`, agent `codex` |
| `docs/sessions/2026-10-15-portal-xirp-resume/22-NEW-FINDING-gh-pr-view-closed-vs-merged.md` | lines 165–177 (the false-positive claim) |
| `docs/sessions/2026-10-15-portal-xirp-resume/STATE.md` | row V (the audit result), line 1008 ("PR #25 / #26 — orphan issue link flagged in §22, NOT edited") |
| `gh search issues "26" --owner KooshaPari --include-prs` | 29 hits across KooshaPari repos, none for `KooshaPari/KCode` PR #26 |