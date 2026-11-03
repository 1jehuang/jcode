# Jcode/KCode Read-Write Audit — Issue Index

**Repo:** `KooshaPari/Jcode` (a.k.a. `KooshaPari/KCode`)
**Worktree:** `~/wt-jcode-audit` @ `master`
**HEAD audited:** `0737fb93f` (fix(tui-style): restore print_calibration_scores test body lost in rebase)
**Audit date:** 2026-10-11
**Reference CI run:** `38118557038` (conclusion: failure) on `KooshaPari/KCode`

This index lists every issue produced by the comprehensive read-write audit. It covers the six
requested categories: bugs, gaps, features, optimization, polish, security. Each issue file follows
the standard template (`ID / Category / Severity / File / Status`, then Description / Evidence /
Suggested fix / Effort).

> Scope note: this audit is **read-write to `docs/issues/` only**. No source code was modified and
> no commits were made.

---

## Severity summary

| Severity | Count |
|----------|-------|
| critical | 1 |
| high | 6 |
| medium | 19 |
| low | 4 |
| **Total** | **30** |

### Per-category counts

| Category | Prefix | Count |
|----------|--------|-------|
| Bugs | BUG | 11 |
| Gaps | GAP | 5 |
| Features | FEAT | 3 |
| Optimization | PERF | 3 |
| Polish | POLISH | 5 |
| Security | SEC | 3 |
| **Total** | | **30 issues (+ this README = 31 files)** |

> Note: BUG-001/BUG-002 are two call sites of the same defect (counted separately because they are
> distinct files and both must change). FEAT-001 and PERF-002 overlap (unwrap/panic policy) and are
> intentionally cross-referenced rather than merged.

---

## Bugs

| ID | Sev | Title | File |
|----|-----|-------|------|
| [BUG-001](bugs/BUG-001-crate-shell-module-missing-app-core.md) | critical | `crate::shell` missing in kcode-app-core → Windows E0433 | crates/kcode-app-core/src/server/client_actions.rs:39 |
| [BUG-002](bugs/BUG-002-crate-shell-module-missing-tui.md) | high | `crate::shell` missing in kcode-tui → Windows E0433 | crates/kcode-tui/src/tui/app/input.rs:94 |
| [BUG-003](bugs/BUG-003-tui-truncation-off-by-one.md) | medium | TUI truncation off-by-one assertions | crates/kcode-tui/src/tui/ui_input.rs |
| [BUG-004](bugs/BUG-004-windows-cross-target-anyhow-in-io-result.md) | high | Windows cross-target E0277 anyhow-in-io::Result | reject_dev_home_symlink_path call site |
| [BUG-005](bugs/BUG-005-session-picker-onboarding-banner-regression.md) | medium | Session-picker onboarding banner regression | session_picker_tests.rs:1428 |
| [BUG-006](bugs/BUG-006-tui-string-drift-status-account-label.md) | medium | TUI status/account-label string drift | crates/kcode-tui/src |
| [BUG-007](bugs/BUG-007-macos-e2e-session-flow-missing-helper.md) | high | macOS e2e session_flow ENOENT (helper binary) | tests/e2e/session_flow.rs |
| [BUG-008](bugs/BUG-008-typescript-sdk-stale-path-enoent.md) | medium | TS SDK 6 ENOENT stale-path failures | sdk/typescript |
| [BUG-009](bugs/BUG-009-install-ps1-resolve-kcode-path-update-missing.md) | medium | install.ps1 missing Resolve-KcodePathUpdate | scripts/install.ps1:296 |
| [BUG-010](bugs/BUG-010-rustfmt-drift.md) | medium | Format job fails (~107 files) | repo-wide |
| [BUG-011](bugs/BUG-011-master-history-lossy-rebase.md) | high | master contains lossy rebase merge (#33) | git history |

## Gaps

| ID | Sev | Title | File |
|----|-----|-------|------|
| [GAP-001](gaps/GAP-001-tool-mod-mcp-parity-todos.md) | high | `tool/mod.rs` re-introduce-KCode TODOs (9+) | tool/mod.rs:623…1737 |
| [GAP-002](gaps/GAP-002-overnight-fork-depth-permissions.md) | medium | Overnight fork-depth/permission semantics missing | overnight.rs:215 |
| [GAP-003](gaps/GAP-003-auto-dream-no-llm-consolidation.md) | medium | auto-dream consolidation is a no-op | kcode-auto-dream/src/lib.rs:110 |
| [GAP-004](gaps/GAP-004-stdin-detect-windows-unimplemented.md) | medium | Windows stdin detection unimplemented | stdin_detect.rs:367 |
| [GAP-005](gaps/GAP-005-swarm-rename-not-wired.md) | low | Swarm rename not wired to coordinator | tui_state.rs:2368 |

## Features

| ID | Sev | Title | File |
|----|-----|-------|------|
| [FEAT-001](features/FEAT-001-no-unwrap-expect-lint-gate.md) | medium | No unwrap/expect lint+CI gate | Cargo.toml / ci.yml |
| [FEAT-002](features/FEAT-002-error-chain-humanization.md) | medium | Error-chain humanization for users | error surfacing paths |
| [FEAT-003](features/FEAT-003-config-validation.md) | low | Config validation absent | config load path |

## Optimization

| ID | Sev | Title | File |
|----|-----|-------|------|
| [PERF-001](optimization/PERF-001-clone-heavy-hot-paths.md) | medium | Heavy `.clone()` (7,556 sites); hot-path audit | repo-wide |
| [PERF-002](optimization/PERF-002-unwrap-dominant-error-path.md) | low | unwrap/panic density hides error paths | repo-wide |
| [PERF-003](optimization/PERF-003-sync-io-in-async-paths.md) | medium | Audit blocking I/O in async runtime | async call sites |

## Polish

| ID | Sev | Title | File |
|----|-----|-------|------|
| [POLISH-001](polish/POLISH-001-readme-upstream-links.md) | medium | README still links upstream 1jehuang/jcode (22×) | README.md |
| [POLISH-002](polish/POLISH-002-changelog-stale.md) | medium | CHANGELOG stale at v0.85.1-k1.1.0 | CHANGELOG.md |
| [POLISH-003](polish/POLISH-003-version-string-mismatch.md) | medium | Version mismatch Cargo 0.90.0-k1.0.0 vs runtime v0.93.0 | Cargo.toml |
| [POLISH-004](polish/POLISH-004-unsafe-blocks-undocumented.md) | medium | 334 `unsafe` occurrences, undocumented | kcode-terminal-launch/src/lib.rs |
| [POLISH-005](polish/POLISH-005-panic-density.md) | medium | 642 `panic!` sites, several in server/runtime | app-core/src/server |

## Security

| ID | Sev | Title | File |
|----|-----|-------|------|
| [SEC-001](security/SEC-001-shell-command-construction.md) | high | Shell command construction via sh -c / pwsh -Command | ssh.rs:718,857; client_actions.rs:36 |
| [SEC-002](security/SEC-002-path-traversal-canonicalize.md) | medium | Path traversal containment after canonicalize | navigation.rs:290; desktop.rs:38 |
| [SEC-003](security/SEC-003-secret-logging-review.md) | low | Verify no secrets logged (preventive) | logging call sites |

---

## Raw metrics gathered

| Metric | Value |
|--------|-------|
| Workspace crates (`crates/`) | 96 |
| TODO/FIXME/XXX/HACK/BUG comments | 103 |
| `dbg!` | 0 |
| `eprintln!` | 757 |
| `.unwrap()` (non-test heuristic) | 4,688 |
| `panic!` | 642 |
| `unsafe` occurrences | 334 |
| `.clone()` | 7,556 |
| command-spawn sites (`Command::new`/`process::Command`) | 369 |
| `cargo fmt --all -- --check` diff regions | 397 |
| `--dry-run`/`dry_run` references | 55 |

## CI failing jobs (run 38118557038)

Build & Test (macos-latest) · Setup Friction Eval (Linux installer) · Build & Test (ubuntu-latest) ·
TypeScript SDK · Windows Cross-Target Check (Linux) · Quality Guardrails · Build & Test
(windows-latest) · Format — all **failure**.

---

## Suggested triage order

1. **BUG-001 / BUG-002** — Windows build is structurally broken (two `crate::shell` refs). Unblocks
   the Windows and cross-target jobs.
2. **BUG-004** — the other Windows cross-target type error.
3. **BUG-011** — verify the lossy rebase is fully healed (`cargo check --workspace`).
4. **BUG-010** — one `cargo fmt --all` commit clears the Format gate and reduces diff noise.
5. **BUG-003 / BUG-005 / BUG-006** — clear the Ubuntu TUI lib failures.
6. **BUG-007 / BUG-008 / BUG-009** — CI plumbing (macOS e2e helper, TS SDK path, installer name).
7. **SEC-001 / SEC-002** — trust-boundary reviews.
8. **GAP-001** — the parity checklist (largest outstanding functional debt).
9. Everything else as polish.
