# [BUG-009] install.ps1 missing `Resolve-KcodePathUpdate` (Jcode naming drift)

**ID:** BUG-009
**Category:** bug
**Severity:** medium
**File:** scripts/install.ps1:296
**Status:** open

## Description
The Setup Friction Eval (Linux installer) and Windows installer checks expect a function named
`Resolve-KcodePathUpdate`, but `scripts/install.ps1` still defines `Resolve-JcodePathUpdate`. The
decollision renamed `jcode` to `kcode` across the codebase but missed the PowerShell installer's
public function name, so the evaluation (and any external caller keying on the Kcode name) cannot
find it.

## Evidence
```
grep -n "function Resolve-" scripts/install.ps1
# 97:  function Resolve-JcodeReleaseTagFromUri(...)
# 296: function Resolve-JcodePathUpdate { ... }
# 509: function Resolve-OptionalPath(...)
# 896: function Resolve-JcodeWindowsArtifact(...)
grep -rIn "KcodePathUpdate" .        # -> no matches anywhere
```
- CI job "Setup Friction Eval (Linux installer) :: failure" — install.ps1 missing
  `Resolve-KcodePathUpdate`.

## Suggested fix
Rename the installer's public functions to the `Kcode` prefix (or add thin `Resolve-KcodePathUpdate`
wrappers) consistently across `install.ps1`, `uninstall.ps1`, and
`scripts/test_windows_launcher_install.ps1` (the test already keys on the Jcode name, so both sides
must move together). Grep for remaining `Jcode`/`Resolve-Jcode` identifiers in `scripts/*.ps1`.

## Effort
- S (< 30m)
