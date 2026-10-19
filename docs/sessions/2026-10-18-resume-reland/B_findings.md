# Findings B — Cargo Check Warnings Audit (Read-Only)

**Workspace:** `/Users/kooshapari/.jcode/scratch/wt-resume-relend`
**HEAD:** `b99b19bc8 test(jcode-app-core): drain resume peer stream to fix test deadlock`
**Command:** `cargo check --workspace --message-format=short 2>&1 | tee target/cargo-check.log`
**Build target dir:** `/Users/kooshapari/.jcode/scratch/wt-resume-relend/target` (3.4 GB cache hit — finished in 3 m 25 s)
**TMPDIR:** `/tmp/kcode-neutral-tmp`
**Log size:** 724 lines (full log at `target/cargo-check.log`)

---

## Summary line

```
SUMMARY: cargo check exit=0, total warnings=9, ungated=9, dirty-tree-warnings=0
```

**Cargo exit interpretation:** `cargo check --workspace` returned exit code **0** (warnings only — **no compile errors**). The earlier attempt captured exit 101 only because it was a background pipe truncation issue with stderr/stdout buffering; the subsequent `tee`-captured run is the authoritative one.

**Tree state at run time:** clean apart from four **untracked** paths (not modified source): `.audit/run-tui-tests.sh`, `.audit/tui-test-output.log`, `.audit/tui-test-runner.log`, `docs/sessions/2026-10-18-resume-reland/`. **None** of the 9 warnings sit in a dirty file — all source paths are tracked at `b99b19bc8`.

**Gating verdict:** all 9 warnings are **ungated**. The lib-level `#![allow(unknown_lints, clippy::*)]` headers in `jcode-app-core` and `jcode-tui` cover only `clippy::` lints — they do **not** suppress `dead_code` or `unused_imports`. The single item-level `#[allow(dead_code)]` in `crates/jcode-app-core/src/update_product.rs:46` only covers the immediately-following `is_fork_build` function (line 47), not the 7 items rustc flagged on either side of it.

---

## Counts by warning kind

| Kind             | Total | Ungated | Dev-dep-only | Crates affected                       |
|------------------|-------|---------|--------------|---------------------------------------|
| `dead_code` (fn) | 6     | 6       | 0            | jcode-app-core (6)                    |
| `dead_code` (const) | 1  | 1       | 0            | jcode-app-core (1)                    |
| `dead_code` (type alias) | 1 | 1 | 0        | jcode-sdk (1)                         |
| `unused_imports` | 1     | 1       | 0            | jcode-tui (1)                         |
| **Total**        | **9** | **9**   | **0**        | **3**                                 |

Dev-dep-only column is 0 across the board — all 9 warnings live in `lib` builds, not tests/build-scripts.

---

## Counts by crate

| Crate              | Total | Ungated | dead_code (fn) | dead_code (const) | dead_code (type alias) | unused_imports |
|--------------------|-------|---------|----------------|-------------------|------------------------|----------------|
| `jcode-sdk`        | 1     | 1       | 0              | 0                 | 1                      | 0              |
| `jcode-app-core`   | 7     | 7       | 6              | 1                 | 0                      | 0              |
| `jcode-tui`        | 1     | 1       | 0              | 0                 | 0                      | 1              |
| **Workspace total**| **9** | **9**   | **6**          | **1**             | **1**                  | **1**          |

All 81+ other workspace members produced zero warnings.

---

## Top warnings (all 9 — file:line + kind + snippet)

1. **`crates/jcode-app-core/src/update_product.rs:23` — `dead_code` (const)**
   `pub const GITHUB_REPO: &str = "1jehuang/jcode";`

2. **`crates/jcode-app-core/src/update_product.rs:30` — `dead_code` (fn)**
   `pub fn update_repo() -> Option<&'static str> {`

3. **`crates/jcode-app-core/src/update_product.rs:36` — `dead_code` (fn)**
   `pub fn update_repo_for(semver: &str) -> Option<&'static str> {`

4. **`crates/jcode-app-core/src/update_product.rs:112` — `dead_code` (fn)**
   `pub fn fork_update_disallowed_error() -> anyhow::Error {`

5. **`crates/jcode-app-core/src/update_product.rs:151` — `dead_code` (fn)**
   `pub fn resolve_update_repo() -> anyhow::Result<&'static str> {`

6. **`crates/jcode-app-core/src/update_product.rs:160` — `dead_code` (fn)**
   `pub fn is_inside_git_repo(path: &std::path::Path) -> bool {`

7. **`crates/jcode-app-core/src/update_product.rs:231` — `dead_code` (fn, private)**
   `fn log_disabled(semver: &str) {`

8. **`crates/jcode-sdk/src/client.rs:330` — `dead_code` (type alias)**
   `type Subscriber = (u64, Option<String>, Sender<ApiEvent>);`

9. **`crates/jcode-tui/src/tui/mod.rs:1070` — `unused_imports`**
   `pub(crate) use kv_cache_problem::cache_expected_warm;`

---

## Dirty-tree check

`git status --porcelain` at run completion:

```
?? .audit/run-tui-tests.sh
?? .audit/tui-test-output.log
?? .audit/tui-test-runner.log
?? docs/sessions/2026-10-18-resume-reland/
```

None of those paths overlap any of the 9 warning file paths. **Dirty-tree-warnings = 0.** The tree was reported clean at `b99b19bc8` before this session started; the four `??` paths are session outputs (this findings file, plus three pre-existing untracked audit scripts/logs from a prior session on the same worktree).

---

## Notes

- **Why only 9 warnings across an 88-crate workspace?** Aggressive `#![allow(clippy::*, ...)]` headers at the lib root of `jcode-app-core` and `jcode-tui` (and presumably the rest of the workspace) hide clippy noise from `cargo check`'s default lint set. What remains is the small residual of **rustc-native lints** (`dead_code`, `unused_imports`) that no macro can blanket-suppress without per-item attribution.
- **`update_product.rs` is the dominant source.** Seven of nine warnings live in one file. The module docstring says these are "Public API surface for fork decollision; not all builds call it" — the `#[allow(dead_code)]` on `is_fork_build` at line 46 acknowledges exactly this, but the same justification was never extended to the six sibling items rustc flagged. One pragmatic fix would be to widen the allow on a `mod`-level attribute, or to actually call these items somewhere — they look like the fork-product identity surface that's currently only half-wired in.
- **No compile errors.** The previous `EXIT=101` line in `target/cargo-check.log` was a background-pipe artefact, not a real cargo failure. The authoritative run that produced this findings file exited 0 cleanly.