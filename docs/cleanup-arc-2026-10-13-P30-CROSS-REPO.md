# P30-cross-repo: Cross-Repository RUSTSEC Audit (2026-10-13)

**Date**: 2026-10-13
**Source**: Companion to `docs/cleanup-arc-2026-10-13-P30-SECURITY-FIX.md`
**Scope**: 56+ Cargo.lock files under `~/CodeProjects/Phenotype/repos/*` and `~/wt-*`

---

## TL;DR

The 7 RUSTSEC vulns found in jcode (`origin/master` 09b0afba6) affect **20 jcode worktrees/forks** (same lock file) plus **6 other Phenotype repos** that share the same vulnerable crate versions. The P30 Cargo.lock-only fix pattern from jcode can be ported to each repo with the same commands.

| Repo | quinn-proto | h2 | rustls | P30 vulns | Risk |
|------|-------------|-----|--------|-----------|------|
| `jcode` (master) | 0.11.15 ✗ | 0.4.13 ✗ | 0.23.37 ✗ | 7 | critical |
| `forgecode` | 0.11.18 ✓ | 0.4.13 ✗ | 0.23.45 ✓ | 1 (h2) | medium |
| `substrate` | 0.11.15 ✗ | 0.4.19 ✓ | 0.23.41 ✗ | 5+ | high |
| `Tracera` | 0.11.15 ✗ | 0.4.18 partial | 0.23.41 ✗ | 5+ | high |
| `Agentora` | 0.11.15 ✗ | 0.4.16 ✗ | 0.23.40 ✗ | 5+ | high |
| `Apisync` | 0.11.15 ✗ | 0.4.16 ✗ | 0.23.41 ✗ | 5+ | high |
| `wt-agentora-clippy-20260911` | 0.11.15 ✗ | 0.4.16 ✗ | 0.23.40 ✗ | 5+ | high |

`AgilePlus`, `helios-cli`, `pheno`, `phenoAI`, `PhenoShared`, `phenotype-fabric`, `sharecli` are already on the fixed trio (0.11.18 / 0.4.19 / 0.23.45) — **no action needed**.

---

## Jcode worktree cluster (all 20 with vulnerable trio)

These are all jcode forks/worktrees that share the same Cargo.lock (post-PR-#33-revert, pre-P30-fix):

```
jcode-acp-native
jcode-decollide-kcode-r2
jcode-dev-isolation-review-followup
jcode-dev-isolation-socket-review
jcode-dev-isolation-storage-followup
jcode-dev-isolation
jcode-fork-parent
jcode-patch
jcode-poke-context
jcode-pr10-build-20261009
jcode-pr27-grok-dev-auth
jcode-pr27-minor-review-fixes
jcode-pr27-pid-test-windows
jcode-pr27-review-scope
jcode-pr27-reviewed-fixes
jcode-pr27-reviewed-fixes-b
jcode-pr8-on-upstream-20261005
jcode-pr8-review
jcode-ratatui-guard
jcode-rebase-v0.90
jcode-src
jcode-stack-rebase-20261008
jcode-upstream-baseline
jcode-upstream-issue-draft
```

All 24 worktrees (1 main + 23 worktrees) have `quinn-proto=0.11.15, h2=0.4.13, rustls=0.23.37` and the same 7 RUSTSEC vulns. The fix is identical for all: apply the 2 security branches from jcode (`security/rustsec-2026-0258-0285` and `security/quinn-proto-0.11.19`).

**Recommended action**: Only fix the 2-3 worktrees that are actively used. The others are throwaway worktrees that will be re-created from origin/master after the security PRs land.

**Highest-priority worktrees to fix (active work):**
- `jcode` (main, needs PRs opened — see P30 doc)
- `jcode-rebase-v0.90` (PR #33 revert branch, will be replaced)
- `jcode-upstream-sync` (already has 0.4.20 + 0.23.45 — only quinn-proto 0.11.15 needs fixing)
- `jcode-win-anchor` (already has 0.4.19 + 0.23.45 — only quinn-proto 0.11.15 needs fixing)

---

## Other Phenotype repos with same P30 vulns

### `forgecode` — 1 remaining vuln

- quinn-proto 0.11.18 ✓ (fixed)
- h2 0.4.13 ✗ (RUSTSEC-2026-0258, DoS)
- rustls 0.23.45 ✓ (fixed)

**Fix**: `cargo update -p h2 --precise 0.4.19` (1 Cargo.lock line).

### `substrate` — 5+ vulns

- quinn-proto 0.11.15 ✗ (RUSTSEC-2026-0338, 0339, 0340, 0341, 0342, 0343)
- h2 0.4.19 ✓ (fixed)
- rustls 0.23.41 ✗ (older version; check 0.23.41 advisories)

**Fix**: `cargo update -p quinn-proto --precise 0.11.19` (and possibly rustls bump).

### `Tracera` — 5+ vulns

- quinn-proto 0.11.15 ✗ (all 5 quinn-proto RUSTSEC)
- h2 0.4.18 (between 0.4.13 and 0.4.19; check if 0.4.18 has 0258 fixed or not — need cargo audit)
- rustls 0.23.41 ✗

**Fix**: same pattern; need `cargo audit` to confirm exact h2 status.

### `Agentora`, `Apisync`, `wt-agentora-clippy-20260911` — 5+ vulns each

All 3 repos have quinn-proto 0.11.15, h2 0.4.16, rustls 0.23.40-0.23.41. All need the same Cargo.lock bumps.

---

## Already-fixed repos (no action)

| Repo | quinn-proto | h2 | rustls | Notes |
|------|-------------|-----|--------|-------|
| `AgilePlus` | 0.11.18 ✓ | 0.4.19 ✓ | 0.23.45 ✓ | clean |
| `helios-cli` | 0.11.17 | 0.4.19 ✓ | 0.23.43 | partial — check 0.11.17 fix coverage |
| `pheno` | 0.11.18 ✓ | 0.4.19 ✓ | 0.23.45 ✓ | clean |
| `phenoAI` | 0.11.18 ✓ | 0.4.19 ✓ | 0.23.45 ✓ | clean |
| `PhenoShared` | 0.11.18 ✓ | 0.4.19 ✓ | 0.23.45 ✓ | clean |
| `phenotype-fabric` | (no quinn-proto) | 0.4.19 ✓ | 0.23.45 ✓ | clean |
| `sharecli` | 0.11.17 | 0.4.19 ✓ | 0.23.45 ✓ | partial — check 0.11.17 fix coverage |

Note: 0.11.17 fixes 0339/0341/0342, but 0338/0340/0343 require 0.11.18. So `helios-cli` and `sharecli` (on 0.11.17) may still have 3 quinn-proto RUSTSEC vulns.

---

## Cross-repo fix pattern (3 commands each)

For each affected repo, the fix is:

```bash
cd /path/to/repo
git checkout -b security/rustsec-2026-0258-0285 origin/master
cargo update -p h2 --precise 0.4.19
cargo update -p rustls --precise 0.23.45
git add Cargo.lock
git commit -m "deps: bump h2 + rustls per RUSTSEC-2026-0258/0285"

git checkout -b security/quinn-proto-0.11.19 origin/master
cargo update -p quinn-proto --precise 0.11.19
git add Cargo.lock
git commit -m "deps: bump quinn-proto 0.11.15 -> 0.11.19 (RUSTSEC-2026-0338-0343)"

# Push both branches and open PRs
```

Each repo needs its own PR. Total: 2 branches × 7 affected repos = 14 PRs (if all are tackled).

---

## Recommendation

1. **jcode P30 first** (already done) — operator opens 2 PRs (blocked on pre_tool hook)
2. **Cross-repo P30 second** — create 12-14 more Cargo.lock-only PRs across the affected repos
3. **Prioritize active repos first**: `forgecode`, `substrate`, `Tracera`, `Agentora`, `Apisync` (these are likely used)
4. **Defer jcode worktree cluster** — only fix the 4 worktrees listed above; the rest are throwaway

This is a separate workstream from the cleanup arc. The cross-repo PRs should be done as a batch to maximize reuse of the P30 pattern.

---

tx-agent: jcode
tx-task: P30-cross-repo
tx-validated: manual
tx-scope: docs:cleanup-arc
tx-intent: surface cross-repo P30 audit with 7 affected repos + 24 jcode worktrees
