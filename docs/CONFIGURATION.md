# Configuration reference (`config.toml`)

Jcode reads a single TOML file:

```
~/.jcode/config.toml
```

The directory can be relocated with `$JCODE_HOME` (then the file is
`$JCODE_HOME/config.toml`).

Precedence, lowest to highest:

1. Built-in defaults
2. `config.toml`
3. Environment variables (`JCODE_*`)
4. Runtime slash commands (`/model`, `/effort`, `/colors`, ...), some of which
   write back to `config.toml`

Useful commands:

| Command | Effect |
| --- | --- |
| `/config` | Print the effective settings |
| `/config init` | Write a commented default `config.toml` |
| `/config edit` | Open `config.toml` in `$EDITOR` |

Parsing is deliberately lenient. Unknown keys are ignored, and unknown enum
values fall back to the default rather than failing the whole file, so a stale
setting cannot brick startup. A syntactically invalid file, however, is
reported and the defaults are used.

Ambient mode also honors a project-level `.jcode/config.toml` for its
`[ambient]` section.

---

## `[server]`

| Key | Type | Default | Meaning |
| --- | --- | --- | --- |
| `wake_mode` | `"internal"` \| `"external"` | `internal` | Who owns autonomous wake execution. `internal`: the daemon starts and interrupts turns itself. `external`: the daemon only emits wake requests and its operator schedules turns. |

## `[provider]`

Model selection, reasoning effort, failover, and retry behavior.

| Key | Type | Default | Meaning |
| --- | --- | --- | --- |
| `default_model` | string | unset | e.g. `"claude-opus-5"`, `"copilot:claude-opus-4.6"` |
| `default_provider` | string | unset | `claude`, `openai`, `copilot`, `openrouter`, ... |
| `openai_reasoning_effort` | string | `"low"` | `none`, `minimal`, `low`, `medium`, `high`, `xhigh`, `max` |
| `anthropic_reasoning_effort` | string | unset | `none`, `low`, `medium`, `high`, `xhigh` (`max` maps to the strongest supported) |
| `openai_transport` | string | unset | `auto`, `websocket`, `https` |
| `openai_service_tier` | string | `"priority"` | `priority`, `flex` |
| `openai_native_compaction_mode` | string | `"auto"` | `auto`, `explicit`, `off` |
| `openai_native_compaction_threshold_tokens` | int | `200000` | Trigger point for auto native compaction |
| `preserve_reasoning_context` | bool | `true` | Keep provider-native reasoning items for later turns where supported |
| `cross_provider_failover` | `"countdown"` \| `"manual"` | `countdown` | `countdown` shows a cancelable 3s countdown, then resends to another provider. `manual` (aliases `off`, `none`, `disabled`) never auto-resends. |
| `same_provider_account_failover` | bool | `true` | Try another account on the same provider before switching provider |
| `copilot_premium` | string | unset | `normal`, `one`, `zero` (`zero` = never consume premium requests) |
| `model_picker_providers` | array of strings | unset | When non-empty, `/model` lists only these providers/api-methods/profiles. The active model's routes stay visible regardless. |
| `stream_idle_timeout_secs` | int | `180` | Base no-data streaming timeout; scaled up automatically at high reasoning effort |
| `max_retries` | int | `8` | Total attempts on transient provider errors, including the first |
| `retry_backoff_cap_secs` | int | `30` | Max exponential backoff between retries |

## `[providers.<name>]`

Named profiles for OpenAI-compatible, Anthropic-compatible, and OpenRouter
endpoints. Each profile becomes selectable as `<name>:<model>`.

```toml
[providers.my-gateway]
type = "openai-compatible"          # or "anthropic-compatible", "openrouter"
base_url = "https://llm.example.com/v1"
auth = "bearer"                     # "bearer" | "header" | "none"
auth_header = "X-Api-Key"           # when auth = "header"
api_key_env = "MY_GATEWAY_API_KEY"  # preferred over inline api_key
default_model = "my-model-v2"
headers = { X-Org = "acme" }        # extra headers on every request
model_catalog = true                # fetch the model list from the endpoint
provider_routing = false
allow_provider_pinning = false
requires_api_key = true
env_file = "~/.config/gateway.env"
extra_body = { chat_template_kwargs = { thinking = true } }
supports_reasoning_effort = true    # unset = auto-detect from the model id
disable_reasoning_heuristics = false

[[providers.my-gateway.models]]
id = "my-model-v2"
reasoning = true
reasoning_effort = "high"
context_window = 262144
input = ["text", "image"]
```

`extra_body` is merged into every chat/completions request body and overrides
jcode-generated fields, which is how non-standard backend parameters get
through.

## `[auth]`

| Key | Type | Default | Meaning |
| --- | --- | --- | --- |
| `trusted_external_sources` | array | `[]` | External auth source ids jcode may read (credentials managed by other tools) |
| `trusted_external_source_paths` | array | `[]` | Path-bound approvals for the same |

## `[display]`

TUI presentation. Most of these have a matching slash command or hotkey.

| Key | Type | Default | Meaning |
| --- | --- | --- | --- |
| `diff_mode` | enum | `inline` | `off`, `inline`, `full-inline`, `pinned`, `file` |
| `diff_line_wrap` | bool | `true` | Wrap long lines in the pinned diff pane |
| `queue_mode` | bool | `false` | Queue input until the current turn finishes |
| `auto_server_reload` | bool | `true` | Reload the daemon when a newer server binary appears |
| `mouse_capture` | bool | `true` | Enables wheel scroll, disables native terminal selection |
| `debug_socket` | bool | `false` | External control socket |
| `emoji` | bool | `true` | Emoji in TUI/CLI output |
| `centered` | bool | `false` | Center all content |
| `show_thinking` | bool | `true` | Legacy switch; also gates whether reasoning is requested at all |
| `reasoning_display` | enum | `full` | `off`, `full`, `current` (`current` collapses each trace once the model commits) |
| `diagram_mode` | enum | `none` | `none` (inline only), `margin`, `pinned` |
| `markdown_spacing` | enum | `compact` | `compact`, `document` |
| `latex_rendering` | enum | `image` | `none`, `unicode`, `image` |
| `pin_images` | bool | `true` | Pin images read by tools to the side pane |
| `pin_todos` | bool | `true` | Sticky todo list at the top of the transcript |
| `idle_animation` | bool | `false` | Animation before the first prompt |
| `prompt_entry_animation` | bool | `true` | Animate a prompt line entering the viewport |
| `disabled_animations` | array | `[]` | Disable variants by name, e.g. `["donut", "orbit_rings"]` |
| `performance` | string | auto | `auto`, `full`, `reduced`, `minimal` |
| `animation_fps` | int | `60` | 1-120 |
| `redraw_fps` | int | `60` | 1-120 |
| `prompt_preview` | bool | `true` | Truncated preview of the previous prompt once it scrolls away |
| `compact_notifications` | bool | `false` | Single-line swarm/file-activity notices |
| `copy_badge_alt_label` | string | `""` | Override the Alt/Option glyph in copy badges |
| `show_agentgrep_output` | bool | `false` | Full agentgrep output inline instead of a summary |
| `show_bash_output` | bool | `false` | Last few bash output lines under the tool summary |
| `tool_call_details` | bool | `false` | Show command/path detail after the model's stated intent |
| `keybinding_hints` | bool | `true` | Occasional "there is a shortcut for this" nudges |
| `theme` | string | `auto` | `auto` (OSC 11 background detection), `dark`, `light` |
| `active_sessions_manager` | bool | `false` | Left arrow on empty input opens the live-session picker |
| `external_sessions` | bool | `true` | Include Claude Code / Codex / Pi / OpenCode / Cursor transcripts in `/resume` |
| `usage_display` | string | `left` | `left` or `used` wording for the usage percentage |
| `overscroll_status` | enum | `overscroll` | `off`, `on`, `overscroll` (elastic reveal below the input) |

### `[display.native_scrollbars]`

| Key | Type | Default |
| --- | --- | --- |
| `chat` | bool | `true` |
| `side_panel` | bool | `true` |

### `[display.colors]`

Per-role hex overrides, e.g. `user = "#8ab4f8"`. Ad hoc widget shades follow
the role they belong to. See `docs/TUI_COLOR_CONFIGURATION.md`; `/colors` lists
roles, `/colors generate <#rrggbb>` derives a palette, `/colors harmony` scores
it, `/colors export` prints the TOML.

## `[keybindings]`

Chord syntax is `modifier+key`, e.g. `"ctrl+k"`, `"alt+shift+up"`, `"pageup"`.
Empty string unbinds. Defaults differ per platform (macOS prefers `cmd`).

| Key | Default | Action |
| --- | --- | --- |
| `scroll_up` / `scroll_down` | `ctrl+k` / `ctrl+j` | Line scroll |
| `scroll_page_up` / `scroll_page_down` | `alt+u` / `alt+d` | Page scroll |
| `scroll_up_fallback` / `scroll_down_fallback` | unset | Secondary scroll chords |
| `scroll_prompt_up` / `scroll_prompt_down` | `ctrl+[` / `ctrl+]` | Jump between prompts |
| `scroll_bookmark` | `ctrl+g` | Stash position, jump to bottom, press again to return |
| `model_switch_next` / `model_switch_prev` | `ctrl+tab` / `ctrl+shift+tab` | Cycle models |
| `fallback_switch` | `ctrl+y` | Accept the post-error fallback offer and resend |
| `effort_increase` / `effort_decrease` | `cmd+right` / `cmd+left` (macOS), `alt+...` elsewhere | Reasoning effort |
| `centered_toggle` | `alt+c` | Centered layout |
| `auto_poke_toggle` | `ctrl+p` | Auto follow-up on incomplete todos |
| `workspace_left/down/up/right` | `alt+h/j/k/l` | Workspace navigation |
| `side_panel_toggle` | `alt+m` | Side panel |
| `copy_selection_toggle` | `alt+y` | Copy/selection mode |
| `diagram_pane_toggle` | `alt+t` | Diagram pane position |
| `typing_scroll_lock_toggle` | `alt+s` | Scroll lock while typing |
| `diff_mode_cycle` | `alt+g` | Cycle `diff_mode` |
| `info_widget_toggle` | `alt+i` | Info widget |
| `todo_card_toggle` | `alt+x` | Inline todo card |
| `swarm_panel_focus` | `alt+n` | Focus the inline swarm panel (inline spawn mode only) |
| `new_terminal` | unset | Spawn a fresh session in a new terminal window |
| `open_resume` | `cmd+b` (macOS), `alt+r` elsewhere | Open the `/resume` picker |
| `session_picker_enter` | `current-terminal` | Enter action in the picker; `new-terminal` swaps it. Ctrl+Enter always does the other one. |

Conflict detection is described in `docs/KEYMAP_CONFLICTS.md`.

## `[features]`

| Key | Type | Default | Meaning |
| --- | --- | --- | --- |
| `check_updates` | bool | `true` | Persistent equivalent of `--no-update` when false |
| `update_channel` | `"stable"` \| `"main"` | `stable` | Releases only, or latest commits |
| `memory` | bool | `true` | Memory retrieval/extraction |
| `swarm` | bool | `true` | Swarm coordination |
| `mermaid` | bool | `true` | Mermaid rendering and related model guidance |
| `auto_poke` | bool | `true` | Default auto-poke state (`/poke on|off` overrides per session) |
| `message_timestamps` | bool | `true` | Inject timestamps into user messages and tool results |
| `persist_memory_injections` | bool | `false` | Write recalled memories into session history instead of ephemeral suffix messages |
| `kv_cache_miss_notices` | bool | `true` | Loud in-chat alarm when a request avoidably misses the prefix cache |

## `[tools]`

| Key | Type | Default | Meaning |
| --- | --- | --- | --- |
| `profile` | string | `full` | `full`, `acp`, `minimal`/`lite`, `none` |
| `enabled` | array | `[]` | Allow-list; when set only these tools are exposed. `"*"` or `"all"` exposes everything. |
| `disabled` | array | `[]` | Removed after profile/allow-list resolution |
| `disable_base_tools` | bool | `false` | Drop all built-ins unless `enabled` is given |
| `mcp_tools` | enum | `auto` | `auto`, `eager` (all MCP tools as top-level definitions), `deferred` (only `mcp_search`/`mcp_call`) |
| `mcp_tools_token_threshold` | int | `8000` | In `auto`, defer MCP tools once their definitions exceed this token estimate |

## `[acp]`

| Key | Type | Default | Meaning |
| --- | --- | --- | --- |
| `profile` | string | `standard` | Client compatibility: `standard`, `extended`, `full` |
| `tool_profile` | string | `acp` | Tool profile used when `jcode acp` starts its own daemon |

## `[agents]`

Swarm workers and the memory sidecar.

| Key | Type | Default | Meaning |
| --- | --- | --- | --- |
| `swarm_model` | string | inherit | Default worker model. `"inherit"`/`"coordinator"` = same as the spawner. |
| `swarm_effort` | string | inherit | Default worker reasoning effort |
| `swarm_spawn_mode` | enum | `inline` | `visible`, `headless`, `inline`, `auto` |
| `swarm_strip_layout` | enum | `vertical` | `vertical` (one agent per row) or `horizontal` (chips) |
| `swarm_gallery_max_pct` | int | `40` | Max percent of chat height the inline gallery may take (1-90) |
| `swarm_max_concurrent_agents` | int | `32` | Live-worker RAM budget. `0` disables this guard, leaving only the hard cap. |
| `memory_model` | string | auto | Model override for the memory sidecar |
| `memory_sidecar_enabled` | bool | `true` | LLM precision-judge memory path. `false` opts into the lower-precision no-LLM hybrid. |
| `memory_rerank_cadence` | int | `3` | Minimum turns between listwise LLM reranks (0/1 = every turn) |
| `memory_rerank_votes` | int | `2` | Independent judges per fired rerank |
| `memory_rerank_min_agree` | int | `2` | Judge agreement needed to inject a memory (clamped to 1..=votes) |
| `memory_embedding_backend` | string | `local` | `local` (bundled MiniLM ONNX, offline) or `openai` |
| `memory_embedding_model` | string | `text-embedding-3-small` | Remote embedding model |
| `memory_embedding_base_url` | string | OpenAI | Override for OpenAI-compatible embedding gateways |
| `memory_embedding_dim` | int | inferred | Override the remote embedding dimensionality |

A keyless `memory_embedding_backend = "openai"` silently degrades to local.

## `[terminal]`

| Key | Type | Default | Meaning |
| --- | --- | --- | --- |
| `spawn_hook` | string | unset | External command that takes over headed session spawns, e.g. `"tmux new-window"`. Receives `JCODE_SPAWN_*` env metadata. Falls back to built-in detection if it fails. |
| `focus_hook` | string | unset | Command used to raise an existing session window, with `JCODE_FOCUS_SESSION_ID` / `JCODE_FOCUS_TITLE` |
| `preferred` | string | unset | macOS terminal for hotkey/in-app spawns: `ghostty`, `iterm2`, `wezterm`, `warp`, `alacritty`, `vscode`, `terminal`. Re-run `jcode setup-hotkey` after changing. |

See `docs/SPAWN_HOOK.md`.

## `[hooks]`

External commands at lifecycle points. Values are a single command string or an
array of commands. Hooks receive `JCODE_HOOK_*` env vars plus a
`JCODE_HOOK_PAYLOAD` JSON mirror.

| Key | Type | Default | Meaning |
| --- | --- | --- | --- |
| `turn_start` | string/array | unset | Turn begins. Fields: `MODEL`, `SOURCE`. |
| `turn_end` | string/array | unset | Turn ends. Fields: `STATUS`, `DURATION_MS`, `MODEL`, `LAST_ASSISTANT_TEXT`. |
| `session_start` | string/array | unset | Session created or resumed |
| `session_end` | string/array | unset | Session closed normally |
| `pre_tool` | string/array | unset | Gate before each tool call. Exit 0 allows, exit 2 blocks (stderr goes back to the model), anything else fails open. |
| `post_tool` | string/array | unset | After each tool call. Fields: `TOOL_NAME`, `STATUS`, `DURATION_MS`, `OUTPUT_BYTES`. |
| `pre_tool_timeout_ms` | int | `5000` | Gate timeout before failing open |

All hooks except `pre_tool` are detached fire-and-forget observers. See
`docs/HOOKS.md`.

## `[compaction]`

| Key | Type | Default | Meaning |
| --- | --- | --- | --- |
| `mode` | enum | `reactive` | `reactive` (fixed threshold), `proactive` (predicted growth), `semantic` (topic shift + relevance) |
| `lookahead_turns` | int | `15` | Proactive: turns of token growth projected |
| `ewma_alpha` | float | `0.3` | Proactive: growth smoothing, higher = more recency bias |
| `proactive_floor` | float | `0.40` | Minimum context fill before any proactive check fires |
| `min_samples` | int | `3` | Token snapshots required before a proactive check |
| `stall_window` | int | `5` | Stable turns before suppressing proactive compaction |
| `min_turns_between_compactions` | int | `10` | Cooldown |
| `topic_shift_threshold` | float | `0.45` | Semantic: cosine similarity below which a topic shift is declared |
| `relevance_keep_threshold` | float | `0.65` | Semantic: similarity above which a message is kept verbatim |
| `goal_window_turns` | int | `5` | Semantic: recent turns used to build the current-goal embedding |

## `[websearch]`

| Key | Type | Default | Meaning |
| --- | --- | --- | --- |
| `engine` | enum | `duckduckgo` | `duckduckgo`, `bing`, `searxng` |
| `fallback_engines` | array | `["bing"]` | Keyless engines tried after the preferred one fails |
| `bing_api_key` | string | unset | Prefer the env var below |
| `bing_api_key_env` | string | `JCODE_BING_API_KEY` | |
| `bing_market` | string | `en-US` | |
| `searxng_url` | string | unset | SearXNG base URL |
| `searxng_url_env` | string | `JCODE_SEARXNG_URL` | |

SearXNG is the escape hatch on hosts where DuckDuckGo/Bing block requests via
TLS fingerprinting.

## `[notifications]`

Local desktop notifications for interactive sessions.

| Key | Type | Default | Meaning |
| --- | --- | --- | --- |
| `turn_complete` | bool | `true` | Notify when a turn finishes |
| `turn_complete_min_secs` | int | `120` | Minimum turn duration to notify |
| `turn_complete_todo_min_secs` | int | `30` | Lower threshold when the session has todos |
| `turn_complete_only_when_unfocused` | bool | `true` | Requires a terminal that reports focus events |
| `turn_complete_sound` | string | `Glass` | macOS sound name; empty disables. Ignored elsewhere. |

## `[ambient]`

Background autonomous work. See `docs/AMBIENT_MODE.md`.

| Key | Type | Default | Meaning |
| --- | --- | --- | --- |
| `enabled` | bool | `false` | |
| `provider` / `model` | string | auto | Overrides for ambient cycles |
| `allow_api_keys` | bool | `false` | When false, OAuth credentials only |
| `api_daily_budget` | int | unset | Daily token budget when API keys are allowed |
| `min_interval_minutes` | int | `5` | |
| `max_interval_minutes` | int | `120` | |
| `pause_on_active_session` | bool | `true` | |
| `proactive_work` | bool | `true` | False = garden-only (memory maintenance, no code changes) |
| `work_branch_prefix` | string | `ambient/` | |
| `visible` | bool | `true` | Show the cycle in a terminal window |

## `[safety]`

Remote notification and reply channels used mainly by ambient mode. Prefer env
vars for every secret. See `docs/SAFETY_SYSTEM.md`.

| Group | Keys |
| --- | --- |
| ntfy | `ntfy_topic`, `ntfy_server` (default `https://ntfy.sh`) |
| Desktop | `desktop_notifications` (default `true`) |
| Email | `email_enabled`, `email_to`, `email_from`, `email_smtp_host`, `email_smtp_port` (587), `email_password` (prefer `JCODE_SMTP_PASSWORD`), `email_imap_host`, `email_imap_port` (993), `email_reply_enabled` |
| Telegram | `telegram_enabled`, `telegram_bot_token`, `telegram_chat_id`, `telegram_reply_enabled` |
| Discord | `discord_enabled`, `discord_bot_token`, `discord_channel_id`, `discord_bot_user_id`, `discord_reply_enabled` |
| Jade relay | `jade_relay_enabled`, `jade_relay_api_base`, `jade_relay_token` (prefer `JCODE_JADE_RELAY_TOKEN`), `jade_relay_token_id`, `jade_relay_user_id`, `jade_relay_session_id`, `jade_relay_reply_enabled`, `jade_relay_launch_enabled`, `jade_relay_launch_working_dir` |

The `*_reply_enabled` and `jade_relay_launch_enabled` flags turn an inbound
message into an agent directive or a local session launch. All default to
`false`. Enable them only on channels you control.

## `[gateway]`

WebSocket gateway for the iOS and web clients.

| Key | Type | Default |
| --- | --- | --- |
| `enabled` | bool | `false` |
| `port` | int | `7643` |
| `bind_addr` | string | `0.0.0.0` |

## `[power]`

| Key | Type | Default | Meaning |
| --- | --- | --- | --- |
| `prevent_sleep_while_streaming` | bool | `true` | Block system sleep while a session streams. Linux also blocks lid-switch suspend via logind; Windows still obeys its power plan for lid/power-button actions. Display sleep is unaffected. `JCODE_DISABLE_POWER_INHIBIT` forces it off. |

## `[dictation]`

| Key | Type | Default | Meaning |
| --- | --- | --- | --- |
| `command` | string | `""` | Shell command that prints the transcript to stdout |
| `mode` | enum | `send` | `insert`, `append`, `replace`, `send` |
| `key` | string | `off` | In-app hotkey |
| `timeout_secs` | int | `90` | `0` = no timeout |

## `[autoreview]` and `[autojudge]`

| Key | Type | Default | Meaning |
| --- | --- | --- | --- |
| `enabled` | bool | `false` | Automatic end-of-turn review / execution judging |
| `model` | string | unset | Model override for those sessions |

## `[sponsors]`

Integration discovery (the `discover_tools` surface).

| Key | Type | Default | Meaning |
| --- | --- | --- | --- |
| `enabled` | bool | `true` | When false, no discovery categories, no tool, no network calls |
| `endpoint` | string | `https://api.jcode.sh/v1/discovery` | |

This section is omitted from written config when it matches the shipped
default, so a save never freezes today's default into the file.

## `[launch_hotkeys]` (macOS)

Global "open a new jcode here" hotkeys.

| Key | Type | Default | Meaning |
| --- | --- | --- | --- |
| `enabled` | bool | undecided | Explicit opt in/out |
| `imported` | bool | `false` | Set once after auto-import bakes `entries`; prevents clobbering later edits |
| `entries` | array of tables | `[]` | Empty = built-in defaults |

```toml
[[launch_hotkeys.entries]]
chord = "cmd+;"
dir = "/Users/me/projects/app"   # or "$HOME", "$LAST_DIR", "$LAST_REPO"
label = "app"
self_dev = false
```

---

## Environment overrides

Nearly every setting has a `JCODE_*` counterpart that wins over the file, which
is how per-invocation and wrapper overrides work. Common ones:

| Variable | Overrides |
| --- | --- |
| `JCODE_HOME` | Config/state directory |
| `JCODE_MODEL`, `JCODE_PROVIDER` | `[provider].default_model` / `default_provider` |
| `JCODE_OPENAI_REASONING_EFFORT`, `JCODE_ANTHROPIC_REASONING_EFFORT` | Reasoning effort |
| `JCODE_TOOLS`, `JCODE_DISABLED_TOOLS`, `JCODE_TOOL_PROFILE`, `JCODE_DISABLE_BASE_TOOLS` | `[tools]` |
| `JCODE_MCP_TOOLS`, `JCODE_MCP_TOOLS_TOKEN_THRESHOLD` | MCP exposure |
| `JCODE_SWARM_MODEL`, `JCODE_SWARM_EFFORT`, `JCODE_SWARM_SPAWN_MODE`, `JCODE_SWARM_MAX_CONCURRENT_AGENTS` | `[agents]` |
| `JCODE_MEMORY_ENABLED`, `JCODE_MEMORY_MODEL`, `JCODE_MEMORY_EMBEDDING_BACKEND` | Memory |
| `JCODE_DIFF_MODE`, `JCODE_REASONING_DISPLAY`, `JCODE_PERFORMANCE`, `JCODE_MOUSE_CAPTURE` | `[display]` |
| `JCODE_SPAWN_HOOK`, `JCODE_FOCUS_HOOK` | `[terminal]` (empty string disables a config hook) |
| `JCODE_HOOK_PRE_TOOL`, `JCODE_HOOK_TURN_END`, ... | `[hooks]` |
| `JCODE_CHECK_UPDATES`, `JCODE_UPDATE_CHANNEL` | `[features]` |
| `JCODE_STREAM_IDLE_TIMEOUT_SECS`, `JCODE_MAX_RETRIES`, `JCODE_RETRY_BACKOFF_CAP_SECS` | Provider resilience |
| `JCODE_GATEWAY_ENABLED`, `JCODE_GATEWAY_PORT`, `JCODE_GATEWAY_BIND_ADDR` | `[gateway]` |
| `JCODE_DISABLE_POWER_INHIBIT` | Forces `[power]` off |
| `JCODE_WAKE_MODE` | `[server].wake_mode` |

The authoritative list is `CONFIG_ENV_KEYS` in
`crates/jcode-base/src/config.rs`; the struct definitions in
`crates/jcode-config-types/src/` are the source of truth for every key and
default documented here.
