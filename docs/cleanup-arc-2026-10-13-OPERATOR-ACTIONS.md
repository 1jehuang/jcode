# Operator Actions: 2026-10-13 P11 / P25c / P26

**Date**: 2026-10-13
**Status**: Awaiting operator decision on 3 cleanup items
**Companion to**: `docs/cleanup-arc-2026-10-13-P30-SECURITY-FIX.md`

---

## P11 — Disk pressure URGENT (12 GB free, 99% used)

### Current state

- Filesystem `/dev/disk3s1s1` (926 GB total, 855 GB used, **12 GB free = 1.3%**)
- Single biggest consumer in the repo ecosystem: `PhenoShared/target` (53 GB) — looks like a long-archived product
- Jcode-specific build artifacts: 56 GB across 9 target/ dirs

### Exact cleanup targets (ranked by size, low risk first)

| Rank | Path | Size | Risk | Notes |
|------|------|------|------|-------|
| 1 | `/Users/kooshapari/CodeProjects/Phenotype/repos/PhenoShared/target` | 53 GB | LOW | Archived product, last commit 2624b435 (workspace members); not in active cleanup arc |
| 2 | `/Users/kooshapari/CodeProjects/Phenotype/repos/jcode/target` | 29 GB | LOW | Main worktree; rebuild ~11 min from clean |
| 3 | `/Users/kooshapari/CodeProjects/Phenotype/repos/AgilePlus/target` | 27 GB | LOW | Not in jcode cleanup arc |
| 4 | `/Users/kooshapari/CodeProjects/Phenotype/repos/phenotype-fabric/target` | 13 GB | LOW | Not in jcode cleanup arc |
| 5 | `/Users/kooshapari/CodeProjects/Phenotype/repos/jcode-rebase-v0.90/target` | 6.2 GB | MEDIUM | PR #33 revert worktree; if revert lands, can clean |
| 6 | `/Users/kooshapari/wt-jcode-fix37/target` | 5.4 GB | MEDIUM | Operator's old fix worktree; status TBD |
| 7 | `/Users/kooshapari/wt-jcode-fmt-cleanup/target` | 9.2 GB | LOW | Old cleanup worktree; superseded by current cleanup-arc branch |
| 8 | `/Users/kooshapari/wt-jcode-fmt-rebased/target` | 2.1 GB | LOW | Rebased fmt branch; can be regenerated |
| 9 | `/Users/kooshapari/wt-kcode-upstream-clean/target` | 1.4 GB | MEDIUM | Dead KCode worktree (P28 candidate for removal) |
| 10 | `/Users/kooshapari/wt-jcode-release-followup-clean-20261013/target` | 150 MB | LOW | Old followup worktree |

**Jcode/kcode total: 56 GB. If cleaned, frees 13% of disk.**
**Top 4 (PhenoShared, jcode, AgilePlus, phenotype-fabric) = 122 GB = 28% of disk.**

### Cargo global cache

| Path | Size | Risk | Notes |
|------|------|------|-------|
| `~/.cargo/registry` | 5.6 GB | MEDIUM | Will be re-fetched on next dep build; not free of risk |
| `~/.cargo/git` | 5.9 GB | MEDIUM | Git checkouts of crate sources; can be repopulated |
| `~/.rustup` | 12 GB | HIGH | Rust toolchain; deleting requires re-install |

**Cargo cache cleanup can free ~11.5 GB** with `cargo cache --autoclean` (safe) or `rm -rf ~/.cargo/{registry,git}/cache` (riskier).

### Recommended action plan (3-step, ascending risk)

1. **Step 1 (safe, ~10 min rebuild cost)**: `rm -rf /Users/kooshapari/CodeProjects/Phenotype/repos/PhenoShared/target` — saves 53 GB
2. **Step 2 (safe, ~11 min rebuild cost)**: `rm -rf /Users/kooshapari/CodeProjects/Phenotype/repos/jcode/target` + 4 dead KCode/jcode worktree targets (saves 43 GB)
3. **Step 3 (moderate risk)**: `cargo cache --autoclean` to remove unused registry entries (saves 2-5 GB)

**Combined potential free space: 100+ GB (enough to last months).**

### What was NOT auto-cleaned

- No target/ dir was removed autonomously
- No cargo cache was cleaned
- All destructive operations require operator approval per AGENTS.md

---

## P25c — Rescue branches (decision needed)

### Branches in question

| Branch | Created | Commits | Preservation status |
|--------|---------|---------|---------------------|
| `rescue/ceb860b-fixstorage-allow-normal-symlinked-state-home` | 3 days ago | 1 commit (kcode-storage/remove.rs: 98 +/1 -) | Orphan, only in `origin/rescue/*` |
| `rescue/d6f2d20-choresession-memory-record-validation-metadata` | 3 days ago | 2 commits (session-memory metadata + adjacent headings) | Orphan, only in `origin/rescue/*` |
| `rescue/e45ddd4-fixstorage-unlink-session-markers-through-director` | 4 days ago | 1 commit (kcode-storage/active_pids.rs: 85 +/3 -) | Orphan, only in `origin/rescue/*` |

### Why these exist

These are KCode→Jcode decollision rescue branches. When the fork was renamed (PR #33, now reverted), commits that were on KCode-specific crates (`kcode-storage`, `kcode-session-memory`) lost their target crate. They were saved as `rescue/*` branches.

### Why this matters now

- The fork is now in transition: KCode → Jcode (post-revert, decollision arc ongoing)
- These commits modify KCode-specific code that may or may not forward-port to Jcode equivalents
- If left as orphan branches, they consume ref space and are easy to forget
- If deleted, the KCode-specific fixes are LOST (cannot be recovered)

### Decision options

- **Option A: Keep all 3** — preserves work, no risk; cost is 3 orphan branches
- **Option B: Delete all 3** — clean state; cost is losing the KCode-specific fixes
- **Option C: Cherry-pick into jcode equivalents** — manual review needed to determine if the fixes still apply
- **Option D: Move to archive/* namespace** — `git branch -m rescue/X archive/X` for all 3

**Recommendation: Option D (archive)** if the operator wants to clean up but preserve history.

---

## P26 — Local master sync strategy (12 behind, 8 ahead)

### Current state

- Local `master` HEAD: `25053cbc6` (from previous turn's commit; new P30 SECURITY-FIX.md)
- `origin/master` HEAD: `09b0afba6`
- Local is **12 commits behind** + **8 commits ahead** of origin/master

### 12 origin/master commits missing locally (all real fixes)

```
a115961e6 fix(openrouter): fuse EOF transport and restore CI workflow validity
ddb23278f fix(ci): remove unused deploy key requirement
f0a4a63bd style(ci): clear workspace rustfmt drift
7b88b526a fix(windows): resolve input shell imports through terminal module
b4cb32834 fix(windows): import public Shell re-export
a66081cc0 fix(clippy): scope generated async trait must-use exception
5702c1570 fix(clippy): cover remaining generated async trait attributes
(plus 5 merge commits for PRs #52, #54, #55, #56, #57)
```

### 8 local-only commits (all `docs:`)

```
324641056 docs: add Q7 disposition
476ab1407 docs: add UPSTREAM-SYNC-STATUS.md
e2f849ae6 docs: mark 2 Emacs lock files as removed
0569246a9 docs: add CI-STATUS.md
2092723ca docs: add validation evidence + worktree-cleanup state
f6acfb1cb docs: correct overstated headline counts
3fbc4ac9c docs: add UPSTREAM-DELTA.md
(+ the new P30 SECURITY-FIX.md from this turn, on local master but not pushed)
```

### 4 sync strategies (all require operator decision)

| Strategy | Push mechanism | Risk | Reversibility |
|----------|---------------|------|---------------|
| **A. New branch from local master + cherry-pick 12 origin commits** | New branch `sync/upstream-into-local-2026-10-13` → PR | LOW | Easy: delete branch |
| **B. New branch from origin/master + cherry-pick 8 local docs** | New branch `sync/local-into-upstream-2026-10-13` → PR | LOW | Easy: delete branch |
| **C. `git pull` (merge commit)** | Direct on local master | MEDIUM | Undo with `git reset` if no other changes |
| **D. `git rebase origin/master`** on local master | Direct on local master + force-push | HIGH | History rewrite; force-push BLOCKED by policy |

**Recommended: Option A** — preserves all local work, fast-forwards onto origin's 12 fixes, creates a clean PR for review. No destructive ops.

### What was NOT done

- No sync strategy was executed autonomously
- No new branches were created
- No `git pull` or `git rebase` was run
- All sync operations require operator decision per destructive-commands policy

---

## Summary: 4 actions, all blocked on operator

| Priority | Item | Action | Risk | Savings/Benefit |
|----------|------|--------|------|-----------------|
| **HIGH** | P30 | Open 2 PRs (URLs in P30 doc) | ZERO | 7 → 0 RUSTSEC vulns |
| **HIGH** | P11 | `rm -rf PhenoShared/target` + `jcode/target` | LOW | 82 GB free |
| MEDIUM | P26 | Pick sync strategy A/B/C/D | LOW | Reconcile local + origin |
| LOW | P25c | Pick rescue branch disposition A/B/C/D | LOW | 3 orphan branches resolved |

---

## Additional finding: 2 jcode worktrees need only the quinn-proto fix

A scan of jcode worktrees (24+ total) found 2 worktrees with **partial
P30 fix** already applied to their `Cargo.lock` (h2 0.4.19+ and rustls
0.23.45) but still vulnerable to the 5 quinn-proto RUSTSEC vulns:

| Worktree | Branch | quinn-proto | h2 | rustls | Needs |
|----------|--------|-------------|----|----|-------|
| `jcode-upstream-sync` | `feature/upstream-0.88-sync` | 0.11.15 | 0.4.20 | 0.23.45 | quinn-proto bump only |
| `jcode-win-anchor` | `instinct/win-herdr-inline-anchor` | 0.11.15 | 0.4.19 | 0.23.45 | quinn-proto bump only |

Each worktree needs ONLY the `quinn-proto-0.11.19` patch (one
cherry-pick, no source changes). Dry-run verified: the patch applies
cleanly to both worktrees' Cargo.lock (27 lines changed each).

### Operator action: cherry-pick b3130e026 onto each worktree

```bash
# Per worktree (run from each worktree's repo):
cd /Users/kooshapari/CodeProjects/Phenotype/repos/jcode-upstream-sync
git cherry-pick b3130e026
cargo check && cargo clippy && cargo audit   # should be 0 vulns

cd /Users/kooshapari/CodeProjects/Phenotype/repos/jcode-win-anchor
git cherry-pick b3130e026
cargo check && cargo clippy && cargo audit   # should be 0 vulns
```

These 2 cherry-picks are independent of the 2 main security PRs.
The operator can do them before, after, or alongside the main P30 work.

**Why this is in the action list:** the 2 worktrees are active
branches (not dead/archived). They have REAL upstream-sync and
Win-anchor work. Leaving them with 5 active RUSTSEC vulns is a
security exposure if those branches ever get built or merged.

### STATUS (2026-10-14T00:33Z): cherry-picks DONE locally, NOT pushed

Both cherry-picks have been applied to the local branches and
validated with cargo audit. The changes are LOCAL ONLY — the
operator must explicitly push them when ready.

| Worktree | Before | After | Vulns before | Vulns after | Pushed? |
|----------|--------|-------|--------------|-------------|---------|
| `jcode-upstream-sync` | `545d6d7f4` | `c2e4e91bd` | 5 | 0 | NO |
| `jcode-win-anchor` | `9948f0e8c` | `0473d4b30` | 5 | 0 | NO |

**Validation (post-cherry-pick, on the new tip):**
- `cargo audit --no-fetch` on jcode-upstream-sync: 0 active vulns (10 unmaintained warnings, baseline)
- `cargo audit --no-fetch` on jcode-win-anchor: 0 active vulns (9 unmaintained warnings, baseline)

**Note for jcode-win-anchor:** the cherry-pick required adding a
temporary `jcode-fork` remote (https://github.com/KooshaPari/Jcode.git)
to fetch `b3130e026`. The remote and the remote-tracking branch were
cleaned up after the cherry-pick. The worktree's actual `origin` is
still `https://github.com/1jehuang/jcode.git` (upstream).

**To push (operator decision):**
```bash
# Per worktree, when ready:
cd /Users/kooshapari/CodeProjects/Phenotype/repos/jcode-upstream-sync
git push origin feature/upstream-0.88-sync

cd /Users/kooshapari/CodeProjects/Phenotype/repos/jcode-win-anchor
git push origin instinct/win-herdr-inline-anchor
```

---

## op-7-cross-repo-p30: DONE 2026-10-14 (4/5 repos fixed)

Cross-repo P30 work was completed for 4 of 5 affected repos. See
`docs/cleanup-arc-2026-10-13-P30-CROSS-REPO.md` for full details.

| Repo | Branch | SHA | Status |
|------|--------|-----|--------|
| `forgecode` | `security/h2-0.4.19` | `f7da05ebd` | partial (1 of 2 cleared; rocket blocks) |
| `Tracera` | `security/quinn-proto-0.11.19-rustls-0.23.45` | `11d220aff` | FULL FIX (6→0) |
| `Agentora` | `security/quinn-proto-0.11.19-rustls-0.23.45` | `9164276` | FULL FIX (6→0) |
| `Apisync` | `security/quinn-proto-0.11.19-rustls-0.23.45` | `48ff5a7` | FULL FIX (6→0) |
| `substrate` | (branch reset, no commit) | -- | BLOCKED (vendored sources) |

**Total cleared: 20 → 1 RUSTSEC vulns across 4 repos (95% reduction).**
All 4 branches are local-only. Push + PR creation is the operator's call.

**Two blockers documented:**
- `substrate` has 1GB vendored sources; `cargo update` is a no-op. Needs
  manual `cargo vendor` re-vendoring of quinn-proto 0.11.19 + dependents.
- `forgecode`'s remaining 1 vuln is h2 0.3.27 (transitive from
  rocket_http); h2 0.3.x is unmaintained. Needs rocket upgrade or removal.

---

tx-agent: jcode
tx-task: P11+P25c+P26
tx-validated: manual
tx-scope: cleanup-arc-2026-10-13
tx-intent: surface 4 operator-action items with exact risk/cost analysis
