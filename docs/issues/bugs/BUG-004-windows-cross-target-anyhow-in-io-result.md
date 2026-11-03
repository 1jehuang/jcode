# [BUG-004] Windows cross-target `E0277`: `?` on `anyhow::Error` inside `io::Result` at reject-dev-home-symlink call site

**ID:** BUG-004
**Category:** bug
**Severity:** high
**File:** reject_dev_home_symlink_path call site (symbol not found by name in current tree; see notes)
**Status:** open

## Description
The Windows cross-target check fails with `E0277`: an `anyhow::Error` is propagated with `?` into a
function returning `io::Result`. `anyhow::Error` does not convert into `io::Error` implicitly, so
the cfg-gated Windows path does not type-check even though host builds pass. The offending call is
at a `reject_dev_home_symlink_path` invocation.

## Evidence
- CI job "Windows Cross-Target Check (Linux) :: failure" — E0277 `?` on `anyhow::Error` in
  `io::Result` at a `reject_dev_home_symlink_path` call site.
- A name search in the current tree returned no matches:
  ```
  grep -rIn "reject_dev_home_symlink_path" --include="*.rs" crates src   # -> no matches
  ```
  The function may have been renamed or lives behind a cfg/feature not visible on this host; this
  needs confirmation on the exact CI SHA.

## Suggested fix
At the call site, map the error explicitly (`reject_dev_home_symlink_path(...).map_err(io::Error::other)?`
or `.map_err(|e| io::Error::new(io::ErrorKind::Other, e))?`). Grep the Windows cfg block to locate
the exact line and confirm the symbol name against the CI sha (6bb97aa0...).

## Effort
- S (< 30m)
