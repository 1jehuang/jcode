# [BUG-006] TUI status-string and account-label drift

**ID:** BUG-006
**Category:** bug
**Severity:** medium
**File:** crates/kcode-tui/src (status/account label assertions in lib tests)
**Status:** open

## Description
Among the 11 Ubuntu lib-test failures in CI run 38118557038 were "status string drift" and "account
label drift" failures. User-visible status text and account labels rendered by the TUI no longer
match the strings the tests expect, which typically means a wording change landed without updating
the golden expectations (or a provider/account label was mutated by the decollision rename).

## Evidence
- CI: "Build & Test (ubuntu-latest) :: failure" — mentions status string drift and account label
  drift among the TUI lib failures.

## Suggested fix
Run `cargo test -p kcode-tui --lib` on Ubuntu (or `cargo test -p kcode-tui --lib -- --nocapture`
on macOS for parity) and reconcile each drifted expectation with the intended copy. Centralize the
status/label strings in one module so tests assert against the same constants the UI renders.

## Effort
- M (1-2h)
