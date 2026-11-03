# [GAP-002] Overnight mode: fork-depth/permission semantics not re-introduced

**ID:** GAP-002
**Category:** gap
**Severity:** medium
**File:** crates/kcode-app-core/src/overnight.rs:215
**Status:** open

## Description
Overnight mode carries a TODO noting that the KCode fork's fork-depth and permission semantics were
not re-introduced after the rebase. Users relying on the fork's overnight autonomy/permission
behavior get upstream defaults instead, a silent behavioral difference for a security-sensitive
("autonomous overnight") feature.

## Evidence
```
grep -n "TODO: re-introduce" crates/kcode-app-core/src/overnight.rs
# 215: /// TODO: re-introduce the KCode fork-depth/permission semantics from the ...
```

## Suggested fix
Restore the fork-depth/permission semantics from the fork branch, or make the omission explicit in
user-facing docs and gate the overnight feature accordingly. Add a test asserting the permission
ceiling overnight runs enforce.

## Effort
- L (half day)
