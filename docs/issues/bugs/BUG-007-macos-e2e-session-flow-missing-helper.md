# [BUG-007] macOS e2e `session_flow` tests fail with ENOENT (missing helper binary)

**ID:** BUG-007
**Category:** bug
**Severity:** high
**File:** tests/e2e/session_flow.rs (module registered in tests/e2e/main.rs:16)
**Status:** open

## Description
Three `session_flow` e2e tests on macOS fail with `ENOENT` because a helper binary the tests spawn
is not present in the expected location. The tests invoke an external helper (agent/session helper)
by path, and the CI build does not produce or copy it before running the e2e suite, so the spawn
fails. This makes the macOS e2e job red independently of product behavior.

## Evidence
- CI: "Build & Test (macos-latest) :: failure" — 3 `session_flow` tests hit ENOENT (missing helper
  binary).
- Module wiring:
  ```
  tests/e2e/main.rs:16: mod session_flow;
  ```

## Suggested fix
Identify the helper path the `session_flow` tests resolve (env var or `target/debug/<name>`),
ensure the CI macOS job builds that artifact (or set the env var to an existing path), and add a
clearer failure message that names the missing path. Consider asserting helper existence in a
fixture so the failure is actionable.

## Effort
- M (1-2h)
