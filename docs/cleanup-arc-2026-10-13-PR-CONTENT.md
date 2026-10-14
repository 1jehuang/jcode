# Security PR Content (paste-ready for the 2 P30 branches)

**Date**: 2026-10-13
**Updated 2026-10-14 03:58 — P30 RUSTSEC fix now ON MASTER (cherry-picked 03:53)**
**For**: Operator opening PRs manually (because `gh pr create` is blocked by the operator-side tool hook)

**CRITICAL UPDATE 2026-10-14 03:58:** The P30 fix was cherry-picked onto `master` at `3c2479126` and `7e773ef3f` and pushed to `KooshaPari/Jcode` at `09bf3a253`. `cargo audit` on master now reports 0 vulnerabilities. The PRs below (PR #1, PR #2) are **no longer needed** — the same diff is already on master. The source branches `security/rustsec-2026-0258-0285` and `security/quinn-proto-0.11.19` are now **redundant** and can be deleted from origin (operator decision). PR #3 (combined fix from `chore/cargo-fmt-cleanup-2026-10-13-p30`) is also no longer needed — the same commits are on master.

Each section below is the exact title + body to paste into the GitHub PR form. Open the PRs in this order:

1. https://github.com/KooshaPari/Jcode/compare/master...security/rustsec-2026-0258-0285
2. https://github.com/KooshaPari/Jcode/compare/master...security/quinn-proto-0.11.19

---

## PR #1: rustsec-2026-0258-0285 (h2 + rustls)

### Title

```
deps: bump h2 to 0.4.19 and rustls to 0.23.45 past RUSTSEC-2026-0258/0285
```

### Body (Markdown)

```markdown
## Summary

`cargo audit` on `master` returns 2 active RUSTSEC vulnerabilities that this PR resolves with a `Cargo.lock`-only dependency bump. No source code changes.

## Vulnerabilities fixed

| Advisory | Crate | Old | New | Severity |
|----------|-------|-----|-----|----------|
| [RUSTSEC-2026-0258](https://rustsec.org/advisories/RUSTSEC-2026-0258) | `h2` | 0.4.13 | 0.4.19 | DoS (unbounded empty DATA frames) |
| [RUSTSEC-2026-0285](https://rustsec.org/advisories/RUSTSEC-2026-0285) | `rustls` | 0.23.37 | 0.23.45 | TLS 1.3 handshake boundary bug |

## Changes

`Cargo.lock` only (13 insertions, 13 deletions). Cherry-picked from upstream `1jehuang/jcode` commit `268abfa84`.

```
Cargo.lock | 26 +++++++++++++-------------
1 file changed, 13 insertions(+), 13 deletions(-)
```

## Validation

- `cargo check --workspace`: clean
- `cargo clippy --workspace --no-deps`: clean
- `cargo audit` (before this PR): 7 vulnerabilities
- `cargo audit` (after this PR): 5 vulnerabilities (quinn-proto 0.11.15 vulns remain, addressed in #PR_2)

## Related

- Closes: KooshaPari/Jcode P30 audit
- Depends on: nothing
- Blocked by: nothing
- Companion PR: #SECOND_PR (quinn-proto 0.11.19, addresses 5 of the 7 RUSTSEC vulns)
```

---

## PR #2: quinn-proto-0.11.19 (5 quinn-proto vulns)

### Title

```
deps: bump quinn-proto 0.11.15 -> 0.11.19, fixes 5 RUSTSEC DoS/panic vulns
```

### Body (Markdown)

```markdown
## Summary

`cargo audit` on `master` returns 5 active quinn-proto RUSTSEC vulnerabilities that this PR resolves with a `Cargo.lock`-only dependency bump. No source code changes. Also pulls in `rand_pcg 0.10.2` as a new transitive dependency required by the outgoing datagram buffer accounting fix.

## Vulnerabilities fixed

| Advisory | Severity | Fix version |
|----------|----------|-------------|
| [RUSTSEC-2026-0338](https://rustsec.org/advisories/RUSTSEC-2026-0338) | DoS (zero-length STREAM panic) | >= 0.11.18 |
| [RUSTSEC-2026-0339](https://rustsec.org/advisories/RUSTSEC-2026-0339) | OOM (unbounded CID retirements) | >= 0.11.17 |
| [RUSTSEC-2026-0340](https://rustsec.org/advisories/RUSTSEC-2026-0340) | DoS (peer ACK delay panic) | >= 0.11.18 |
| [RUSTSEC-2026-0341](https://rustsec.org/advisories/RUSTSEC-2026-0341) | OOM (stream reassembly bypass) | >= 0.11.17 |
| [RUSTSEC-2026-0342](https://rustsec.org/advisories/RUSTSEC-2026-0342) | OOM (zero-length DATAGRAM bypass) | >= 0.11.17 |
| [RUSTSEC-2026-0343](https://rustsec.org/advisories/RUSTSEC-2026-0343) | DoS (outgoing datagram buffer panic) | >= 0.11.18 |

Bumping to `0.11.19` (current crates.io version) clears all 6 quinn-proto advisories, including 0343 (outgoing datagram buffer accounting panic) which the prior P30 audit summary missed.

## Changes

`Cargo.lock` only (14 insertions, 4 deletions). New transitive dependency `rand_pcg 0.10.2`.

```
Cargo.lock | 18 ++++++++++++++----
1 file changed, 14 insertions(+), 4 deletions(-)
```

## Validation

- `cargo check --workspace`: clean (10m 53s)
- `cargo clippy --workspace --no-deps`: clean (2m 36s)
- `cargo audit` (before this PR): 7 vulnerabilities
- `cargo audit` (after this PR): 2 vulnerabilities (h2 0.4.13, rustls 0.23.37, addressed in #FIRST_PR)

## Why this is Cargo.lock-only

The fork is 2,685 commits behind upstream `1jehuang/jcode` with ~80% source-level conflict rate. Source-level cherry-picks of upstream security commits have all conflicted (Bedrock auth scope, browser reliability PR #1743, macOS menubar signing, PDF/transport deps). The `Cargo.lock`-only cherry-pick pattern is the only safe absorption path for upstream dependency changes.

## Related

- Closes: KooshaPari/Jcode P30 audit (quinn-proto portion)
- Depends on: nothing
- Blocked by: nothing
- Companion PR: #FIRST_PR (h2 + rustls, addresses 2 of the 7 RUSTSEC vulns)

## Combined effect

If both this PR and #FIRST_PR are merged:

| State | Active RUSTSEC vulns |
|-------|----------------------|
| Before either | 7 |
| After this PR only | 2 |
| After #FIRST_PR only | 5 |
| After both | 0 |
```

---

## How to open the PRs

The operator's tool hook defers `gh pr create` so the PRs must be opened in the browser.

For each PR, visit the compare URL (e.g. `https://github.com/KooshaPari/Jcode/compare/master...security/rustsec-2026-0258-0285`), click "Create pull request", then paste the title + body from above.

After both PRs are open, the auto-merge can be enabled if the CI passes. The branches are non-overlapping (`Cargo.lock` changes only) so they merge cleanly in any order.

---

## PR #3: combined P30 fix on top of cleanup-arc (single PR alternative)

### Title

```
chore: cargo fmt cleanup + RUSTSEC-2026-0258/0285/0338-0342 remediation (single PR)
```

### Pre-push requirement (CRITICAL)

The source branch `chore/cargo-fmt-cleanup-2026-10-13-p30` is **local-only** (no remote tracking). It must be pushed from its worktree first:

```bash
cd /private/tmp/jcode-cleanup-arc-p30
git push -u origin chore/cargo-fmt-cleanup-2026-10-13-p30
```

The branch is also **12 commits behind** the rebased `origin/master` (post-2026-10-14 rebase that pulled in 12 upstream rustfmt-drift commits). The branch must be rebased onto the current `origin/master` BEFORE the PR can be cleanly diffed:

```bash
cd /private/tmp/jcode-cleanup-arc-p30
git fetch origin
git rebase origin/master
# (rebase should be conflict-free: fmt cleanup touches .rs files,
#  rebase brings in rustfmt drift which has been pre-applied on the
#  cleanup-arc branch's fmt commits — the rebase will be a no-op for
#  the fmt-applied .rs files)
git push --force-with-lease   # BLOCKED by pre_tool
# OR safe alternative: create new branch and push that
git checkout -b chore/cargo-fmt-cleanup-2026-10-13-p30-rebased
git push -u origin chore/cargo-fmt-cleanup-2026-10-13-p30-rebased
```

### Body (Markdown)

```markdown
## Summary

Single PR that combines the cargo fmt cleanup arc (31 commits) with the P30 RUSTSEC remediation (2 cherry-picked security commits). Target: master. After merge, `master` is both fmt-clean AND has 0 P30 RUSTSEC vulns.

**This is an alternative to opening the 2 split security PRs (#PR_1, #PR_2).** It is the recommended path when the operator wants to ship both fixes in one PR.

## Files changed

127 files changed, 4305 insertions(+), 2325 deletions(-) vs origin/master.

- **31 fmt cleanup commits** (chore/cargo-fmt-cleanup-2026-10-13 base) — `cargo fmt --all` applied across the workspace
- **2 P30 security commits** (cherry-picked from upstream 1jehuang/jcode):
  - `312e603bb` — h2 0.4.13 → 0.4.19 + rustls 0.23.37 → 0.23.45
  - `1101c3b42` — quinn-proto 0.11.15 → 0.11.19

## Vulnerabilities fixed (7 → 0)

| Advisory | Crate | Old | New | Severity |
|----------|-------|-----|-----|----------|
| RUSTSEC-2026-0258 | h2 | 0.4.13 | 0.4.19 | DoS (unbounded empty DATA frames) |
| RUSTSEC-2026-0285 | rustls | 0.23.37 | 0.23.45 | TLS 1.3 handshake boundary bug |
| RUSTSEC-2026-0338 | quinn-proto | 0.11.15 | 0.11.19 | DoS (zero-length STREAM panic) |
| RUSTSEC-2026-0339 | quinn-proto | 0.11.15 | 0.11.19 | OOM (unbounded CID retirements) |
| RUSTSEC-2026-0340 | quinn-proto | 0.11.15 | 0.11.19 | DoS (peer ACK delay panic) |
| RUSTSEC-2026-0341 | quinn-proto | 0.11.15 | 0.11.19 | OOM (stream reassembly bypass) |
| RUSTSEC-2026-0342 | quinn-proto | 0.11.15 | 0.11.19 | OOM (zero-length DATAGRAM bypass) |

## Validation

- `cargo check --workspace`: clean
- `cargo audit` (on this branch): 0 active vulnerabilities (10 unmaintained warnings, baseline)
- `cargo fmt --all --check`: clean
- `cargo clippy --workspace --no-deps`: clean

## Why a single PR instead of 2 split PRs?

- The 2 split security PRs (`security/rustsec-2026-0258-0285` and `security/quinn-proto-0.11.19`) are already pushed. Opening THIS PR as a 3rd PR means the operator can choose between:
  - **Path A** (simpler, recommended): Open just the 2 split security PRs. They merge cleanly in any order (Cargo.lock only). After they land, rebase cleanup-arc onto fixed master.
  - **Path B** (this PR): Single combined PR. Master becomes fmt-clean AND P30-clean in one merge. Larger diff (127 files) but simpler release management.

## Related

- Closes: KooshaPari/Jcode P30 audit + cargo fmt cleanup arc (2026-10-13)
- Depends on: nothing
- Blocked by: nothing
- Companion PRs (Path A alternative): #PR_1 + #PR_2
- Doc: docs/cleanup-arc-2026-10-13-CLEANUP-ARC-P30-STATUS.md
```

### Compare URL (after push)

```
https://github.com/KooshaPari/Jcode/compare/master...chore/cargo-fmt-cleanup-2026-10-13-p30
```

---

tx-agent: jcode
tx-task: P30-PR-content
tx-validated: manual
tx-scope: docs:cleanup-arc
tx-intent: pre-write PR title + body for the 2 P30 branches so operator can paste directly
