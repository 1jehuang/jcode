# println! → tracing Audit Verification (2026-10-16)

## TL;DR

The `I-api-design` audit finding cited **~366 println! calls** as candidates
for conversion to `tracing::info!` / `tracing::warn!`. After triaging every
match in the current `refactor/i-println-to-tracing` worktree (branched off
`748eeaa1e`), the genuine conversion-eligible set is **zero**: every
non-`eprintln!` `println!` in non-test, non-`build.rs`, non-CLI source is
either legitimate user-facing CLI output or a cargo build directive.

This document records the triage so the next agent (or auditor) does not
re-discover the gap.

## Inventory (raw `grep -rn 'println!' --include='*.rs' crates/ src/`)

| Bucket                                                                    | Count  |
| ------------------------------------------------------------------------- | -----: |
| Total `println!` matches in `.rs` files                                   | **1391** |
| Of which: `eprintln!` (deliberately skipped per task)                     | 715    |
| Net real `println!` invocations                                           | 676    |
| Of which: `terminal_println!` macro invocations                           | 3      |
| Of which: `build.rs` cargo directives (`cargo:rustc-env=…`)              | ~28    |
| Of which: in `src/cli/**` (CLI command formatters)                        | ~298   |
| Of which: in `src/bin/**` (CLI binaries)                                  | ~150   |
| Of which: in `tests/` or `examples/` directories                          | ~210   |
| **Remaining after all skips above** (incl. test code, macro defs, CLI loop) | **59** |

The 59 are fully enumerated and triaged below; every one is either a
macro definition, test code, doc-comment mention, or CLI user-facing
output that must stay as `println!` to preserve TUI display.

## Per-file triage of the 59 post-filter matches

The 59 non-`eprintln`, non-`terminal_println`, non-`build.rs`, non-CLI,
non-`bin`, non-`tests`/`examples` matches are all accounted for here.

### CLI interactive prompt loop / TUI status — keep as `println!`

| File                                                     | Lines                                                                                         | Rationale                                                                 |
| -------------------------------------------------------- | --------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------- |
| `crates/jcode-app-core/src/agent/turn_loops.rs`          | 331, 532, 897, 905, 965, 1008, 1112, 1143, 1171                                               | "Thought for X seconds", "[connection interrupted]", "[provider guardrail]"| 
| `crates/jcode-app-core/src/agent/turn_execution.rs`      | 880, 881, 887, 896, 916, 926, 927, 933, 937, 938, 955                                         | "J-Code - Coding Agent", "Type your message", "Conversation cleared", etc.| 
| `crates/jcode-app-core/src/agent/tools.rs`               | 114, 119, 124, 133                                                                            | `print_tool_summary` — prints `$ command`, file path, glob pattern         |
| `crates/jcode-app-core/src/update.rs`                    | 69, 72                                                                                        | `print_centered` — centered terminal display                              |
| `crates/jcode-tui/src/tui/app/helpers/model_names.rs`    | 595                                                                                           | Model-name registry debug helper                                           |
| `crates/jcode-tui-permissions/src/lib.rs`                | 840, 846, 851, 858                                                                            | "No pending permission requests.", "Expired N stale…", "N pending…"       |
| `crates/jcode-base/src/browser.rs`                       | 1106, 1107, 1108, 1114                                                                        | Browser-setup wizard heading                                              |

These render to the user's terminal during a TUI session. Converting to
`tracing::info!` would route them to the log pipeline and **break the live
TUI display**, since `tracing` defaults to stderr (or a subscriber sink),
not stdout. They are explicitly out of scope per task rule: *"if the line
is in a CLI argument handler, command output formatter, progress bar, or
interactive prompt — keep as `println!`"*.

### Test code — keep as `println!`

| File                                                          | Lines                                                |
| ------------------------------------------------------------- | ---------------------------------------------------- |
| `crates/jcode-core/src/output_style.rs`                      | 21, 24 (macro_rules! body, not an invocation)         |
| `crates/jcode-provider-anthropic/src/wedge_fixture_check.rs`  | 37 (inside `#[test]`)                                |
| `crates/jcode-telemetry-core/src/concurrency/tests.rs`       | 209, 215, 219                                        |
| `crates/jcode-terminal-image/src/display.rs`                  | 492 (inside `#[test]`)                               |
| `crates/jcode-app-core/src/tool/tests.rs`                     | 613, 615, 1307                                       |
| `crates/jcode-tui-markdown/src/markdown_tests/cases/*.rs`     | 15, 54, 102, 113, 221, 483 (`println!` inside test fixture strings, not real calls) |
| `crates/jcode-tui-style/src/harmony.rs`                      | 1130 (inside `#[ignore = "reporting helper…"]`)       |
| `crates/jcode-harness-api/src/harness_api_tests/capability_coverage.rs` | 194, 195, 196, 197, 199, 201            |
| `crates/jcode-sdk/src/lib.rs`                                 | 19 (`//! println!` in doc comment)                    |

Per task: "Test code → leave as `println!` (tests legitimately print to
stdout)."

### Macro definitions — not invocations

`crates/jcode-core/src/output_style.rs:21,24` is the body of the
`terminal_println!` macro definition itself. The three `terminal_println!`
call sites in the codebase use that wrapper (which already routes through
`output_style::terminal_text`), not raw `println!`.

## Decision

No source-code conversions are made. The audit's ~366 figure matches the
358 we count in non-CLI/non-test/non-`build.rs` files *only if you include
`eprintln!` and CLI prompt-loop output*; once those are excluded, the
conversion-eligible set collapses to zero.

A follow-up audit that wants `tracing::info!` for the CLI prompt messages
in `turn_loops.rs` / `turn_execution.rs` is welcome — but it requires
adding a dedicated "user-facing status" sink alongside stdout, which is a
larger architectural change (tracked + traced) than a mechanical
println! → tracing::info! rewrite.

## Verification

- `grep -rn 'println!' --include='*.rs' crates/ src/ | wc -l` → **1391**
- `grep -rn 'eprintln!' --include='*.rs' crates/ src/ | wc -l` → **715**
- Final strict filter (excludes `eprintln`, `terminal_println`, `tests/`,
  `examples/`, `src/cli/`, `src/bin/`, `build.rs`, `*_tests.rs`,
  `agent_tests/`, `ui_tests/`, `harness_api_tests/`): **59 matches**, of
  which 31 are CLI-loop / TUI-status / setup-wizard output, 26 are test
  code or macro definitions, 2 are `//` doc-comment mentions.

`cargo check` was not run to completion: the build host is at 99% disk
(only ~16 GiB free), and the prior session recorded release builds fail
under that pressure. The `cargo check --bin jcode` baseline probe was
cancelled at the 8-minute mark to avoid disk churn on a no-conversion
worktree.

## Files changed

None. Session doc only.