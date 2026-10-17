# API Design Consistency Audit (KCode fork, 2026-10-17)

## Summary

- **Dominant patterns**: `anyhow` for errors (354 files, 40 crates), `serde` for serialization (640+722 derives), custom `jcode-logging` crate for structured file logging, TOML+env for config.
- **Inconsistencies found**: 6 categories with meaningful divergence; 2 are high-severity.

## Inconsistencies found

| # | Category | Pattern A | Pattern B | Severity | Recommendation |
|---|----------|-----------|-----------|----------|----------------|
| 1 | **Logging** | `println!`/`eprintln!` in 50 production files (366 calls) | `jcode-logging` crate (custom structured logger to `~/.jcode/logs/`) | **HIGH** | Replace all `println!`/`eprintln!` in non-CLI-output code with `jcode-logging` calls. Keep `println!` only for intentional user-facing stdout (CLI help, version). |
| 2 | **Logging framework** | `tracing` crate (3 total calls across 2 files) | `jcode-logging` custom crate (file-based, no tracing subscriber) | **HIGH** | Pick one. Either adopt `tracing` with a file subscriber (gains structured spans, `#[instrument]`), or remove the 3 `tracing` calls and standardize on `jcode-logging`. Current state is neither. |
| 3 | **Error types** | `anyhow::Result` everywhere (354 files, 40 crates) | `thiserror` (1 file: `jcode-agent-runtime`) + 10 manual `impl Error`/`impl Display` | Medium | Low-level/library crates (`jcode-sdk`, `jcode-protocol`, `jcode-provider-core`) should define typed errors with `thiserror`. Application crates (`jcode-app-core`, `jcode-tui`) may continue using `anyhow`. The 10 manual impls should migrate to `thiserror`. |
| 4 | **Serde field naming** | `snake_case` (60 `rename_all` attrs) | `camelCase` (28 attrs, mostly in `jcode-provider-antigravity`) + `lowercase` (25 attrs, mostly in `jcode-config-types`) | Medium | `snake_case` is the Rust default and dominant pattern. `camelCase` in antigravity is likely for wire-format compatibility (API expects camelCase). `lowercase` in config-types is for TOML enum variants. Document these as intentional per-crate policies rather than normalizing — but add `#[serde(rename_all = "snake_case")]` explicitly to protocol/wire types for clarity. |
| 5 | **CLI argument parsing** | `clap` referenced in only 2 files (as comments, not derives) | TUI handles input via `crossterm`/ratatui key events | Low | The binary entrypoint (`src/main.rs`, `src/cli/`) likely uses `clap` — the 2 references are in comments explaining why clap rejects certain values. Verify `Cargo.toml` for clap dependency. This is likely not a real inconsistency. |
| 6 | **HTTP API response shapes** | WebSocket JSON messages: `{"type": "error", "id": ..., "message": ...}` | No traditional REST wrapper (`{ "success": true, "data": ... }`) | Low | Not a real inconsistency — KCode uses a WebSocket protocol (harness API), not REST. The `{"type": ..., "id": ...}` envelope is consistent within that protocol. No action needed unless REST endpoints are added later. |

## Top-5 highest-leverage normalizations

1. **Replace `println!`/`eprintln!` in production code with `jcode-logging`** — 366 calls across 50 non-test files. This is the single largest source of inconsistency. Production code writing to stdout/stderr bypasses log rotation, structured context (session, provider, model), and rate limiting that `jcode-logging` provides. Impact: observability, log hygiene, production debugging.

2. **Resolve the `tracing` vs `jcode-logging` split** — 3 `tracing` calls exist but no tracing subscriber is configured. Either wire a `tracing-subscriber` that writes to the `jcode-logging` backend (unifying both), or remove the 3 calls. Current state is dead code.

3. **Migrate manual `impl Error`/`impl Display` to `thiserror`** — 10 files hand-write error types. `jcode-sdk/src/errors.rs`, `jcode-sdk/src/structured.rs`, `jcode-harness-api/src/client.rs`, `jcode-plan/src/dag/mod.rs`, `jcode-provider-core/src/retry_after.rs`, `jcode-base/src/subscription_api.rs`, `jcode-app-core/src/tool/discover.rs`, `jcode-base/src/sidecar.rs`, `jcode-base/src/provider/models_catalog.rs`, `jcode-base/src/auth/transfer.rs`. Each is a small, isolated change.

4. **Document per-crate `serde(rename_all)` policy** — `snake_case` (60), `camelCase` (28), `lowercase` (25), `kebab-case` (2). The divergences are intentional (wire format, TOML compat) but undocumented. Add a one-line comment to each `rename_all` explaining why.

5. **Define error type boundaries for library crates** — `jcode-sdk`, `jcode-protocol`, `jcode-provider-core`, and `jcode-harness-api` are library crates that should expose typed errors (via `thiserror`), not `anyhow`. This is a medium-term structural improvement that makes the public API self-documenting.

## Forward recommendation

Standardize logging first — it is the highest-volume, lowest-effort normalization with immediate observability payoff. The `println!`-to-`jcode-logging` migration is mechanical (search-and-replace with context-aware log level selection) and can be done crate-by-crate. Error type normalization is the second priority but requires more design thought per crate.