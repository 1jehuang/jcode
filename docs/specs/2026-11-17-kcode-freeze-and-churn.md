# KCode / jcode freeze and churn evaluation

Status: proposed, effective immediately for this workstation
Date: 2026-11-17
Owner: the lane that wrote this (kitty/paste/build-fix lane)

## 1. Why a freeze

Measured on this host, not estimated:

| Thing | Count | Notes |
|---|---|---|
| Branches on `KooshaPari/KCode` | **211** | `git ls-remote --heads fork` |
| Open PRs against `1jehuang/jcode` | **100** | `gh pr list --state open` |
| Local branches in `E:\koosh\CodeProjects\jcode` | 14 | of which 4 are `layerb/*`, 2 `consolidation/*` |
| Local worktrees | 6 | 3 are my scaffolding |
| Stashes | 0 | good |
| `origin/master` state | **does not compile** | `agents_status::todos_updated` dangling call, issue #2196 |
| Local `master` | 246 commits behind | stale |
| Installed binary | `5f70a539d` | keyboard scrub only |

Two effects are already visible in live chats: 429 rate limits from the worker
provider, and leaked `OSC 11` reply text (`]11;r2c3…`) inside composers because the
running binary predates the reply-fragment scrub.

A lane cannot verify anything when the base does not build and every PR is stacked
on a different broken point. So: freeze new branch/worktree creation, drain what
exists, then evaluate.

## 2. Freeze contract

Frozen:
- creating new branches, worktrees, forks, or "just in case" clones;
- opening PRs that are not one of the in-flight set below;
- rebasing or force-pushing anything except your own PR head.

Allowed while frozen:
- fixing a red check on a PR you own;
- docs (this document included);
- read-only audits and inventories.

Lift criteria (all must hold):
1. `origin/master` builds and its three build jobs are green.
2. Every PR in the drain order is merged or closed with a reason.
3. Local and fork branch counts are at or below the target in section 6.
4. No live chat runs a binary older than the last merged fix.

## 3. Drain order and per-PR contract

| Order | PR | Intent | Base | Depends on |
|---|---|---|---|---|
| 1 | #2195 | `master` compiles again | `master` | — |
| 2 | #2186 | Windows keyboard protocol + composer row | `master` | #2195 |
| 3 | #2193 | one prompt per paste | `master` | #2195, #2186 (test gating) |
| 4 | ctrl-click links | plain click must not open URLs | `master` | #2195 |
| 5 | todo header visibility | hide / one-line mode | `master` | #2195 |

Contract for every PR in this set:
- **One intent.** A PR that touches an unrelated file is wrong even if the change is good. (This rule exists because #2195 shipped a stray staged `paste_guard.rs` and failed the warning budget.)
- **Base dependency declared** in the body when the branch is stacked, with the commit that carries it.
- **Evidence is a number or a named test**, reproducible by a reader: no "verified manually".
- **No `--update` on budgets** to make a gate pass. If a budget is wrong, that is a separate decision with a reason.

## 4. Behaviour contract (BDD-style acceptance)

The Windows defects have an executable vocabulary. A claim is only valid in these terms:

```gherkin
Scenario: the keyboard-protocol leak
  Given a real ConPTY and an isolated JCODE_HOME
  When a stale kitty-capable terminal sends one CSI-u report
  Then the composer text must not contain the report
  And ordinary typing must still reach the composer
  And the negative control binary must still reproduce the leak

Scenario: a pasted block
  Given a resumed session and one write of a seven-line block
  When the operator presses Enter once
  Then the client's prompt history holds exactly one entry
  And that entry contains all seven lines
  And the control binary must still submit one prompt per line

Scenario: the resume menu
  Given the picker initialised a terminal
  When it hands over to the child client
  Then no keyboard-protocol push may reach the stream
  And the stack-neutral clear must be present
  # typing after the handover is NOT verified and must not be claimed
```

Harness verdicts are the only accepted evidence: `GREEN`, `BUG_REPRODUCED`,
`MENU_SELECTED`, `MENU_TERMINAL_CLEAN`, `FIXED`, `BUG`, `OTHER`.
A single green run is not evidence; N identical runs bracketed by controls are.

## 5. Binary contract

The installed binary is `5f70a539d` (keyboard scrub). It does **not** contain the
paste fix, the reconnect repaint, or the OSC reply-fragment scrub.

Rules:
- A chat that evaluates a fix must build the branch it is evaluating, never the
  installed binary.
- Build recipe on this host (the session env points `CARGO_TARGET_DIR` at a dead
  drive, so always override):
  ```
  set CARGO_TARGET_DIR=<your worktree>\target
  cargo --config build.rustc-wrapper="" build -p jcode --bin jcode
  ```
- Before claiming a fix works in a running chat, state the commit the chat runs.

## 6. Churn disposition

Local, this lane's own scaffolding — delete now:

| Item | Disposition |
|---|---|
| worktree `baseline2` (detached at `c6fed3c3b`) | remove; baseline already recorded in the eval log |
| branch `backup-stray-stage` (`054e4431e`) | delete; superseded by `feat/paste-port2` |
| branch `cleanbuild` (`00f0bbd04`) | delete the duplicate name; the commit stays reachable via `fix/master-todo-tracker-build` |
| branch `feat/paste-ctrlenter` (local `d9b0d9aa8`) | delete the stale local ref; the PR head lives on the fork as `feat/paste-ctrlenter` |

Needs an owner decision (not mine to delete):

| Item | Question |
|---|---|
| `layerb/appcore`, `layerb/base`, `layerb/ci`, `layerb/tui` | 195–226 commits ahead of `master` each; superseded by newer work? |
| `consolidation/phase1-clean` | 351 commits ahead; is it the intended consolidation vehicle? |
| `consolidation/upstream-20261114` | 0 ahead, on 7 remotes: a pointer, not work |
| `fix/bounded-git-status` | 1 commit, pushed, no PR: open a PR or drop |
| local `master` 246 behind | fast-forward only, no local commits |
| 211 fork branches, 100 open PRs | needs a triage pass: close-by-default with a reason |

Target after freeze: ≤ 20 fork branches, ≤ 20 open PRs, ≤ 2 worktrees, 0 stashes.

## 7. Executable audit (read-only)

```bash
# local churn
git branch -vv; git stash list; git worktree list
# per-branch: unpushed work?
for b in $(git for-each-ref --format='%(refname:short)' refs/heads); do
  echo "$b $(git rev-list --count "$b" ^origin/master)"
done
# cloud churn
git ls-remote --heads fork | wc -l
gh pr list --repo 1jehuang/jcode --state open --limit 200 --json number,headRefName
# same-branch duplicates (local vs fork vs PR head)
```

## 8. Definition of done

The freeze is done when `origin/master` builds, the drain-order PRs are merged or
closed with reasons, the branch/PR counts meet the targets in section 6, and every
live chat states a binary commit that contains the fixes it relies on.
