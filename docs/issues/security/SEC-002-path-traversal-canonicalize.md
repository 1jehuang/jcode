# [SEC-002] Path handling relies on `canonicalize()`; verify traversal containment

**ID:** SEC-002
**Category:** security
**Severity:** medium
**File:** crates/kcode-tui/src/tui/app/navigation.rs:290,293; crates/kcode-selfdev-types/src/desktop.rs:9,38,80; crates/kcode-storage/src/lib.rs:224
**Status:** open

## Description
Several sites canonicalize a caller-influenced path and then join a relative component
(`repository.join(relative).canonicalize()`). Canonicalizing after the join is the correct order,
but the code does not appear to assert the result stays within the intended root. Anywhere
`relative` (or a config path) can contain `..` or a symlink, this risks reading/writing outside the
intended directory — the classic traversal/escape class.

## Evidence
```
crates/kcode-tui/src/tui/app/navigation.rs:290: repository.and_then(|path| path.canonicalize().ok())
crates/kcode-tui/src/tui/app/navigation.rs:293: repository.join(relative).canonicalize()
crates/kcode-selfdev-types/src/desktop.rs:38:  let expected = root.path().canonicalize().unwrap();
```

## Suggested fix
After canonicalizing, assert containment: `resolved.starts_with(canonical_root)`, rejecting `..`
components and symlink escapes with a clear error. Audit `desktop_selfdev.rs` (a self-dev/desktop
tool, security-sensitive) and `storage` for the same pattern. Add a test that a `../` relative path
is rejected.

## Effort
- M (1-2h)
