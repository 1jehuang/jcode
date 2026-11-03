# [POLISH-004] 334 `unsafe` occurrences; document and minimize

**ID:** POLISH-004
**Category:** polish
**Severity:** medium
**File:** repository-wide (crates/kcode-*), concentrated in crates/kcode-terminal-launch/src/lib.rs
**Status:** open

## Description
There are 334 `unsafe` occurrences. Only one crate (`kcode-terminal-guard`) opts into a strict policy
(`#![deny(unsafe_code)]`). Most `unsafe` blocks (notably in `kcode-terminal-launch`, including
`std::env::set_var` calls that are unsound under concurrency in newer Rust) lack a `// SAFETY:`
justification, making review and future edition upgrades risky.

## Evidence
```
grep -rIn "unsafe " --include="*.rs" crates src | wc -l                        # -> 334
grep -rIn "deny(unsafe_code)" --include="*.rs" crates src                      # -> only kcode-terminal-guard
sed -n '1195,1200p' crates/kcode-terminal-launch/src/lib.rs
# unsafe { std::env::set_var("KCODE_TERMINAL", value) }
```

## Suggested fix
Require a `#![warn(clippy::undocumented_unsafe_blocks)]` (or `deny`) at the workspace level, add
`// SAFETY:` comments to each block, and migrate `env::set_var`/`remove_var` call sites to a
centralized, serialized wrapper. Extend `#![deny(unsafe_code)]` to crates that need no `unsafe`.

## Effort
- L (half day)
