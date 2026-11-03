# [GAP-001] `tool/mod.rs` has many "re-introduce KCode's ..." TODOs; tool/MCP dispatch parity missing

**ID:** GAP-001
**Category:** gap
**Severity:** high
**File:** crates/kcode-app-core/src/tool/mod.rs:623,1682,1695,1701,1707,1717,1724,1728,1737
**Status:** open

## Description
The decollision merge left at least nine distinct `TODO: re-introduce KCode's ...` markers in the
tool module. Each represents fork behavior that existed before the rebase but is currently absent
from `master`: per-session tool-name resolution, session-level MCP dispatch policy, alias-table
resolution, remote-compile feature gate, tool inventory refresh, per-server tool eviction, reconcile-
vs-inventory diffing, reverse alias table, and MCP alias permission semantics. These are functional
regressions relative to the intended fork, not just cleanup.

## Evidence
```
grep -n "TODO: re-introduce" crates/kcode-app-core/src/tool/mod.rs
# 623:  TODO: re-introduce KCode's per-session tool-name resolution...
# 1682: TODO: re-introduce KCode's session-level MCP dispatch policy.
# 1695: TODO: re-introduce KCode's alias table-backed resolution.
# 1701: TODO: re-introduce KCode's remote-compile feature gate.
# 1707: TODO: re-introduce KCode's tool inventory refresh logic.
# 1717: TODO: re-introduce KCode's per-server tool eviction logic.
# 1724: TODO: re-introduce KCode's reconcile-vs-inventory diff logic.
# 1728: TODO: re-introduce KCode's reverse alias table.
# 1737: TODO: re-introduce KCode's MCP alias permission semantics...
```

## Suggested fix
Triage each TODO: either restore the behavior from the pre-rebase KCode branch (source available in
the decollision worktrees/repos listed in AGENTS.md) or explicitly drop it and delete the TODO with
a rationale. Track as a checklist so parity gaps are visible rather than buried in comments.

## Effort
- XL (1+ day)
