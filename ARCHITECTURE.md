# Architecture

This document is the **current-state crate map** for the KCode (jcode) workspace.
It complements the topic-specific docs in `docs/` (`SERVER_ARCHITECTURE.md`,
`SWARM_ARCHITECTURE.md`, `MEMORY_ARCHITECTURE.md`, `MODULAR_ARCHITECTURE_RFC.md`,
`MULTI_SESSION_CLIENT_ARCHITECTURE.md`) by giving newcomers a single overview of
all 92 crates in the workspace, the layering between them, and the rationale for
the split.

> **Scope.** The workspace ships as a single `jcode` binary (plus `jcode-harness`,
> `test_api`, and two benchmark binaries). Every other crate is a leaf in the
> compile graph: it is built into the binary via `pub use` re-exports, never as
> a separately loaded library at runtime.

## Layering in 5 lines

1. **Foundation (`*-types`)** — pure data contracts and small pure helpers, no
   I/O, no globals. The wire protocol, message shapes, session shapes, and
   config enums live here.
2. **Base (`jcode-base`)** — the *downward-closed* set of modules the upper
   layer depends on: provider selection, auth, config, session, message, memory,
   telemetry, and their supporting leaves.
3. **App core (`jcode-app-core`)** — the *upper* layer: server, tool, agent,
   ambient, overnight, batch, productivity, plus the presentation-adjacent
   leaves. Re-exports `jcode-base` so existing `crate::<module>` paths resolve
   unchanged.
4. **Presentation (`jcode-tui`)** — the TUI and the offline replay/video export.
   Re-exports `jcode-app-core`. Compiles as a separate rustc unit so terminal
   edits do not invalidate the larger app-core build.
5. **Root + CLI (`jcode`)** — the `main`/`lib` entrypoint plus the CLI on top
   of `jcode-tui`. Re-exports `jcode-tui` so `crate::tui`, `crate::server`,
   `crate::provider` paths still resolve from the CLI code.

The split is purely a **compile-graph** move, not a runtime one: every `pub use`
chain is transparent, and there is no versioned boundary between layers. The
trade-off is that refactors which cross layers (e.g. moving a type from
`jcode-base` to `jcode-app-core`) require re-exporting the old path in the
origin crate while the destination's dependents switch over. See
`docs/CRATE_OWNERSHIP_BOUNDARIES.md` for the ownership rules and the
move-checklist.

## Crate map (92 crates)

Crate names below are grouped by functional concern. Each row gives the
on-disk path (`crates/<name>`) and a one-line summary derived from the
crate's `Cargo.toml` `description` (when present), its top-of-file `//!`
rustdoc, or — for crates that still lack a public description — a hint of
what they hold (the most visible `pub` symbols).

### Foundation — data contracts (`*-types`)

These crates hold the cross-crate DTOs and small pure helpers. They must not
touch filesystem, network, process spawning, TUI state, or globals. The
dependency policy is `serde`, `chrono`, and other `*-types` crates only.

| Crate | Path | Holds |
|---|---|---|
| `jcode-ambient-types` | `crates/jcode-ambient-types` | `UsageSource`, `UsageRecord`, `RateLimitInfo` (ambient-mode usage DTOs) |
| `jcode-auth-types` | `crates/jcode-auth-types` | `AuthState`, `AuthCredentialSource`, `AuthExpiryConfidence`, `AuthRefreshSupport` |
| `jcode-background-types` | `crates/jcode-background-types` | `BackgroundTaskStatus`, `BackgroundTaskProgressKind`, `BackgroundTaskProgress` |
| `jcode-batch-types` | `crates/jcode-batch-types` | `BatchSubcallState`, `BatchSubcallProgress`, `BatchProgress` |
| `jcode-config-types` | `crates/jcode-config-types` | `CompactionMode`, `AgentMode`, `SessionPickerResumeAction`, `keybindings` |
| `jcode-gateway-types` | `crates/jcode-gateway-types` | `PairedDevice`, `PairingCode` (mobile/remote gateway pairing) |
| `jcode-memory-types` | `crates/jcode-memory-types` | `MemoryActivity`, `StepStatus`, `StepResult` (+ `graph` sub-module) |
| `jcode-message-types` | `crates/jcode-message-types` | `ToolCall`, `ToolDefinition`, `Role`, `Message` (the wire DTOs) |
| `jcode-selfdev-types` | `crates/jcode-selfdev-types` | `ReloadRecoveryDirective`, `SelfDevBuildCommand`, `SelfDevBuildTarget` |
| `jcode-session-types` | `crates/jcode-session-types` | `ResumeTarget`, `ResponseStats`, `RenderedMessage`, `RenderedCompactedHistoryInfo` |
| `jcode-side-panel-types` | `crates/jcode-side-panel-types` | `SidePanelPageFormat`, `SidePanelPageSource`, `PersistedSidePanelState` |
| `jcode-task-types` | `crates/jcode-task-types` | `GoalScope`, `GoalStatus`, `GoalStep`, `GoalMilestone` |
| `jcode-tool-types` | `crates/jcode-tool-types` | `ToolOutput`, `ToolImage`, `resolve_tool_name` |
| `jcode-usage-types` | `crates/jcode-usage-types` | `ProviderUsage`, `UsageLimit`, `ProviderUsageProgress`, `CopilotUsageTracker` |

### Foundation — behavior contracts

Slightly heavier than `*-types`: they hold behavior that the lower layer
needs but that still does not reach into the app core or presentation.

| Crate | Path | Holds |
|---|---|---|
| `jcode-compaction-core` | `crates/jcode-compaction-core` | `Summary`, `CompactionEvent`, `CompactionAction`, `CompactionStats` (transcript compaction policies) |
| `jcode-fuzzy` | `crates/jcode-fuzzy` | Typo-resistant subsequence matcher shared by the TUI and desktop UIs |
| `jcode-harness-api` | `crates/jcode-harness-api` | Stable, versioned **client API** for the harness (the public wire contract used by external SDKs) |
| `jcode-harness-api-server` | `crates/jcode-harness-api-server` | Harness API bridge: exposes the versioned API on its own Unix socket and translates to the internal protocol |
| `jcode-sdk` | `crates/jcode-sdk` | Rust SDK for the harness API: connect, drive sessions, stream events |
| `jcode-protocol` | `crates/jcode-protocol` | The internal **client-server protocol** (newline-delimited JSON over Unix socket). `TranscriptMode`, `HistoryMessage` etc. |
| `jcode-render-core` | `crates/jcode-render-core` | Backend-neutral document/render model shared by jcode's front-ends |
| `jcode-schema-dialect` | `crates/jcode-schema-dialect` | Provider JSON-Schema compatibility, expressed as a system rather than a patch queue |
| `jcode-tool-search` | `crates/jcode-tool-search` | Deferred tool loading and search for reduced initial-prompt token cost |
| `jcode-transport` | `crates/jcode-transport` | Local IPC transport: Unix sockets on Unix, named pipes on Windows, one API |

### Provider system

The provider crates split into three layers: **metadata** (login catalog,
display names), **core** (the trait, selection, retry, fingerprint logic),
**per-provider** (provider-shaped request/response helpers), and **per-provider
runtime** (the actual HTTP/CLI driver — kept out of `jcode-base` so provider
edits compile only the affected runtime crate).

| Crate | Path | Holds |
|---|---|---|
| `jcode-provider-metadata` | `crates/jcode-provider-metadata` | `LoginProviderAuthKind`, `LoginProviderTarget`, `LoginProviderAuthStateKey`, `LoginProviderSurface` (login catalog) |
| `jcode-provider-env` | `crates/jcode-provider-env` | `register_api_key_fallback_resolver`, `sanitize_secret_value`, `load_api_key_from_env_or_config` |
| `jcode-provider-core` | `crates/jcode-provider-core` | The `Provider` trait, `attempt_tracker`, `auth_mode`, `catalog_refresh`, `failover`, `fallback_pick`, `fingerprint`, `model_id`, `models`, `pricing`, `reasoning`, `retry_after`, `selection`, `transport` |
| `jcode-provider-doctor` | `crates/jcode-provider-doctor` | Provider-doctor diagnostics (sits downstream of `jcode-base` so doctor edits don't recompile the rest) |
| `jcode-provider-anthropic` | `crates/jcode-provider-anthropic` | Anthropic-shaped request/response helpers (`format_messages` etc.) |
| `jcode-provider-bedrock` | `crates/jcode-provider-bedrock` | `BedrockProvider` (AWS Bedrock adapter) |
| `jcode-provider-copilot` | `crates/jcode-provider-copilot` | Copilot catalog helpers (`PersistedCatalog`, `is_known_display_model`, `max_token_parameter_for_model`, `add_max_token_parameter`) |
| `jcode-provider-openai` | `crates/jcode-provider-openai` | OpenAI request/stream/websocket-health helpers |
| `jcode-provider-openrouter` | `crates/jcode-provider-openrouter` | OpenRouter helpers + `known_providers`, `ModelInfo` |
| `jcode-provider-gemini` | `crates/jcode-provider-gemini` | Gemini runtime state and Code Assist load/response types |
| `jcode-provider-antigravity` | `crates/jcode-provider-antigravity` | Antigravity (Google Cloud Code) catalog types |
| `jcode-provider-anthropic-runtime` | `crates/jcode-provider-anthropic-runtime` | Anthropic Messages API runtime (OAuth + API key, SSE streaming) |
| `jcode-provider-claude-cli-runtime` | `crates/jcode-provider-claude-cli-runtime` | **Deprecated** Claude CLI subprocess transport (kept for backwards-compat) |
| `jcode-provider-copilot-runtime` | `crates/jcode-provider-copilot-runtime` | GitHub Copilot direct API (bearer exchange, tier detection, premium modes) |
| `jcode-provider-cursor-runtime` | `crates/jcode-provider-cursor-runtime` | Cursor ChatService HTTP/2 streaming |
| `jcode-provider-forgecode-runtime` | `crates/jcode-provider-forgecode-runtime` | ForgeCode CLI subprocess transport |
| `jcode-provider-gemini-runtime` | `crates/jcode-provider-gemini-runtime` | Gemini runtime (Code Assist OAuth + Developer API key) |
| `jcode-provider-antigravity-runtime` | `crates/jcode-provider-antigravity-runtime` | Antigravity (Google Cloud Code multiplexed Gemini/Claude) runtime |
| `jcode-provider-grok-build-runtime` | `crates/jcode-provider-grok-build-runtime` | Grok Build subscription runtime over the Grok CLI ACP stdio transport (no xAI HTTP) |
| `jcode-provider-openai-runtime` | `crates/jcode-provider-openai-runtime` | OpenAI runtime (Codex OAuth + API key, Responses API over SSE/WebSocket) |
| `jcode-provider-openrouter-runtime` | `crates/jcode-provider-openrouter-runtime` | OpenRouter / OpenAI-compatible runtime (aggregator + direct profile endpoints) |

### Core behavior (`jcode-core`)

`jcode-core` is the **dependency-light primitives** crate: cross-domain helpers
that do not yet warrant a focused domain crate. See the ownership policy in
`docs/CRATE_OWNERSHIP_BOUNDARIES.md` — clusters that grow should be split out.

| Crate | Path | Holds |
|---|---|---|
| `jcode-core` | `crates/jcode-core` | `console`, `env`, `fs`, `id` (and other small primitives) |

### App core (`jcode-base` + `jcode-app-core`)

The bulk of the runtime. `jcode-base` is the downward-closed layer; `jcode-app-core`
is the upper layer that builds on it. Both compile as separate rustc units to
halve the largest compile unit's peak memory.

| Crate | Path | Holds |
|---|---|---|
| `jcode-base` | `crates/jcode-base` | The foundational layer: `auth`, `background`, `browser`, `bus`, `cache_invalidation`, `config`, `provider`, `session`, `message`, `memory`, `telemetry`, ... |
| `jcode-app-core` | `crates/jcode-app-core` | The upper layer: `agent`, `ambient`, `ambient_runner`, `ambient_scheduler`, `build`, `server`, `tools` |
| `jcode-agent-runtime` | `crates/jcode-agent-runtime` | `SoftInterruptMessage`, `SoftInterruptSource`, `InterruptSignal`, `StreamError` (agent-loop signal types) |
| `jcode-auto-dream` | `crates/jcode-auto-dream` | Background memory consolidation with three-gate triggering (`scan_throttle`, `time_gate`, `session_gate`, `consolidation_lock`, `consolidation_prompt`) |
| `jcode-azure-auth` | `crates/jcode-azure-auth` | Azure auth helpers (binary-only support modules) |
| `jcode-cache-vectors` | `crates/jcode-cache-vectors` | Prompt-cache vector hashing and cache-break detection |
| `jcode-command-risk` | `crates/jcode-command-risk` | Deterministic risk classification for shell commands (drives permission prompts) |
| `jcode-embedding` | `crates/jcode-embedding` | The `Embedder` trait and embedding adapters |
| `jcode-import-core` | `crates/jcode-import-core` | `SessionIndexEntry`, `SessionsIndex`, `ClaudeCodeSessionInfo` (foreign-session import) |
| `jcode-micro-compact` | `crates/jcode-micro-compact` | Micro-compaction: clears old tool results before each API call without summarising |
| `jcode-notify-email` | `crates/jcode-notify-email` | `ReplyAction`, `SendEmailRequest`, `poll_imap_once`, `extract_permission_id` (IMAP-based permission bridge) |
| `jcode-overnight-core` | `crates/jcode-overnight-core` | `OvernightDuration`, `OvernightCommand`, `OvernightRunStatus`, `OvernightManifest` (long-running scheduled runs) |
| `jcode-pdf` | `crates/jcode-pdf` | `extract_text` (PDF attachment reader) |
| `jcode-permission-bubble` | `crates/jcode-permission-bubble` | Permission bubble: subagents inherit parent context and bubble permission prompts back to the parent |
| `jcode-productivity-core` | `crates/jcode-productivity-core` | Productivity report generation (scans local session transcripts, computes roll-ups) |
| `jcode-session-memory` | `crates/jcode-session-memory` | Structured session notes with 10-section layout, token counting, recall |
| `jcode-shell-integration` | `crates/jcode-shell-integration` | Shell detection, hook generation, completions, prompt functions |
| `jcode-storage` | `crates/jcode-storage` | `runtime_dir`, `jcode_dir`, `running_with_sandboxed_home`, `logs_dir` (process-local storage paths) |
| `jcode-swarm-core` | `crates/jcode-swarm-core` | `SwarmRole`, `SwarmLifecycleStatus`, `validate_swarm_tldr`, `derive_swarm_task_label` (the swarm subsystem's public types and validation) |
| `jcode-telemetry-core` | `crates/jcode-telemetry-core` | `DiscoveryTelemetry`, `TelemetryScoreSummary`, `TodoTelemetryUpdate` (event ingestion) |
| `jcode-tool-core` | `crates/jcode-tool-core` | Tool schema helpers (`intent_schema_property`, `accept_large_output_schema_property`, `ensure_intent_in_schema`) + `StdinInputRequest`, `ElicitRequest` |
| `jcode-update-core` | `crates/jcode-update-core` | `DownloadProgress`, `UpdateEstimate`, `GitHubRelease`, `GitHubAsset` (self-update helpers) |

### Plan & graph

| Crate | Path | Holds |
|---|---|---|
| `jcode-plan` | `crates/jcode-plan` | `PlanItem`, `VersionedPlan`, `bridge`, `dag`, `mermaid` (multi-step plan model used by swarm and by the TUI mermaid renderer) |

### TUI presentation

The TUI is split into one orchestrator crate (`jcode-tui`) and a dozen focused
sub-crates so that a TUI edit to a single widget recompiles only that widget,
not the whole app core.

| Crate | Path | Holds |
|---|---|---|
| `jcode-tui` | `crates/jcode-tui` | Orchestrator: `tui` module (ratatui/crossterm state) + `video_export` (offline replay) |
| `jcode-tui-core` | `crates/jcode-tui-core` | `copy_selection`, `graph_topology`, `anchor_stability`, `keybind` (shared TUI primitives) |
| `jcode-tui-style` | `crates/jcode-tui-style` | `color`, `harmony`, `palette`, `theme` (theme + color science) |
| `jcode-tui-render` | `crates/jcode-tui-render` | `chrome`, `layout`, `memory_tiles`, `swarm_gallery` (top-level widget frames) |
| `jcode-tui-messages` | `crates/jcode-tui-messages` | Message-list rendering |
| `jcode-tui-markdown` | `crates/jcode-tui-markdown` | `MarkdownConfigSnapshot`, `ProcessMemorySnapshot` (markdown rendering config + memory guards) |
| `jcode-tui-mermaid` | `crates/jcode-tui-mermaid` | Mermaid diagram rendering for terminal display (renders to PNG, displays via terminal image) |
| `jcode-tui-anim` | `crates/jcode-tui-anim` | Pure, dependency-free math kernels for the TUI idle animations |
| `jcode-tui-visual-debug` | `crates/jcode-tui-visual-debug` | Visual Debug Infrastructure — captures TUI frame state for autonomous debugging by AI agents |
| `jcode-tui-account-picker` | `crates/jcode-tui-account-picker` | `AccountProviderKind`, `AccountPickerCommand`, `AccountPickerItem`, `AccountPickerSummary` |
| `jcode-tui-session-picker` | `crates/jcode-tui-session-picker` | `SessionSource`, `SessionFilterMode`, `SessionInfo`, `ServerGroup` |
| `jcode-tui-permissions` | `crates/jcode-tui-permissions` | Permission-prompt TUI widget |
| `jcode-tui-tool-display` | `crates/jcode-tui-tool-display` | `resolve_display_tool_name`, `canonical_tool_name`, `is_edit_tool_name`, `truncate_middle_display` |
| `jcode-tui-usage-overlay` | `crates/jcode-tui-usage-overlay` | `UsageOverlayStatus`, `UsageOverlayItem`, `UsageOverlaySummary` (the provider-usage overlay) |
| `jcode-tui-workspace` | `crates/jcode-tui-workspace` | `color_support`, `workspace_map`, `workspace_map_widget` (the workspace map overlay) |

### Herdr / multi-harness

`HERDR` is the cross-harness terminal multiplexer. The `jcode-herdr` crate is
both a terminal-integration leaf (it lives in the TUI compile graph) and a
multi-harness peer (it talks to other harnesses through HERDR panes). It is
listed here once and not under *Terminal & platform* above.

| Crate | Path | Holds |
|---|---|---|
| `jcode-herdr` | `crates/jcode-herdr` | Lifecycle state reporting, session identity, screen management, and HERDR pane integration (lives in the TUI compile graph and is the jcode end of the HERDR cross-harness protocol). |

### Build, logging, runtime support

| Crate | Path | Holds |
|---|---|---|
| `jcode-build-meta` | `crates/jcode-build-meta` | Build/runtime version metadata (build script computes git- and version-derived values) |
| `jcode-build-support` | `crates/jcode-build-support` | `BuildManifest`, `install_binary_at_version`, `complete_pending_activation_for_session`, `rollback_pending_activation_for_session` (selfdev activation) |
| `jcode-logging` | `crates/jcode-logging` | File logging, structured events, and thread/task-local logging context (logs to `~/.jcode/logs/`) |
| `jcode-setup-hints` | `crates/jcode-setup-hints` | Platform setup hints shown on startup (Windows hotkeys, terminal detection, ...) |

### Terminal & platform

| Crate | Path | Holds |
|---|---|---|
| `jcode-terminal-detect` | `crates/jcode-terminal-detect` | Terminal emulator detection and escape-sequence helpers |
| `jcode-terminal-image` | `crates/jcode-terminal-image` | Low-level terminal image primitives (Kitty/iTerm2/Sixel) |
| `jcode-terminal-launch` | `crates/jcode-terminal-launch` | `TerminalCommand`, `snapshot_client_terminal_env`, `apply_client_terminal_env`, `SpawnAttempt` |

## How the binary is built

`Cargo.toml` at the root declares five binaries:

| Binary | Path | Purpose |
|---|---|---|
| `jcode` | `src/main.rs` | The default CLI/TUI binary (the `jcode` you run) |
| `test_api` | `src/bin/test_api.rs` | Black-box harness-API test driver |
| `jcode-harness` | `src/bin/harness.rs` | The `jcode-harness` server process started by HERDR panes (long-lived, IPC-driven) |
| `session_memory_bench` | `src/bin/session_memory_bench.rs` | Session-memory write/recall benchmark |
| `memory_recall_bench` | `src/bin/memory_recall_bench.rs` | Memory recall benchmark |

All five binaries are built from the same root `jcode` crate. The compile
graph is: `jcode-base` + every `*-types` crate + every other workspace crate
→ `jcode-app-core` → `jcode-tui` → `jcode` (lib) → binaries.

## Where to read next

| Question | Read |
|---|---|
| What is the wire protocol? | `docs/SERVER_ARCHITECTURE.md`, `crates/jcode-protocol/src/lib.rs` |
| How does the swarm subsystem work? | `docs/SWARM_ARCHITECTURE.md`, `crates/jcode-swarm-core/src/lib.rs` |
| How does memory work? | `docs/MEMORY_ARCHITECTURE.md` |
| Why are the crates split this way? | `docs/MODULAR_ARCHITECTURE_RFC.md`, `docs/CRATE_OWNERSHIP_BOUNDARIES.md` |
| How does an external SDK talk to jcode? | `crates/jcode-harness-api/src/lib.rs`, `crates/jcode-sdk/src/lib.rs` |
| What env vars does jcode read? | `docs/ENVIRONMENT.md` |
| How do I add a new tool? | `crates/jcode-tool-core/src/lib.rs`, `docs/TOOLS.md` (if present) |
| How do I add a new provider? | `crates/jcode-provider-core/src/lib.rs`, then add a new `jcode-provider-<name>` + `jcode-provider-<name>-runtime` pair |

## Open documentation gaps

This map was written in the same docs-gap audit pass that produced
`docs/ENVIRONMENT.md` and the rustdoc pass on the 8 foundation types. The
following items remain:

- **52 of 92 crates still lack a module-level `//!` description.** The
  per-crate descriptions in this document are derived from
  `crates/<name>/Cargo.toml` `description` (where present), the top-of-file
  `//!` block, or a sentence inferred from the most prominent `pub` items.
  Adding one or two lines of `//!` to each of those `lib.rs` files would
  unblock `cargo doc --no-deps --document-private-items` for the whole
  workspace and make this map derivable rather than hand-written.
- **No top-level `Cargo.toml` description for non-provider leaves.** Most
  `jcode-*-types` crates and several runtime leaves do not set
  `description = "..."`, so `cargo metadata` cannot surface them.
- **No `examples/` directory for the 9 most-likely-to-be-reused crates**
  (`jcode-cli`, `jcode-core`, `jcode-protocol`, `jcode-provider-core`,
  `jcode-tool-core`, `jcode-config-types`, `jcode-swarm-core`,
  `jcode-app-core`, `jcode-herdr`). This is a separate docs gap (Fix 5
  of the audit); see `docs/EXAMPLES.md` once written.
