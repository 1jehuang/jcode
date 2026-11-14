# CI/CD Audit - KooshaPari/jcode Fork

**Date:** 2025-09-15 (audited 2026-09-15T03:11Z)
**Repo:** KooshaPari/jcode (fork of 1jehuang/jcode)
**Default branch:** master
**gh auth:** KooshaPari, scopes: delete_repo, gist, project, read:org, repo, workflow

---

## Summary

GitHub Actions is **enabled** on the fork (10 workflows, all `state: active`), but CI is **non-functional**. Every CI run fails immediately with "This run likely failed because of a workflow file issue" and zero jobs started. Additionally, **zero secrets** are configured on the fork, which would block most jobs even if the YAML parsed correctly.

**Verdict: CI/CD is NOT operational on KooshaPari/jcode.**

---

## Workflow Inventory

| # | Workflow | File | State | Trigger | Fork Compatible? |
|---|----------|------|-------|---------|-------------------|
| 1 | CI | ci.yml | active | push/PR to main/master | BROKEN (YAML + secrets) |
| 2 | Release | release.yml | active | push tags v* | NO (missing secrets) |
| 3 | Require Linked Issue | require-issue.yml | active | PR events | YES (no secrets needed) |
| 4 | Announce release on Discord | discord-release.yml | active | release:published / dispatch | NO (missing DISCORD_RELEASE_WEBHOOK) |
| 5 | iOS TestFlight | ios-testflight.yml | active | push to master (ios/) / dispatch | NO (missing Apple secrets) |
| 6 | Publish TypeScript SDK | publish-typescript-sdk.yml | active | workflow_dispatch | CONDITIONAL (needs NPM_TOKEN for publish) |
| 7 | Publish GitHub Packages | publish-github-packages.yml | active | workflow_dispatch | YES (uses GITHUB_TOKEN) |
| 8 | Update weekly stars chart | update-star-history.yml | active | daily cron / dispatch | NO (hardcoded `if: github.repository == '1jehuang/jcode'`) |
| 9 | FreeBSD Smoke | freebsd-smoke.yml | active | push (path) / dispatch / weekly cron | YES (no secrets needed) |
| 10 | Windows Smoke | windows-smoke.yml | active | workflow_dispatch | YES (no secrets needed) |

---

## Root Cause: CI Workflow YAML Issue

**File:** `.github/workflows/ci.yml`
**Problem:** Duplicate `env:` key at the workflow level (lines 3 and 16).

```yaml
name: CI

env:                    # <-- FIRST env block (line 3)
  JCODE_CI: "1"

on:
  push:
    branches: [main, master]
  pull_request:
    branches: [main, master]

concurrency:
  group: ci-${{ github.workflow }}-${{ github.ref }}
  cancel-in-progress: true

env:                    # <-- SECOND env block (line 16, overrides first)
  CARGO_TERM_COLOR: always
  SCCACHE_GHA_ENABLED: "true"
```

YAML duplicate keys cause the second value to override the first. GitHub Actions may silently merge or reject this depending on parser version. The upstream repo (1jehuang/jcode) has the same bug and also shows "workflow file issue" failures on its recent CI runs.

**Impact:** CI run fails immediately with zero jobs started. The `JCODE_CI` environment variable is lost.

**Fix:** Merge both `env:` blocks into one (not in scope for this audit per instructions).

---

## Secrets Status

### Configured on Fork: NONE

`gh secret list -R KooshaPari/jcode` returns empty. Zero secrets are configured.

### Required Secrets (by workflow)

| Secret | Used By | Purpose | Status |
|--------|---------|---------|--------|
| `DEPLOY_KEY` | ci.yml, release.yml | SSH key for checkout with submodules + cargo git deps | MISSING (CRITICAL) |
| `HOMEBREW_DEPLOY_KEY` | release.yml | SSH key to push to homebrew-jcode tap | MISSING |
| `AUR_SSH_KEY` | release.yml | SSH key for AUR package updates | MISSING |
| `AZURE_CLIENT_ID` | release.yml | Azure OIDC for Windows code signing | MISSING |
| `AZURE_TENANT_ID` | release.yml | Azure OIDC for Windows code signing | MISSING |
| `AZURE_SUBSCRIPTION_ID` | release.yml | Azure OIDC for Windows code signing | MISSING |
| `DISCORD_RELEASE_WEBHOOK` | discord-release.yml | Discord webhook for release announcements | MISSING |
| `APPSTORE_API_KEY_P8` | ios-testflight.yml | App Store Connect API key | MISSING |
| `APPSTORE_API_KEY_ID` | ios-testflight.yml | App Store Connect key ID | MISSING |
| `APPSTORE_ISSUER_ID` | ios-testflight.yml | App Store Connect issuer ID | MISSING |
| `WINDOWS_SIGNING_ENDPOINT` (var) | release.yml | Azure signing endpoint | MISSING |
| `WINDOWS_SIGNING_ACCOUNT` (var) | release.yml | Azure signing account name | MISSING |
| `WINDOWS_SIGNING_CERTIFICATE_PROFILE` (var) | release.yml | Azure signing cert profile | MISSING |
| `WINDOWS_SIGNING_REQUIRED` (var) | release.yml | Override to skip signing | MISSING |

### Secrets Already Available

| Secret | Used By | Status |
|--------|---------|--------|
| `GITHUB_TOKEN` | All workflows | Available (auto-provided) |

---

## Workflow-Specific Issues

### ci.yml (CI) -- CRITICAL

**Issues:**
1. Duplicate `env:` key at workflow level (see Root Cause above)
2. `secrets.DEPLOY_KEY` missing -- checkout step with `ssh-key:` will fail
3. `secrets.DEPLOY_KEY` used in `webfactory/ssh-agent` for cargo git dependencies -- all Rust builds need this
4. Jobs affected: `quality`, `build`, `windows-build-test`, `windows-cross-check`

**Even with YAML fix:** Without `DEPLOY_KEY`, jobs that checkout with submodules or need SSH for cargo git deps will fail.

### release.yml (Release)

**Issues:**
1. `secrets.DEPLOY_KEY` missing -- blocks all platform builds
2. `secrets.HOMEBREW_DEPLOY_KEY` missing -- Homebrew formula update step will skip (has `if: env.HOMEBREW_DEPLOY_KEY != ''`)
3. `secrets.AUR_SSH_KEY` missing -- AUR update step will skip (has `if: env.AUR_SSH_KEY != ''`)
4. Azure signing secrets missing -- Windows signing step has `continue-on-error: true` so it won't block
5. Release finalize job references `1jehuang/homebrew-jcode` -- would need updating for fork releases
6. AUR package references `1jehuang/jcode` URLs -- would need updating for fork

**Note:** The release workflow gracefully handles missing optional secrets (HOMEBREW_DEPLOY_KEY, AUR_SSH_KEY) with conditional checks. But `DEPLOY_KEY` is not optional.

### require-issue.yml (Require Linked Issue)

**Status:** Functional. No secrets required. All PRs show `action_required` because the linked-issue check is working as designed.

### discord-release.yml

**Issue:** `secrets.DISCORD_RELEASE_WEBHOOK` missing. The `post_discord_release.py` script will fail without it.

### ios-testflight.yml

**Issues:**
1. `secrets.APPSTORE_API_KEY_P8` missing
2. `secrets.APPSTORE_API_KEY_ID` missing
3. `secrets.APPSTORE_ISSUER_ID` missing
4. This workflow is upstream-specific (Apple Team ID TAS6ARKDN7 is hardcoded)

### update-star-history.yml

**Issue:** Contains `if: github.repository == '1jehuang/jcode'` guard -- will NEVER run on KooshaPari/jcode.

### publish-typescript-sdk.yml

**Issue:** Publishes to npm as `@1jehuang/jcode-sdk` -- would need package name update for fork publishing.

### publish-github-packages.yml, freebsd-smoke.yml, windows-smoke.yml

**Status:** Functional for manual dispatch. No missing secrets required for the core build/test steps.

---

## Recommended Actions

### Priority 1: Fix CI (Blocking)

1. **Merge the duplicate `env:` blocks in `ci.yml`** -- combine into a single env block with all three variables
2. **Create a DEPLOY_KEY secret** on KooshaPari/jcode:
   - Generate an SSH key pair
   - Add the private key as `DEPLOY_KEY` in repo Settings > Secrets
   - Add the public key as a deploy key on 1jehuang/jcode (for submodule/dependency access) OR on KooshaPari/jcode if all deps are self-contained
3. **Verify CI passes** after the above fixes

### Priority 2: Fork-Specific Adjustments (If Releasing from Fork)

4. **Update Homebrew URLs** in release.yml from `1jehuang/jcode` to `KooshaPari/jcode` (or keep upstream URLs if intended)
5. **Update AUR package URLs** similarly
6. **Create fork-specific Homebrew tap** if publishing from fork
7. **Set HOMEBREW_DEPLOY_KEY** if publishing Homebrew formulas from fork

### Priority 3: Optional Workflows

8. **iOS TestFlight:** Only needed if maintaining a forked iOS app. Requires Apple Developer account secrets.
9. **Discord:** Only needed if fork has its own Discord announcements.
10. **Star history:** Either remove the `1jehuang/jcode` guard or replace with `KooshaPari/jcode`.
11. **TypeScript SDK:** Update npm package name if publishing from fork.

### Priority 4: Cost Considerations

12. **Windows runners are billed** -- CI runs on `windows-latest` and `windows-11-arm` consume paid minutes
13. **macOS runners are billed** -- CI runs on `macos-latest` consume paid minutes
14. **Consider** whether Windows/macOS CI jobs are needed for the fork, or if Linux-only CI suffices
15. **FreeBSD VM builds** use QEMU on ubuntu-latest (billed as Linux, not FreeBSD)

---

## CI/CD Flow Diagram

```
PR opened/updated
  -> require-issue.yml (PASS - no secrets needed)
  -> ci.yml (FAIL - YAML issue + missing DEPLOY_KEY)

Push to master
  -> ci.yml (FAIL - same issues)

Tag pushed (v*)
  -> release.yml (BLOCKED - missing DEPLOY_KEY + other secrets)

Manual dispatch
  -> windows-smoke.yml (WORKS)
  -> freebsd-smoke.yml (WORKS)
  -> publish-github-packages.yml (WORKS - uses GITHUB_TOKEN)
  -> ios-testflight.yml (BLOCKED - missing Apple secrets)
  -> publish-typescript-sdk.yml (PARTIAL - publish step needs npm auth)
```

---

*Audit performed as read-only. No workflow files were modified.*
