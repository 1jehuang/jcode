# [GAP-003] auto-dream: consolidation prompt is a no-op (never runs the LLM)

**ID:** GAP-003
**Category:** gap
**Severity:** medium
**File:** crates/kcode-auto-dream/src/lib.rs:110
**Status:** open

## Description
`kcode-auto-dream` contains `// TODO: actually run consolidation prompt via LLM`. The module's
central advertised behavior (consolidating memory via an LLM pass) is not implemented; it currently
does not perform the consolidation it is named for.

## Evidence
```
grep -n "TODO" crates/kcode-auto-dream/src/lib.rs
# 110: // TODO: actually run consolidation prompt via LLM
```

## Suggested fix
Wire the consolidation prompt to the provider abstraction (reuse the existing agent/provider client
used elsewhere in the workspace) and return its result, with a test using a mock provider. Until
then, mark the feature experimental so callers do not assume consolidation occurred.

## Effort
- M (1-2h)
