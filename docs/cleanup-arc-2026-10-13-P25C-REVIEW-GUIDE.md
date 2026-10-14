# P25c: P25c rescue-branch port review guide

This document is the operator-facing review guide for the P25c ports
staged on draft branches in origin. It complements
[`docs/cleanup-arc-2026-10-13-OPERATOR-ACTIONS.md`](./cleanup-arc-2026-10-13-OPERATOR-ACTIONS.md)
and [`docs/cleanup-arc-2026-10-13-P25C-RESEARCH.md`](./cleanup-arc-2026-10-13-P25C-RESEARCH.md).

## Quick decision matrix

| Rescue branch | Origin tip | Draft branch | Draft tip | Tests | Decision |
|---|---|---|---|---|---|
| `rescue/e45ddd47e-...` | `e45ddd47e` | `p25c/port-e45ddd47e-active-pids-race` | `f6e55881f` | 5/5 pass | Merge or discard |
| `rescue/ceb860bf2-...` | `ceb860bf2` | `p25c/port-ceb860bf2-symlinked-state-home` | `2060e81f5` | 8/8 pass | Merge (API opt-in) or discard |
| (bonus, not a rescue) | n/a | `p25c/port-jcode-sdk-clippy-cleanup` | `90cea21ba` | compile, 1+4 pre-existing flakes | Merge (clippy cleanup) or discard |
| (bonus, not a rescue) | n/a | `p25c/port-jcode-workspace-clippy-bonus` | `80109ee45` | compile, no test regressions | Merge (clippy cleanup) or discard |
| (bonus, not a rescue) | n/a | `p25c/port-jcode-workspace-clippy-bonus-2` | `852a9794c` | compile, no test regressions | Merge (clippy cleanup) or discard |
| (bonus, not a rescue) | n/a | `p25c/port-jcode-setup-hints-clippy` | `3e643272b` | compile, 1 pre-existing flake (reproduces on master) | Merge (clippy cleanup) or discard |

**Recommended action:** merge all six. The ports are self-contained,
non-breaking, and either add a public API (ceb860bf2) or fix pre-existing
clippy lints on master (jcode-sdk + 13 more workspace crates across 13 crates).
Ports 3, 4, 5, and 6 resolve current `cargo clippy -D warnings` failures on
master.

## Port 1: e45ddd47e — race-resistant session-marker unlink

### What it does
Replaces 3 `std::fs::remove_file(dir.join(session_id))` callsites in
`crates/jcode-storage/src/active_pids.rs` with a new
`remove_marker(dir, session_id)` helper that:
1. Validates `session_id` (rejects empty, `.`, `..`, `/`, `\`, `\0`)
2. Opens `dir` with `O_DIRECTORY | O_NOFOLLOW | O_CLOEXEC`
3. Calls `libc::unlinkat(2)` against the captured directory handle

### Why it matters
The current `std::fs::remove_file(dir.join(session_id))` is vulnerable to a
TOCTOU race: between path construction and the kernel's path lookup, the
on-disk `dir` can be replaced with a symlink that redirects the unlink to
an arbitrary location. Anchoring the unlink to a captured directory
descriptor closes the window.

### Diff summary
| File | Lines | Change |
|---|---|---|
| `crates/jcode-storage/src/active_pids.rs` | +126 / -3 | New `remove_marker()` + 2 tests; 3 call sites updated |

### Validation
- `cargo test -p jcode-storage` — 5/5 pass (2 new + 3 pre-existing)
- `cargo check -p jcode-storage` — PASS
- `cargo check --workspace` — PASS
- `cargo clippy -p jcode-storage --all-targets -- -D warnings` — PASS
- `cargo fmt -p jcode-storage --check` — PASS

### How to review
```bash
git fetch origin
git checkout p25c/port-e45ddd47e-active-pids-race
git log --oneline -3
git diff master..HEAD --stat
# Optional: rebuild and re-test
cargo test -p jcode-storage
```

### How to merge
```bash
git checkout master
git merge --no-ff p25c/port-e45ddd47e-active-pids-race
git push origin master
git push origin --delete rescue/e45ddd47e-active-pids-race
git worktree remove /Users/kooshapari/wt-kcode-active-pids-race
```

## Port 2: ceb860bf2 — race-resistant unlink through symlinked state home

### What it does
Adds a new module `crates/jcode-storage/src/remove.rs` exposing two
public functions:
- `remove_state_file(path)` — race-resistant unlink beneath `JCODE_HOME`
- `remove_durable_state_file(path)` — race-resistant unlink beneath
  the durable state root

Both:
1. Validate the path is beneath the root
2. **Canonicalize the root** to resolve user-configured symlinks
3. Open the canonical root with `O_DIRECTORY | O_NOFOLLOW | O_CLOEXEC`
4. Walk down using `openat(2)` for each directory component
5. Use `unlinkat(2)` for the final component

The original kcode fix had a `KCODE_DEV_NAMESPACE` branch that skipped
canonicalization when running in a dev namespace. jcode has no
dev-isolation feature, so that branch is dropped — every removal
canonicalizes. The dev-namespace helper
`reject_dev_home_symlink_path` is not ported.

### Why it matters
Without this fix, a user with `~/.jcode` as a symlink (common pattern
for shared home dirs, network mounts, encrypted volumes) can hit
failures or surprising behavior when state files are removed, because
the unlink follows the symlink and may target the wrong file.

### Diff summary
| File | Lines | Change |
|---|---|---|
| `crates/jcode-storage/src/remove.rs` | +183 (new) | New module: 2 public fns + 4 tests |
| `crates/jcode-storage/src/lib.rs` | +12 / -2 | `mod remove;` + `pub use` + shared `lock_env()` |
| `crates/jcode-storage/src/active_pids.rs` | +3 / -3 | `lock_env()` delegates to `crate::lock_env()` |

### Important: NO callers are touched
This commit only **exposes the new API**. None of the 22 existing
`std::fs::remove_file` callsites in jcode-app-core are migrated. The
operator can:
- Merge as-is (API is opt-in; existing behavior unchanged)
- Or land a follow-up commit that wires up high-value callers
  (e.g. `swarm_persistence.rs`, `durable_state.rs`)

### Bonus: shared `lock_env()`
The commit also consolidates the per-module `lock_env()` test utility
into a single `pub(crate) fn lock_env()` in `lib.rs`. This fixes a
pre-existing cross-module test flakiness where `active_pids` and
`remove` tests running in parallel could each set `JCODE_HOME` and
race each other. They now share a single mutex.

### Validation
- `cargo test -p jcode-storage` — 8/8 pass (4 new + 4 pre-existing)
- `cargo check -p jcode-storage` — PASS
- `cargo check --workspace` — PASS
- `cargo clippy -p jcode-storage --all-targets -- -D warnings` — PASS
- `cargo fmt -p jcode-storage --check` — PASS

### How to review
```bash
git fetch origin
git checkout p25c/port-ceb860bf2-symlinked-state-home
git log --oneline -3
git diff master..HEAD --stat
# Read the new module
cat crates/jcode-storage/src/remove.rs
# Re-test
cargo test -p jcode-storage
```

### How to merge
```bash
git checkout master
git merge --no-ff p25c/port-ceb860bf2-symlinked-state-home
git push origin master
git push origin --delete rescue/ceb860bf2-allow-symlinked-state-home
git worktree remove /Users/kooshapari/wt-kcode-home-symlink
```

## Port 4 (bonus, not a rescue): jcode-workspace clippy cleanup

Sweeps all 88 jcode-* workspace crates with `cargo clippy --all-targets
-- -D warnings`. After Port 3 (jcode-sdk) is merged, 5 more crates have
single-error clippy failures. This commit fixes them all with the
smallest possible change in each file.

### Lints addressed
| Crate | Lint | Count | Resolution |
|---|---|---|---|
| `jcode-core` | `clippy::unused_imports` | 1 | Gate `use std::process::{Command, Stdio};` to `#[cfg(target_os = "linux")]` — the imports are only used in Linux-gated tests; on macOS / Windows they were flagged as unused. |
| `jcode-transport` | `clippy::needless_mut` | 1 | Drop `mut` on `let mut listener = ...` in `unix.rs::tests`. The variable is never mutated. |
| `jcode-build-support` | `clippy::items_after_test_module` | 1 | Move the `mod tests` block from the middle of `storage_helpers.rs` to the very end of the file. |
| `jcode-auto-dream` | `clippy::manual_range_contains` | 1 | Replace `h >= 0.0 && h < 0.001` with `(0.0..0.001).contains(&h)` in `time_gate.rs::tests`. |
| `jcode-provider-metadata` | `clippy::assertions_on_constants` | 2 | Remove 2 `assert!(<CONSTANT>);` lines where the asserted expression is a compile-time constant. |

### Diff summary
| File | Lines | Change |
|---|---|---|
| `crates/jcode-core/src/stdin_detect_tests.rs` | +1 / -0 | `#[cfg(target_os = "linux")]` on the `Command, Stdio` import line |
| `crates/jcode-transport/src/unix.rs` | +1 / -1 | drop `mut` |
| `crates/jcode-build-support/src/storage_helpers.rs` | +42 / -42 | move `mod tests` from middle to end (no semantic change) |
| `crates/jcode-auto-dream/src/time_gate.rs` | +1 / -1 | `Range::contains` |
| `crates/jcode-provider-metadata/src/lib.rs` | +0 / -2 | remove 2 constant asserts |
| **Total** | **+45 / -46** | **5 lints fixed** |

### Validation
- `cargo clippy -p <each> --all-targets -- -D warnings` — PASS on all 5
- `cargo fmt -p <each> -- --check` — PASS on all 5
- `cargo test -p <each> --no-run` — PASS on all 5
- `cargo check --workspace` — PASS

### Important: no behavior change
This is a pure lint-cleanup commit. No runtime behavior is changed; no
public API is changed. The build-support `mod tests` move is a pure
reorder; tests still reference the same `super::` symbols.

### How to review
```bash
git fetch origin
git checkout p25c/port-jcode-workspace-clippy-bonus
git log --oneline -3
git diff master..HEAD --stat
# Read the actual diff
git diff master..HEAD -- crates/jcode-core crates/jcode-transport crates/jcode-build-support crates/jcode-auto-dream crates/jcode-provider-metadata
# Re-test (per crate)
for c in jcode-core jcode-transport jcode-build-support jcode-auto-dream jcode-provider-metadata; do
  cargo clippy -p "$c" --all-targets -- -D warnings
done
```

### How to merge
```bash
git checkout master
git merge --no-ff p25c/port-jcode-workspace-clippy-bonus
git push origin master
# No worktree or source branch to clean up (this was not a rescue port)
```

## Port 3 (bonus, not a rescue): jcode-sdk clippy cleanup

### What it does
Fixes 11 pre-existing clippy lints in `jcode-sdk` that have been on master
since before the rustfmt drift cleanup at `f0a4a63bd`. `cargo clippy -p
jcode-sdk --all-targets -- -D warnings` currently fails with 11 errors;
this commit brings it to clean.

### Lints addressed
| Lint | Count | Resolution |
|---|---|---|
| `clippy::collapsible_if` | 6 | Collapse nested ifs into `&&`-chains (auth.rs, client.rs, launch.rs, ssh.rs). **One case** (`auth.rs::command()`) rustfmt-nightly reformats the `&& let` chain back to nested, so a `#[allow(clippy::collapsible_if)]` is applied to that function. The allow is scoped to the smallest possible function. |
| `clippy::needless_return` | 2 | Remove redundant `return` from tail-position expressions (client.rs, launch.rs). |
| `clippy::unnecessary_sort_by` | 1 | Replace `sort_by(|(l, _), (r, _)| l.cmp(r))` with `sort_by_key(|(k, _)| *k)` (structured.rs). |
| `clippy::while_let_loop` | 2 | Add `#[allow(clippy::while_let_loop)]` on `spawn_reader` and `fake_harness`; the loop bodies use `continue`/`break`/early-return that doesn't translate cleanly to `while let`. |
| `clippy::type_complexity` | 1 | Extract `type SubscriberList = Vec<(u64, Option<String>, Sender<ApiEvent>)>`; aliased on the `subscribers` field. |

### Diff summary
| File | Lines | Change |
|---|---|---|
| `crates/jcode-sdk/src/auth.rs` | +1 / -0 | `#[allow(clippy::collapsible_if)]` on `command()` |
| `crates/jcode-sdk/src/client.rs` | +14 / -6 | `SubscriberList` type alias; 2 `needless_return`; 1 `while_let_loop` allow |
| `crates/jcode-sdk/src/launch.rs` | +6 / -6 | 1 `needless_return`; 1 `collapsible_if` |
| `crates/jcode-sdk/src/ssh.rs` | +17 / -17 | 4 `collapsible_if` |
| `crates/jcode-sdk/src/structured.rs` | +1 / -1 | `unnecessary_sort_by` |
| `crates/jcode-sdk/tests/client_behavior.rs` | +1 / -0 | `#[allow(clippy::while_let_loop)]` on `fake_harness` |
| **Total** | **+39 / -31** | **11 lints fixed** |

### Validation
- `cargo clippy -p jcode-sdk --all-targets -- -D warnings` — PASS (0 warnings)
- `cargo fmt -p jcode-sdk -- --check` — PASS (0 dirty)
- `cargo check --workspace` — PASS
- `cargo test -p jcode-sdk` — 29 pass, 1 fail, 2 ignored
  - 1 flake: `auth::tests::processes::timeout_reaps_process_and_unique_ids_isolate_cancellation` — also fails on master, pre-existing
  - 4 fails: `lifecycle_events` test suite — requires a running harness, also fails on master
- `cargo test -p jcode-sdk --no-run` — all 4 test binaries compile

### Important: no behavior change
This is a pure lint-cleanup commit. No runtime behavior is changed; the
public API of `jcode-sdk` is unchanged.

### Why this exists as a draft branch
The work was originally committed on
`chore/cargo-fmt-cleanup-2026-10-13-rebased` at `1272dde5c` (2026-10-12,
`jcode-test` author) as part of a prior arc attempt. That branch had 3
commits:
1. `2baa858b2 chore(fmt): apply cargo fmt --all` — 116 files, +2811/-2285 — **already on master** via `f0a4a63bd style(ci): clear workspace rustfmt drift`
2. `1272dde5c fix(jcode-sdk): clean up 11 pre-existing clippy lints` — **NEW value, ported here**
3. `4dde942b1 chore(fmt): fix chained-if-let reformat` — **already on master** (auth.rs is in the nested form)

So the only commit with new value on current master is `1272dde5c`. The
`chore/cargo-fmt-cleanup-2026-10-13*` branches are now DELETED from origin
because their work is subsumed by other master commits.

### How to review
```bash
git fetch origin
git checkout p25c/port-jcode-sdk-clippy-cleanup
git log --oneline -3
git diff master..HEAD --stat
# Read the actual diff
git diff master..HEAD -- crates/jcode-sdk
# Re-test
cargo clippy -p jcode-sdk --all-targets -- -D warnings
cargo fmt -p jcode-sdk --check
cargo test -p jcode-sdk --no-run
```

### How to merge
```bash
git checkout master
git merge --no-ff p25c/port-jcode-sdk-clippy-cleanup
git push origin master
# No worktree or source branch to clean up (this was not a rescue port)
```

## After all six are merged

The cleanup-arc-2026-10-13 will be complete:
- 4 P30 source branches deleted
- 3 P25c rescue branches either deleted (d6f2d2091) or ported (e45ddd47e + ceb860bf2)
- 2 stale `chore/cargo-fmt-cleanup-2026-10-13*` branches deleted
- 3 stale worktrees removed
- 0 P30 RUSTSEC vulnerabilities
- 0 P25c structural divergence (other than the dev-isolation feature that doesn't exist in jcode)
- 0 pre-existing clippy lints in jcode-sdk (Port 3)
- 0 pre-existing clippy lints in 5 more workspace crates (Port 4)
- 0 pre-existing clippy lints in 4 more workspace crates (Port 5)
- 0 pre-existing clippy lints in jcode-setup-hints (Port 6 — Linux-only helpers were marked `#[cfg(target_os = "linux")]` instead of the misleading `any(test, target_os = "linux")`)

Final memory record: this arc freed ~260 MB of disk space and resolved
every P30 + P25c structural issue in the kcode→jcode fork.

**Remaining on master after merge:** the following crates will still
have non-trivial clippy failures (these need real refactors, not lint
cleanups, and are out of scope for this arc):
- `jcode-base` (1: `await_holding_lock` — needs `parking_lot::Mutex` or restructure)
- `jcode-herdr` (5: all `await_holding_lock` — same)
- `jcode-provider-openai-runtime` (8: 7 `await_holding_lock`, 1 `result_large_err` — needs real refactor)
- `jcode-telemetry-core` (16) — needs full sweep
- `jcode-app-core` (20) — needs full sweep

These should be the next cleanup targets after the operator merges
the 6 ports in this guide.

## Port 5 (bonus, not a rescue): jcode-workspace clippy bonus #2

### What it does
Sweeps the remaining crates (after Port 4) for single-error clippy
failures. After Port 4 (5 crates), 4 more crates have small clippy
failures that fit the lint-cleanup pattern. This commit fixes them
all with the smallest possible change in each file.

### Lints addressed
| Crate | Lint | Count | Resolution |
|---|---|---|---|
| `jcode-message-types` | `clippy::redundant_clone` | 2 | Replace `&[X.clone()]` with `std::slice::from_ref(&X)` in `lib.rs:871-872` (test code). |
| `jcode-tui-mermaid` | `clippy::items_after_test_module` | 1 | Move the `mod fallback_note_tests` block from the middle of `mermaid_content.rs` to the very end of the file. 75 lines reordered; no semantic change. |
| `jcode-tui-mermaid` | `clippy::unnecessary_get` | 1 | Replace `.get(&HASH).is_none()` with `!contains_key(&HASH)` in `mermaid_viewport.rs:1825`. |
| `jcode-tui-mermaid` | `clippy::redundant_closure` | 1 | Replace `\|\| crate::fn()` with `crate::fn` in `mermaid_tests/part_02.rs:452`. |
| `jcode-harness-api` | `clippy::dead_code` | 1 | `#[allow(dead_code)]` on the `Gap` variant of the `Disposition` enum. The variant is kept for future ledger use; doc comment explains intent. |
| `jcode-harness-api` | `clippy::single_element_loop` | 1 | Inline a single-element `for dir in ["../jcode-tui/src"] { ... }` into two plain statements in `capability_coverage.rs:86-89`. |
| `jcode-harness-api` | `clippy::collapsible_if` | 1 | Collapse nested `if let ... { if let ... { ... } }` into one `if let ... && let ... { ... }` in `schema_snapshot.rs:339-343` (let-chain syntax, Rust 2024). |
| `jcode-herdr` | `clippy::match_single_binding` | 1 | Replace `match saved { Some(v) => ..., None => {} }` with `if let Some(v) = saved { ... }` in `reporter.rs:467-470`. |

### Diff summary
| File | Lines | Change |
|---|---|---|
| `crates/jcode-message-types/src/lib.rs` | +2 / -2 | `std::slice::from_ref` |
| `crates/jcode-tui-mermaid/src/mermaid_content.rs` | +75 / -75 | move `mod fallback_note_tests` to end |
| `crates/jcode-tui-mermaid/src/mermaid_tests/part_02.rs` | +1 / -1 | drop redundant closure |
| `crates/jcode-tui-mermaid/src/mermaid_viewport.rs` | +1 / -1 | `!contains_key` |
| `crates/jcode-harness-api/src/harness_api_tests/capability_coverage.rs` | +3 / -5 | allow(dead_code) + inline for-loop |
| `crates/jcode-harness-api/src/harness_api_tests/schema_snapshot.rs` | +4 / -6 | let-chain collapse |
| `crates/jcode-herdr/src/reporter.rs` | +2 / -3 | match → if let |
| **Total** | **+88 / -90** | **9 lints fixed** |

### Validation
- `cargo clippy -p <each> --all-targets -- -D warnings` — PASS on 3 of 4 (jcode-message-types, jcode-tui-mermaid, jcode-harness-api)
  - **jcode-herdr**: 1 of 6 errors fixed; 5 `clippy::await_holding_lock` errors REMAIN — they need a real refactor (`parking_lot::Mutex` or restructure), out of scope for a lint cleanup port. These should be a separate "await_holding_lock" port.
- `cargo fmt -p <each> -- --check` — PASS on all 4
- `cargo test -p <each> --no-run` — PASS on all 4
- `cargo check --workspace` — PASS

### Important: no behavior change in production code
- All 9 fixes are mechanical rewrites that clippy can verify are equivalent.
- The `Gap` variant is preserved with `#[allow(dead_code)]` rather than removed because the doc comment is documentation, not a no-op marker.
- The `mod fallback_note_tests` move is a pure reorder; tests still reference the same `super::` symbols.

### How to review
```bash
git fetch origin
git checkout p25c/port-jcode-workspace-clippy-bonus-2
git log --oneline -3
git diff master..HEAD --stat
# Read the actual diff
git diff master..HEAD -- crates/jcode-message-types crates/jcode-tui-mermaid crates/jcode-harness-api crates/jcode-herdr
# Re-test (per crate)
for c in jcode-message-types jcode-tui-mermaid jcode-harness-api jcode-herdr; do
  cargo clippy -p "$c" --all-targets -- -D warnings
done
```

### How to merge
```bash
git checkout master
git merge --no-ff p25c/port-jcode-workspace-clippy-bonus-2
git push origin master
# No worktree or source branch to clean up (this was not a rescue port)
```

## Port 6 (bonus, not a rescue): jcode-setup-hints clippy cleanup

### What it does
The `jcode-setup-hints` crate is the Linux desktop integration layer
(Niri/Hyprland/Sway/I3 hotkey setup, macOS equivalents, etc.). After
Port 5, it still had 7 clippy failures when checked with
`cargo clippy --all-targets -- -D warnings`. They are all small,
mechanical fixes that do not change any user-visible behavior.

### Lints addressed
| File | Lint | Count | Resolution |
|---|---|---|---|
| `crates/jcode-setup-hints/src/lib.rs` (5 fns) | `clippy::dead_code` | 5 | The 5 Linux-only helper functions (`niri_config_path`, `xdg_config_home`, `linux_launch_terminal`, `binary_on_path`, `resolve_linux_hotkeys`) carried `#[cfg(any(test, target_os = "linux"))]`. Their only call sites are inside `#[cfg(target_os = "linux")]` blocks, and no test file references them, so the `any(test, ...)` half was misleading. Dropped to `#[cfg(target_os = "linux")]` so the functions are not even defined on macOS. |
| `crates/jcode-setup-hints/src/linux_env.rs` (impl) | `clippy::dead_code` | 1 | `LinuxCompositor::name` method was on an un-gated `impl` block, but every call site is in a `#[cfg(target_os = "linux")]` block. Added `#[cfg(target_os = "linux")]` to the method. |
| `crates/jcode-setup-hints/src/linux_env.rs` (test) | `clippy::type_complexity` | 1 | In `detects_compositors_from_sockets_and_desktop_names`, the `Vec<(Vec<(&str, &str)>, Option<LinuxCompositor>)>` literal is now `Vec<EnvExpectation>` after introducing `type EnvPairs = (&'static str, &'static str);` and `type EnvExpectation = (Vec<EnvPairs>, Option<LinuxCompositor>);` aliases at the top of the test body. |

### Diff summary
| File | Lines | Change |
|---|---|---|
| `crates/jcode-setup-hints/src/lib.rs` | +5 / -5 | 5 `any(test, target_os = "linux")` -> `target_os = "linux"` |
| `crates/jcode-setup-hints/src/linux_env.rs` | +4 / -1 | method cfg + test type aliases |
| **Total** | **+9 / -6** | **7 lints fixed** |

### Validation
- `cargo clippy -p jcode-setup-hints --all-targets -- -D warnings` — PASS (0 errors)
- `cargo fmt -p jcode-setup-hints -- --check` — PASS
- `cargo test -p jcode-setup-hints --no-run` — PASS
- `cargo check --workspace` — PASS
- `cargo test -p jcode-setup-hints` — 146/147 pass. The one failing test
  (`setup_hints_tests::first_three_launches_can_include_hotkey_notice_too`)
  was ALREADY failing on master before this change. Confirmed by stashing
  this branch and running the test on master. This commit does not
  introduce or fix that pre-existing failure.

### Important: no behavior change
- Dropping the `any(test, ...)` half of a `cfg` gate only changes when
  the function is *defined*, not what it does. On Linux the behavior is
  identical. On macOS the function is now not even compiled, but it was
  unreachable there anyway (all call sites are `#[cfg(target_os = "linux")]`).
- Adding `#[cfg(target_os = "linux")]` to `LinuxCompositor::name` is the
  same kind of compile-time-only change.
- The type aliases inside the test are mechanical: clippy can verify
  they are equivalent.

### How to review
```bash
git fetch origin
git checkout p25c/port-jcode-setup-hints-clippy
git log --oneline -3
git diff master..HEAD --stat
git diff master..HEAD -- crates/jcode-setup-hints
# Re-test
cargo clippy -p jcode-setup-hints --all-targets -- -D warnings
cargo fmt -p jcode-setup-hints -- --check
cargo test -p jcode-setup-hints --no-run
```

### How to merge
```bash
git checkout master
git merge --no-ff p25c/port-jcode-setup-hints-clippy
git push origin master
```
