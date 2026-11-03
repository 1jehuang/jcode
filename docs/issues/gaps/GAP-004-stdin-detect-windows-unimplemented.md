# [GAP-004] Windows stdin detection unimplemented (`TODO: implement with windows-sys`)

**ID:** GAP-004
**Category:** gap
**Severity:** medium
**File:** crates/kcode-core/src/stdin_detect.rs:367
**Status:** open

## Description
`stdin_detect.rs` has `// TODO: implement with windows-sys crate`, meaning the Windows branch of the
stdin-detection logic is stubbed. On Windows the detector cannot correctly distinguish piped vs
interactive stdin, which affects terminal-launch and input-mode decisions.

## Evidence
```
grep -n "TODO" crates/kcode-core/src/stdin_detect.rs
# 367: // TODO: implement with windows-sys crate
```

## Suggested fix
Implement the Windows path using the `windows-sys` crate (query console mode / pipe type) and add a
test covering both piped and interactive cases under `#[cfg(windows)]`. Until implemented, ensure
the stub returns a documented safe default rather than an incorrect value.

## Effort
- M (1-2h)
