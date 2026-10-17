Three real issues were flagged against PR #94 (`fix(update): fork builds never auto-update from upstream`) by independent reviewers. These are correctness bugs, not style nits. PR #94 must NOT land until they are addressed.

## Issue 1: prerelease detection treats every nonempty suffix as a fork (Kilo WARNING)

`crates/jcode-app-core/src/update_product.rs:67-68` (per Kilo inline comment)

The current logic appears to call any nonempty semver suffix a "fork channel". A real consequence: an upstream prerelease build such as `1.0.0-alpha.1` would be classified as a fork and lose its automatic update path. This breaks jcode's legitimate prerelease flow on the upstream product.

**Expected**: fork-ness must be derived from a known prefix allowlist (the `k`, `p`, `fork-` markers per the operator's fork versioning scheme in `~/AGENTS.md`), not from "any nonempty suffix".

**Repro**: build with `1.0.0-alpha.1` set as the version string and call `update_repo()` — the function returns `None` even though this is an upstream prerelease.

## Issue 2: runtime semver override can erase a fork's identity (Kilo WARNING + CodeAnt HIGH RISK)

`crates/jcode-app-core/src/update_product.rs:117` (per both Kilo and CodeAnt reviews)

`JCODE_RUNTIME_RELEASE_SEMVER` (or equivalent) lets the launch environment override the compiled fork identity. A fork build with a malicious or accidental env override can be reclassified as upstream and pull upstream binaries over the local fork.

**Expected**: a runtime override that does not match a known fork prefix should either be rejected outright or be required to clear a stronger check (signed manifest, not just env-var). The compiled product identity must remain the authority unless a deliberately-signed override is supplied.

**CodeAnt note (HIGH RISK)**: this is the attack path that converts a fork-only update into an upstream-allowed update with a single environment variable. It must be closed before merge.

## Issue 3: size-budget gate FAILING at the current PR head

Reported by the operator in PR #94 (commit `1a3f491a7`, current head):

```
oversized file grew: crates/jcode-app-core/src/update.rs (1771 -> 1784 LOC)
```

Exit code 1 from `scripts/check_code_size_budget.py`. The earlier verification numbers were on a different (`5eb33d748`) commit that is no longer the PR head and was never an ancestor — see the operator's "Correction to my comment above" comment.

**Expected**: gate must exit 0 at the merge commit, the same commit that passes review. Either inline the helper back into `update.rs` (status quo) and raise the baseline, or move it cleanly into `update_product.rs` and keep `update.rs` at-or-under 1771.

## Why this must be fixed before merge

- The review tools flagged Issues 1 and 2 independently with high confidence.
- The size-budget gate (Issue 3) is a local-only hard gate whose failure the CI cannot see (hosted Actions is disabled).
- The intent of the PR — fork builds no longer auto-replacing themselves with upstream binaries — is good, but landing it in this state ships the new attack path described in Issue 2.

## Suggested fix order

1. Tighten `is_fork_build()` to a prefix allowlist (`k`, `p`, `fork-`); add tests for upstream prereleases.
2. Reject runtime semver overrides that would change the product identity, or require a signed override.
3. Re-decide where `update_repo()` / `is_inside_git_repo()` live so the size-budget gate exits 0; re-record the baseline only with justification.
4. Re-run `cargo test -p jcode-app-core --lib update_product` (the operator's earlier attempt was OOM-killed on the build host; this needs ~50 GiB headroom).

Cross-refs: PR #94, C-triage audit (2026-10-16).
