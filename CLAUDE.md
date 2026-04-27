# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project

`jcode` is a coding-agent harness — a Rust workspace producing a single `jcode` binary that runs as a TUI client, a persistent server daemon, and self-developing build infrastructure. It supports many LLM providers (Claude, OpenAI, Gemini, Copilot, Azure, OpenRouter, OpenAI-compatible, etc.), MCP servers, browser automation, and a swarm coordination model where multiple agents collaborate in the same repo via a single server process.

## Common commands

Fast iteration loop (preferred while editing):

```bash
cargo check -q                                  # quick compile check
cargo fmt --all -- --check                      # formatting
scripts/check_warning_budget.sh                 # warning ratchet
python3 scripts/check_code_size_budget.py       # oversized-file ratchet
```

Stricter set (touching core orchestration, providers, server, or TUI):

```bash
cargo check --all-targets --all-features
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-targets --all-features
cargo test --test e2e
```

Single-test invocation:

```bash
cargo test --lib <test_name>                    # by name (substring filter)
cargo test -p <crate-name> <test_name>          # specific workspace member
cargo test --test e2e <test_name>               # one e2e test
cargo test --test provider_matrix <test_name>   # one provider-matrix test
```

Larger refactor verification (all-in-one):

```bash
scripts/refactor_phase1_verify.sh
```

Build / install:

```bash
cargo build --release                           # standard release
scripts/dev_cargo.sh build --release -p jcode --bin jcode   # Linux x86_64 dev wrapper (sccache + clang+lld auto-detection)
scripts/dev_cargo.sh --print-setup              # show active linker/cache
scripts/install_release.sh                      # symlink target/release/jcode into the local-build channel
```

Notes:

- `scripts/cargo_exec.sh` wraps cargo to enforce repo-local conventions; most scripts call it. Prefer the scripts above over raw cargo when they exist.
- `scripts/remote_build.sh` offloads heavy builds to a remote machine when local resources are insufficient.
- The `release` profile uses `opt-level = 1, codegen-units = 256, incremental = true` (fast iteration). The `release-lto` profile is for distribution. A `selfdev` profile (release with `opt-level = 0`) exists for self-dev rebuilds.
- Default features are `embeddings` and `pdf`. Both pull large dep trees; disable with `--no-default-features` for faster compiles when not needed. `jemalloc` and `jemalloc-prof` are opt-in features for the long-running server.

Running it:

```bash
jcode                            # launch TUI (auto-spawns server if needed)
jcode serve                      # run as background server only
jcode connect                    # connect another client to running server
jcode run "say hello"            # one-shot non-interactive
jcode --resume <name>            # resume a session by memorable name
jcode login --provider <name>    # OAuth/key setup per provider
jcode auth-test --all-configured # smoke-test credentials
jcode browser status / setup     # built-in browser tool
```

## Architecture

### Workspace layout

Single root crate (`jcode`) plus thirteen workspace members under `crates/`:

- `jcode-agent-runtime`, `jcode-tui-workspace` — runtime and TUI scaffolding pulled out of the root crate
- `jcode-provider-core`, `jcode-provider-metadata`, `jcode-provider-openrouter`, `jcode-provider-gemini`, `jcode-azure-auth` — provider integrations
- `jcode-embedding` (feature-gated, ~163 deps), `jcode-pdf` (feature-gated) — heavy optional subsystems
- `jcode-notify-email`, `jcode-mobile-core`, `jcode-mobile-sim`, `jcode-desktop` — peripheral surfaces

The root crate produces three binaries: `jcode` (main), `jcode-harness`, `test_api`.

### Single-server, multi-client model

There is exactly one `jcode serve` process per user; TUI clients connect over a Unix socket (named pipe on Windows). When you run `jcode`, it auto-spawns the daemon via `setsid()` if no server exists, then connects as a client. `/reload` execs the server binary in place; clients auto-reconnect. This is why state lives on the server side — sessions, MCP pool, providers, swarm coordination, and durable state survive client disconnects and reloads. See `docs/SERVER_ARCHITECTURE.md`.

Key transport facts:

- Unix socket: `/run/user/$UID/jcode.sock`; debug socket: `jcode-debug.sock`
- Server registry: `~/.jcode/servers.json`
- Logs: `~/.jcode/logs/jcode-YYYY-MM-DD.log`
- Windows uses named pipes via the abstraction in `src/transport/` (zero-cost on Unix — pure type aliases over tokio Unix sockets). See `docs/WINDOWS.md`.

### Major source regions

- `src/cli/` — argument parsing (`args.rs`), startup/dispatch, login flows, auth-test
- `src/server/` — multi-client server runtime, session lifecycle, swarm/comm state, reload/recovery, durable state, debug API. Heavily decomposed into focused files (`client_*`, `comm_*`, `swarm_*`, `debug_*`).
- `src/agent/` — turn-loop engine, streaming, response recovery, compaction, prompting
- `src/provider/` — per-provider HTTP/streaming/auth (`anthropic`, `openai`, `claude`, `gemini`, `copilot`, `cursor`, `openrouter`, `antigravity`, etc.) + dispatch/failover/selection
- `src/auth/` — OAuth flows for each provider, account store, login diagnostics
- `src/tool/` — built-in tools (`bash`, `edit`, `read`, `write`, `grep`, `agentgrep`, `glob`, `webfetch`, `websearch`, `browser`, `mcp`, `memory`, `task`, `todo`, `goal`, `apply_patch`, etc.) and the tool dispatch glue
- `src/tui/` — ratatui renderer; ~115k LOC across ~144 files. The `app/` subtree owns state/input/commands/event reducers; the rest is rendering, markdown, mermaid (via `mermaid-rs-renderer` crate), info widgets, scrollback.
- `src/memory*.rs` + `src/memory/` — embedding-backed memory graph, memory agent, sidecar verifier. See `docs/MEMORY_ARCHITECTURE.md`.
- `src/ambient*.rs`, `src/safety.rs` — proactive ambient mode + safety/permission system. See `docs/AMBIENT_MODE.md`, `docs/SAFETY_SYSTEM.md`.
- `src/protocol.rs` + `src/protocol_*` — newline-delimited JSON wire protocol between server and clients
- `src/mcp/` — MCP server pool (shared across sessions). Config files: `~/.jcode/mcp.json`, `.jcode/mcp.json`, fallback `.claude/mcp.json`.
- `src/transport/` — IPC abstraction (Unix sockets / Windows named pipes)
- `src/platform.rs` — `#[cfg]`-gated platform code; design rule is **zero cost on Unix**
- `tests/e2e/` — end-to-end tests with a `mock_provider` so the agent can be exercised without real network calls
- `build.rs` — derives `JCODE_VERSION` from git metadata

### Self-dev mode

The agent can edit, build, and reload its own binary. There is significant infrastructure around this in `src/cli/selfdev.rs`, `src/server/reload*.rs`, `src/restart_snapshot.rs`, and the install layout under `~/.jcode/builds/{stable,current,canary,versions/<v>}/jcode`. The launcher symlink at `~/.local/bin/jcode` (or `%LOCALAPPDATA%\jcode\bin\jcode.exe`) must be **before `~/.cargo/bin`** in `PATH`.

Use `scripts/refactor_shadow.sh` to run an isolated `JCODE_HOME` + socket pair when iterating on the server, so refactor runs do not collide with your normal sessions.

## Quality guardrails (enforced by CI)

CI gates from `.github/workflows/ci.yml` and `CONTRIBUTING.md`:

- `cargo fmt --all -- --check`
- `cargo check --all-targets --all-features`
- `cargo clippy --all-targets --all-features -- -D warnings`
- `scripts/check_warning_budget.sh` — warning count is ratcheted **downward only**; do not `--update` to permit growth
- `scripts/check_code_size_budget.py` — oversized-file ratchet; same rule
- `scripts/check_test_size_budget.py`, `scripts/check_panic_budget.py` — test-size and panic-call ratchets
- `scripts/security_preflight.sh --strict` (uses `cargo-audit`)
- Test matrix: `cargo test --lib --bins`, `cargo test --test provider_matrix`, `cargo test --test e2e` on Linux + macOS, plus a Windows job
- Mobile job: `cargo test -p jcode-mobile-core -p jcode-mobile-sim`

File-size targets (from `CONTRIBUTING.md`):

- Production Rust files: prefer <800 LOC, hard cap 1200 LOC unless documented
- Functions: prefer <100 LOC; split into helpers/reducers/service methods otherwise
- When you shrink an oversized file, run `scripts/check_code_size_budget.py --update` to lock in the win

Refactoring rules:

- Behavior-preserving extraction first, then logic changes
- Don't mix unrelated cleanup with feature work unless required for safety
- Prefer typed enums/structs over new stringly-typed states
- Don't silently swallow errors on persistence/protocol/lifecycle paths

## Workflow conventions (from AGENTS.md)

- Commit small and often as features/fixes complete; push when finished. Try to commit even if the tree is dirty due to parallel agents.
- Prefer `cargo check`, targeted tests, and dev builds while iterating; rebuild release at the end.
- Bump `Cargo.toml` version on releases (patch/minor based on the changes since the previous release).
- Use `scripts/remote_build.sh` if a local build gets killed for resource reasons — check resource availability before kicking off heavy builds.

## Further reading (in `docs/`)

`SERVER_ARCHITECTURE.md`, `SWARM_ARCHITECTURE.md`, `MEMORY_ARCHITECTURE.md`, `MEMORY_BUDGET.md`, `AMBIENT_MODE.md`, `SAFETY_SYSTEM.md`, `WINDOWS.md`, `BROWSER_PROVIDER_PROTOCOL.md`, `MULTI_SESSION_CLIENT_ARCHITECTURE.md`, `REFACTORING.md`, `CODE_QUALITY_10_10_PLAN.md`, `COMPILE_PERFORMANCE_PLAN.md`, `DESKTOP_CODEBASE_ARCHITECTURE.md`, `CLIENT_CORE_PRESENTATION_SPLIT_PLAN.md`.
