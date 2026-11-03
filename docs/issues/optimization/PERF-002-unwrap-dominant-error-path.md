# [PERF-002] `unwrap()`/`panic!` density obscures error-path performance and correctness

**ID:** PERF-002
**Category:** optimization
**Severity:** low
**File:** repository-wide (crates/kcode-*)
**Status:** open

## Description
~4,688 non-test `.unwrap()` sites and ~642 `panic!` sites mean many would-be recoverable error
conditions abort. Beyond the correctness angle (FEAT-001), this hides slow/failing paths instead of
letting them be measured or handled, and makes it hard to reason about tail latency under load.

## Evidence
```
grep -rIn "\.unwrap()" --include="*.rs" crates src | grep -v "/tests/" | wc -l   # -> 4688
grep -rIn "panic!(" --include="*.rs" crates src | wc -l                          # -> 642
```

## Suggested fix
Pair with FEAT-001: convert panic-prone hot paths to returned errors, then measure. Prioritize paths
that run per-message or per-frame.

## Effort
- M (1-2h)
