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

**Recommended action:** merge all three. The ports are self-contained,
non-breaking, and either add a public API (ceb860bf2) or fix pre-existing
clippy lints on master (jcode-sdk). The clippy port is the only one that
resolves a current `cargo clippy -D warnings` failure on master.

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

## After all three are merged

The cleanup-arc-2026-10-13 will be complete:
- 4 P30 source branches deleted
- 3 P25c rescue branches either deleted (d6f2d2091) or ported (e45ddd47e + ceb860bf2)
- 2 stale `chore/cargo-fmt-cleanup-2026-10-13*` branches deleted
- 3 stale worktrees removed
- 0 P30 RUSTSEC vulnerabilities
- 0 P25c structural divergence (other than the dev-isolation feature that doesn't exist in jcode)
- 0 pre-existing clippy lints in jcode-sdk (the bonus port)

Final memory record: this arc freed ~260 MB of disk space and resolved
every P30 + P25c structural issue in the kcode→jcode fork.

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
