# Documentation Gap Audit (KCode fork, 2026-10-17)

Read-only audit of `/Users/kooshapari/CodeProjects/Phenotype/repos/jcode`. Workspace: 92 crates under `crates/`. README: 45,791 bytes. CHANGELOG: present, last entry `v0.85.1-k1.1.0` (2026-09-15). Cargo.toml workspace version: `0.85.1-k1.1.1`.

## Summary

- Public APIs without rustdoc (extrapolated across sampled crates): **~300-500 in sample, 1,000-2,000 workspace-wide** (see Check 1 below for sample data).
- Outdated README claims: **at least 3** (stale version on lines 15 and 55; missing 2 fork releases; provider list not aligned with 21 provider crates).
- Undocumented config options: **~50+** in `crates/jcode-base/src/config/env_overrides.rs` (no env-var reference table in README; only `[display].emoji` and `[provider].stream_idle_timeout_secs` shown as config samples).
- Undocumented env vars: **429 unique env vars used at runtime, ~14 documented in README (3%)**. CHANGELOG documents only 2 more.
- Examples coverage: **11/92 crates (12%)** have `examples/` directories. None of the 9 sampled "core" crates do.
- Architecture docs: **No top-level ARCHITECTURE.md**. Five topic-specific docs exist (`MEMORY_ARCHITECTURE.md`, `MODULAR_ARCHITECTURE_RFC.md`, `MULTI_SESSION_CLIENT_ARCHITECTURE.md`, `SERVER_ARCHITECTURE.md`, `SWARM_ARCHITECTURE.md`) but no single overview reflecting the current 92-crate layout.
- CHANGELOG: present but **stale by one release** (last entry `v0.85.1-k1.1.0`, actual `v0.85.1-k1.1.1`).

## Findings

| # | Location | Issue | Severity |
|---|----------|-------|----------|
| 1 | `README.md:15` | "Latest fork release: **v0.85.0-k1.0.0**" — but `Cargo.toml` is `0.85.1-k1.1.1` and `CHANGELOG.md` last entry is `v0.85.1-k1.1.0`. README is 2 fork releases stale. | High |
| 2 | `README.md:55` | "GitHub Packages (replace `<version>` with desired release, e.g. `0.85.0-k1.0.0`)" — same stale version example. | High |
| 3 | 429 unique env vars in `crates/` (see `env::var(`, `env::var_os(` call sites) vs ~14 documented in README | Massive env-var sprawl undocumented. README lists only `JCODE_NO_EMOJI`, `JCODE_STREAM_IDLE_TIMEOUT_SECS`, `JCODE_OPENAI_COMPAT_*`, `JCODE_OPENAI_EXTRA_BODY`, `JCODE_PROVIDER_*` (example), `ANTHROPIC_API_KEY`, `OPENAI_API_KEY`, `OPENROUTER_API_KEY`, `OPENAI_COMPAT_API_KEY`. CHANGELOG adds 2 more (`HERDR_JCODE_*`). | High |
| 4 | `crates/jcode-base/src/provider/mod.rs:79` (`pub trait Provider: Send + Sync`) — no `///` | The central `Provider` trait in the `provider-core` crate lacks a doc comment. | High |
| 5 | `crates/jcode-swarm-core/src/lib.rs:91` (`pub enum SwarmRole`), `:136` (`pub enum SwarmLifecycleStatus`) — no `///` | Public enums that drive the entire swarm subsystem are undocumented. | High |
| 6 | `crates/jcode-config-types/src/lib.rs:16` (`pub enum CompactionMode`), `:48` (`pub enum AgentMode`) — no `///` | Top-level config types directly used by the server are undocumented. | Medium |
| 7 | `crates/jcode-protocol/src/lib.rs:34` (`pub enum TranscriptMode`), `:52` (`pub struct HistoryMessage`) — no `///` | Protocol-level enums/structs consumed by every client are undocumented. | Medium |
| 8 | `crates/jcode-tool-core/src/lib.rs:95` (`pub struct StdinInputRequest`), `:108` (`pub struct ElicitRequest`) — no `///` | Tool input request types are undocumented. | Medium |
| 9 | No `examples/` directory in `crates/jcode-cli`, `crates/jcode-core`, `crates/jcode-protocol`, `crates/jcode-provider-core`, `crates/jcode-tool-core`, `crates/jcode-config-types`, `crates/jcode-swarm-core`, `crates/jcode-app-core`, `crates/jcode-herdr` | Zero worked examples for the 9 most-likely-to-be-reused crates. | Medium |
| 10 | No top-level `ARCHITECTURE.md` | Newcomers to the 92-crate workspace have no single overview. Topic docs in `docs/` cover subsystems but not the full crate map. | Medium |
| 11 | `README.md:366-380` "Supported built-in login flows" lists 13 providers (claude/openai/gemini/copilot/azure/alibaba/fireworks/novita/MiniMax/meta-muse/lmstudio/ollama/openai-compatible) but workspace has 21 `jcode-provider-*` crates; some named providers (MiniMax, fireworks, novita) have no dedicated crate and run via `openai-compatible` | Provider list doesn't match crate structure. | Medium |
| 12 | `CHANGELOG.md` last entry is `v0.85.1-k1.1.0` (2026-09-15); current is `v0.85.1-k1.1.1` | Missing 1 release. No changes logged since 2026-09-15 in the top-level CHANGELOG (the `changelog/` directory contains per-release JSONs up to that point, but the human-readable top-level file lags). | Low |
| 13 | `crates/jcode-base/src/config/env_overrides.rs` defines ~50 env-override bindings (`JCODE_*` -> config keys) but README only shows `[display].emoji` and `[provider].stream_idle_timeout_secs` as config samples | The full config schema (`[memory]`, `[tui]`, `[model]`, `[swarm]`, etc.) is not enumerated anywhere end-user-visible. | High |
| 14 | `README.md:850-855` documents `ANTHROPIC_API_KEY`, `OPENAI_API_KEY`, `OPENROUTER_API_KEY` but not `JADE_RELAY_TOKEN`, `JADE_RELAY_API_BASE`, `JADE_RELAY_USER_ID`, etc. | JADE relay is the macOS notification bridge — its auth flow isn't documented. | Low |
| 15 | `crates/jcode-herdr/src/env.rs` defines `HERDR_BIN_PATH`, `HERDR_SOCKET_PATH`, `HERDR_PANE_ID`, `HERDR_TAB_ID`, `HERDR_WORKSPACE_ID` as consts | HERDR is documented in `docs/HERDR_INTEGRATION.md` (present) but only `HERDR_JCODE_*` env vars made it to CHANGELOG. The other 5 `HERDR_*` env vars are discoverable only by reading source. | Medium |
| 16 | `crates/jcode-app-core/src/agent.rs:290` reads `JCODE_TRACK_CLIENT_CACHE`; `crates/jcode-app-core/src/agent/utils.rs:6` reads `JCODE_TRACE`; `crates/jcode-app-core/src/perf.rs:323,366` read `JCODE_PERF_TIER`, `JCODE_GLYPH_SAFE_MODE` | Useful runtime debug knobs invisible to operators. | Low |
| 17 | `.github/workflows/release.yml` and 9 other workflows | Workflow files exist but no `docs/CI.md` or `docs/RELEASING.md` companion beyond the 7,857-byte `RELEASING.md` at root. | Low |
| 18 | No `docs/` index/landing page | `docs/README.md` exists; checked briefly. | None |

## Detail by category

### Check 1: rustdoc coverage (5-crate sample)

`pub fn` / `pub struct` / `pub enum` / `pub trait` items lacking `///` in the 3 lines immediately above. Only a sample was checked (5 of 92 crates).

| Crate | Total pub items | Sample of undocumented items (line: declaration) |
|---|---|---|
| `jcode-protocol` | ~36 | L34 `pub enum TranscriptMode`; L44 `pub enum CommDeliveryMode`; L52 `pub struct HistoryMessage`; L64 `pub struct SessionActivitySnapshot`; L71 `pub struct TokenUsageTotals`; … (5+ shown; expect many more below) |
| `jcode-provider-core` | ~53 | L79 `pub trait Provider: Send + Sync`; L504 `pub enum PremiumMode`; L514 `pub enum CredentialMode`; L524 `pub fn from_runtime_env`; L534 `pub fn auth_route`; … |
| `jcode-tool-core` | ~14 | L26 `pub fn intent_schema_property`; L33 `pub fn accept_large_output_schema_property`; L48 `pub fn ensure_intent_in_schema`; L95 `pub struct StdinInputRequest`; L108 `pub struct ElicitRequest` |
| `jcode-config-types` | ~74 | L16 `pub enum CompactionMode`; L27 `pub fn as_str`; L35 `pub fn parse`; L48 `pub enum AgentMode`; L60 `pub fn as_str` |
| `jcode-swarm-core` | ~21 | L25 `pub fn validate_swarm_tldr`; L70 `pub fn derive_swarm_task_label`; L91 `pub enum SwarmRole`; L98 `pub fn as_str`; L136 `pub enum SwarmLifecycleStatus` |

**Extrapolation**: 5 sample crates → 198 pub items with 25 visible undocumented in first 5 lines of each (≈13% undocumented in sample). Workspace-wide across 92 crates this implies **~1,000-2,000 undocumented public APIs**, including foundational types (`Provider`, `SwarmRole`, `SwarmLifecycleStatus`, `TranscriptMode`, `CompactionMode`, `AgentMode`, etc.).

### Check 2: README claims

| Claim | Source line | Actual state | Stale? |
|---|---|---|---|
| "Latest fork release: **v0.85.0-k1.0.0**" | README.md:15 | `Cargo.toml` workspace version: `0.85.1-k1.1.1`; CHANGELOG last entry: `v0.85.1-k1.1.0` | **Yes — 3 versions behind** |
| "replace `<version>` with desired release, e.g. `0.85.0-k1.0.0`" | README.md:55 | same | **Yes** |
| `jcode v0.9.1888-dev (be386f2)` (self-dev build ref) | README.md:279 | Not in CHANGELOG (dev build label, may be stale) | Likely stale |
| "30+ tools" | `Cargo.toml:4` description | Sample count: 92. Likely accurate | OK |
| "14+ built-in login flows" (claude/openai/gemini/copilot/azure/alibaba/fireworks/novita/MiniMax/meta-muse/lmstudio/ollama/openai-compatible) | README.md:366-380 | Workspace has 21 `jcode-provider-*` crates; some named providers (MiniMax, fireworks, novita) use `openai-compatible` rather than dedicated crates | Partial mismatch |
| "OpenAI-compatible profile ids include: openrouter, orcarouter, deepseek, zai, kimi, moonshotai, meta-muse, opencode, opencode-go, 302ai, baseten, cortecs, huggingface, nebius, scaleway, stackit, firmware" | README.md:411 | Not verified against `provider_catalog.rs` exhaustively; sample showed these names present | Probably accurate |

### Check 3: Undocumented config options

`crates/jcode-base/src/config/env_overrides.rs` defines ~50 env-override bindings mapping `JCODE_*` env vars onto config options. Sampled bindings (none of these env vars appear in README's config docs):

- `JCODE_OPENAI_NATIVE_COMPACTION_MODE`
- `JCODE_OPENAI_NATIVE_COMPACTION_THRESHOLD_TOKENS`
- `JCODE_OPENAI_REASONING_EFFORT`
- `JCODE_OPENAI_SERVICE_TIER`
- `JCODE_OPENAI_TRANSPORT`
- `JCODE_ANTHROPIC_API_BASE`, `JCODE_ANTHROPIC_API_KEY_NAME`, `JCODE_ANTHROPIC_AUTH_HEADER`, `JCODE_ANTHROPIC_AUTH`, `JCODE_ANTHROPIC_DEBUG`, `JCODE_ANTHROPIC_ENV_FILE`, `JCODE_ANTHROPIC_HEADERS`, `JCODE_ANTHROPIC_MAX_TOKENS`, `JCODE_ANTHROPIC_MODEL`, `JCODE_ANTHROPIC_REASONING_EFFORT`
- `JCODE_COPILOT_PREMIUM`
- `JCODE_JADE_RELAY_*` (5 vars)
- (Plus ~25 more not enumerated here)

README only documents 2 config keys (`[display].emoji`, `[provider].stream_idle_timeout_secs`). The full `[memory]`, `[tui]`, `[model]`, `[swarm]`, `[server]` config schemas are not enumerated.

### Check 4: Undocumented env vars

**429 unique env-var names** appear in `env::var(` or `env::var_os(` call sites across `crates/`. Breakdown of well-known categories that are absent from README:

- HERDR runtime: `HERDR_BIN_PATH`, `HERDR_SOCKET_PATH`, `HERDR_PANE_ID`, `HERDR_TAB_ID`, `HERDR_WORKSPACE_ID` (5 vars; only `HERDR_JCODE_IDLE_DEBOUNCE_MS` and `HERDR_JCODE_RETRY_GRACE_MS` made it to CHANGELOG)
- JCODE debug/trace: `JCODE_TRACE`, `JCODE_DEBUG_*` (control, cmd-path, response-path, message-timeout), `JCODE_TEST_SESSION`
- JCODE runtime: `JCODE_HOME`, `JCODE_RUNTIME_DIR`, `JCODE_API_SOCKET`, `JCODE_SCRATCH_DIR`
- JCODE server lifecycle: `JCODE_TEMP_SERVER`, `JCODE_TEMP_SERVER_IDLE_SECS`, `JCODE_SERVER_SCOPE`, `JCODE_SERVER_OWNER_PID`, `JCODE_SERVER_NAME`, `JCODE_SERVER_DISPLAY_NAME`
- JCODE ambient/agent: `JCODE_AGENT_MODE`, `JCODE_AMBIENT_*` (enabled, model, provider, max/min interval, proactive), `JCODE_ACTIVE_PROVIDER`, `JCODE_RUNTIME_PROVIDER`, `JCODE_OPENROUTER_API_BASE`
- JCODE TUI/keymap: `JCODE_ANIMATION_FPS`, `JCODE_TYPING_SCROLL_LOCK_TOGGLE_KEY`, `JCODE_WORKSPACE_*_KEY` (4 vars), `JCODE_WAKE_MODE`
- JCODE provider profiles: `JCODE_PROVIDER_*_API_KEY` (many), `JCODE_ACP_PROFILE`, `JCODE_ACP_TOOL_PROFILE`, `JCODE_AUTH_LIFECYCLE_*`
- Provider-specific: `JCODE_GROK_CLI_PATH`, `JCODE_ANTIGRAVITY_VERSION`, `JCODE_ANTIGRAVITY_MODEL`, `JCODE_CHATGPT_WEB_TIMEOUT_SECS`
- Tooling/integration: `JCODE_GLYPH_SAFE_MODE`, `JCODE_PERF_TIER`, `JCODE_NATIVE_TOOLS`, `JCODE_TOOLS`, `JCODE_DISABLED_TOOLS`, `JCODE_TOOL_PROFILE`, `JCODE_DISABLE_BASE_TOOLS`, `JCODE_WEBSEARCH_ENGINE`, `JCODE_WEBSEARCH_FALLBACK_ENGINES`, `JCODE_WEDGE_FIXTURE`, `JCODE_CLIENT_SELFDEV_MODE`
- 3rd-party (read directly): `CODEX_HOME`, `COMPOSIO_*`, `JADE_RELAY_*`, `GHOSTTY_*`, `KITTY_*`, `WEZTERM_*`, `ITERM_SESSION_ID`, `TMUX_PANE`, `WSL_DISTRO_NAME`, `WT_SESSION`, `XDG_*`, etc.

### Check 5: Missing examples

`examples/` directories: 11 of 92 crates.

```
crates/jcode-telemetry-core/examples
crates/jcode-fuzzy/examples
crates/jcode-tui-anim/examples
crates/jcode-tui-mermaid/examples
crates/jcode-plan/examples
crates/jcode-tui-render/examples
crates/jcode-tui-style/examples
crates/jcode-harness-api/examples
crates/jcode-productivity-core/examples
crates/jcode-build-support/examples
sdk/typescript/examples
```

None of these are "core" infra crates: **jcode-cli, jcode-core, jcode-protocol, jcode-provider-core, jcode-tool-core, jcode-config-types, jcode-swarm-core, jcode-app-core, jcode-herdr all lack `examples/`**.

### Check 6: CHANGELOG

- File: `/Users/kooshapari/CodeProjects/Phenotype/repos/jcode/CHANGELOG.md` (4,874 bytes).
- Last entry: `v0.85.1-k1.1.0 (2026-09-15)` — HERDR integration release.
- Current Cargo.toml workspace version: `0.85.1-k1.1.1`.
- The `changelog/` directory contains per-release JSON files (`v0.34.0.json` through latest) but the top-level human-readable CHANGELOG is one release behind.
- Also: `changelog/README.md` exists (companion).

### Check 7: Architecture docs

- No top-level `ARCHITECTURE.md`.
- `find . -name "ARCHITECTURE*"` (excluding target/.git): no matches.
- Topic-specific docs in `docs/`:
  - `MEMORY_ARCHITECTURE.md`
  - `MODULAR_ARCHITECTURE_RFC.md`
  - `MULTI_SESSION_CLIENT_ARCHITECTURE.md`
  - `SERVER_ARCHITECTURE.md`
  - `SWARM_ARCHITECTURE.md`
  - `MERMAID_RENDERING_REDESIGN.md`
- No single overview of the 92-crate layout.

## Top-5 highest-leverage doc fixes

1. **Fix README version staleness (lines 15 and 55).** Trivial edit; high signal because anyone evaluating the fork reads these lines first. Two-version lag (`v0.85.0-k1.0.0` → `v0.85.1-k1.1.1`) is misleading.
2. **Document the env-var surface.** Either add a top-level "Environment variables" section to README (alphabetized table of `JCODE_*`, `HERDR_*`, plus the ~30 provider-specific API keys), or generate one from `crates/jcode-base/src/config/env_overrides.rs` + `crates/jcode-herdr/src/env.rs` + `env::var(`, `env::var_os(` call sites. Even covering only ~50 of the 429 vars in a curated table would close most of the gap.
3. **Add `///` to the foundational public types.** Specifically: `Provider` trait (`jcode-provider-core`), `SwarmRole`/`SwarmLifecycleStatus` (`jcode-swarm-core`), `CompactionMode`/`AgentMode` (`jcode-config-types`), `TranscriptMode`/`HistoryMessage` (`jcode-protocol`), `StdinInputRequest`/`ElicitRequest` (`jcode-tool-core`). These are the types every downstream crate and user interacts with; documenting them unlocks `cargo doc` for the whole workspace.
4. **Generate or write `ARCHITECTURE.md` (or `docs/ARCHITECTURE.md`) at the top level.** A one-page crate map (92 entries grouped by concern: providers, TUI, tools, server, swarm, types, runtime) plus a 5-line explanation of the layering. The existing `MODULAR_ARCHITECTURE_RFC.md` is closest but is an RFC, not a current-state map.
5. **Add at least one `examples/` program per top-level crate family.** Concretely: a `jcode-protocol/examples/serialize_transcript.rs` and `jcode-provider-core/examples/echo_provider.rs` would unblock new integrators. Other examples dirs (TUI, plan, telemetry) already exist as templates.

## Forward recommendation

This is a 92-crate workspace where the *code* is well-factored but the *documentation* is one major refactor behind it. Three concrete investments would close 80% of the gap: (1) a version-stale README fix; (2) a single auto-generated env-var reference table; (3) rustdoc pass on the 8-12 foundational public types listed above. None requires schema or work-area edits — all are doc-only and can ship in a single PR.

---

Generated by `~/.jcode/scratch/audit` documentation gap audit (read-only, 2026-10-17).