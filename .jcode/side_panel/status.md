# Project Status Panel

## Current goal

Jcode `/init` swarm analysis completed for `/home/chapzin/jcode-harness`.

## Swarm status

- architect: reported, completed
- qa: reported, completed
- documenter: reported, completed
- tooling-security: reported, ready
- Barrier: all required discovery reports received before synthesis

## Detected stack

- Rust 2024 workspace rooted at `Cargo.toml`
- Root package/binaries: `jcode` (`src/main.rs`) and `jcode-harness` (`src/bin/harness.rs`)
- Telemetry worker: `telemetry-worker/package.json` uses `npx wrangler`

## Validation candidates

```bash
cargo fmt --check
cargo check -p jcode
cargo test -p jcode project_init --lib -- --nocapture
cargo test -p jcode test_init_command --lib -- --nocapture
cargo test -p jcode skill::tests --lib
cargo test -p jcode clean_code --lib
cargo test --test e2e harness_cli -- --nocapture
cargo run -q -p jcode --bin jcode-harness -- skills list --json | python3 -m json.tool >/dev/null
cargo run -q -p jcode --bin jcode-harness -- skills doctor --json | python3 -m json.tool >/dev/null
```

For self-dev builds, prefer coordinated `selfdev build target=auto`.

## Architecture risks

- Root crate compile fan-out and broad runtime coupling.
- Swarm/session lifecycle persistence and concurrency.
- Provider/auth/network surfaces and secret boundaries.
- Embedded skills determinism and offline behavior.
- JSON/NDJSON harness CLI compatibility.

## MCP/security status

- `.jcode/mcp.json` has no active MCP servers.
- MCP remains review-first and disabled by default.
- Do not store credentials, tokens, `.env` values, private keys, or deployment secrets in memory or docs.

## Open questions

See `.jcode/INIT_QUESTIONS.md`. Additional project-specific questions are listed in `.jcode/init/SWARM_ANALYSIS_REPORT.md`.
