# [BUG-010] Format job fails: ~107 files have rustfmt diffs

**ID:** BUG-010
**Category:** bug
**Severity:** medium
**File:** repository-wide (`cargo fmt --all`)
**Status:** open

## Description
The "Format" CI job fails because a large number of files are not `rustfmt`-clean (~107 per CI;
`cargo fmt --all -- --check` reports 397 diff regions locally on master at 0737fb93f). This is a
hygiene gate that blocks merges and masks real change noise in diffs.

## Evidence
```
cd ~/wt-jcode-audit && cargo fmt --all -- --check 2>/dev/null | grep -c "^Diff in"   # -> 397
```
- CI job "Format :: failure" (run 38118557038) — 107 files with rustfmt diffs.

## Suggested fix
Run `cargo fmt --all`, commit the result as a single mechanical formatting commit, and add a
pre-commit/lefthook `cargo fmt --check` gate so drift cannot reaccumulate. Ensure the workspace pins
a rustfmt toolchain (`rust-toolchain.toml`) to make formatting deterministic across runners.

## Effort
- S (< 30m)
