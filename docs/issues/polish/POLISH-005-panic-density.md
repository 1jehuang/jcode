# [POLISH-005] 642 `panic!` sites; several in server/runtime code

**ID:** POLISH-005
**Category:** polish
**Severity:** medium
**File:** crates/kcode-app-core/src/server/*.rs (and others)
**Status:** open

## Description
642 `panic!` calls exist, some in server/runtime paths rather than tests (e.g. `runtime.rs:480
panic!("task spawned after shutdown")`, `debug_swarm_write.rs`, `debug_server_state.rs`). In a
long-lived server, a panic can abort a task or the process; these should be errors or explicit
invariants documented as unreachable.

## Evidence
```
grep -rIn "panic!(" --include="*.rs" crates src | wc -l   # -> 642
grep -rIn "panic!(" crates/kcode-app-core/src/server/*.rs | head
# server/runtime.rs:480:  .spawn(|_| async { panic!("task spawned after shutdown") })
```

## Suggested fix
Triage production `panic!` sites: convert recoverable ones to typed errors, and for true invariants
use `unreachable!`/`debug_assert!` with a comment, or a `#[cold]` abort helper. Enforce via
`clippy::panic` in a scoped lint (FEAT-001).

## Effort
- M (1-2h)
