# [BUG-002] `crate::shell` module does not exist in kcode-tui; Windows build fails (E0433)

**ID:** BUG-002
**Category:** bug
**Severity:** high
**File:** crates/kcode-tui/src/tui/app/input.rs:94
**Status:** open

## Description
The same defect as BUG-001, at a second call site. `build_input_shell_command` in the TUI's
`input.rs` uses `use crate::shell::detect::Shell;` under `#[cfg(windows)]`. `kcode-tui` contains
`tui/app/run_shell.rs` but no `shell` module at the crate root, so the Windows arm cannot resolve
`crate::shell`. This is a duplicate of the app-core break and would independently fail the Windows
build once BUG-001 is fixed.

## Evidence
```
grep -rIn "crate::shell" --include="*.rs" crates src
# crates/kcode-tui/src/tui/app/input.rs:94:        use crate::shell::detect::Shell;
# crates/kcode-app-core/src/server/client_actions.rs:39:        use crate::shell::detect::Shell;
find crates/kcode-tui/src -path '*shell*'   # -> only tui/app/run_shell.rs
```

## Suggested fix
Apply the same fix as BUG-001 (create/re-export a `shell` module or use a crate-qualified path).
The two call sites should share one helper to prevent recurrence.

## Effort
- S (< 30m)
