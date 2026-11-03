# [PERF-003] Audit for blocking I/O on the async runtime

**ID:** PERF-003
**Category:** optimization
**Severity:** medium
**File:** crates/kcode-*/src (async call sites)
**Status:** open

## Description
The codebase performs many `std::fs` and `std::process::Command` operations (369 command-spawn
sites). Any of these invoked from an async task without `spawn_blocking`/`tokio::fs` will block the
runtime thread and stall concurrent sessions. This is a review candidate; the audit did not run a
runtime profile to confirm specific blockers.

## Evidence
```
grep -rInE "Command::new|process::Command" --include="*.rs" crates src | grep -v test | wc -l   # -> 369
```

## Suggested fix
Static-review async fns that call `std::process::Command::output/status` or `std::fs` directly;
wrap them in `tokio::task::spawn_blocking` or switch to async equivalents. Confirm with a stress
test running two concurrent sessions.

## Effort
- M (1-2h)
