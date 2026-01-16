# Repository Guidelines

## Project Structure & Module Organization
- `src/` is the core library and CLI entry point (`src/main.rs`). Key areas include `src/agent.rs`, `src/provider/`, `src/mcp/`, `src/tool/`, and `src/tui/`.
- `src/bin/` holds auxiliary binaries: `test_api.rs` (Claude SDK smoke test) and `harness.rs` (tool harness).
- `tests/e2e/` contains integration tests and mock providers.
- `scripts/` includes helper scripts like `agent_trace.sh` and `test_e2e.sh`.
- Docs live in `README.md`, `OAUTH.md`, and `CLAUDE.md`.

## Build, Test, and Development Commands
- `cargo install --path .`: install the local CLI.
- `cargo build --release`: rebuild the release binary; `jcode` is symlinked to `target/release/jcode`.
- `jcode`: launch the TUI.
- `jcode serve` / `jcode connect`: start the daemon and attach a client.
- `cargo test`: run unit + integration tests.
- `cargo test --test e2e`: run only end-to-end tests.
- `cargo run --bin test_api`: Claude Agent SDK smoke test.
- `cargo run --bin jcode-harness -- --include-network`: exercise tool harness with optional network calls.
- `scripts/agent_trace.sh`: end-to-end agent trace (set `JCODE_PROVIDER=openai|claude`).

## Coding Style & Naming Conventions
- Rust 2021 style; format with `cargo fmt`.
- Files/modules use `snake_case`; types/traits use `CamelCase`; functions use `snake_case`.
- Keep CLI flags and subcommands consistent with existing `clap` patterns.

## Testing Guidelines
- Unit tests live alongside modules under `src/` using `#[cfg(test)]`.
- Integration and provider mocks live in `tests/e2e/`.
- Before shipping changes that affect providers, run `cargo test` and `cargo run --bin test_api`.
- Use `scripts/test_e2e.sh` for a full preflight (binary check + targeted suites).

## Commit & Pull Request Guidelines
- Commit messages are concise, imperative, and often start with verbs like “Add …” or “Fix …” (sometimes `Fix:` prefixes).
- PRs should include a short summary, rationale, and the exact test commands run.
- Note which provider you validated (`openai` or `claude`) and update docs when CLI behavior changes.

## Security & Configuration Tips
- OAuth credentials live at `~/.codex/auth.json` and `~/.claude/.credentials.json`; never commit secrets.
- For Claude SDK usage, set `JCODE_CLAUDE_SDK_PYTHON` as documented in `CLAUDE.md`.
