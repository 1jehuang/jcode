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

## Cross-repo fix pattern (verified 2026-10-14)

For each affected repo, the fix is:
```bash
cd /path/to/repo
# Stash operator's uncommitted work if any
git stash push -m "P30-stash" -u
# Create security branch from main
git checkout -b security/quinn-proto-0.11.19-rustls-0.23.45 origin/main
# Apply fix
cargo update -p quinn-proto --precise 0.11.19
cargo update -p rustls --precise 0.23.45
# If h2 < 0.4.16, also run: cargo update -p h2@<oldver> --precise 0.4.19
git add Cargo.lock
git commit -m "fix(deps): bump quinn-proto 0.11.15→0.11.19, rustls 0.23.4x→0.23.45"
# Return to main and pop stash
git checkout main && git checkout <original-feature-branch> && git stash pop
```

### Cross-repo fix results (2026-10-14)

| Repo | Pre-fix vulns | Post-fix vulns | Branch | Status |
|------|---------------|----------------|--------|--------|
| `forgecode` | 2 (RUSTSEC-2026-0258 × 2: h2 0.4.13 + h2 0.3.27) | 1 (h2 0.3.27 from rocket_http) | `security/h2-0.4.19` @ `f7da05ebd` | partial (rocket blocks full fix) |
| `Tracera` | 6 (5 quinn + 1 rustls) | 0 (6 unmaintained warnings) | `security/quinn-proto-0.11.19-rustls-0.23.45` @ `11d220aff` | FULL FIX |
| `Agentora` | 6 (5 quinn + 1 rustls) | 0 (1 unmaintained warning) | `security/quinn-proto-0.11.19-rustls-0.23.45` @ `9164276` | FULL FIX |
| `Apisync` | 6 (5 quinn + 1 rustls) | 0 (1 unmaintained warning) | `security/quinn-proto-0.11.19-rustls-0.23.45` @ `48ff5a7` | FULL FIX |
| `substrate` | 6 (5 quinn + 1 rustls) | BLOCKED | (vendor/ complex) | vendored sources — needs manual re-vendoring |

**Total cleared across 4 repos: 20 → 1 (95% reduction).**

### substrate: vendored-sources blocker

`substrate` has 1GB of vendored sources (`vendor/quinn-proto-0.11.15/`,
`vendor/quinn-0.11.11/`, etc.) and `.cargo/config.toml` configured with
`replace-with = "vendored-sources"`. Simple `cargo update` is a no-op because
the lockfile is constrained by the vendored source. Full fix requires:
1. Delete `vendor/quinn-proto-0.11.15/`
2. Add `vendor/quinn-proto-0.11.19/` (run `cargo vendor` with new lockfile)
3. Re-vendor any dependent crates (quinn, quinn-0.11.11)
4. Validate with `cargo check` + `cargo test`

This is a multi-hour effort. Branch `security/quinn-proto-0.11.19-rustls-0.23.45`
was created during exploration but reset to pre-fix state (no commits). Use it
as a starting point: `cd substrate && git checkout security/quinn-proto-0.11.19-rustls-0.23.45`
and re-apply after re-vendoring.

### forgecode: rocket_http h2 0.3.27 blocker

forgecode's `security/h2-0.4.19` branch fixes h2 0.4.13 → 0.4.19 (clears
RUSTSEC-2026-0258 in the 0.4.x dep chain). However, `rocket_http` pulls
in h2 0.3.27 transitively, which is also affected by RUSTSEC-2026-0258.
h2 0.3.x is unmaintained; the fix only exists in 0.4.x. Resolving this
would require upgrading or removing the rocket dependency.

### All 4 branches: local only (push remains operator decision)

All 4 security branches are local commits, not pushed. Operator must
explicitly push when ready and open PRs. Branches follow the jcode
convention: `security/<crate>-<target-version>[-<other-crate>-<other-version>]`.

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
