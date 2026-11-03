# Branch Inventory & Consolidation Plan

**Repo:** `KooshaPari/KCode` (GitHub redirects to `KooshaPari/Jcode`; `gh repo view` in this
clone resolves to the *upstream* `1jehuang/jcode`, so all KCode PR queries must use
`--repo KooshaPari/KCode` explicitly)
**Date:** 2026-10-11
**Operator goal:** collapse every KCode branch to **1 branch** (master), PR'ing the audit
findings and any remaining unique work first.
**Mode:** READ-ONLY inventory. Nothing was pushed, deleted, merged, or reset.

## Reference points

| Ref | SHA | Meaning |
|---|---|---|
| `origin/master` | `f19b47b19` | Revert "refactor(decollision): rename jcode to kcode (#33)" (#36) |
| `origin/master~1` | `901b33108` | refactor(decollision): rename jcode to kcode (#33) |
| `origin/master~2` | `046ea2af5` | last pre-decollision base |
| local `master` | `0737fb93f` | **diverged**: #33 merge + a test-body fix, does *not* contain the #36 revert |
| local `HEAD` | `4fdfa0db8` | `fix/issue-44-17-broken-tests` |

> **Interpretation of ahead/behind:** counts are `git rev-list --left-right --count
> origin/master...<branch>` → `BEHIND  AHEAD` relative to `origin/master` (post-revert).
> Branches that still carry the #33 decollision show huge "ahead" counts (912–2181) because
> `origin/master` was *rewound* by the #36 revert, not because they are 2000 commits of new work.

---

## A. Local worktree clones (`git worktree list` + standalone clones)

All under `~/CodeProjects/Phenotype/repos/` unless noted. `origin` = KCode unless stated.

| Path | Checked-out branch | dirty | Notes |
|------|--------------------|-------|-------|
| `jcode` (primary) | `fix/issue-44-17-broken-tests` | 0 | holds all 31 local branches |
| `jcode-acp-native` | `codex/acp-native-remote` | 0 | PR #10 |
| `jcode-decollide-kcode-r2` | `feature/decollide-kcode-r2` | 4 | PR #34 |
| `jcode-dev-isolation` | `codex/jcode-dev-isolation` | 0 | PR #27 **merged** |
| `jcode-dev-isolation-review-followup` | `codex/jcode-dev-isolation-review-followup` | 0 | local-only commits |
| `jcode-dev-isolation-socket-review` | `codex/jcode-dev-isolation-socket-review` | 0 | local-only commits |
| `jcode-dev-isolation-storage-followup` | `codex/jcode-dev-isolation-storage-followup` | 0 | local-only commits |
| `jcode-fork-parent` | *detached* `115170054` | 0 | scratch |
| `jcode-poke-context` | `codex/poke-context` | 2 | PR #25 |
| `jcode-pr10-build-20261009` | *detached* `09680ac23` | 0 | scratch (= acp-native tip) |
| `jcode-pr27-grok-dev-auth` | `codex/pr27-grok-dev-auth-20261010` | 0 | local-only commits |
| `jcode-pr27-minor-review-fixes` | `codex/pr27-minor-review-fixes-20261010` | 3 | local-only commits |
| `jcode-pr27-pid-test-windows` | `codex/pr27-pid-test-windows` | 0 | local-only commits |
| `jcode-pr27-review-scope` | `codex/pr27-review-scope` | 0 | local-only commits |
| `jcode-pr27-reviewed-fixes` | `codex/pr27-reviewed-fixes-20261009` | 0 | local-only commits |
| `jcode-pr27-reviewed-fixes-b` | `codex/pr27-reviewed-fixes-20261009b` | 0 | local-only commits |
| `jcode-pr8-on-upstream-20261005` | `codex/jcode-pr8-on-upstream-20261005` | 1 | local-only commits |
| `jcode-pr8-review` | `codex/pr8-review-fixes` | 0 | mostly absorbed (remote_refs=6) |
| `jcode-ratatui-guard` | `feature/ratatui-buffer-guard` | 0 | PR #28 merged |
| `jcode-rebase-v0.90` | `revert/pr-33-merge` | 0 | PR #36 merged |
| `jcode-src` | `master` | 0 | **origin = 1jehuang/jcode (upstream)** |
| `jcode-patch` | `fix/opencode-go-responses-api` | **10** | origin = upstream; uncommitted work |
| `jcode-stack-rebase-20261008` | `codex/jcode-stack-e613-20261008` | 0 | separate KCode clone (2 branches) |
| `jcode-upstream-issue-draft` | `docs/upstream-ratatui-buffer-bounds-2026-10` | 0 | local-only commits |
| `jcode-upstream-sync` | `feature/upstream-0.88-sync` | 0 | PR #23 |
| `jcode-upstream-baseline` | *detached* `ee4cd3db3` | 0 | scratch |
| `jcode-win-anchor` | `instinct/win-herdr-inline-anchor` | 0 | worktree of `jcode-src` (upstream) |
| `~/wt-jcode-audit` | `master` | (this doc) | **plan-file target** |
| `~/wt-jcode-fix1` | `fix/run-38118557038-windows` | 0 | local-only commits |
| `~/wt-kcode-pr27-integration` | `codex/pr27-integrated-fixes-20261011` | 0 | local-only commits |
| `~/wt-kcode-small-terminal` | `fix/kcode-small-terminal-resume` | 0 | PR #35 |
| `~/wt-kcode-upstream-clean` | `feature/upstream-0.88-sync-clean` | 0 | local-only commits |
| `~/wt-kcode-active-pids-race` | *detached* `e45ddd47e` | ? | scratch |
| `~/wt-kcode-heading-boundary` | *detached* `d6f2d2091` | ? | scratch |
| `~/wt-kcode-home-symlink` | *detached* `ceb860bf2` | ? | scratch |
| `jcode-fork-parent-target` | — | — | **not a git repo** (empty dir) |
| `jcode-upstream-baseline-target` | — | — | **not a git repo** (empty dir) |

**Counts:** 32 git working trees (29 in the primary repo's worktree set incl. `~/wt-*`,
plus 3 standalone clones `jcode-src`, `jcode-patch`, `jcode-stack-rebase-20261008`).
2 empty non-repo dirs.

---

## B. Local branches

### In `~/CodeProjects/Phenotype/repos/jcode` (primary clone) — 31 branches

| Branch | Tip | Last subject | Behind/Ahead of origin/master | Remote refs | PR | Notes |
|--------|-----|--------------|-------------------------------|-------------|----|-------|
| `master` | `0737fb93f` | fix(tui-style): restore print_calibration_scores test body lost in rebase | 1 / 1 | 0 | — | **diverged from origin/master**; lacks the #36 revert |
| `revert/pr-33-merge` | `d9cb64ce4` | Revert "refactor(decollision): rename jcode to kcode (#33)" | 1 / 1 | 1 | #36 merged | content landed as `f19b47b19`; branch SHA differs |
| `fix/issue-44-17-broken-tests` | `4fdfa0db8` | fix(tests): resolve 17 broken kcode-app-core --lib tests (#44) | 0 / 98 | 1 | **#44 open** | current HEAD |
| `fix/issue-44-17-broken-tests-WRONG-BASE` | `0737fb93f` | fix(tui-style): restore print_calibration_scores test body lost in rebase | 1 / 1 | 0 | — | **superseded** (same tip as local master) |
| `feature/herdr-plugin-manifest` | `c6bc0d07a` | docs(herdr-session): refresh branch state table post cherry-pick | 0 / 95 | 0 | **#8 open** | local tip ahead of pushed `6bb97aa07` |
| `feature/herdr-plugin-manifest-rebased` | `02625d662` | docs(herdr-session): refresh branch state table post cherry-pick | 0 / 97 | 2 | — | rebase twin of the above |
| `fix/issue-38-cargo-fmt` | `abb1730b8` | fix(cargo-fmt): remove 6 stale untracked files accidentally added | 0 / 95 | 2 | — | likely absorbed into #44 line |
| `codex/poke-context` | `2e93042f6` | feat(tui): add /poke context recovery launcher | 2 / 1 | 1 | **#25 open** | |
| `codex/acp-native-remote` | `09680ac23` | merge: carry private test fixtures and bounded TUI qualification | 3 / 58 | 1 | **#10 open (CONFLICTING)** | draft |
| `codex/jcode-dev-isolation` | `e311305ff` | Merge remote-tracking branch 'origin/codex/jcode-dev-isolation' … | 3 / 68 | 2 | #27 **merged** | absorbable |
| `codex/jcode-dev-isolation-review-followup` | `701187308` | fix(jcode-dev): guard installer version symlinks | 3 / 28 | 2 | — | follow-up to #27 |
| `codex/jcode-dev-isolation-socket-review` | `111ebfae6` | fix(jcode-dev): avoid duplicate socket override validation | 3 / 34 | 2 | — | follow-up to #27 |
| `codex/jcode-dev-isolation-storage-followup` | `a15f1a2c5` | fix(jcode-dev): harden isolated persistence | 3 / 29 | 2 | — | follow-up to #27 |
| `codex/pr27-grok-dev-auth-20261010` | `34b6a836c` | fix(auth): isolate dev Grok credentials | 3 / 66 | 0 | — | **local-only commits** |
| `codex/pr27-integrated-fixes-20261011` | `aaab746f0` | fix(pr27): integrate reviewed fixes | 3 / 66 | 2 | — | integration of pr27 stack |
| `codex/pr27-minor-review-fixes-20261010` | `8ab024ddc` | fix(session-memory): preserve template section separators | 3 / 62 | 2 | — | |
| `codex/pr27-pid-test-windows` | `c7fe8592c` | test(storage): keep dead pid test portable | 3 / 66 | 0 | — | **local-only commits** |
| `codex/pr27-review-scope` | `c02ed19cb` | chore(git): record integration ledger correction | 3 / 33 | 2 | — | |
| `codex/pr27-reviewed-fixes-20261009` | `fe3f2a3a0` | fix(isolation): align KCode review guards and test paths | 3 / 59 | 2 | — | |
| `codex/pr27-reviewed-fixes-20261009b` | `8ab024ddc` | fix(session-memory): preserve template section separators | 3 / 62 | 2 | — | duplicate tip of minor-review-fixes |
| `codex/pr8-review-fixes` | `681e99edc` | test(review): use private capture fixtures and focused TUI features | 3 / 31 | 6 | — | largely absorbed |
| `codex/jcode-pr8-on-upstream-20261005` | `3f8dc3960` | test(openrouter): cover the zero-chunk empty stream shape | 3 / 2128 | 0 | — | **local-only**; carries whole upstream history |
| `codex/jcode-stack-e613-20261008` | (in `jcode-stack-rebase-20261008`) | — | — | — | — | separate clone |
| `docs/upstream-ratatui-buffer-bounds-2026-10` | `af7797492` | docs(upstream): draft ratatui-core buffer-bounds issue | 3 / 36 | 0 | — | **local-only**; filed as ratatui#2813 |
| `feature/decollide-kcode-r2` | `d1e286cc2` | feat(decollide): Phase 2 — external-infra source refs (2 of 2 PRs) | 3 / 52 | 1 | **#34 open** | dir has 4 dirty files |
| `feature/ratatui-buffer-guard` | `c7419a3c7` | fix(tui): restore inherited modes from INHERITED_MODES_ENV … | 3 / 38 | 1 | #28 **merged** | absorbable |
| `feature/rebase-onto-v0.90.0` | `5c6a882ff` | fix(post-rebase): complete jcode->kcode rename + dedup + cargo check | 2 / 912 | 1 | #33 merged→**reverted** | the decollision line |
| `feature/upstream-0.88-sync` | `545d6d7f4` | docs(session): round 19 — cascade completion, PR #23 CLEAN | 3 / 2180 | 1 | **#23 open** | |
| `feature/upstream-0.88-sync-clean` | `27abe6cc7` | docs(closure): append post-push verification to issue-closure dossier | 3 / 2181 | 1 | — | twin of #23 |
| `fix/kcode-small-terminal-resume` | `81f2abbb8` | test(tui): exercise compact resume frames down to one cell | 3 / 84 | 1 | **#35 open** | |
| `fix/reap-detached-bash-children` | `05c7b0182` | fix(herdr): stop holding the reporter mutex across await points | 7 / **0** | 24 | — | **fully absorbed** (ahead=0) |
| `fix/run-38118557038-windows` | `ab13d419b` | fix(windows): resolve crate::shell path + anyhow->io::Error mapping | 3 / 117 | 0 | — | **local-only commits** |

### In `jcode-stack-rebase-20261008` (separate clone)

| Branch | Tip | Notes |
|--------|-----|-------|
| `codex/jcode-stack-e613-20261008` | (2 local branches) | stack-rebase scratch; superseded |

### In `jcode-src` / `jcode-patch` / `jcode-win-anchor` — **upstream `1jehuang/jcode`**, out of scope

`jcode-patch` has **10 uncommitted files** on `fix/opencode-go-responses-api` (upstream
branch, already merged there as PR #4). Not part of KCode consolidation.

---

## C. Remote branches on `origin` = `KooshaPari/KCode` (33 heads)

| Branch | Have local copy? | Open PR | Notes |
|--------|------------------|---------|-------|
| `master` | yes (diverged) | #1732? no (that is upstream) | |
| `chore/enable-dependabot` | **no** | — | merged #9; stale |
| `codex/acp-native-remote` | yes | #10 | |
| `codex/jcode-dev-isolation` | yes | #27 merged | |
| `codex/poke-context` | yes | #25 | |
| `feature/decollide-kcode-r2` | yes | #34 | |
| `feature/herdr-plugin-manifest` | yes (diverged) | #8 | |
| `feature/herdr-plugin-manifest-rebased` | yes | — | |
| `feature/ratatui-buffer-guard` | yes | #28 merged | |
| `feature/rebase-onto-v0.90.0` | yes | #33 reverted | |
| `feature/upstream-0.88-sync` | yes | #23 | |
| `feature/upstream-0.88-sync-clean` | yes | — | |
| `fix/api-attach-preserve-cwd` | **no** | — | also on upstream |
| `fix/desktop-observer-turn-completion` | **no** | — | also on upstream |
| `fix/fix-responses-api` | **no** | — | |
| `fix/herdr-reporter-source-namespace` | **no** | — | |
| `fix/issue-44-17-broken-tests` | yes | **#44** | |
| `fix/issue-triage-20260907` | **no** | — | also on upstream |
| `fix/issue-triage-20260910-pr` | **no** | — | also on upstream |
| `fix/jcode-small-terminal-resume` | **no** | #44 twin | same fix, `jcode` naming |
| `fix/kcode-small-terminal-resume` | yes | #35 | |
| `fix/native-reasoning-markdown` | **no** | — | also on upstream |
| `fix/reap-detached-bash-children` | yes | — | absorbed |
| `fix/tui-focus-paste-panic-master` | **no** | — | |
| `impl/daemon-identity-handshake-20260929` | **no** | #14 draft | |
| `impl/effect-recovery-hook-20260930` | **no** | #20 draft | |
| `impl/forgecode-provider-contract-20260930` | **no** | #16 draft | |
| `impl/macos-trust-no-startup-resign-20260930` | **no** | #18 draft | |
| `impl/macos-trust-policy-20260930` | **no** | #22 draft | |
| `oauth-api-equivalent-usage-20260907` | **no** | — | also on upstream |
| `revert/pr-33-merge` | yes | #36 merged | |
| `spec/harness-mature-recovery-20260929` | **no** | #12 draft | |
| `fix/run-38118557038-windows` | yes | — | (worktree `~/wt-jcode-fix1`) |

**Remote-only branches: 16** (no local copy).

## D. Open PRs on `KooshaPari/KCode` (13)

| # | Title | Branch → Base | Draft | Mergeable |
|---|-------|---------------|-------|-----------|
| 44 | fix(tui): support tiny terminals in Jcode session resume | `fix/jcode-small-terminal-resume` → master | no | MERGEABLE |
| 35 | fix(tui): support tiny terminal session resume | `fix/kcode-small-terminal-resume` → master | no | MERGEABLE |
| 34 | feat(decollide): Phase 2 — external-infra source refs | `feature/decollide-kcode-r2` → master | no | MERGEABLE |
| 25 | feat(tui): add /poke context recovery launcher | `codex/poke-context` → master | no | MERGEABLE |
| 23 | Merge upstream v0.88.0 into KCode | `feature/upstream-0.88-sync` → master | no | MERGEABLE |
| 22 | fix(macos): make executable trust repair explicit opt-in | `impl/macos-trust-policy-20260930` | **draft** | MERGEABLE |
| 20 | feat(tool): add optional durable effect recovery hook | `impl/effect-recovery-hook-20260930` | **draft** | MERGEABLE |
| 18 | fix(macos): preserve installed executable trust identity | `impl/macos-trust-no-startup-resign-20260930` | **draft** | MERGEABLE |
| 16 | fix(provider): fail closed when ForgeCode CLI lacks contract | `impl/forgecode-provider-contract-20260930` | **draft** | MERGEABLE |
| 14 | feat(server): expose exact daemon identity in ping handshake | `impl/daemon-identity-handshake-20260929` | **draft** | MERGEABLE |
| 12 | spec: mature-first KCode recovery | `spec/harness-mature-recovery-20260929` | **draft** | MERGEABLE |
| 10 | feat(acp): discover sessions and broker remote interactions | `codex/acp-native-remote` | **draft** | **CONFLICTING** |
| 8 | fix(openrouter): stop double-polling dead unfold after EOF | `feature/herdr-plugin-manifest` | no | MERGEABLE |

### E. Merged / closed KCode PRs

| # | Title | Branch | Merged at |
|---|-------|--------|-----------|
| 36 | revert: refactor(decollision) rename jcode to kcode | `revert/pr-33-merge` | 2026-10-11 |
| 33 | refactor(decollision): rename jcode to kcode | `feature/rebase-onto-v0.90.0` | 2026-10-09 (later reverted) |
| 31 | feat(decollide): rename jcode -> kcode (1 of 2) | `feature/decollide-kcode` | 2026-10-08 |
| 28 | fix(tui): guard ratatui buffer overflow in degenerate terminals | `feature/ratatui-buffer-guard` | 2026-10-06 |
| 27 | feat(jcode): isolate dev runtime state | `codex/jcode-dev-isolation` | 2026-10-11 |
| 9 | chore(deps): enable Dependabot | `chore/enable-dependabot` | 2026-09-20 |
| 6 | feat: replace emojis with Unicode symbols | — | 2026-09-13 |
| 4 | fix: Responses API support for OpenCode Go | `fix/opencode-go-responses-api` | 2026-09-13 |
| 2, 1 | (superseded duplicates of #4) | `fix/opencode-go-responses-api` | closed, unmerged |

## F. Tags

123+ tags, all inherited from upstream (`v0.1.0` … `v0.14.x`, plus `readme-assets`,
`tb21-confstep-943eae93`, `tui-refactor-base-20260306`). **None are KCode-specific**; no
KCode fork tags exist. Left untouched.

## G. Stashes

None (`git stash list` empty in the primary clone).

---

## Consolidation plan

### Target branch
**`master`** — the single surviving branch. All unique work must land on `master` before
any other branch is deleted. **Blocking decision** (operator): master currently has two
incompatible identities —

- `origin/master` = `f19b47b19` (names are `jcode*`; #33 decollision **reverted**)
- local `master` = `0737fb93f` (contains #33 decollision + a test-body fix; **no revert**)

The fork is named **KCode**, so the intended end-state is presumably the *decollision* line,
but #33 was reverted because `cargo check --workspace` failed on the merged tree. **Do not
pick silently.** Options are listed under "Manual review items".

### Branches to absorb (priority order)
1. `fix/issue-44-17-broken-tests` (PR #44) — newest, master-based, fixes 17 broken tests.
2. `fix/kcode-small-terminal-resume` (PR #35) — tiny-terminal resume fix.
3. `fix/jcode-small-terminal-resume` (PR #44) — **twin of #35**; keep one, close the other.
4. `codex/poke-context` (PR #25) — /poke context recovery.
5. `feature/herdr-plugin-manifest` (PR #8) — openrouter EOF panic + herdr manifest.
6. `codex/acp-native-remote` (PR #10, **CONFLICTING**) — ACP session discovery (draft).
7. `feature/decollide-kcode-r2` (PR #34) — decollision phase 2 (2 of 2).
8. `feature/upstream-0.88-sync` (PR #23) + `feature/upstream-0.88-sync-clean` — upstream v0.88.
9. Draft `impl/*` and `spec/*` PRs #12/#14/#16/#18/#20/#22 — decide merge-or-close.
10. local-only follow-up commits (no PR yet):
    - `codex/jcode-dev-isolation-{review-followup,socket-review,storage-followup}` (post-#27)
    - `codex/pr27-grok-dev-auth-20261010`, `codex/pr27-pid-test-windows`
    - `codex/pr27-integrated-fixes-20261011` (+ `~/wt-kcode-pr27-integration`)
    - `fix/run-38118557038-windows`, `codex/pr27-minor-review-fixes-20261010`,
      `codex/pr27-review-scope`, `codex/pr27-reviewed-fixes-20261009{,b}`, `codex/pr8-review-fixes`
    - `docs/upstream-ratatui-buffer-bounds-2026-10` (issue draft; ratatui#2813 already filed)

### Branches to skip / delete without merging
| Branch | Why |
|--------|-----|
| `fix/reap-detached-bash-children` | ahead=0; already reachable from origin/master |
| `fix/issue-44-17-broken-tests-WRONG-BASE` | superseded; tip == local master tip |
| `codex/pr27-reviewed-fixes-20261009b` | duplicate tip of `...-minor-review-fixes-20261010` (`8ab024ddc`) |
| `feature/herdr-plugin-manifest-rebased` | rebase twin of `feature/herdr-plugin-manifest`; keep one |
| `feature/upstream-0.88-sync-clean` | twin of `feature/upstream-0.88-sync`; keep one |
| `codex/jcode-pr8-on-upstream-20261005` | local-only scratch carrying 2128 upstream commits |
| `codex/pr8-review-fixes` | largely absorbed (remote_refs=6) |
| `revert/pr-33-merge` | content already on origin/master |
| `chore/enable-dependabot`, `oauth-api-equivalent-usage-20260907`, `fix/issue-triage-*`, `fix/native-reasoning-markdown`, `fix/api-attach-preserve-cwd`, `fix/desktop-observer-turn-completion` | fully merged upstream; stale remote branches |
| `codex/jcode-stack-e613-20261008` | stack-rebase scratch |
| `jcode-patch` uncommitted work | upstream branch, out of KCode scope |

### Branches behind master (need rebase before merge)
`master`(1), `revert/pr-33-merge`(1), `...-WRONG-BASE`(1), `codex/poke-context`(2),
`feature/rebase-onto-v0.90.0`(2), and behind=3: `codex/acp-native-remote`,
`codex/jcode-dev-isolation{,-review-followup,-socket-review,-storage-followup}`,
`codex/pr27-*`, `codex/pr8-review-fixes`, `codex/jcode-pr8-on-upstream-20261005`,
`docs/upstream-ratatui-buffer-bounds-2026-10`, `feature/decollide-kcode-r2`,
`feature/ratatui-buffer-guard`, `feature/upstream-0.88-sync{,-clean}`,
`fix/kcode-small-terminal-resume`, `fix/run-38118557038-windows`. `fix/reap-detached-bash-children`(7).

### Conflicts to expect
- `codex/acp-native-remote` — PR #10 is **CONFLICTING** against master today.
- `feature/rebase-onto-v0.90.0` vs `origin/master` — the #33 merge is the *inverse* of the
  #36 revert; re-landing decollision will conflict across ~88 crates and every `jcode*`
  path. This is the single largest conflict surface.
- `codex/pr27-*` stack — 8 branches share a common base with overlapping edits to
  `crates/kcode-storage` and `session-memory`; two branches expose identical tips.

### Step-by-step consolidation order
1. **Freeze**: announce freeze; no new branches.
2. **Decide master identity** (see Manual review items) — required before anything else.
3. Rebase the merge-ready PRs (#44, #35, #25, #8) onto master; merge each after green CI.
4. Close duplicate PRs (#44 vs #35) once one lands.
5. Rebase + merge `feature/upstream-0.88-sync` (PR #23); delete `-clean` twin.
6. Triage the 6 draft `impl/*`/`spec/*` PRs: merge the useful ones or close with a note.
7. Squash the local-only `codex/pr27-*` + follow-up branches into a single PR (or close as
   superseded once #27 follow-ups are confirmed present).
8. Re-attempt decollision (`feature/decollide-kcode-r2` → PR #34) only after master is
   healthy and `cargo check --workspace` passes on the merge.
9. **Delete remote branches** (one at a time, after each PR merge).
10. **Remove local branches and worktrees**: `git worktree remove <path>` for the 29
    worktrees, then delete the 31 local branches.
11. Archive/remove scratch dirs: `jcode-fork-parent-target`, `jcode-upstream-baseline-target`,
    `~/wt-kcode-*` detached scratch worktrees, `jcode-stack-rebase-20261008`.

### Validation gates
- After each rebase: `cargo check --workspace` (mandatory — this is exactly what #33 failed).
- After each merge to master: `cargo fmt --all -- --check` and `cargo clippy`.
- Before deleting any branch: `git rev-list --count origin/master..<branch>` must be **0**.
- Before removing a worktree: `git -C <path> status --porcelain` must be empty
  (note `jcode-decollide-kcode-r2` = 4 dirty, `jcode-poke-context` = 2, `jcode-patch` = 10).
- Final: `git branch -a` shows only `master`; `git worktree list` shows one entry.

### Risks
1. **Master identity ambiguity** — merging into the wrong master silently discards either the
   decollision or the revert. Highest risk; needs an explicit operator decision.
2. **Dirty worktrees** — `jcode-decollide-kcode-r2` (4), `jcode-poke-context` (2),
   `jcode-pr27-minor-review-fixes` (3), `jcode-pr8-on-upstream-20261005` (1),
   `jcode-patch` (10). Deleting without inspection loses work.
3. **Local-only commits** exist on 8 branches (`remote_refs=0`) — deleting the remote branch
   would not recover them; they must be pushed or explicitly discarded.
4. **Diverged pushed branches** — `feature/herdr-plugin-manifest` local tip `c6bc0d07a`
   differs from pushed `6bb97aa07`; PR #8 targets the pushed tip.
5. **Disk pressure** — build host was at 96–98% during the revert work; release builds
   failed. Free ≥50 GiB before any `cargo build --release` validation.
6. **Upstream confusion** — many remote branches here mirror `1jehuang/jcode`; do not delete
   those from the upstream repo.

### Manual review items (operator decision required)
1. **Which master?** Re-land decollision (KCode identity, `kcode*` names) or ship the
   reverted (upstream `jcode*`) master? Everything downstream depends on this.
2. `jcode-patch`: 10 uncommitted files on an upstream branch — preserve or discard?
3. `fix/reap-detached-bash-children`: 7 commits behind, 0 ahead — confirm safe to delete.
4. The 6 draft PRs (#12/#14/#16/#18/#20/#22): merge, or close as abandoned?
5. `feature/upstream-0.88-sync` brings 2180 upstream commits — confirm intent to land now.
6. Audit-findings PR: the operator asked to "PR the audit findings" — this document is the
   findings; confirm it should be committed to a branch and opened as a PR against master.

---

## Inventory summary

| Metric | Value |
|---|---|
| Git working trees | 32 (29 primary-repo worktrees incl. `~/wt-*`, + 3 standalone clones) |
| Non-repo dirs | 2 (`jcode-fork-parent-target`, `jcode-upstream-baseline-target`) |
| Local KCode branches | 31 |
| Remote branches on `KooshaPari/KCode` | 33 (16 remote-only, no local copy) |
| Local-only branches (commits not on any remote) | 8 |
| Open PRs on KCode | 13 (6 draft) |
| Merged/closed PRs | 10 |
| Fully absorbed branches (ahead=0) | 1 (`fix/reap-detached-bash-children`) |
| Branches behind master | 19 |
| KCode-specific tags | 0 |
| Stashes | 0 |
| **Target final state** | 1 branch (`master`) |
