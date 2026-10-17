# Environment variables

This document catalogues every `JCODE_*` and selected third-party environment
variable that the workspace reads at runtime. The total count is **625** env vars across the workspace (audit 2026-10-17).

Source of truth: each `std::env::var(...)`, `env::var(...)`, or `env::var_os(...)`
call site in `crates/` and `src/`. The first table is generated from
`crates/jcode-base/src/config/env_overrides.rs`. The other tables are derived
from a workspace-wide `rg` pass.

If a variable is missing or undocumented, please file a docs issue; this file
is regenerated on each release cut.

<!-- auto-generated from crates/* source on 2026-10-17 by raven (top5 doc fixes) -->

## Quick reference

- Total env vars documented: **625**
  - `JCODE_*` in env_overrides: **151**
  - `JCODE_*` read directly elsewhere: **344**
  - Third-party: **130**
- Run `rg -N 'std::env::var\(\"[A-Z]' crates src` to regenerate this catalog
- Run `rg -N 'std::env::var\(' crates src | rg -o '"[A-Z][A-Z0-9_]*"' | sort -u`
  to see every string-literal env-var lookup the workspace performs

---

### Server/operator behavior

| Env var | Default | Used by | Description |
|---|---|---|---|
| `JCODE_WAKE_MODE` | (unset) | jcode-base | Override `server.wake_mode` config field |

### Keybindings

| Env var | Default | Used by | Description |
|---|---|---|---|
| `JCODE_SCROLL_UP_KEY` | (unset) | jcode-base | Override `keybindings.scroll_up` config field |
| `JCODE_SCROLL_DOWN_KEY` | (unset) | jcode-base | Override `keybindings.scroll_down` config field |
| `JCODE_SCROLL_PAGE_UP_KEY` | (unset) | jcode-base | Override `keybindings.scroll_page_up` config field |
| `JCODE_SCROLL_PAGE_DOWN_KEY` | (unset) | jcode-base | Override `keybindings.scroll_page_down` config field |
| `JCODE_MODEL_SWITCH_KEY` | (unset) | jcode-base | Override `keybindings.model_switch_next` config field |
| `JCODE_MODEL_SWITCH_PREV_KEY` | (unset) | jcode-base | Override `keybindings.model_switch_prev` config field |
| `JCODE_EFFORT_INCREASE_KEY` | (unset) | jcode-base | Override `keybindings.effort_increase` config field |
| `JCODE_EFFORT_DECREASE_KEY` | (unset) | jcode-base | Override `keybindings.effort_decrease` config field |
| `JCODE_CENTERED_TOGGLE_KEY` | (unset) | jcode-base | Override `keybindings.centered_toggle` config field |
| `JCODE_SCROLL_PROMPT_UP_KEY` | (unset) | jcode-base | Override `keybindings.scroll_prompt_up` config field |
| `JCODE_SCROLL_PROMPT_DOWN_KEY` | (unset) | jcode-base | Override `keybindings.scroll_prompt_down` config field |
| `JCODE_SCROLL_BOOKMARK_KEY` | (unset) | jcode-base | Override `keybindings.scroll_bookmark` config field |
| `JCODE_SCROLL_UP_FALLBACK_KEY` | (unset) | jcode-base | Override `keybindings.scroll_up_fallback` config field |
| `JCODE_SCROLL_DOWN_FALLBACK_KEY` | (unset) | jcode-base | Override `keybindings.scroll_down_fallback` config field |
| `JCODE_WORKSPACE_LEFT_KEY` | (unset) | jcode-base | Override `keybindings.workspace_left` config field |
| `JCODE_WORKSPACE_DOWN_KEY` | (unset) | jcode-base | Override `keybindings.workspace_down` config field |
| `JCODE_WORKSPACE_UP_KEY` | (unset) | jcode-base | Override `keybindings.workspace_up` config field |
| `JCODE_WORKSPACE_RIGHT_KEY` | (unset) | jcode-base | Override `keybindings.workspace_right` config field |
| `JCODE_SIDE_PANEL_TOGGLE_KEY` | (unset) | jcode-base | Override `keybindings.side_panel_toggle` config field |
| `JCODE_COPY_SELECTION_TOGGLE_KEY` | (unset) | jcode-base | Override `keybindings.copy_selection_toggle` config field |
| `JCODE_DIAGRAM_PANE_TOGGLE_KEY` | (unset) | jcode-base | Override `keybindings.diagram_pane_toggle` config field |
| `JCODE_TYPING_SCROLL_LOCK_TOGGLE_KEY` | (unset) | jcode-base | Override `keybindings.typing_scroll_lock_toggle` config field |
| `JCODE_DIFF_MODE_CYCLE_KEY` | (unset) | jcode-base | Override `keybindings.diff_mode_cycle` config field |
| `JCODE_INFO_WIDGET_TOGGLE_KEY` | (unset) | jcode-base | Override `keybindings.info_widget_toggle` config field |
| `JCODE_NEW_TERMINAL_KEY` | (unset) | jcode-base | Override `keybindings.new_terminal` config field |

### Dictation

| Env var | Default | Used by | Description |
|---|---|---|---|
| `JCODE_DICTATION_COMMAND` | (unset) | jcode-base | Override `dictation.command` config field |
| `JCODE_DICTATION_MODE` | (unset) | jcode-base | Override `—` config field |
| `JCODE_DICTATION_KEY` | (unset) | jcode-base | Override `dictation.key` config field |
| `JCODE_DICTATION_TIMEOUT_SECS` | (unset) | jcode-base | Override the `dictation.timeout_secs` duration |

### Tools

| Env var | Default | Used by | Description |
|---|---|---|---|
| `JCODE_TOOL_PROFILE` | (unset) | jcode-base | Override `tools.profile` config field |
| `JCODE_TOOLS` | (unset) | jcode-base | Override `tools.enabled` config field |
| `JCODE_DISABLED_TOOLS` | (unset) | jcode-base | Override `tools.disabled` config field |
| `JCODE_DISABLE_BASE_TOOLS` | (unset) | jcode-base | Override `tools.disable_base_tools` config field |
| `JCODE_MCP_TOOLS` | (unset) | jcode-base | Override `tools.mcp_tools` config field |
| `JCODE_MCP_TOOLS_TOKEN_THRESHOLD` | (unset) | jcode-base | Override `tools.mcp_tools_token_threshold` config field |

### ACP adapter

| Env var | Default | Used by | Description |
|---|---|---|---|
| `JCODE_ACP_PROFILE` | (unset) | jcode-base | Override `acp.profile` config field |
| `JCODE_ACP_TOOL_PROFILE` | (unset) | jcode-base | Override `acp.tool_profile` config field |

### Display

| Env var | Default | Used by | Description |
|---|---|---|---|
| `JCODE_DIFF_MODE` | (unset) | jcode-base | Override `display.diff_mode` config field |
| `JCODE_SHOW_DIFFS` | (unset) | jcode-base | Override `display.diff_mode` config field |
| `JCODE_PIN_IMAGES` | (unset) | jcode-base | Override `display.pin_images` config field |
| `JCODE_PIN_TODOS` | (unset) | jcode-base | Override `display.pin_todos` config field |
| `JCODE_DISPLAY_CENTERED` | (unset) | jcode-base | Override `display.centered` config field |
| `JCODE_DIFF_LINE_WRAP` | (unset) | jcode-base | Override `display.diff_line_wrap` config field |
| `JCODE_QUEUE_MODE` | (unset) | jcode-base | Override `display.queue_mode` config field |
| `JCODE_AUTO_SERVER_RELOAD` | (unset) | jcode-base | Override `display.auto_server_reload` config field |
| `JCODE_MOUSE_CAPTURE` | (unset) | jcode-base | Override `display.mouse_capture` config field |
| `JCODE_DEBUG_SOCKET` | (unset) | jcode-base | Override `display.debug_socket` config field |
| `JCODE_NO_EMOJI` | (unset) | jcode-base | Override `—` config field |
| `JCODE_SHOW_THINKING` | (unset) | jcode-base | Override `display.show_thinking` config field |
| `JCODE_REASONING_DISPLAY` | (unset) | jcode-base | Override `—` config field |
| `JCODE_DEFAULT_REASONING_DISPLAY` | (unset) | jcode-base | Override `—` config field |
| `JCODE_MARKDOWN_SPACING` | (unset) | jcode-base | Override `display.markdown_spacing` config field |
| `JCODE_IDLE_ANIMATION` | (unset) | jcode-base | Override `display.idle_animation` config field |
| `JCODE_PROMPT_ENTRY_ANIMATION` | (unset) | jcode-base | Override `display.prompt_entry_animation` config field |
| `JCODE_DISABLED_ANIMATIONS` | (unset) | jcode-base | Override `display.disabled_animations` config field |
| `JCODE_ACTIVE_SESSIONS_MANAGER` | (unset) | jcode-base | Override `display.active_sessions_manager` config field |
| `JCODE_EXTERNAL_SESSIONS` | (unset) | jcode-base | Override `display.external_sessions` config field |
| `JCODE_PERFORMANCE` | (unset) | jcode-base | Override `display.performance` config field |
| `JCODE_ANIMATION_FPS` | (unset) | jcode-base | Override `display.animation_fps` config field |
| `JCODE_REDRAW_FPS` | (unset) | jcode-base | Override `display.redraw_fps` config field |
| `JCODE_COPY_BADGE_ALT_LABEL` | (unset) | jcode-base | Override `display.copy_badge_alt_label` config field |
| `JCODE_COMPACT_NOTIFICATIONS` | (unset) | jcode-base | Override `display.compact_notifications` config field |
| `JCODE_SHOW_AGENTGREP_OUTPUT` | (unset) | jcode-base | Override `display.show_agentgrep_output` config field |
| `JCODE_SHOW_BASH_OUTPUT` | (unset) | jcode-base | Override `display.show_bash_output` config field |
| `JCODE_TOOL_CALL_DETAILS` | (unset) | jcode-base | Override `display.tool_call_details` config field |
| `JCODE_LATEX_RENDERING` | (unset) | jcode-base | Override `display.latex_rendering` config field |
| `JCODE_CHAT_NATIVE_SCROLLBAR` | (unset) | jcode-base | Override `display.native_scrollbars.chat` config field |
| `JCODE_SIDE_PANEL_NATIVE_SCROLLBAR` | (unset) | jcode-base | Override `display.native_scrollbars.side_panel` config field |

### Features

| Env var | Default | Used by | Description |
|---|---|---|---|
| `JCODE_MEMORY_ENABLED` | (unset) | jcode-base | Override `features.memory` config field |
| `JCODE_SWARM_ENABLED` | (unset) | jcode-base | Override `features.swarm` config field |
| `JCODE_ENABLE_MERMAID` | (unset) | jcode-base | Override `features.mermaid` config field |
| `JCODE_CHECK_UPDATES` | (unset) | jcode-base | Override `features.check_updates` config field |
| `JCODE_AUTO_POKE` | (unset) | jcode-base | Override `features.auto_poke` config field |
| `JCODE_MESSAGE_TIMESTAMPS` | (unset) | jcode-base | Override `features.message_timestamps` config field |
| `JCODE_PERSIST_MEMORY_INJECTIONS` | (unset) | jcode-base | Override `features.persist_memory_injections` config field |
| `JCODE_KV_CACHE_MISS_NOTICES` | (unset) | jcode-base | Override `features.kv_cache_miss_notices` config field |
| `JCODE_UPDATE_CHANNEL` | (unset) | jcode-base | Override `features.update_channel` config field |

### Agents (spawned helper sessions)

| Env var | Default | Used by | Description |
|---|---|---|---|
| `JCODE_SWARM_MODEL` | (unset) | jcode-base | Override the `agents.swarm_model` model name |
| `JCODE_SWARM_EFFORT` | (unset) | jcode-base | Override `agents.swarm_effort` config field |
| `JCODE_SWARM_SPAWN_MODE` | (unset) | jcode-base | Override `agents.swarm_spawn_mode` config field |
| `JCODE_SWARM_STRIP_LAYOUT` | (unset) | jcode-base | Override `agents.swarm_strip_layout` config field |
| `JCODE_SWARM_MAX_CONCURRENT_AGENTS` | (unset) | jcode-base | Override `agents.swarm_max_concurrent_agents` config field |
| `JCODE_MEMORY_MODEL` | (unset) | jcode-base | Override the `agents.memory_model` model name |
| `JCODE_MEMORY_SIDECAR_ENABLED` | (unset) | jcode-base | Toggle `agents.memory_sidecar_enabled` on/off |
| `JCODE_MEMORY_EMBEDDING_BACKEND` | (unset) | jcode-base | Override `agents.memory_embedding_backend` config field |
| `JCODE_MEMORY_EMBEDDING_MODEL` | (unset) | jcode-base | Override the `agents.memory_embedding_model` model name |
| `JCODE_MEMORY_EMBEDDING_BASE_URL` | (unset) | jcode-base | Override the `agents.memory_embedding_base_url` URL/path |
| `JCODE_MEMORY_EMBEDDING_DIM` | (unset) | jcode-base | Override `agents.memory_embedding_dim` config field |

### Terminal spawning

| Env var | Default | Used by | Description |
|---|---|---|---|
| `JCODE_SPAWN_HOOK` | (unset) | jcode-base | Override `terminal.spawn_hook` config field |
| `JCODE_FOCUS_HOOK` | (unset) | jcode-base | Override `terminal.focus_hook` config field |
| `JCODE_HOOK_PRE_TOOL_TIMEOUT_MS` | (unset) | jcode-base | Override the `hooks.pre_tool_timeout_ms` duration |

### Web search

| Env var | Default | Used by | Description |
|---|---|---|---|
| `JCODE_WEBSEARCH_ENGINE` | (unset) | jcode-base | Override `websearch.engine` config field |
| `JCODE_WEBSEARCH_FALLBACK_ENGINES` | (unset) | jcode-base | Override `—` config field |
| `JCODE_BING_API_KEY` | (unset) | jcode-base | Override the `websearch.bing_api_key` keybinding value (string) |
| `JCODE_BING_API_KEY_ENV` | (unset) | jcode-base | Override the `websearch.bing_api_key_env` keybinding value (string) |
| `JCODE_BING_MARKET` | (unset) | jcode-base | Override `websearch.bing_market` config field |
| `JCODE_SEARXNG_URL` | (unset) | jcode-base | Override the `websearch.searxng_url` URL/path |
| `JCODE_TRUSTED_EXTERNAL_AUTH_SOURCES` | (unset) | jcode-base | Override `—` config field |

### Autoreview

| Env var | Default | Used by | Description |
|---|---|---|---|
| `JCODE_AUTOREVIEW_ENABLED` | (unset) | jcode-base | Override `autoreview.enabled` config field |
| `JCODE_AUTOREVIEW_MODEL` | (unset) | jcode-base | Override `autoreview.model` config field |

### Autojudge

| Env var | Default | Used by | Description |
|---|---|---|---|
| `JCODE_AUTOJUDGE_ENABLED` | (unset) | jcode-base | Override `autojudge.enabled` config field |
| `JCODE_AUTOJUDGE_MODEL` | (unset) | jcode-base | Override `autojudge.model` config field |

### Ambient

| Env var | Default | Used by | Description |
|---|---|---|---|
| `JCODE_AMBIENT_ENABLED` | (unset) | jcode-base | Override `ambient.enabled` config field |
| `JCODE_AMBIENT_PROVIDER` | (unset) | jcode-base | Override `ambient.provider` config field |
| `JCODE_AMBIENT_MODEL` | (unset) | jcode-base | Override `ambient.model` config field |
| `JCODE_AMBIENT_MIN_INTERVAL` | (unset) | jcode-base | Override `ambient.min_interval_minutes` config field |
| `JCODE_AMBIENT_MAX_INTERVAL` | (unset) | jcode-base | Override `ambient.max_interval_minutes` config field |
| `JCODE_AMBIENT_PROACTIVE` | (unset) | jcode-base | Override `ambient.proactive_work` config field |

### Safety / notifications

| Env var | Default | Used by | Description |
|---|---|---|---|
| `JCODE_NTFY_TOPIC` | (unset) | jcode-base | Override `safety.ntfy_topic` config field |
| `JCODE_NTFY_SERVER` | (unset) | jcode-base | Override `safety.ntfy_server` config field |
| `JCODE_SMTP_PASSWORD` | (unset) | jcode-base | Override `safety.email_password` config field |
| `JCODE_EMAIL_TO` | (unset) | jcode-base | Override `safety.email_to` config field |
| `JCODE_IMAP_HOST` | (unset) | jcode-base | Override `safety.email_imap_host` config field |
| `JCODE_EMAIL_REPLY_ENABLED` | (unset) | jcode-base | Toggle `safety.email_reply_enabled` on/off |
| `JCODE_TELEGRAM_BOT_TOKEN` | (unset) | jcode-base | Override `safety.telegram_bot_token` config field |
| `JCODE_TELEGRAM_CHAT_ID` | (unset) | jcode-base | Override `safety.telegram_chat_id` config field |
| `JCODE_TELEGRAM_REPLY_ENABLED` | (unset) | jcode-base | Toggle `safety.telegram_reply_enabled` on/off |
| `JCODE_DISCORD_BOT_TOKEN` | (unset) | jcode-base | Override `safety.discord_bot_token` config field |
| `JCODE_DISCORD_CHANNEL_ID` | (unset) | jcode-base | Override `safety.discord_channel_id` config field |
| `JCODE_DISCORD_BOT_USER_ID` | (unset) | jcode-base | Override `safety.discord_bot_user_id` config field |
| `JCODE_DISCORD_REPLY_ENABLED` | (unset) | jcode-base | Toggle `safety.discord_reply_enabled` on/off |

### Jade cloud relay channel

| Env var | Default | Used by | Description |
|---|---|---|---|
| `JCODE_JADE_RELAY_API_BASE` | (unset) | jcode-base | Override the `safety.jade_relay_api_base` URL/path |
| `JCODE_JADE_RELAY_TOKEN` | (unset) | jcode-base | Override `safety.jade_relay_token` config field |
| `JCODE_JADE_RELAY_TOKEN_ID` | (unset) | jcode-base | Override `safety.jade_relay_token_id` config field |
| `JCODE_JADE_RELAY_USER_ID` | (unset) | jcode-base | Override `safety.jade_relay_user_id` config field |
| `JCODE_JADE_RELAY_SESSION_ID` | (unset) | jcode-base | Override `safety.jade_relay_session_id` config field |
| `JCODE_JADE_RELAY_ENABLED` | (unset) | jcode-base | Toggle `safety.jade_relay_enabled` on/off |
| `JCODE_JADE_RELAY_REPLY_ENABLED` | (unset) | jcode-base | Toggle `safety.jade_relay_reply_enabled` on/off |
| `JCODE_JADE_RELAY_LAUNCH_ENABLED` | (unset) | jcode-base | Toggle `safety.jade_relay_launch_enabled` on/off |
| `JCODE_JADE_RELAY_LAUNCH_WORKING_DIR` | (unset) | jcode-base | Override `safety.jade_relay_launch_working_dir` config field |
| `JCODE_AMBIENT_VISIBLE` | (unset) | jcode-base | Override `ambient.visible` config field |

### Gateway (iOS/web)

| Env var | Default | Used by | Description |
|---|---|---|---|
| `JCODE_GATEWAY_ENABLED` | (unset) | jcode-base | Override `gateway.enabled` config field |
| `JCODE_GATEWAY_PORT` | (unset) | jcode-base | Override `gateway.port` config field |
| `JCODE_GATEWAY_BIND_ADDR` | (unset) | jcode-base | Override `gateway.bind_addr` config field |

### Power management

| Env var | Default | Used by | Description |
|---|---|---|---|
| `JCODE_PREVENT_SLEEP_WHILE_STREAMING` | (unset) | jcode-base | Override `power.prevent_sleep_while_streaming` config field |

### Provider

| Env var | Default | Used by | Description |
|---|---|---|---|
| `JCODE_MODEL` | (unset) | jcode-base | Override the `provider.default_model` model name |
| `JCODE_PROVIDER` | (unset) | jcode-base | Override the `provider.default_provider` provider |
| `JCODE_OPENAI_REASONING_EFFORT` | (unset) | jcode-base | Override `provider.openai_reasoning_effort` config field |
| `JCODE_ANTHROPIC_REASONING_EFFORT` | (unset) | jcode-base | Override `provider.anthropic_reasoning_effort` config field |
| `JCODE_OPENAI_TRANSPORT` | (unset) | jcode-base | Override `provider.openai_transport` config field |
| `JCODE_OPENAI_SERVICE_TIER` | (unset) | jcode-base | Override `provider.openai_service_tier` config field |
| `JCODE_OPENAI_NATIVE_COMPACTION_MODE` | (unset) | jcode-base | Override `provider.openai_native_compaction_mode` config field |
| `JCODE_OPENAI_NATIVE_COMPACTION_THRESHOLD_TOKENS` | (unset) | jcode-base | Override `provider.openai_native_compaction_threshold_tokens` config field |
| `JCODE_PRESERVE_REASONING_CONTEXT` | (unset) | jcode-base | Override `provider.preserve_reasoning_context` config field |
| `JCODE_CROSS_PROVIDER_FAILOVER` | (unset) | jcode-base | Override `provider.cross_provider_failover` config field |
| `JCODE_SAME_PROVIDER_ACCOUNT_FAILOVER` | (unset) | jcode-base | Override `provider.same_provider_account_failover` config field |
| `JCODE_STREAM_IDLE_TIMEOUT_SECS` | (unset) | jcode-base | Override the `provider.stream_idle_timeout_secs` duration |
| `JCODE_MAX_RETRIES` | (unset) | jcode-base | Override `provider.max_retries` config field |
| `JCODE_RETRY_BACKOFF_CAP_SECS` | (unset) | jcode-base | Override the `provider.retry_backoff_cap_secs` duration |
| `JCODE_COPILOT_PREMIUM` | (unset) | jcode-base | Override `provider.copilot_premium` config field |


---

### Other JCODE_* env vars (workspace-wide)

These JCODE_-prefixed env vars are read by their respective crates but not
listed in `crates/jcode-base/src/config/env_overrides.rs`. They are read
directly via `std::env::var(...)` from each crate's `apply_*` helper or
module-level const.

| Env var | Default | Used by | Description |
|---|---|---|---|
| `JCODE_ACCOUNT_EMAIL` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_ACCOUNT_ID` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_ACTIVE_PROVIDER` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_AGENT_MODE` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_ALLOW_BROWSER_IN_TESTS` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_ALLOW_CODEX_LEGACY_AUTH` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_ALLOW_SERVER_VERSION_MISMATCH` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_ANTHROPIC_API_BASE` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_ANTHROPIC_API_KEY_NAME` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_ANTHROPIC_AUTH` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_ANTHROPIC_AUTH_HEADER` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_ANTHROPIC_DEBUG` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_ANTHROPIC_ENV_FILE` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_ANTHROPIC_HEADERS` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_ANTHROPIC_MAX_TOKENS` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_ANTHROPIC_MODEL` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_ANTIGRAVITY_CLIENT_ID` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_ANTIGRAVITY_CLIENT_SECRET` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_ANTIGRAVITY_MODEL` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_ANTIGRAVITY_VERSION` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_API_BASE` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_API_KEY` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_API_SOCKET` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_AUTH_LIFECYCLE_CEREBRAS_API_KEY` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_AUTH_LIFECYCLE_LIVE` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_AUTH_LIFECYCLE_OPENCODE_API_KEY` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_AUTH_LIFECYCLE_SMOKE` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_AUTH_LIFECYCLE_STREAM_SMOKE` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_AUTH_TEST_BINARY` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_AUTH_TIMING` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_BASE_SEMVER` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_BEDROCK_ENABLE` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_BEDROCK_LIVE_TEST` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_BEDROCK_MAX_TOKENS` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_BEDROCK_MODEL` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_BEDROCK_PROFILE` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_BEDROCK_REGION` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_BEDROCK_STOP_SEQUENCES` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_BEDROCK_TEMPERATURE` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_BEDROCK_TOP_P` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_BEDROCK_VALIDATE_STS` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_BIN` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_BROWSER_AUTOLAUNCH` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_BUILD_CHANGELOG_RAW` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_BUILD_GIT_DATE` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_BUILD_GIT_DIRTY` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_BUILD_GIT_HASH` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_BUILD_GIT_TAG` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_BUILD_METADATA_FILE` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_BUILD_SEMVER` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_CHANGELOG` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_CHATGPT_WEB_TIMEOUT_SECS` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_CI` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_CI_BUILD` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_CLAUDE_AUTH_CODE` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_CLAUDE_CLI_MODEL` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_CLAUDE_CLI_PARTIAL` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_CLAUDE_CLI_PATH` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_CLAUDE_CLI_PERMISSION_MODE` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_CLAUDE_SDK_PARTIAL` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_CLAUDE_SDK_PERMISSION_MODE` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_CLIENT_` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_CLIENT_DISPLAY` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_CLIENT_HERDR_PANE_ID` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_CLIENT_RUNTIME_MEMORY_LOG` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_CLIENT_RUNTIME_MEMORY_LOG_ATTRIBUTION_INTERVAL_SECS` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_CLIENT_RUNTIME_MEMORY_LOG_ATTRIBUTION_JSON_DELTA_THRESHOLD_MB` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_CLIENT_RUNTIME_MEMORY_LOG_ATTRIBUTION_MIN_SPACING_SECS` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_CLIENT_RUNTIME_MEMORY_LOG_EVENT_PROCESS_MIN_SPACING_SECS` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_CLIENT_RUNTIME_MEMORY_LOG_PROCESS_INTERVAL_SECS` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_CLIENT_RUNTIME_MEMORY_LOG_PSS_DELTA_THRESHOLD_MB` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_CLIENT_SELFDEV_MODE` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_CLIENT_UNTRUSTED_CLIENT_VAR` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_CLIENT_ZELLIJ_SESSION_NAME` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_CLI_BOOTSTRAP_LOGIN` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_COPILOT_ALLOW_GH_AUTH_TOKEN` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_COPILOT_MODEL` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_COPILOT_PREFETCH_STARTUP_GRACE_MS` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_CURSOR_AGENT_HOST` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_CURSOR_CLIENT_VERSION` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_CURSOR_CLI_PATH` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_CURSOR_CLI_VERSION` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_CURSOR_MODEL` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_DEBUG_CMD_PATH` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_DEBUG_CONTROL` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_DEBUG_MESSAGE_TIMEOUT_SECS` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_DEBUG_RESPONSE_PATH` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_DEFERRED_AUTH_BOOTSTRAP` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_DEV_CARGO_SCRIPT` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_DISABLE_CLAUDE_MCP` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_DISABLE_KITTY_KEYBOARD` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_DISABLE_POWER_INHIBIT` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_DISABLE_PRICING_REFRESH` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_DISCOVERY_BENCHMARK` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_DVIPNG_COMMAND` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_E2E_BIN` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_EMBEDDING_IDLE_UNLOAD_SECS` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_FAKE_GROK_ACP_HANG` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_FAKE_GROK_ACP_LOG` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_FAKE_GROK_ACP_PAYMENT_REQUIRED` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_FOCUS_SESSION_ID` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_FOCUS_TITLE` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_FORCE_PRICING_REFRESH` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_FORGECODE_CLI_MODEL` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_FORGECODE_CLI_PATH` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_FORGECODE_CLI_PERMISSION_MODE` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_FRESH_SPAWN` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_GATEWAY_HOST` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_GEMINI_CLI_PATH` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_GEMINI_FORCE_OAUTH` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_GEMINI_MODEL` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_GIT_DATE` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_GIT_HASH` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_GIT_TAG` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_GLIBC_ARENA_MAX` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_GLIBC_MMAP_THRESHOLD` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_GLYPH_SAFE_MODE` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_GMAIL_BACKEND` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_GPT53_SPARK_OK` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_GPT54_1M_OK` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_GPT54_OK` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_GROK_CLI_PATH` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_HEAP_RETENTION_TRIM_MB` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_HOME` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_HOOKS_DISABLED` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_HOOK_CWD` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_HOOK_EVENT` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_HOOK_PAYLOAD` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_HOOK_POST_TOOL` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_HOOK_PRE_TOOL` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_HOOK_SESSION_END` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_HOOK_SESSION_ID` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_HOOK_SESSION_START` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_HOOK_TURN_END` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_HOOK_TURN_START` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_INITIAL_PROVIDER_EXPLICIT` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_INSTALL_DIR` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_IN_DEV_CARGO` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_ISSUE_DRIVEN_LIVE_COMPLETION_SMOKE` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_ISSUE_DRIVEN_LIVE_MODEL` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_ISSUE_DRIVEN_LIVE_PROVIDER` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_ISSUE_DRIVEN_LIVE_STREAM_SMOKE` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_ITERM2_IMAGES` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_JADE_RELAY_DEVICE_ID` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_JADE_RELAY_PROCESS_EXISTING_PROMPTS` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_JADE_SESSIONS_HELPER` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_LATEX_COMMAND` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_LIVE_ANTHROPIC_ALLOW_OAUTH` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_LIVE_ANTHROPIC_MODEL` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_LIVE_ANTHROPIC_PROMPT` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_LIVE_ANTHROPIC_REASONING_EFFORT` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_LIVE_ANTHROPIC_REQUIRE_THINKING` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_LIVE_ANTHROPIC_SYSTEM` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_LIVE_OPENROUTER_MAX_TOKENS` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_LIVE_OPENROUTER_MODELS` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_LIVE_OPENROUTER_REASONING_EFFORT` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_LIVE_SMOKE_TIMEOUT_SECS` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_LIVE_TEST_COVERAGE` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_LIVE_TEST_LEDGER` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_LOGIN_QR` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_LOG_JSON` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_LOG_MARK` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_LOG_MODEL_PICKER_TIMING` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_LOG_SERVED_MODEL` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_LOG_SERVICE_TIER` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_MACOS_NOTIFICATION_BROKER_APP` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_MACOS_NOTIFICATION_INBOX` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_MCP_EXPANSION_TEST_VALUE` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_MD_FUZZ_ITERS` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_MD_FUZZ_SEED` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_MERMAID_PICKER_PROBE` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_MIGRATE_BINARY` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_MMDR_SIZE_API_DISABLE` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_NAMED_PROVIDER_PROFILE` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_NON_INTERACTIVE` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_NO_AUTO_UPDATE` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_NO_BROWSER` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_NO_COLOR` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_NO_MENUBAR` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_NO_TELEMETRY` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_OAUTH_TEST_LEDGER` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_OCR_IMG` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_ONBOARDING_SCREENSHOT_DIR` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_OPENAI_API_BASE` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_OPENAI_COMPAT_API_BASE` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_OPENAI_COMPAT_API_KEY_NAME` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_OPENAI_COMPAT_DEFAULT_MODEL` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_OPENAI_COMPAT_ENV_FILE` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_OPENAI_COMPAT_LOCAL_ENABLED` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_OPENAI_COMPAT_SETUP_URL` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_OPENAI_EXTRA_BODY` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_OPENAI_LIVE_MODEL` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_OPENAI_MAX_OUTPUT_TOKENS` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_OPENAI_MODEL` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_OPENAI_PREWARM` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_OPENAI_PROMPT_CACHE_KEY` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_OPENAI_PROMPT_CACHE_RETENTION` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_OPENAI_WS_IDLE_RECONNECT_SECS` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_OPENCODE_ZEN_LIVE_STREAM_TEST` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_OPENCODE_ZEN_LIVE_TOOL_MODEL` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_OPENCODE_ZEN_LIVE_TOOL_TEST` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_OPENROUTER_ALLOW_NO_AUTH` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_OPENROUTER_API_BASE` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_OPENROUTER_API_KEY_NAME` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_OPENROUTER_AUTH_HEADER` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_OPENROUTER_AUTH_HEADER_NAME` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_OPENROUTER_CACHE_NAMESPACE` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_OPENROUTER_DEFAULT_MODEL` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_OPENROUTER_DYNAMIC_BEARER_PROVIDER` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_OPENROUTER_ENV_FILE` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_OPENROUTER_MAX_TOKENS` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_OPENROUTER_MODEL` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_OPENROUTER_MODEL_CATALOG` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_OPENROUTER_NO_FALLBACK` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_OPENROUTER_PROVIDER` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_OPENROUTER_PROVIDER_FEATURES` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_OPENROUTER_STATIC_MODELS` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_OPENROUTER_THINKING` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_OPENROUTER_TRANSPORT_STATE` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_PDFLATEX_COMMAND` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_PDFTOCAIRO_COMMAND` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_PERF_TIER` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_PKG_VERSION` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_PROGRESS` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_PROVIDER_BAR_API_KEY` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_PROVIDER_ENV_TEST_KEY` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_PROVIDER_ENV_TEST_VALUE` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_PROVIDER_FOO_API_KEY` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_PROVIDER_MY_API_API_KEY` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_PROVIDER_MY_GATEWAY_API_KEY` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_PROVIDER_PROFILE_ACTIVE` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_PROVIDER_PROFILE_NAME` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_QR_LOGIN_TEST_CHILD` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_QUIET` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_READY_FD` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_RELEASE_BUILD` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_RELOAD_AUTH_STATUS` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_RELOAD_DETACH_COMMAND` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_RELOAD_FAST_START` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_RELOAD_GAP_STARTED_MS` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_REMOTE_BOOTSTRAP_MODEL_CATALOG` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_REPO_DIR` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_RESUME_TERMINAL` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_RESUMING` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_ROUNDED_PILLS` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_RUNTIME_DIR` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_RUNTIME_MEMORY_LOG` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_RUNTIME_MEMORY_LOG_ATTRIBUTION_INTERVAL_SECS` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_RUNTIME_MEMORY_LOG_ATTRIBUTION_JSON_DELTA_THRESHOLD_MB` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_RUNTIME_MEMORY_LOG_ATTRIBUTION_MIN_SPACING_SECS` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_RUNTIME_MEMORY_LOG_EVENT_PROCESS_MIN_SPACING_SECS` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_RUNTIME_MEMORY_LOG_INTERVAL_SECS` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_RUNTIME_MEMORY_LOG_PROCESS_INTERVAL_SECS` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_RUNTIME_MEMORY_LOG_PSS_DELTA_THRESHOLD_MB` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_RUNTIME_PROVIDER` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_RUNTIME_RELEASE_GIT_DATE` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_RUNTIME_RELEASE_GIT_HASH` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_RUNTIME_RELEASE_GIT_TAG` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_RUNTIME_RELEASE_SEMVER` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_RUN_AUTO_POKE` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_RUN_AUTO_POKE_MAX_TURNS` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_RUN_MCP` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_RUN_MCP_WAIT_MS` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_SCHEMA_QUIRKS_PATH` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_SCRATCH_DIR` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_SCREENSHOT_DISABLED` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_SCREENSHOT_SCOPE` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_SELFDEV` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_SELFDEV_BUILD_WAIT_SECS` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_SELFDEV_RELOAD_TIMEOUT_SECS` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_SELFDEV_REQUEST_HISTORY_LIMIT` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_SEMVER` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_SERVER_DISPLAY_NAME` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_SERVER_NAME` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_SERVER_OWNER_PID` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_SERVER_SCOPE` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_SESSION_PICKER_INCLUDE_OLD_SAVED` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_SESSION_PICKER_MAX_SESSIONS` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_SESSION_SEARCH_BENCH_REAL` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_SHOW_LOGIN_QR` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_SHOW_TUI_LOGIN_QR` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_SOCKET` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_SPAWN_COMMAND` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_SPAWN_COORDINATOR_SESSION_ID` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_SPAWN_CWD` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_SPAWN_KIND` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_SPAWN_LABEL` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_SPAWN_PROGRAM` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_SPAWN_SESSION_ID` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_SPAWN_SWARM_ID` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_SPAWN_TITLE` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_SSH_BINARY` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_SSH_REMOTE` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_SSH_SERVER_SOCKET` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_SSH_TEST_BINARY` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_SSH_UI_TEST` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_SSH_WORKING_DIR` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_SUBSCRIPTION_ACTIVE` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_SWARM_DORMANT_PLAN_RETENTION_SECS` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_SWARM_ID` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_SWARM_IDLE_WORKER_REAP_SECS` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_SWARM_STATUS_BROADCAST_TERMINAL_SECS` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_SWARM_STATUS_DEBOUNCE_MEMBER_THRESHOLD` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_SWARM_STATUS_DEBOUNCE_MS` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_SWARM_TASK_HEARTBEAT_SECS` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_SWARM_TASK_STALE_AFTER_SECS` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_SWARM_TASK_SWEEP_INTERVAL_SECS` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_SWARM_TERMINAL_MEMBER_GC_INTERVAL_SECS` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_SWARM_TERMINAL_MEMBER_RETENTION_SECS` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_TELEMETRY_DISABLED` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_TEMP_SERVER` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_TEMP_SERVER_IDLE_SECS` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_TERMINAL` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_TEST_ALLOW_MEMORY_EMBEDDINGS` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_TEST_BUNDLE` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_TEST_CAPTURE` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_TEST_CLIENT_VERSION_OVERRIDE` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_TEST_CONCURRENCY_DIR` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_TEST_HEADLESS_STARTUP_RECOVERY_DELAY_MS` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_TEST_OUTPUT` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_TEST_PERF_ASSERTIONS` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_TEST_REGISTRY_CRASH` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_TEST_SESSION` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_TEST_SESSION_LATENCY_CHILD` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_THEME` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_TIER` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_TOOL_OK` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_TRACE` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_TRACK_CLIENT_CACHE` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_TUI_FLICKER_DETECTION` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_TUI_INHERITED_MODES` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_TUI_INHERITED_THEME` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_TUI_LOGIN_QR` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_TUI_PROFILE` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_TUI_SCROLL_TRACE` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_TUI_SLOW_FRAME_MS` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_UPDATE_SEMVER` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_USE_CLAUDE_CLI` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_VERSION` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_WATCHDOG` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_WATCHDOG_HEARTBEAT_SECS` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_WATCHDOG_STALL_SECS` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_WEDGE_FIXTURE` | (unset) | various | See call sites in `crates/` and `src/` |
| `JCODE_WINDOW_TITLE` | (unset) | various | See call sites in `crates/` and `src/` |


---

### Third-party env vars read at runtime

These env vars come from upstream libraries and third-party integrations
that jcode reads directly. They are listed here for completeness; consult
the upstream documentation for the canonical reference.

| Env var | Default | Used by | Description |
|---|---|---|---|
| `ALACRITTY_SOCKET` | (unset) | jcode-terminal-detect | Alacritty IPC socket path |
| `ALACRITTY_WINDOW_ID` | (unset) | jcode-terminal-detect | Alacritty window ID |
| `ANTHROPIC_API_KEY` | (unset) | jcode-provider-anthropic | Anthropic Claude API key (used by jcode-provider-anthropic) |
| `ANTHROPIC_AUTH_TOKEN` | (unset) | jcode-provider-anthropic | Alternative Anthropic auth token (OAuth-style) |
| `ANTHROPIC_BASE_URL` | (unset) | jcode-provider-anthropic | Override Anthropic API base URL |
| `BASH_ENV` | (unset) | various | Bash startup script for non-interactive shells |
| `BASH_VERSION` | (unset) | various | Bash version (set when invoked under bash) |
| `BROWSER` | (unset) | various | Default browser executable |
| `BROWSER_SESSION` | (unset) | various | Browser session ID |
| `CARGO` | (unset) | various | Cargo executable path |
| `CARGO_HOME` | (unset) | jcode-build scripts | Cargo home directory |
| `CARGO_MANIFEST_DIR` | (unset) | jcode-build scripts | Workspace manifest directory |
| `CARGO_NET_OFFLINE` | (unset) | jcode-build scripts | Cargo network offline mode |
| `CARGO_REGISTRIES_CRATES_IO_PROTOCOL` | (unset) | jcode-build scripts | crates.io registry protocol (sparse/git) |
| `CARGO_TARGET_DIR` | (unset) | jcode-build scripts | Cargo target directory override |
| `CARGO_TERM_COLOR` | (unset) | jcode-build scripts | Cargo color output setting |
| `CARGO_TERM_PROGRESS_WHEN` | (unset) | jcode-build scripts | Cargo progress output |
| `CODE_ASSIST_API_VERSION` | (unset) | various | Code Assist API version |
| `CODE_ASSIST_ENDPOINT` | (unset) | various | Code Assist endpoint URL |
| `COHERE_API_KEY` | (unset) | various | Cohere API key |
| `COLORTERM` | (unset) | jcode-tui | Color-support hint |
| `COMPOSIO_API_KEY` | (unset) | jcode-composio | Composio API key |
| `COMPOSIO_BASE_URL` | (unset) | jcode-composio | Composio API base URL |
| `COMPOSIO_GMAIL_AUTH_CONFIG_ID` | (unset) | jcode-composio | Composio Gmail auth config |
| `COMPOSIO_GMAIL_CONNECTED_ACCOUNT_ID` | (unset) | jcode-composio | Composio connected account for Gmail |
| `COMPOSIO_GMAIL_USER_ID` | (unset) | jcode-composio | Composio Gmail user ID |
| `COMPOSIO_USER_ID` | (unset) | jcode-composio | Composio user identifier |
| `COMPUTERNAME` | (unset) | various | Windows machine name |
| `COMTEGRA_API_KEY` | (unset) | various | Comtegra API key (Comtegra provider) |
| `CURSOR_ACCESS_TOKEN` | (unset) | jcode-provider-cursor | Cursor IDE access token |
| `CURSOR_API_KEY` | (unset) | jcode-provider-cursor | Cursor API key (alt) |
| `CURSOR_REFRESH_TOKEN` | (unset) | jcode-provider-cursor | Cursor IDE refresh token |
| `DEEPSEEK_API_KEY` | (unset) | various | DeepSeek API key |
| `DISPLAY` | (unset) | various | X11 display |
| `DO_NOT_TRACK` | (unset) | various | Telemetry opt-out (1 = off) |
| `EDITOR` | (unset) | various | Default editor executable |
| `FIREWORKS_API_KEY` | (unset) | various | Fireworks API key |
| `FISH_VERSION` | (unset) | various | Fish shell version |
| `GEMINI_API_ENDPOINT` | (unset) | jcode-provider-gemini | Gemini API endpoint override |
| `GEMINI_API_KEY` | (unset) | jcode-provider-gemini | Google Gemini API key |
| `GEMINI_API_VERSION` | (unset) | jcode-provider-gemini | Gemini API version (e.g. v1, v1beta) |
| `GHOSTTY_BIN_DIR` | (unset) | jcode-terminal-detect | Ghostty terminal binary directory |
| `GHOSTTY_RESOURCES_DIR` | (unset) | jcode-terminal-detect | Ghostty resources directory |
| `GOOGLE_CLOUD_PROJECT` | (unset) | jcode-provider-gemini | Google Cloud project ID for Gemini Code Assist |
| `GOOGLE_CLOUD_PROJECT_ID` | (unset) | jcode-provider-gemini | Same as GOOGLE_CLOUD_PROJECT, alt name |
| `GROK_API_KEY` | (unset) | jcode-provider-grok | xAI Grok API key |
| `GROK_CLI_PATH` | (unset) | jcode-provider-grok | Override grok CLI binary path |
| `GROK_DEPLOYMENT_KEY` | (unset) | jcode-provider-grok | Grok deployment key |
| `HERDR_BIN_PATH` | (unset) | jcode-herdr | Path to the `herdr` binary |
| `HERDR_ENV` | (unset) | jcode-herdr | Reserved for herdr environment tagging |
| `HERDR_JCODE_IDLE_DEBOUNCE_MS` | (unset) | jcode-herdr | Debounce window for herdr->jcode idle events (ms) |
| `HERDR_JCODE_RETRY_GRACE_MS` | (unset) | jcode-herdr | Retry grace window after disconnects (ms) |
| `HERDR_PANE_ID` | (unset) | jcode-herdr | Identifier of the herdr pane hosting jcode |
| `HERDR_SOCKET_PATH` | (unset) | jcode-herdr | herdr IPC socket path |
| `HERDR_TAB_ID` | (unset) | jcode-herdr | Tab ID within herdr |
| `HERDR_WORKSPACE_ID` | (unset) | jcode-herdr | herdr workspace identifier |
| `HOME` | (unset) | various | User home directory |
| `HOSTNAME` | (unset) | various | Hostname |
| `HOSTTYPE` | (unset) | various | CPU arch string |
| `HUGGINGFACE_API_KEY` | (unset) | various | HuggingFace API key |
| `ITERM_PID` | (unset) | jcode-terminal-detect | iTerm2 process ID |
| `ITERM_PROFILE` | (unset) | jcode-terminal-detect | iTerm2 profile name |
| `ITERM_SESSION_ID` | (unset) | jcode-terminal-detect | iTerm2 session ID |
| `JADE_RELAY_API_BASE` | (unset) | jcode-jade-relay | JADE Cloud relay API base |
| `JADE_RELAY_DEFAULT_GROUP` | (unset) | jcode-jade-relay | JADE relay default macOS notification group |
| `JADE_RELAY_DEFAULT_PRIORITY` | (unset) | jcode-jade-relay | JADE relay notification priority default |
| `JADE_RELAY_DEFAULT_RELAX` | (unset) | jcode-jade-relay | JADE relay relax-days default |
| `JADE_RELAY_PAYLOAD_KIND` | (unset) | jcode-jade-relay | JADE relay payload type override |
| `JADE_RELAY_TOKEN` | (unset) | jcode-jade-relay | JADE Cloud relay auth token |
| `JADE_RELAY_USER_ID` | (unset) | jcode-jade-relay | JADE Cloud relay user ID |
| `JPM_LAUNCH_AGENT` | (unset) | various | LaTeX-related, ignored by jcode |
| `KIMI_API_KEY` | (unset) | various | Moonshot Kimi API key |
| `KITTY_LISTEN_ON` | (unset) | jcode-terminal-detect | Kitty socket listen address |
| `KITTY_PID` | (unset) | jcode-terminal-detect | Kitty terminal PID |
| `LANG` | (unset) | various | Locale |
| `LC_ALL` | (unset) | various | Locale override |
| `LC_CTYPE` | (unset) | various | Character classification locale |
| `LOGNAME` | (unset) | various | Login name |
| `MACHTYPE` | (unset) | various | CPU/OS string |
| `MANPATH` | (unset) | various | Manpage search path |
| `MISTRAL_API_KEY` | (unset) | various | Mistral API key |
| `MOONSHOT_API_KEY` | (unset) | various | Moonshot API key |
| `NOVITA_API_KEY` | (unset) | various | Novita API key |
| `OLDPWD` | (unset) | various | Previous working directory |
| `OPENAI_API_KEY` | (unset) | jcode-provider-openai, jcode-provider-openai-compat | OpenAI API key (used by jcode-provider-openai) |
| `OPENAI_BASE_URL` | (unset) | jcode-provider-openai, jcode-provider-openai-compat | OpenAI-compatible API base URL override |
| `OPENAI_COMPAT_API_KEY` | (unset) | jcode-provider-openai, jcode-provider-openai-compat | Generic OpenAI-compatible API key fallback |
| `OPENAI_COMPAT_BASE_URL` | (unset) | jcode-provider-openai, jcode-provider-openai-compat | Generic OpenAI-compatible base URL |
| `OPENAI_EXTRA_BODY` | (unset) | jcode-provider-openai, jcode-provider-openai-compat | JSON merged into every OpenAI request body |
| `OPENAI_ORGANIZATION` | (unset) | jcode-provider-openai, jcode-provider-openai-compat | OpenAI organization ID header |
| `OPENROUTER_API_BASE` | (unset) | jcode-provider-openai-compat | OpenRouter API base URL override |
| `OPENROUTER_API_KEY` | (unset) | jcode-provider-openai-compat | OpenRouter API key |
| `OSTYPE` | (unset) | various | OS kind |
| `PAGER` | (unset) | various | Default pager |
| `PATH` | (unset) | various | Executable search path |
| `PWD` | (unset) | various | Working directory |
| `RUSTUP_DIST_SERVER` | (unset) | jcode-base telemetry | rustup dist server override |
| `RUSTUP_HOME` | (unset) | jcode-base telemetry | rustup home directory |
| `RUSTUP_TOOLCHAIN` | (unset) | jcode-base telemetry | Default rust toolchain |
| `RUST_BACKTRACE` | (unset) | jcode-base telemetry | Backtrace mode for panics |
| `RUST_LOG` | (unset) | jcode-base telemetry | tracing/log filter |
| `SHELL` | (unset) | various | Login shell |
| `SSH_AUTH_SOCK` | (unset) | jcode-base | SSH agent socket |
| `SSH_CLIENT` | (unset) | jcode-base | SSH client info |
| `SSH_CONNECTION` | (unset) | jcode-base | SSH connection metadata |
| `SSH_ORIGINAL_COMMAND` | (unset) | jcode-base | Original SSH command |
| `SSH_TTY` | (unset) | jcode-base | SSH TTY device |
| `TERM` | (unset) | jcode-tui | Terminal type |
| `TERM_PROGRAM` | (unset) | jcode-tui | Terminal program name (iTerm, Apple_Terminal, ...) |
| `TERM_VERSION` | (unset) | jcode-tui | Terminal program version |
| `TMUX` | (unset) | various | tmux socket path (set inside tmux sessions) |
| `TMUX_PANE` | (unset) | various | tmux pane identifier |
| `TZ` | (unset) | various | Timezone |
| `USER` | (unset) | various | Username |
| `VISUAL` | (unset) | various | Default visual editor |
| `WAYLAND_DISPLAY` | (unset) | various | Wayland display socket |
| `WEZTERM_CONFIG_DIR` | (unset) | jcode-terminal-detect | WezTerm config directory |
| `WEZTERM_PANE` | (unset) | jcode-terminal-detect | WezTerm pane ID |
| `WSLENV` | (unset) | various | WSL env-var passthrough list |
| `WSL_DISTRO_NAME` | (unset) | various | WSL distro name (Windows) |
| `WT_PROFILE_ID` | (unset) | various | Windows Terminal profile ID |
| `WT_SESSION` | (unset) | various | Windows Terminal session UUID |
| `XDG_CACHE_HOME` | (unset) | jcode-base | XDG cache base directory |
| `XDG_CONFIG_HOME` | (unset) | jcode-base | XDG config base directory |
| `XDG_DATA_HOME` | (unset) | jcode-base | XDG data base directory |
| `XDG_RUNTIME_DIR` | (unset) | jcode-base | XDG runtime directory |
| `XDG_SESSION_TYPE` | (unset) | jcode-base | XDG session kind (wayland/x11) |
| `ZAI_API_KEY` | (unset) | various | Z.AI API key |
| `ZDOTDIR` | (unset) | various | Zsh config dir |
| `ZSH_VERSION` | (unset) | various | Zsh shell version |


---

## Regeneration

```bash
# Snapshot all unique env-var names:
rg -N 'env::var\([^)]+\)' crates src \
  | rg -o '"[A-Z][A-Z0-9_]*"' | sort -u > docs/ENVIRONMENT.md.tmp

# Inspect a single var's call sites:
rg -N 'JCODE_WAKE_MODE' crates src

# Diff this file against a fresh grep:
diff <(rg -N 'env::var\(' crates src | rg -o '"[A-Z][A-Z0-9_]*"' | sort -u) \
     <(awk -F'|' '/^\| `[A-Z]/ {print "\""$2"\""}' docs/ENVIRONMENT.md | sort -u)
```
