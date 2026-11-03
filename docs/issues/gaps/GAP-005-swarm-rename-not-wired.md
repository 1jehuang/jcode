# [GAP-005] Swarm rename command is not wired to the coordinator

**ID:** GAP-005
**Category:** gap
**Severity:** low
**File:** crates/kcode-tui/src/tui/app/tui_state.rs:2368,2371
**Status:** open

## Description
The TUI accepts a swarm-rename action but only sets a placeholder status notice; the rename is never
sent to the swarm coordinator. The user-visible command appears to succeed while doing nothing.

## Evidence
```
grep -n "TODO" crates/kcode-tui/src/tui/app/tui_state.rs
# 2368: // TODO: Send rename command to swarm coordinator
# 2371: self.set_status_notice("Rename sent (TODO: wire to swarm coordinator)");
```

## Suggested fix
Emit the rename command through the existing swarm event/channel path and only report success after
the coordinator acknowledges. Add a test that asserts the command is emitted.

## Effort
- M (1-2h)
