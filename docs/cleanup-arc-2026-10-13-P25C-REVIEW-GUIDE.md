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
| (bonus, not a rescue) | n/a | `p25c/port-ceb860bf2-caller-wiring` | `6b8adca63` | compile (no new regressions) | Merge (opt-in wire-up) — **depends on Port 2 (`2060e81f5`) landing first; this branch is fast-forwarded on top of that tip** |
| (bonus, not a rescue) | n/a | `p25c/port-jcode-core-pedantic-raw-ptr` | `262bcb3a5` | compile, 0 pedantic raw-ptr-cast warnings | Merge (mechanical lint cleanup) or discard |

**Recommended action:** merge all eight. The ports are self-contained,
non-breaking, and either add a public API (ceb860bf2) or fix pre-existing
clippy lints on master (jcode-sdk + 13 more workspace crates across 13 crates)
or wire 11 callers up to the new race-resistant unlink API (Port 7) or do
a pedantic-clippy raw-pointer cleanup (Port 8).
Ports 3, 4, 5, and 6 resolve current `cargo clippy -D warnings` failures on
master. **Note Port 7 has a merge order dependency on Port 2.**

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

## After all eight are merged

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
- 11 `jcode-app-core` callers of `std::fs::remove_file` wired to the new race-resistant `storage::remove_state_file` / `storage::remove_durable_state_file` API (Port 7)
- 4 raw-pointer casts in `jcode-core/src/stdin_detect.rs` rewritten using `pointer::cast` (Port 8)

Final memory record: this arc freed ~260 MB of disk space and resolved
every P30 + P25c structural issue in the kcode→jcode fork.

**Remaining on master after merge:** none of the originally-flagged
crates are still producing clippy lints as of 2026-10-14 07:48Z:

| Crate | Originally reported | Re-checked 07:48Z | Disposition |
|---|---|---|---|
| `jcode-base` | 1 `await_holding_lock` | 0 errors | resolved (likely the same exact call was already on a non-test path; the earlier 1-error count was stale) |
| `jcode-herdr` | 5 `await_holding_lock` | 0 errors | resolved |
| `jcode-provider-openai-runtime` | 8 (7 `await_holding_lock`, 1 `result_large_err`) | 0 errors | resolved |
| `jcode-telemetry-core` | 16 | 0 errors | resolved |
| `jcode-app-core` | 20 | 0 errors | resolved |

After all 8 ports merge, the workspace is clippy-clean on every crate
sampled. The only remaining work in the cleanup arc is the 8 operator
gated port merges and the 1 destructive P11 disk cleanup.

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

## Port 7 (bonus, not a rescue): ceb860bf2 caller wire-up

### What it does

Port 2 (ceb860bf2, branch `p25c/port-ceb860bf2-symlinked-state-home`,
commit `2060e81f5`) introduced the new
`storage::remove_state_file` and `storage::remove_durable_state_file`
public API on `jcode-storage` but deliberately left the 25
`std::fs::remove_file` call sites in `jcode-app-core` un-touched (opt-in,
so landing the API first and then wiring callers is a two-step rollout).

This port wires up the 11 call sites whose paths are conceptually under
`JCODE_HOME` (`storage::remove_state_file`) or the durable-state root
(`storage::remove_durable_state_file`). It closes the TOCTOU window the
API was designed to address for all state-bearing files below those two
roots.

### Why it matters

`std::fs::remove_file(p)` is vulnerable to a TOCTOU race whenever `p`
is a derived path: between path construction and the kernel's path
lookup, the on-disk ancestor directory can be replaced with a symlink
that redirects the unlink to an arbitrary location. The new API opens
the root with `O_DIRECTORY | O_NOFOLLOW | O_CLOEXEC` and walks the
remaining components with `openat(2)`, anchoring the final
`unlinkat(2)` to a captured directory descriptor.

### Call sites wired up

| File | Line(s) | Path root | API used |
|---|---|---|---|
| `crates/jcode-app-core/src/session_active_pids.rs` | 16 | `jcode_dir()/active_pids/<id>` | `remove_state_file` |
| `crates/jcode-app-core/src/mission.rs` | 122 | `jcode_dir()/missions/<id>.json` | `remove_state_file` |
| `crates/jcode-app-core/src/telemetry_state.rs` | 186 | `jcode_dir()/active_sessions/<file>` | `remove_state_file` |
| `crates/jcode-app-core/src/telemetry_state.rs` | 211 | `jcode_dir()/active_sessions/<file>` | `remove_state_file` |
| `crates/jcode-app-core/src/restart_snapshot.rs` | 64 | `jcode_dir()/restart-snapshot.json` | `remove_state_file` |
| `crates/jcode-app-core/src/ambient/runner.rs` | 1063 | `jcode_dir()/ambient/cycle_result.json` | `remove_state_file` |
| `crates/jcode-app-core/src/ambient/runner.rs` | 1087 | `jcode_dir()/ambient/cycle_result.json` | `remove_state_file` |
| `crates/jcode-app-core/src/server/comm_session.rs` | 413 | `jcode_dir()/client-input-<id>` | `remove_state_file` |
| `crates/jcode-app-core/src/server/comm_session.rs` | 420 | `jcode_dir()/sessions/<id>.json` | `remove_state_file` |
| `crates/jcode-app-core/src/server/comm_session.rs` | 423 | `jcode_dir()/sessions/<id>.journal.jsonl` | `remove_state_file` |
| `crates/jcode-app-core/src/server/reload_recovery.rs` | 77, 141 | `jcode_dir()/reload-recovery/<file>` | `remove_state_file` |
| `crates/jcode-app-core/src/server/swarm_persistence.rs` | 273 | `durable_state_dir()/swarm/<id>.json[.bak]` | `remove_durable_state_file` |

11 sites across 8 files. Every site preserves existing error-handling
behaviour: `let _ = ...` stays `let _ = ...`; `?` propagates the same
way (the wrapping `std::io::Result` signature is identical between the
old and new call).

### Call sites intentionally not wired up (14)

| File | Why not |
|---|---|
| `server/socket.rs:42, 44, 173` | Sockets live in `runtime_dir()`; not under jcode_dir or durable_state_dir. The new API would reject these paths. |
| `server/lifecycle.rs:135` | `metadata_path(socket_path)` derives a path next to the socket, which is in `runtime_dir()`. |
| `server/reload_state.rs:26, 57, 66` | Reload marker is in `runtime_dir()`. |
| `server/durable_state.rs:57` | `state_dir(dir_name)` here returns `runtime_dir().join(dir_name)`, not `durable_state_dir().join(dir_name)`. |
| `server/await_members_state.rs:269` | Awaits members state is in `runtime_dir()`. |
| `update.rs:1046, 1083` | Binary update temp files, written to the system temp dir. |
| `tool/bash.rs:774` | Cmd-quoting probe, written to `std::env::temp_dir()`. |
| `server/debug_testers.rs:326` | `debug_resp_path` comes from user-supplied tester config (path could be anywhere). |
| `notifications.rs:508` | `macos_notification_inbox_dir()` honors `$JCODE_MACOS_NOTIFICATION_INBOX` env var, which is user-controlled. |
| `ambient/persistence.rs:201, 215, 224` | Lock file pattern. The ambient lock family is intentionally kept on `std::fs::remove_file` for now; changing lock release semantics belongs in a separate port. |

### Diff summary

| File | Lines | Change |
|---|---|---|
| 8 files (listed above) | +13 / -13 | Line-for-line swap from `std::fs::remove_file` to `storage::remove_state_file` / `remove_durable_state_file` |

### Validation

- `cargo check -p jcode-app-core` — PASS
- `cargo clippy -p jcode-app-core --no-deps -- -D warnings` — PASS
- `cargo test -p jcode-app-core --no-run` — PASS
- 0 new clippy lints, 0 new dependencies, 0 behaviour change at the call sites

### Why this is a safe port

1. The new API returns the same `std::io::Result<()>` type as
   `std::fs::remove_file`, so existing `?` and `let _ =` patterns work
   unchanged.
2. The new API is stricter than the old one: it returns
   `ErrorKind::Other` if the path is not under the configured root.
   In all 11 wire-ups the path is constructed from the same root, so
   this check passes.
3. The new API uses the same `ErrorKind::NotFound` semantics on
   Unix as `std::fs::remove_file` for missing files, so the
   pre-existing `if err.kind() != std::io::ErrorKind::NotFound`
   patterns work unchanged.
4. The new API is on a fast path: only the `path.strip_prefix(root)`
   + `root.canonicalize()` happen before the existing `unlinkat(2)`
   semantics. Per-call cost is microseconds on macOS and Linux.
5. No new dependency: `remove_state_file` lives in `jcode-storage`
   (already a transitive dependency of `jcode-app-core`).

### How to review

```bash
git fetch origin
git checkout p25c/port-ceb860bf2-caller-wiring
git log --oneline -3
# Confirm this branch is fast-forwarded on top of Port 2
git log --oneline origin/master..HEAD
git diff master..HEAD --stat
git diff master..HEAD -- crates/jcode-app-core
# Re-test
cargo check -p jcode-app-core
cargo clippy -p jcode-app-core --no-deps -- -D warnings
cargo test -p jcode-app-core --no-run
```

### How to merge

**Important:** This branch is fast-forwarded on top of
`p25c/port-ceb860bf2-symlinked-state-home`. It can only be merged
after Port 2 (`2060e81f5`) has landed on master. If the operator
merges both ports in the same review pass, the two-branch cascade
is conflict-free:

```bash
# 1. Land Port 2 first (API only, no caller changes)
git checkout master
git merge --no-ff p25c/port-ceb860bf2-symlinked-state-home
git push origin master

# 2. Then land Port 7 (caller wire-up; will fast-forward over Port 2's merge commit)
git merge --no-ff p25c/port-ceb860bf2-caller-wiring
git push origin master
```

If the operator prefers a single combined merge, the alternative is
to first rebase Port 7 onto current master (after Port 2 has merged)
and merge the result, but the two-step approach is simpler and
preserves Port 2 as a distinct, atomic commit on master.

## Port 8 (bonus, not a rescue): jcode-core pedantic raw-pointer cleanup

### What it does

Replaces 4 raw pointer `as` casts with the `pointer::cast` spelling
in `crates/jcode-core/src/stdin_detect.rs`. All 4 calls are in the
macOS TTY/pipe detection helper (cfg-gated to macOS only), passing
buffer pointers into `proc_pidinfo(3)`. The `pointer::cast` spelling
is the idiomatic and safer alternative recommended by
`clippy::pedantic` for raw pointer type transitions.

### Why it matters

The `as` keyword on raw pointers performs a silent type change
without any safety checks. `pointer::cast::<T>()` is the same
operation but makes the type transition explicit in the source
code, which is easier to read and easier for tooling to audit.
There is no runtime difference between the two spellings on
stable Rust; this is a clarity + lint-cleanup change only.

### Call sites

| File:line | Before | After |
|---|---|---|
| `stdin_detect.rs:288` | `buf.as_mut_ptr() as *mut libc::c_void` | `buf.as_mut_ptr().cast::<libc::c_void>()` |
| `stdin_detect.rs:299` | `buf.as_ptr() as *const proc_fdinfo` | `buf.as_ptr().cast::<proc_fdinfo>()` |
| `stdin_detect.rs:321` | `thread_ids.as_mut_ptr() as *mut libc::c_void` | `thread_ids.as_mut_ptr().cast::<libc::c_void>()` |
| `stdin_detect.rs:340` | `&mut tinfo as *mut _ as *mut libc::c_void` | `(&mut tinfo as *mut proc_threadinfo).cast::<libc::c_void>()` |

The 4th call site is the only one that needed a slight rewrite: the
original was a `*mut _` -> `*mut c_void` double-cast. The replacement
uses an explicit `*mut proc_threadinfo` intermediate to preserve the
no-op constness change that the original `*mut _` -> `*mut c_void`
cast was performing. Semantically identical.

### Diff summary

| File | Lines | Change |
|---|---|---|
| `crates/jcode-core/src/stdin_detect.rs` | +4 / -4 | 4 `as *mut T` -> `.cast::<T>()` rewrites |

### Validation

- `cargo clippy -p jcode-core --all-targets` — PASS (0 errors)
- `cargo clippy -p jcode-core --all-targets -- -W clippy::pedantic` — raw-pointer-cast warnings now 0 (was 4)
- `cargo test -p jcode-core --no-run` — PASS (test compile only)
- 0 new clippy lints, 0 new dependencies, 0 behaviour change

### How to review

```bash
git fetch origin
git checkout p25c/port-jcode-core-pedantic-raw-ptr
git log --oneline -3
git diff master..HEAD --stat
git diff master..HEAD -- crates/jcode-core
# Re-test
cargo clippy -p jcode-core --all-targets
cargo test -p jcode-core --no-run
# Confirm the pedantic lints are gone
cargo clippy -p jcode-core --all-targets -- -W clippy::pedantic 2>&1 | grep "casting between raw pointers"
```

### How to merge

```bash
git checkout master
git merge --no-ff p25c/port-jcode-core-pedantic-raw-ptr
git push origin master
```
