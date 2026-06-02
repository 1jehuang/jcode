# Reference Repo Summaries

Static summaries of all 7 repos for quick lookup without cloning.

---

## 1. oh-my-openagent
**URL:** https://github.com/code-yeongyu/oh-my-openagent  
**Stack:** TypeScript, Bun, OpenCode plugin  
**What it is:** A powerful OpenCode plugin that adds named agents (Prometheus, Atlas, Hephaestus, Sisyphus-Junior) with model-variant routing, tmux session management, and multi-agent delegation via `delegate-task`.

**Key Patterns:**
- **Agent factory pattern**: `createAtlasAgent()`, `createHephaestusAgent()` — each agent is a factory with model-variant routing
- **Prompt variants per model**: `default.md` (Claude), `gpt.md`, `gemini.md`, `kimi.md` — same agent, different prompt per provider
- **Delegate-task orchestration**: Atlas spawns Sisyphus-Junior subagents via `task()` calls; never self-reports, always verifies
- **Model resolution pipeline**: `resolveModel(input)` → UI override → agent-specific → fallback chain
- **Tmux integration**: `createTmuxSession()`, `spawnPane()` for multi-pane agent workflows
- **Session management**: `SessionCursor`, `trackInjectedPath()`, `SessionToolsStore`
- **Config migration**: `migrateConfigFile()` with `AGENT_NAME_MAP`, `HOOK_NAME_MAP`, `MODEL_VERSION_MAP`

**Key Files:**
- `src/agents/atlas/agent.ts` — orchestrator agent factory
- `src/agents/prometheus/system-prompt.ts` — strategic planner prompt loader
- `src/agents/hephaestus/agent.ts` — autonomous deep worker
- `src/agents/sisyphus-junior/agent.ts` — category-spawned executor
- `src/shared/index.ts` — barrel export of 297 utility files
- `src/shared/model-availability.ts` — `resolveModel()`, `checkModelAvailability()`
- `packages/prompts-core/` — model-neutral prompt markdown files

---

## 2. opencode
**URL:** https://github.com/anomalyco/opencode  
**Stack:** TypeScript, Bun, SST, monorepo (Turbo)  
**What it is:** The open source AI coding agent. Terminal UI with provider abstraction, extension system, desktop app, and a well-structured monorepo.

**Key Patterns:**
- **Provider abstraction**: Clean separation between LLM provider and agent logic
- **Monorepo layout**: `packages/` with `tui/`, `desktop/`, `web/`, `identity/`
- **SST for infra**: Config-as-code for cloud deployment
- **Desktop + TUI**: Supports both Electron-style desktop and pure terminal modes
- **Zed extension**: `packages/extensions/zed/` for IDE integration

**Key Files:**
- `packages/tui/` — terminal UI implementation
- `packages/desktop/` — Electron-style desktop wrapper
- `sst.config.ts` — infrastructure config
- `turbo.json` — monorepo build pipeline

---

## 3. oh-my-pi
**URL:** https://github.com/can1357/oh-my-pi  
**Stack:** TypeScript + Rust, Bun ≥ 1.3.14  
**What it is:** Fork of Pi by @mariozechner. "A coding agent with the IDE wired in." 40+ providers, 32 built-in tools, 13 LSP ops, 27 DAP ops, ~27k lines of Rust core.

**Key Patterns:**
- **Benchmarked tool use**: Every tool is tuned for first-attempt success rate; `packages/typescript-edit-benchmark/` has full benchmark harness
- **LSP integration**: 13 language server protocol operations built in
- **DAP integration**: 27 debug adapter protocol operations built in
- **Multi-provider**: 40+ providers with provider-neutral abstraction
- **Rust core + TS surface**: Performance-critical code in Rust, developer-facing API in TypeScript
- **Mutation testing**: `src/mutations.ts` for benchmark task generation

**Key Files:**
- `packages/typescript-edit-benchmark/src/` — full benchmark framework
- `packages/typescript-edit-benchmark/src/tasks.ts` — benchmark task definitions
- `packages/typescript-edit-benchmark/src/runner.ts` — benchmark runner
- `packages/typescript-edit-benchmark/src/prompts/` — benchmark prompt templates

---

## 4. codebuff
**URL:** https://github.com/CodebuffAI/codebuff  
**Stack:** TypeScript, multi-agent pipeline  
**What it is:** AI coding assistant that coordinates specialized agents. Beats Claude Code 61% vs 53% on 175+ coding tasks. Has a `freebuff` free tier.

**Key Patterns:**
- **4-agent pipeline**: File Picker → Planner → Editor → Reviewer — each is a specialized agent
- **Tree-sitter code map**: `packages/code-map/` uses tree-sitter for language-aware code parsing across 10+ languages
- **Agent composition**: Multi-agent as a *strategy*, not just concurrency — each agent has a specific role
- **Custom agent builder**: `/init` command generates agent scaffolding
- **Eval-driven development**: `evals/` directory with 175+ tasks across real open-source repos

**Key Files:**
- `packages/code-map/src/index.ts` — code map entry point
- `packages/code-map/src/languages.ts` — language detection
- `packages/code-map/src/tree-sitter-queries/` — per-language AST queries
- `evals/README.md` — eval methodology

---

## 5. codex (OpenAI Codex CLI)
**URL:** https://github.com/openai/codex  
**Stack:** TypeScript, Node.js  
**What it is:** OpenAI's official local coding agent CLI. Single binary, sandboxed execution, ChatGPT plan integration.

**Key Patterns:**
- **Sandbox-first execution**: All tool use is sandboxed; firewall init script at `scripts/init_firewall.sh`
- **Container execution**: `run_in_container.sh` for isolated runs
- **Hardened tool use**: Security-first design, execution policy, network isolation
- **Multiple install paths**: npm, Homebrew, binary releases — portable distribution
- **Bazel build**: `BUILD.bazel`, `MODULE.bazel` for reproducible builds

**Key Files:**
- `codex-cli/bin/codex.js` — CLI entry point
- `codex-cli/scripts/init_firewall.sh` — firewall/sandbox setup
- `codex-cli/scripts/run_in_container.sh` — container execution
- `codex-cli/package.json` — deps and scripts

---

## 6. claude-code (CCB — Claude Code Best)
**URL:** https://github.com/claude-code-best/claude-code  
**Stack:** TypeScript, Bun  
**What it is:** Decompiled/reconstructed Claude Code (CCB = 踩踩背) with many enterprise features: Pipe IPC multi-instance, ACP protocol (Zed/Cursor IDE), Remote Control Docker deployment, Langfuse monitoring, Web Search, Computer Use, Chrome Use, Voice Mode, Sentry, GrowthBook.

**Key Patterns:**
- **Pipe IPC**: `main/sub` auto-orchestration + LAN cross-machine zero-config discovery; `/pipes` panel + `Shift+↓` + message broadcast routing
- **ACP Protocol**: Session resume, Skills, permission bridging for Zed/Cursor
- **Remote Control**: Docker self-hosted remote UI — watch Claude Code from your phone
- **Langfuse monitoring**: Every agent loop step is observable and can be converted to datasets
- **Feature flags**: GrowthBook integration for enterprise feature gating
- **Memory management**: `/dream` command for memory consolidation
- **Poor Mode**: Disable memory extraction + typing suggestions to reduce concurrent requests

**Key Files:**
- `src/types/message.ts` — message types
- `src/types/tools.ts` — tool type definitions
- `src/types/plugin.ts` — plugin system types
- `src/types/hooks.ts` — hook system

---

## 7. pi-agent-rust
**URL:** https://github.com/Dicklesworthstone/pi_agent_rust  
**Stack:** Rust 2024 edition, `asupersync` async runtime, `rich_rust` TUI  
**What it is:** High-performance Rust port of Pi Agent by Jeffrey Emanuel. Single binary, <100ms startup, <50MB idle memory, SQLite sessions, WASM extension security, io_uring fast lane.

**Key Patterns:**
- **SQLite session store**: `src/session_sqlite.rs` — segmented log + offset index, O(index+tail) reopen on large histories
- **Hostcall security model**: Capability-gated hostcalls: `tool`/`exec`/`http`/`session`/`ui`/`events`; two-stage exec guard; trust lifecycle `pending→acknowledged→trusted→killed`
- **io_uring fast lane**: `src/hostcall_io_uring_lane.rs` — deterministic dispatch, typed opcodes, bounded shard queues
- **WASM extension runtime**: `src/pi_wasm.rs` — startup prewarm, warm isolate reuse, DCG/heredoc AST signals for dangerous shell detection
- **SSE streaming parser**: Tracks scanned bytes, handles UTF-8 tails, normalizes chunk boundaries, interns event-type strings
- **Multi-provider**: `src/providers/` — Anthropic, OpenAI, Vertex, Azure, Cohere, GitLab, Copilot
- **Benchmarks**: `benches/` — tools, semantic context, session save, TUI perf, extensions
- **Shadow dual execution**: Automatic backoff on divergence; compatibility-lane kill switches

**Key Files:**
- `src/session_sqlite.rs` — session persistence
- `src/agent_cx.rs` — agent execution context
- `src/extension_dispatcher.rs` — WASM extension dispatch
- `src/hostcall_io_uring_lane.rs` — fast-path hostcall routing
- `src/providers/anthropic.rs` — Anthropic provider
- `src/pi_wasm.rs` — WASM runtime
- `benches/` — full benchmark suite
- `Cargo.toml` — deps: `asupersync`, `rich_rust`, edition 2024

---

## Cross-Repo Quick Reference

| Feature Domain | Best Source Repo(s) |
|----------------|---------------------|
| Multi-agent orchestration | oh-my-openagent (Atlas/delegate-task), codebuff (4-agent pipeline) |
| Model/provider abstraction | oh-my-openagent (resolveModel), oh-my-pi (40+ providers), pi-agent-rust (src/providers/) |
| Session persistence | pi-agent-rust (SQLite), claude-code (memory/dream) |
| Security & sandboxing | codex (firewall), pi-agent-rust (capability gates, trust lifecycle) |
| Benchmarking | oh-my-pi (typescript-edit-benchmark), pi-agent-rust (benches/) |
| IDE integration | oh-my-pi (LSP/DAP), claude-code (ACP/Zed/Cursor) |
| Streaming | pi-agent-rust (SSE parser), opencode (provider abstraction) |
| Extension/plugin system | pi-agent-rust (WASM), oh-my-openagent (OpenCode plugin), claude-code (ACP) |
| Monitoring/observability | claude-code (Langfuse, Sentry), pi-agent-rust (runtime risk ledger) |
| TUI design | opencode (terminal UI), pi-agent-rust (rich_rust), oh-my-pi (IDE-wired) |
| Code understanding | codebuff (tree-sitter code map), oh-my-openagent (ripgrep-cli) |
| Prompt engineering | oh-my-openagent (per-model prompt variants), oh-my-pi (benchmark prompts) |