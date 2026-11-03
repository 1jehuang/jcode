# [POLISH-003] Version string mismatch: Cargo `0.90.0-k1.0.0` vs runtime `v0.93.0`

**ID:** POLISH-003
**Category:** polish
**Severity:** medium
**File:** Cargo.toml (workspace version) vs reported runtime version
**Status:** open

## Description
The workspace manifest declares `version = "0.90.0-k1.0.0"` but the running binary reports
`v0.93.0`. Release/packaging tooling (npm SDKs, installers, `--version`) and issue reports key off
these strings, so a mismatch undermines version-based debugging and release automation.

## Evidence
```
grep -m1 "^version" ~/wt-jcode-audit/Cargo.toml   # version = "0.90.0-k1.0.0"
# Session context: Jcode version: v0.93.0 (9948f0e8c)
```

## Suggested fix
Determine the single source of truth for the version (workspace `Cargo.toml` vs a build-time
`git describe`), make `--version` derive from it, and align the SDK/installer version stamps. Add a
CI check that fails if they diverge.

## Effort
- S (< 30m)
