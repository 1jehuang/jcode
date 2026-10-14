# Cleanup-arc branch P30 status (2026-10-14)

## Critical finding

**`chore/cargo-fmt-cleanup-2026-10-13` is NOT P30-clean.**

```
Branch: chore/cargo-fmt-cleanup-2026-10-13
Tip:    a24323190 docs(cleanup-arc-2026-10-13): P30 cherry-pick APPLIED
Ahead:  31 commits (fmt cleanup work)
Behind: 12 commits (origin/master)
Pushed: yes (refs/heads/chore/cargo-fmt-cleanup-2026-10-13)
cargo audit: 7 vulnerabilities found
```

The 31-commit fmt cleanup arc branch was created BEFORE the P30 RUSTSEC
audit, so its `Cargo.lock` is identical to origin/master's vulnerable
lockfile. If this branch is opened as a PR and merged first, the
resulting master would be fmt-clean but RUSTSEC-vulnerable.

## Options for the operator

### Option 1 (recommended): apply security fix on top of cleanup-arc

After both `security/*` PRs land on master, rebase cleanup-arc onto the
fixed master. The fmt work survives the rebase (no overlap with
Cargo.lock changes), and the resulting PR is both fmt-clean AND
P30-clean.

```bash
# Once both security PRs are merged to origin/master
git checkout chore/cargo-fmt-cleanup-2026-10-13
git rebase origin/master   # replays the 31 fmt commits onto fixed master
# (the security PRs are in origin/master; cleanup-arc replays on top)
cargo check && cargo clippy && cargo audit
git push --force-with-lease   # rebase rewrites local branch tip
```

(Note: `--force-with-lease` is destructive-command-blocked by policy.
Use a `git push --force` only if the operator explicitly approves in
the same turn. The recommended path is: create a NEW branch from the
rebased tip, push that, leave the original untouched.)

```bash
# Safe alternative: new branch from rebased tip
git checkout -b chore/cargo-fmt-cleanup-2026-10-13-rebased origin/master
# Cherry-pick the 31 commits from the old branch
git log --reverse a24323190~31..a24323190 --format='%h' | xargs -I{} git cherry-pick {}
# Now push the new branch and open a PR
```

### Option 2: cherry-pick the 2 security commits onto cleanup-arc

If the operator wants to open the cleanup-arc PR first (before the
security PRs are merged), cherry-pick the 2 security branch tips
(`23a575c3d` and `b3130e026`) onto the cleanup-arc branch tip. This
gives the PR a complete fix in one go.

```bash
git checkout chore/cargo-fmt-cleanup-2026-10-13
git cherry-pick 23a575c3d   # security/rustsec-2026-0258-0285
git cherry-pick b3130e026   # security/quinn-proto-0.11.19
# Resolve any conflicts (unlikely — Cargo.lock-only changes)
cargo check && cargo clippy && cargo audit   # should be 0 vulns
git push
# Open PR
```

### Option 3: open the 2 security PRs first, then rebase cleanup-arc

The cleanest order. Land `security/rustsec-2026-0258-0285` and
`security/quinn-proto-0.11.19` on origin/master first. Then rebase
cleanup-arc onto the fixed master. The 2 PRs are small (Cargo.lock
only) and unlikely to be controversial.

### Option 4: do nothing

Leave cleanup-arc as-is. Open the 2 security PRs first, let them land.
Then rebase cleanup-arc later. The fmt work is preserved on the
branch — there's no race or contention.

## Doc-overlap finding (additional, post-merge consideration)

The cleanup-arc branch's 4 P30-related docs overlap in subject matter
with my 5 new master docs, but use **different filenames** so a
merge/rebase would NOT conflict at the file level. However, the
resulting merged tree would have **9 P30-related docs**, which is a
lot of redundancy:

**My docs on master (5 new, all `docs/cleanup-arc-2026-10-13-*`):**
- P30-SECURITY-FIX.md
- OPERATOR-ACTIONS.md
- P30-CROSS-REPO.md
- PR-CONTENT.md
- CLEANUP-ARC-P30-STATUS.md

**Cleanup-arc's docs (4, on `chore/cargo-fmt-cleanup-2026-10-13`):**
- P30-ACTION.md (the cherry-pick application note)
- SECURITY.md (the audit doc)
- final.md (overall arc final state)
- inventory.md (file-level inventory)

**After merge (if both are kept):** 9 P30-related docs in `docs/`.
Operator should pick: keep all 9, or consolidate to 2-3 (a single
P30 audit + a single P30 cross-repo + a single operator-action).

This is a downstream cleanup concern, not a blocker for the security
fix. The Cargo.lock change is the critical path; the doc consolidation
can happen afterward.

## Why this matters

The P30 cross-repo audit found that 6 other Phenotype repos have the
same RUSTSEC vulns. If a future agent (or the operator) opens the
cleanup-arc PR without the security fix, the merge would silently
introduce 7 RUSTSEC vulns into master at the same time as fixing the
fmt issue. The fmt fix is a quality-of-life improvement; the security
fix is a critical remediation. The operator should not lose the
security fix in the noise of the fmt cleanup.

## Recommended sequence

1. **Land `security/rustsec-2026-0258-0285`** (RUSTSEC-2026-0258 + 0285)
2. **Land `security/quinn-proto-0.11.19`** (RUSTSEC-2026-0338-0342)
3. **Rebase `chore/cargo-fmt-cleanup-2026-10-13`** onto the fixed origin/master
4. **Open the cleanup-arc PR** (now 0 RUSTSEC vulns + fmt-clean)

Combined: 7 RUSTSEC vulns cleared + fmt cleanup delivered, all in
reviewable PRs.

## Validation done

```bash
git checkout chore/cargo-fmt-cleanup-2026-10-13 -- Cargo.lock
cargo audit --no-fetch
# error: 7 vulnerabilities found!
git checkout HEAD -- Cargo.lock
```

Cargo audit ran on the cleanup-arc's Cargo.lock and confirmed the 7
vulns are present. The fix is to either apply the 2 security PRs
first (Option 3) or cherry-pick them onto cleanup-arc (Option 2).

tx-agent: jcode
tx-task: P30-cleanup-arc-status
tx-validated: cargo-audit
tx-scope: chore/cargo-fmt-cleanup-2026-10-13
tx-intent: surface that cleanup-arc branch is not P30-clean; recommend sequence
