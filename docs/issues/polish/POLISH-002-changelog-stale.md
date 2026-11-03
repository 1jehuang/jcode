# [POLISH-002] CHANGELOG is stale (last entry v0.85.1-k1.1.0)

**ID:** POLISH-002
**Category:** polish
**Severity:** medium
**File:** CHANGELOG.md
**Status:** open

## Description
`CHANGELOG.md` stops at `v0.85.1-k1.1.0 (2026-09-15)`, but the workspace `Cargo.toml` version is
`0.90.0-k1.0.0` and the runtime reports `v0.93.0`. None of the intervening changes (including the
large decollision rename and the lossy-rebase fix) are recorded, so the changelog is not a reliable
release history.

## Evidence
```
head -8 CHANGELOG.md            # last entry: ## v0.85.1-k1.1.0 (2026-09-15)
grep -m1 "^version" Cargo.toml  # version = "0.90.0-k1.0.0"
```

## Suggested fix
Adopt a release process that appends a CHANGELOG entry per version tag (from conventional commits),
backfill entries for the released versions between 0.85.1 and current, and consider `git-cliff`/
`cargo-release` automation to prevent drift.

## Effort
- M (1-2h)
