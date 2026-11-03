# [BUG-001] `crate::shell` module does not exist in kcode-app-core; Windows build fails (E0433)

**ID:** BUG-001
**Category:** bug
**Severity:** critical
**File:** crates/kcode-app-core/src/server/client_actions.rs:39
**Status:** open

## Description
`build_input_shell_command` unconditionally references `use crate::shell::detect::Shell;`
inside a `#[cfg(windows)]` block. There is no `shell` module in `kcode-app-core`: the crate's
`lib.rs` exports `agent, ambient, ... server, tool, terminal, ...` but no `shell`. On Linux the
reference is cfg-gated away so the build passes; on `--target *-pc-windows-*` the crate fails to
compile with `E0433: failed to resolve: could not find 'shell' in the crate root`.

## Evidence
- CI job "Windows Cross-Target Check (Linux)" and "Build & Test (windows-latest)" both failed on run 38118557038.
- Reproduction (local):
  ```
  grep -rIn "mod shell" crates/kcode-app-core/src/    # -> no matches
  find crates/kcode-app-core/src -name 'shell*'         # -> no matches
  sed -n '37,40p' crates/kcode-app-core/src/server/client_actions.rs
  # fn build_input_shell_command ... #[cfg(windows)] { use crate::shell::detect::Shell; ... }
  ```

## Suggested fix
Either (a) add a `shell` module to `kcode-app-core` (or re-export the relevant detect logic from
the existing `kcode-shell-integration` crate), or (b) point the Windows arm at the correct crate-
qualified path (e.g. `kcode_shell_integration::detect::Shell`) and remove the bogus `crate::shell`
reference. Verify with `cargo check --target x86_64-pc-windows-msvc -p kcode-app-core`.

## Effort
- S (< 30m)
