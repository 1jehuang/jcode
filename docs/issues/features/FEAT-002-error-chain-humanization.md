# [FEAT-002] Error-chain humanization for end users

**ID:** FEAT-002
**Category:** feature
**Severity:** medium
**File:** crates/kcode-*/src (error surfacing paths)
**Status:** open

## Description
There is extensive use of `anyhow`/`io` errors (see BUG-004) but no demonstrated layer that
humanizes error chains for CLI/TUI users (context-preserving, actionable messages without raw
source-chain dumps). Errors currently propagate close to their origin, so users can see internal
types/paths.

## Evidence
- BUG-004 shows a raw `anyhow::Error` reaching an `io::Result` boundary.
- `grep -rIl "use tracing"` and `grep -rIl "use log::"` both returned 0 files; there is no obvious
  centralized structured error/log formatting layer.

## Suggested fix
Introduce a single `render_error_chain(&dyn std::error::Error) -> String` used at the CLI/TUI
boundary, with `{:#}`-style chain rendering plus a "hint" line for common failures (auth, network,
missing binary). Add snapshot tests for representative errors.

## Effort
- M (1-2h)
