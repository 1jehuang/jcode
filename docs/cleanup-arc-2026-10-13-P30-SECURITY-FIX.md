# P30: Critical RUSTSEC Audit + Remediation (2026-10-13)

**Date**: 2026-10-13 (audit) — **RESOLVED 2026-10-14 04:00Z (cherry-pick onto master)**
**Final branch state**: P30 fix is on `master` (`f4d0cc4e5` → `491a52bb5`). The 3 source branches (PR #1 source `security/rustsec-2026-0258-0285`, PR #2 source `security/quinn-proto-0.11.19`, PR #3 source `chore/cargo-fmt-cleanup-2026-10-13-p30`) are now redundant.

## What was found

`cargo audit` on `origin/master` (`09b0afba6`, pre-fix baseline) returned **7 active vulnerabilities**:

| ID | Crate | Version | Severity | Fix |
|---|---|---|---|---|
| RUSTSEC-2026-0258 | h2 | 0.4.13 | DoS (unbounded empty DATA frames) | Upgrade to 0.4.19 |
| RUSTSEC-2026-0285 | rustls | 0.23.37 | TLS 1.3 handshake boundary bug | Upgrade to 0.23.45 |
| RUSTSEC-2026-0338 | quinn-proto | 0.11.15 | DoS (zero-length STREAM panic) | Upgrade to >=0.11.18 |
| RUSTSEC-2026-0339 | quinn-proto | 0.11.15 | OOM (unbounded CID retirements) | Upgrade to >=0.11.17 |
| RUSTSEC-2026-0340 | quinn-proto | 0.11.15 | DoS (peer ACK delay panic) | Upgrade to >=0.11.18 |
| RUSTSEC-2026-0341 | quinn-proto | 0.11.15 | OOM (stream reassembly bypass) | Upgrade to >=0.11.17 |
| RUSTSEC-2026-0342 | quinn-proto | 0.11.15 | OOM (zero-length DATAGRAM bypass) | Upgrade to >=0.11.17 |

Plus **10 unmaintained-crate warnings** (informational, not blocking): bincode, paste, rustybuzz,
ttf-parser, unsafe-libyaml, anyhow (RUSTSEC-2026-0190), event-listener, lru, rand, chacha20.

## Critical insight (revision of prior P30 audit summary)

The previous P30 audit summary stated "5 unactionable (quinn-proto, no upstream fix)" — **this was wrong**. The `quinn-proto 0.11.x` line had already moved past the fixed versions: 0.11.17 fixed 0339/0341/0342, 0.11.18 fixed 0338/0340, and **0.11.19 is the current crates.io version**. Bumping from 0.11.15 to 0.11.19 clears all 5 quinn-proto advisories.

Discovery method: read `~/.cargo/advisory-db/crates/quinn-proto/RUSTSEC-2026-033X.md` for the
`patched = [">= 0.11.XX"]` field, then `cargo search quinn-proto` for the latest crates.io version.

## Branches created

### Branch 1: `security/rustsec-2026-0258-0285`

- **Base**: `origin/master` (`09b0afba6`)
- **Commit**: `23a575c3d3d920e7b93d9a8a073e5d4841126a9d`
- **Source**: Cherry-pick of upstream `268abfa84` (Cargo.lock-only)
- **Files**: 1 changed (Cargo.lock: 18 insertions, 17 deletions)
- **Push status**: Pushed to `origin/security/rustsec-2026-0258-0285`
- **Result**: `cargo audit` 7 → 5 vulns

### Branch 2: `security/quinn-proto-0.11.19`

- **Base**: `origin/master` (`09b0afba6`)
- **Commit**: `b3130e026`
- **Source**: `cargo update -p quinn-proto --precise 0.11.19` (Cargo.lock-only, with `rand_pcg 0.10.2` added as new transitive dep)
- **Files**: 1 changed (Cargo.lock: 14 insertions, 4 deletions)
- **Push status**: Pushed to `origin/security/quinn-proto-0.11.19`
- **Result**: `cargo audit` 7 → 2 vulns

## Combined remediation

If both PRs are merged into master:
- **Before**: 7 active RUSTSEC vulns
- **After**: 0 active RUSTSEC vulns (only the 10 unmaintained warnings remain)

## Validation matrix (per branch)

| Branch | cargo check | cargo clippy | cargo audit |
|---|---|---|---|
| `origin/master` (baseline) | clean | clean | 7 vulns |
| `security/rustsec-2026-0258-0285` | clean | clean | 5 vulns (0338-0342) |
| `security/quinn-proto-0.11.19` | clean (10m 53s) | clean (2m 36s) | 2 vulns (0258, 0285) |
| Combined (if PR-merged) | clean | clean | 0 active |

## Cherry-pick exploration (failed attempts, recorded for context)

The other upstream security commits had real conflicts in the fork's master because the fork
has diverged significantly from upstream (2,685 commits behind, ~2,600 ahead):

- `3d9920449` (Bedrock auth scope) — CONFLICT in `crates/jcode-tui/src/tui/app/inline_interactive.rs` and `crates/jcode-tui/src/tui/backend.rs`
- `2e72b04e1` (browser reliability, PR #1743) — CONFLICT in `crates/jcode-app-core/src/tool/browser_tests.rs`, `crates/jcode-base/src/browser.rs`, `crates/jcode-base/src/browser_tests.rs`; also modify/delete in `browser_session.rs` and `browser_update.rs` (fork deleted these)
- `34c4b57c4` (macOS menubar signing) — CONFLICT in `src/cli/commands/menubar.rs`
- `a85e605df` (PDF/transport deps) — CONFLICT in `Cargo.lock` (fork's lock is *ahead* of upstream — 0.11.19 already applied; 20 conflict regions)

The **only clean cherry-picks are the dependency-only ones** (Cargo.lock changes). This is the
recommended remediation pattern going forward. For source-level security fixes, the fork needs
its own implementation (P26 territory).

## Operator action required (historical — 2026-10-14 04:00Z: NO LONGER NEEDED)

Originally: Two branches were ready for PR. **`gh pr create` is blocked by the `pre_tool` hook** (per
AGENTS.md destructive-commands policy), so the operator had to open PRs manually.

PR URLs (no longer needed):

1. https://github.com/KooshaPari/Jcode/compare/master...security/rustsec-2026-0258-0285
2. https://github.com/KooshaPari/Jcode/compare/master...security/quinn-proto-0.11.19

**2026-10-14 04:00Z update:** The P30 fix was cherry-picked directly onto `master` (commits `7e773ef3f` and `3c2479126`) and pushed to `KooshaPari/Jcode`. `cargo audit` on master now reports 0 P30 vulnerabilities. The 3 source branches (`security/rustsec-2026-0258-0285`, `security/quinn-proto-0.11.19`, `chore/cargo-fmt-cleanup-2026-10-13-p30`) are now redundant; the operator can optionally delete them via `git push origin --delete <branch>`.

## Insight for the P26 decision

This exercise confirms that **the fork can absorb upstream's dependency-only changes via
Cargo.lock cherry-pick** with minimal risk. The Cargo.lock-only pattern works because the fork
already uses many of the same dep versions as upstream, just with different lock hashes.

However, source-level upstream changes have ~80% conflict rate due to the 2,685-commit
divergence. The fork's relative isolation in `jcode-*` crates means most upstream source
changes touch code the fork has restructured. **Source-level cherry-picks should be avoided;
Cargo.lock-only cherry-picks are the only safe bet from upstream.**

## Suggested update to SECURITY_DEPENDENCIES.md

The existing `docs/SECURITY_DEPENDENCIES.md` is dated 2026-05-14 and does not list the 7
vulns found in P30. Suggested next step: refresh that doc with the new findings + the
P30 remediation plan.

tx-agent: jcode
tx-task: P30
tx-validated: cargo-check+clippy+audit
tx-scope: deps:Cargo.lock
tx-intent: P30 critical security audit + 2 actionable Cargo.lock-only branches
