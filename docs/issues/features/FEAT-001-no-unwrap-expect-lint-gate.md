# [FEAT-001] No CI/lint gate for `unwrap()`/`expect()` in production code

**ID:** FEAT-001
**Category:** feature
**Severity:** medium
**File:** Cargo.toml / .github/workflows/ci.yml
**Status:** open

## Description
The workspace has ~4,688 `.unwrap()` calls outside obvious test paths and ~642 `panic!` sites, but
no `clippy::unwrap_used`/`expect_used` deny configuration and no CI gate that flags them. This makes
panic-in-production regressions easy to introduce and hard to catch in review.

## Evidence
```
grep -rIn "\.unwrap()" --include="*.rs" crates src | grep -v "/tests/" | wc -l   # -> 4688
grep -rIn "panic!" --include="*.rs" crates src | wc -l                          # -> 642
grep -rIn "unwrap_used\|expect_used" Cargo.toml crates/*/Cargo.toml            # -> none
```

## Suggested fix
Add a workspace `[workspace.lints.clippy]` block with `unwrap_used = "warn"` (or `deny`) for
non-test code, allow it in `#[cfg(test)]` modules, and wire `cargo clippy --all-targets -- -D warnings`
into CI. Ratchet gradually with a per-crate allowlist to avoid a big-bang lockout.

## Effort
- M (1-2h)
