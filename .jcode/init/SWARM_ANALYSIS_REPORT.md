# Jcode Init Swarm Analysis Report

Generated: `<generated-at>`
Root: `<project-root>`
Branch: `feature/embedded-skills-harness`

## Barrier status

All required discovery roles reported before synthesis:

| Role | Session | Status | Scope |
| --- | --- | --- | --- |
| architect | `<architect-session>` | completed | repository structure, architecture boundaries, workflows, high-risk areas |
| qa | `<qa-session>` | completed | tests, CI gaps, validation strategy, untested risk |
| documenter | `<documenter-session>` | completed | README, docs, onboarding, AGENTS.md gaps |
| tooling-security | `<tooling-security-session>` | ready/reported | package managers, MCP, secrets boundaries, automation risk |

## Evidence read for synthesis

- Init files: `.jcode/init/SWARM_ANALYSIS_PLAN.md`, `.jcode/INIT_REPORT.md`, `.jcode/INIT_QUESTIONS.md`, `.jcode/SKILLS_PLAN.md`, `.jcode/MCP_PLAN.md`.
- Repository roots: `Cargo.toml`, `README.md`, `AGENTS.md`.
- Harness docs: `docs/JCODE_HARNESS_RELEASE_GATES.md`, plus discovery-agent reads of `docs/SKILLS_HARNESS.md`, `docs/CLEAN_CODE_GUARDIAN.md`, `docs/CODEX_BOOTSTRAP.md`, `docs/JCODE_HARNESS_INIT_SWARM.md`, `docs/SERVER_ARCHITECTURE.md`, `docs/SWARM_ARCHITECTURE.md`, and `docs/CRATE_OWNERSHIP_BOUNDARIES.md`.
- Tooling/security files: `.jcode/mcp.json`, `telemetry-worker/package.json`, `.github/workflows/{ci.yml,windows-smoke.yml,release.yml}`.

## Project-specific architecture summary

This is a Rust 2024 workspace with the root package `jcode` and many workspace crates. `Cargo.toml` defines the root library at `src/lib.rs`, the primary `jcode` binary at `src/main.rs`, and the automation-facing `jcode-harness` binary at `src/bin/harness.rs`. The workspace includes specialized crates for provider/auth types, storage, swarm, TUI rendering, side-panel, update, mobile, desktop, and tool protocol boundaries.

The current fork focus is not a generic Rust app. It is a jcode self-development and `jcode-harness` product branch centered on:

- Embedded offline skills and skill routing.
- Offline `clean-code` quality gate behavior.
- The `jcode-harness` CLI and JSON/NDJSON automation contracts.
- Interactive `/init` bootstrap that writes static scaffold files, then queues LLM-driven swarm analysis.
- Existing jcode compatibility for `jcode run`, `jcode serve`, `jcode connect`, and providers.

## Core workflows

- Main product CLI: `jcode` from `src/main.rs`.
- Harness CLI: `jcode-harness` from `src/bin/harness.rs`.
- Init workflow: `src/project_init.rs` and docs in `docs/JCODE_HARNESS_INIT_SWARM.md`; release gates require `/init` to spawn architecture, QA, documentation, and tooling/security discovery roles and block synthesis on a report barrier.
- Skill workflow: `src/skill.rs`, `src/skill_pack.rs`, and docs in `docs/SKILLS_HARNESS.md`; built-ins must remain offline and deterministic.
- Swarm/session workflow: server and swarm code, including `src/server/swarm.rs`, remains high-risk because it coordinates concurrent sessions and lifecycle state.
- Self-development workflow: `AGENTS.md` says to prefer fast iteration, remote builds if local resources are insufficient, rebuild when done, and use debug socket for runtime-level debugging.

## High-risk areas

1. **Root crate compile fan-out**: the root package still owns large CLI/runtime surfaces while many crates have been split out. Changes in root modules can trigger broad rebuilds and broad regression risk.
2. **Swarm lifecycle and persistence**: init, swarm, background tasks, and side-panel status depend on reliable concurrent reporting and session-state transitions.
3. **Provider/auth/network boundaries**: `Cargo.toml` includes HTTP, OAuth, provider, and browser/open-url dependencies. Secrets and credentials must not be captured in memory or generated docs.
4. **Embedded skills behavior**: built-in skill availability, precedence, JSON output, and offline operation are release-critical for this branch.
5. **Harness output contracts**: JSON and NDJSON modes are automation-facing and should be treated as compatibility surfaces.
6. **Automation scripts and release delivery**: release workflows use write permissions and deployment secrets, and installer scripts/download paths should be reviewed carefully.
7. **Telemetry worker**: `telemetry-worker/package.json` has deploy and remote D1 migration commands using `npx wrangler`, but no test/lint script was found in that package.

## QA and validation findings

Evidence-backed validation candidates from repo files:

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
cargo run -q -p jcode --bin jcode-harness -- run "review this diff" --json --mock-response ok | python3 -m json.tool >/dev/null
cargo run -q -p jcode --bin jcode-harness -- run "review this diff" --ndjson --mock-response ok | while read -r line; do printf '%s\n' "$line" | python3 -m json.tool >/dev/null; done
scripts/dev_cargo.sh build --profile selfdev -p jcode --bin jcode
```

When using this Jcode self-development harness, prefer coordinated `selfdev build target=auto` over ad hoc local builds.

CI evidence from QA discovery: workflows cover formatting, check, clippy, ratchet scripts, mobile tests, provider matrix, e2e, Windows targeted smoke, and security preflight. Gap: main lib/bin unit tests appear compiled more often than fully run in CI, while `scripts/test_ci_suites.py` and `scripts/test_fast.sh` define runnable lib/bin suites. Python socket/debug scripts under `tests/` are manual or not clearly referenced by workflows. Telemetry worker lacks package-local test/lint scripts.

## Documentation and onboarding findings

- `README.md` describes the product and mentions the embedded skills harness at line 14.
- `AGENTS.md` has useful workflow, install, debug socket, and embedded skills guardrails, but it is thin for this repo size.
- Recommended `AGENTS.md` improvements: add repository map, primary binaries, crate ownership boundaries, validated commands, embedded skills invariants, testing/CI gates, auth/secrets boundaries, and self-dev/debug-socket flow.
- Generated `.context` documentation should be treated cautiously until reconciled. Discovery found stale references claiming TypeScript entrypoints and a `crates/jcode` main binary, while `Cargo.toml` shows the actual root package and binaries.

## Tooling, MCP, and security findings

- `.jcode/mcp.json` currently has no active MCP servers: `{ "mcpServers": {} }`.
- `.jcode/MCP_PLAN.md` is correctly review-first and says not to auto-install MCP servers.
- Candidate MCP categories are browser/Playwright, GitHub/GitLab, database, and docs/search, but each requires explicit credential and permission review.
- `telemetry-worker/package.json` uses `npx wrangler` for dev/deploy/remote D1 migrations. No Node lockfile was reported by tooling-security discovery.
- Workflows use deployment secrets and third-party actions. Release has `permissions: contents: write`.
- Release Homebrew update was reported to disable SSH strict host key checking in `.github/workflows/release.yml`.
- Installer/build scripts download remote assets/scripts without signature verification, although release docs mention generated `SHA256SUMS`.
- No `pull_request_target` workflow trigger was reported.

## Updated recommendations

### Skills

Keep recommended initial skills task-routed rather than globally injected:

- `rust`: default for implementation and review in this Rust workspace.
- `karpathy-guidelines`: use for concise engineering judgment and repo hygiene.
- `optimization`: use for performance, compile-time, memory, and multi-session scaling work.
- `clean-code-guardian`: use when touching production code or preparing release gates, especially via offline `clean-code check`.

### MCP

Keep MCP disabled by default. Review candidates in this order:

1. Browser/Playwright for UI QA and docs screenshots, if needed.
2. GitHub for issues/PR/release automation, requiring token scoping and no token persistence in repo docs.
3. Docs/search with network boundaries documented.
4. Database/telemetry only with strict read/write boundaries and no credential capture.

### Side-panel default

The side panel should show current goal, swarm status, validation commands, architecture risks, security/MCP boundaries, and open questions from `.jcode/INIT_QUESTIONS.md`.

## Open questions for humans

From `.jcode/INIT_QUESTIONS.md` and discovery gaps:

1. What exact command set must pass before work is considered done for normal feature work versus release work?
2. Which directories are forbidden to edit besides standard generated/build/secret paths?
3. Which provider, telemetry, and deployment credentials exist and must never enter memory or docs?
4. Should generated `.context` docs be regenerated or corrected now that stale Rust workspace references were found?
5. Should `cargo test --lib --bins` or `python3 scripts/test_ci_suites.py lib-bins` become a CI gate?
6. Should `telemetry-worker` add explicit test/lint scripts before deployment workflows are considered gated?
