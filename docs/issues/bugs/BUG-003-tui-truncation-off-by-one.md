# [BUG-003] TUI truncation off-by-one assertions failing (Ubuntu lib tests)

**ID:** BUG-003
**Category:** bug
**Severity:** medium
**File:** crates/kcode-tui/src/tui/ui_input.rs (CI reported ~L1265/L1284; source has drifted)
**Status:** open

## Description
CI run 38118557038 reported truncation off-by-one failures in the TUI lib tests at
`crates/kcode-tui/src/tui/ui_input.rs` around lines 1265 and 1284. Line numbers have since drifted
(the current file at those lines contains `openai_reset_status_hint` tests). The related
truncation assertions now live near L1361 (`assert!(rendered.len() <= 15)`) and L1380
(`assert_eq!(total_width, 7, "exact cumulative width")`). At least one count/width boundary is
wrong for the expected width, producing a deterministic assertion failure on Ubuntu.

## Evidence
- CI: "Build & Test (ubuntu-latest) :: failure" with 11 lib-test failures including truncation
  off-by-ones at `crates/kcode-tui/src/tui/ui_input.rs:1265/1284`.
- Current assertions:
  ```
  crates/kcode-tui/src/tui/ui_input.rs:1361: assert!(rendered.len() <= 15, "got {}: {rendered}", rendered.len());
  crates/kcode-tui/src/tui/ui_input.rs:1380: assert_eq!(total_width, 7, "exact cumulative width: got {total_width}");
  ```

## Suggested fix
Reproduce with `cargo test -p kcode-tui --lib`, capture the actual vs expected widths, and correct
the truncation arithmetic (width accounting for the ellipsis/marker width). Prefer a property-style
test that asserts `result_width <= max_width` for a matrix of inputs rather than a single magic
constant.

## Effort
- S (< 30m)
