# Repository Guidelines

## Development Workflow

- **Commit as you go** - Make small, focused commits after completing each feature or fix
- **Push when done** - Push all commits to remote when finishing a task or session
- **Rebuild and install when done** - Run `cargo build --release && cp target/release/jcode ~/.local/bin/`
- **Test before committing** - Run `cargo test` to verify changes
- **Bump version for releases** - Update version in `Cargo.toml` when making releases

## Versioning

jcode uses **auto-incrementing** semantic versioning (`v0.1.X`).

**Automatic (patch):**
- Build number auto-increments on every `cargo build`
- Stored in `~/.jcode/build_number`
- Example: `v0.1.1` → `v0.1.2` → `v0.1.3` ...

**Manual (major/minor):**
- For big changes, manually update major/minor version in `Cargo.toml`
- **Minor** (0.1.x → 0.2.0): New features, significant enhancements
- **Major** (0.x.x → 1.0.0): Breaking changes to CLI, config, or APIs

The build also includes git hash and `-dev` suffix for uncommitted changes (e.g., `v0.1.47-dev (abc1234)`).

## Project Structure & Module Organization
- `src/` is the core library and CLI entry point (`src/main.rs`). Key areas include `src/agent.rs`, `src/provider/`, `src/mcp/`, `src/tool/`, and `src/tui/`.
- `src/bin/` holds auxiliary binaries: `test_api.rs` (Claude SDK smoke test) and `harness.rs` (tool harness).
- `tests/e2e/` contains integration tests and mock providers.
- `scripts/` includes helper scripts like `agent_trace.sh` and `test_e2e.sh`.
- Docs live in `README.md`, `OAUTH.md`, and `CLAUDE.md`.

## Build, Test, and Development Commands
- `cargo install --path .`: install the local CLI.
- `cargo build --release && cp target/release/jcode ~/.local/bin/`: rebuild and install the release binary.
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
- **Manual testing** - After making TUI changes, manually test in a real terminal to verify behavior.

## Commit & Pull Request Guidelines
- Commit messages are concise, imperative, and often start with verbs like “Add …” or “Fix …” (sometimes `Fix:` prefixes).
- PRs should include a short summary, rationale, and the exact test commands run.
- Note which provider you validated (`openai` or `claude`) and update docs when CLI behavior changes.

## Security & Configuration Tips
- OAuth credentials live at `~/.codex/auth.json` and `~/.claude/.credentials.json`; never commit secrets.
- For Claude SDK usage, set `JCODE_CLAUDE_SDK_PYTHON` as documented in `CLAUDE.md`.
