use super::*;
use std::path::PathBuf;

impl Config {
    /// Create a default config file with comments
    pub fn create_default_config_file() -> anyhow::Result<PathBuf> {
        let path = Self::path().ok_or_else(|| anyhow::anyhow!("No config path"))?;

        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let default_content = r#"# jcode configuration file
# Location: ~/.jcode/config.toml
#
# Environment variables override these settings.
# Run `/config` in jcode to see current settings.

[keybindings]
# Scroll keys (vim-style by default)
# Supports: ctrl, alt, shift modifiers + any key
# Examples: "ctrl+k", "alt+j", "ctrl+shift+up", "pageup"
scroll_up = "ctrl+k"
scroll_down = "ctrl+j"
scroll_page_up = "alt+u"
scroll_page_down = "alt+d"

# Model switching
model_switch_next = "ctrl+tab"
model_switch_prev = "ctrl+shift+tab"

# Reasoning effort switching (OpenAI models)
# Defaults: cmd+right / cmd+left on macOS, alt+right / alt+left elsewhere.
# Alt/Option+Left/Right move by word in the input box.
effort_increase = "@EFFORT_INCREASE@"
effort_decrease = "@EFFORT_DECREASE@"

# Centered mode toggle key
centered_toggle = "alt+c"

# Jump between user prompts
# Ctrl+1..4 resizes the pinned side panel to 25/50/75/100%.
# Ctrl+5..9 jumps by recency rank (5 = 5th most recent).
scroll_prompt_up = "ctrl+["
scroll_prompt_down = "ctrl+]"

# Scroll bookmark toggle (stash position, jump to bottom, press again to return)
scroll_bookmark = "ctrl+g"

# Optional fallback scroll bindings (useful on macOS terminals that forward Command)
# Leave unset by default; on macOS Cmd+K / Cmd+J move up / down by prompt instead.
scroll_up_fallback = ""
scroll_down_fallback = ""

# Workspace navigation (Niri-style)
# Comma-separate multiple bindings to add aliases.
workspace_left = "alt+h"
workspace_down = "alt+j"
workspace_up = "alt+k"
workspace_right = "alt+l"

# Pane / mode toggles
side_panel_toggle = "alt+m"
copy_selection_toggle = "alt+y"
diagram_pane_toggle = "alt+t"
typing_scroll_lock_toggle = "alt+s"
diff_mode_cycle = "alt+g"
info_widget_toggle = "alt+i"
# Focus the inline swarm panel (list of agents this session manages). While
# focused: j/k select, o pops the selected agent out to a new terminal, esc
# exits. Active only with agents.swarm_spawn_mode = "inline".
swarm_panel_focus = "alt+w"

# Spawn a fresh jcode session in a new terminal window, reusing the current
# session's working directory. Companion to the system-wide launch hotkeys.
# On macOS, `jcode setup-hotkey` installs three global launch hotkeys:
#   Cmd+;        new jcode in your home directory
#   Cmd+'        new jcode in your last project directory
#   Cmd+Shift+'  new jcode self-dev session (last jcode repo)
# Default: Cmd+Shift+; on macOS, Alt+Shift+; elsewhere. Set "" to disable.
# Note: some macOS terminals intercept Cmd combos; if so, pick another binding.
# new_terminal = "cmd+shift+;"

# Open the /resume session picker.
# Default: Cmd+B on macOS, Alt+R on Windows/Linux. Set "" to disable.
# open_resume = "cmd+b"

# /resume picker Enter behavior. Options: "current-terminal" or "new-terminal".
# By default Enter resumes in this terminal; Ctrl+Enter performs the alternate action.
session_picker_enter = "current-terminal"

[dictation]
# External speech-to-text command.
# The command should record/transcribe speech and print the final transcript to stdout.
# You can include any tool-specific flags here too, for example a grammar target.
# Examples:
# command = "~/.local/bin/my-whisper-script"
# command = "~/.local/bin/my-whisper-script --grammar-target code"
command = ""

# How to apply the transcript inside jcode: insert|append|replace|send
mode = "send"

# Optional in-app hotkey to trigger dictation. Set to "off" to disable.
# Example: "alt+;"
key = "off"

# Max seconds to wait for the dictation command to finish (0 = no timeout)
timeout_secs = 90

[display]
# Diff display mode: "off", "inline" (default), "full-inline", "pinned" (dedicated pane), or "file"
diff_mode = "inline"

# Center all content by default (default: false)
centered = false

# Pin read images to a side pane (default: true)
pin_images = true

# Wrap long lines in the pinned diff pane (default: true)
# Set to false for horizontal scrolling instead of wrapping
diff_line_wrap = true

# Queue mode: wait until assistant is done before sending next message
queue_mode = false

# Automatically reload the remote server when a newer server binary is detected (default: true)
auto_server_reload = true

# Capture mouse events (enables scroll wheel; disables terminal text selection)
mouse_capture = true

# Enable debug socket for external control/testing (default: false)
debug_socket = false

# Show thinking/reasoning content (default: true)
show_thinking = true

# How to display reasoning/thinking content: "off", "full", or "current".
#   off     - never show reasoning
#   full    - keep every reasoning trace in the transcript
#   current - show only the live reasoning; collapse it once the model commits
#             an assistant message or runs a tool, then show the next one
# When unset, falls back to show_thinking (true => full, false => off).
reasoning_display = "current"

# Markdown spacing style: "compact" (chat/TUI) or "document" (docs-like)
# markdown_spacing = "compact"

# Show idle animation before first prompt (default: true)
idle_animation = true

# Briefly animate a user prompt line when it enters the viewport (default: true)
prompt_entry_animation = true

# Render swarm/file-activity notifications in a compact single-line form
# instead of the full multi-line card with diff preview (default: false)
# compact_notifications = false

# Show the full agentgrep tool output inline in the transcript instead of just
# the one-line summary (default: false). Useful when you want to read search
# results directly in the chat.
# show_agentgrep_output = false

# Disable specific animation variants by name.
# Examples: ["donut"] or ["donut", "orbit_rings"]
# Legacy aliases such as "three_rings" and "gyroscope" are still accepted.
# disabled_animations = []

# Performance tier: auto/full/reduced/minimal (default: auto)
# auto = detect system load, memory, terminal type, SSH, and apply extra caps for WSL/Windows Terminal
# full = all animations enabled
# reduced = skip idle animations, keep spinners
# minimal = disable all animations, slower redraw rate
# performance = "auto"

# Animation FPS (idle animation): 1-120 (default: 60)
# Runtime policy may cap this lower on slower environments such as WSL/Windows Terminal.
# animation_fps = 60

# Active redraw FPS (processing, streaming, spinners): 1-120 (default: 60)
# Runtime policy may cap this lower on slower environments such as WSL/Windows Terminal.
# redraw_fps = 60

# Label shown for the Alt/Option modifier in copy badges.
# Empty = auto ("⌥" on macOS, "Alt" elsewhere). Examples: "Option", "Alt", "⌥".
# copy_badge_alt_label = ""

[features]
# Memory: retrieval + extraction sidecar features
memory = true
# Swarm: multi-session coordination features
swarm = true
# Inject timestamps into user messages and tool results sent to the model
message_timestamps = true
# Persist memory injections into session history instead of sending them as request-only ephemeral context
persist_memory_injections = false
# Show an in-chat warning when a request misses the KV cache for a harness-caused
# (avoidable) reason: system prompt, tool set, or message prefix changed. These
# should essentially never happen and indicate a prefix-cache bug.
kv_cache_miss_notices = true
# Update channel: "stable" (releases only) or "main" (latest commits on push)
# Set to "main" for bleeding edge updates every time code is pushed
update_channel = "stable"

[websearch]
# Preferred websearch engine: "duckduckgo", "bing", or "searxng".
engine = "duckduckgo"
# Keyless HTML engines to try if the preferred engine fails. Default falls back to Bing HTML.
fallback_engines = ["bing"]
# Bring your own Bing Search API key for primary Bing searches. Prefer using an env var.
# Fallback Bing searches intentionally use keyless HTML search.
# bing_api_key_env = "JCODE_BING_API_KEY"
# bing_api_key = ""
# Bing market/region, for example "en-US" or "zh-CN".
bing_market = "en-US"
# SearXNG instance for the "searxng" engine. On some hosts (commonly Linux),
# DuckDuckGo and Bing block scraped requests via TLS fingerprinting / IP
# reputation and return an anti-bot page with no results. Pointing at a SearXNG
# instance (self-hosted or trusted public) with the JSON format enabled avoids
# this. Configure here or via the JCODE_SEARXNG_URL environment variable, then
# set engine = "searxng" or add it to fallback_engines.
# searxng_url = "https://searx.example.org"

[tools]
# Controls which built-in tools are sent to the model.
# Profiles: "full" (default), "acp", "minimal"/"lite", or "none".
# acp keeps core coding tools plus batch for generic ACP clients.
# minimal keeps core coding tools only: bash, read, write, edit, multiedit,
# apply_patch, patch, agentgrep, glob, grep, and ls.
profile = "full"
# Explicit allow-list. When non-empty, only these tools are exposed.
# enabled = ["bash", "read", "write", "apply_patch", "agentgrep", "ls"]
# Privacy-sensitive or stub tools such as gmail and lsp are disabled by default.
# To expose every tool including default-disabled tools, use: enabled = ["*"]
# Hide selected tools after applying the profile/allow-list.
# disabled = ["browser", "gmail", "lsp", "swarm"]
# Disable all built-in tools unless enabled is set.
disable_base_tools = false

[acp]
# Agent Client Protocol adapter compatibility profile: standard, extended, or full.
# standard emits only spec-compatible ACP messages.
# extended/full additionally emit ignorable _jcode/* extension notifications.
profile = "standard"
# Tool profile requested when `jcode acp` starts the daemon itself.
# Existing daemons keep their current server-wide tool config.
tool_profile = "acp"

[provider]
# Default model (optional, uses provider default if not set)
# Set via /model picker with Ctrl+B to save as default
# default_model = "claude-fable-5"
# Default provider (optional: claude|anthropic-api|openai|openai-api|copilot|openrouter|...)
# When set, this provider is preferred on startup if available.
#   claude        = Claude via OAuth/subscription (token in ~/.jcode/auth.json)
#   anthropic-api = Claude via direct Anthropic API key (ANTHROPIC_API_KEY env
#                   or ~/.config/jcode/anthropic.env). API-key mode does NOT fall
#                   back to OAuth; configure the key first.
# `claude` and `anthropic-api` are distinct providers with distinct credentials.
# See docs/AUTH_CREDENTIAL_SOURCES.md for where each credential lives.
# default_provider = "copilot"
# OpenAI reasoning effort (none|low|medium|high|xhigh)
openai_reasoning_effort = "low"
# Anthropic reasoning effort for Claude reasoning models (none|low|medium|high; xhigh on Opus 4.7; max aliases to the strongest supported level)
# Defaults to the strongest supported level for Claude Opus models (xhigh on Opus 4.7/4.8, high on older Opus) when unset; other models keep their own default.
# anthropic_reasoning_effort = "medium"
# OpenAI transport mode (auto|websocket|https)
# openai_transport = "auto"
# OpenAI service tier override (priority|flex)
# Defaults to `priority` to match Codex /fast behavior for OpenAI OAuth
# (higher speed, higher usage). Set to "off" to disable.
openai_service_tier = "priority"
# Preserve provider-native reasoning/thinking for future-turn context when supported.
# Applies to OpenRouter, Anthropic, and OpenAI native reasoning replay. Display is separate.
preserve_reasoning_context = true
# Cross-provider failover when the same prompt would be resent elsewhere.
# countdown = 3-second countdown before retrying on another provider; press Esc to cancel (default)
# manual = show a notice and let you switch yourself
# cross_provider_failover = "manual"
# Try another account on the same provider before switching providers (default: true)
# same_provider_account_failover = false
cross_provider_failover = "countdown"
# Copilot premium mode: "normal" (default), "one" (first msg only), "zero" (all free)
# Set to "zero" if you have premium Copilot and want free requests
# copilot_premium = "zero"
# Max seconds to wait for streaming data before timing out a request with no
# data received. Raise this for slow reasoning models (e.g. DeepSeek) that think
# silently for minutes before emitting tokens. Default: 180.
# Also overridable per-launch via JCODE_STREAM_IDLE_TIMEOUT_SECS.
# stream_idle_timeout_secs = 600

[agents]
# Defaults for spawned helper agents (swarm workers, subagents, sidecars).
# All keys are optional; the values below are the built-in defaults.
#
# Default model for spawned swarm/subagent sessions.
# Leave unset (or "inherit"/"coordinator") so workers inherit the model of the
# session that spawned them. Set a concrete model only to pin every worker to it.
# Env override: JCODE_SWARM_MODEL
# swarm_model = "inherit"
#
# How swarm-created agents are spawned:
#   "visible"  - open a headed terminal window (default; alias: "headed")
#   "headless" - create the worker in-process with no terminal window
#   "inline"   - in-process (no window), shown as a live gallery viewport in the coordinator
#   "auto"     - try visible first, fall back to headless if no window can open
# The swarm tool's per-call `spawn_mode` overrides this when set.
# Env override: JCODE_SWARM_SPAWN_MODE
swarm_spawn_mode = "visible"
#
# Max percentage (1-90) of the chat height the inline swarm gallery band may use.
# Unset = built-in default (40%). Lower values keep more transcript visible; set
# near the minimum to collapse the gallery to a thin strip.
# swarm_gallery_max_pct = 40
#
# Model for the memory sidecar (relevance/extraction). Unset = sidecar auto-select.
# Env override: JCODE_MEMORY_MODEL
# memory_model = "claude-haiku-4"
#
# Whether the memory sidecar (LLM precision judge) handles relevance/extraction.
# Default true: the LLM precision-judge path is the only reliably productive
# memory mode. Set false only to opt into the lower-precision no-LLM hybrid path.
# When this is true but no LLM backend is reachable (logged out), memory goes
# dormant instead of degrading to the no-LLM path. Env: JCODE_MEMORY_SIDECAR_ENABLED
# memory_sidecar_enabled = true
#
# Minimum turns between Mode-2 memory reranks (cadence floor). The expensive
# listwise LLM rerank runs at most once per this many turns; skipped turns fall
# back to hybrid-ordered surfacing. A topic change always forces a rerank. Set 1
# to rerank every turn. Default 3.
# memory_rerank_cadence = 3
#
# High-precision consensus rerank: run N independent LLM judges per fired rerank
# and inject only memories that >= memory_rerank_min_agree of them agree on.
# Default 2 judges / 2 agreement -> injection precision ~1.0 with ~100% clean
# (zero memory) on no-memory turns, at 2 LLM calls per fired turn. Set votes=1
# for the cheaper single-judge path (precision ~0.77).
# memory_rerank_votes = 2
# memory_rerank_min_agree = 2
#
# Embedding backend for memory dense-retrieval. "local" (default) uses the
# bundled all-MiniLM-L6-v2 ONNX model (no network); "openai" uses a remote
# OpenAI / OpenAI-compatible /v1/embeddings endpoint (requires OPENAI_API_KEY;
# silently falls back to local when no key is found). Vectors from different
# models live in separate spaces and are never compared, so switching is safe.
# Env override: JCODE_MEMORY_EMBEDDING_BACKEND
# memory_embedding_backend = "local"
# memory_embedding_model = "text-embedding-3-small"
# memory_embedding_base_url = "https://api.openai.com/v1"
# memory_embedding_dim = 1536

[terminal]
# External command that takes over headed session spawns (swarm agents,
# resume-in-new-terminal, self-dev windows, restart restores).
#
# When set, jcode runs `<spawn_hook> <jcode-binary> <args...>` instead of
# opening a terminal emulator itself. The hook receives JCODE_SPAWN_* env vars
# describing the spawn so multiplexers/wrappers can decide where it appears:
#   JCODE_SPAWN_KIND        - "swarm-agent", "resume", "selfdev", "restart", ...
#   JCODE_SPAWN_SESSION_ID  - session the window will run
#   JCODE_SPAWN_TITLE       - suggested window/tab title
#   JCODE_SPAWN_CWD         - session working directory (also the hook's cwd)
#   JCODE_SPAWN_PROGRAM     - jcode binary path
#   JCODE_SPAWN_COMMAND     - full shell-escaped command line
#   JCODE_SPAWN_SWARM_ID / JCODE_SPAWN_COORDINATOR_SESSION_ID (swarm spawns)
# If the hook fails to start, jcode falls back to built-in terminal detection.
# Env override: JCODE_SPAWN_HOOK (set empty to disable a config hook).
#
# Examples:
#   spawn_hook = "tmux new-window"                # tmux window per agent
#   spawn_hook = "kitty @ launch --type=tab --"   # kitty tab per agent
#   spawn_hook = "~/bin/jcode-spawn-router"       # custom placement script
# spawn_hook = ""
#
# External command used to focus/raise an existing session window, replacing
# the built-in wmctrl/xdotool title search. Receives JCODE_FOCUS_SESSION_ID
# and JCODE_FOCUS_TITLE env vars. Pair with spawn_hook so the program that
# placed the window also brings it to the front.
# Env override: JCODE_FOCUS_HOOK (set empty to disable a config hook).
#
# Example:
#   focus_hook = "~/bin/jcode-focus-router"
# focus_hook = ""
#
# macOS only: terminal that the Cmd+; launch hotkey and in-app session spawns
# open jcode into. One of: ghostty, iterm2, wezterm, warp, alacritty, vscode,
# terminal (Apple Terminal). Preferred over the legacy
# ~/.jcode/preferred_terminal.json file. After changing this, re-run
# `jcode setup-hotkey` so the generated launcher script (Cmd+;) picks it up.
# preferred = "ghostty"

[notifications]
# Desktop notifications for interactive sessions (macOS Notification Center /
# Linux notify-send). Separate from [safety], which covers ambient-mode
# ntfy/email/channel notifications.
#
# Notify when an agent turn finishes. Fires only for long turns and, by
# default, only while the terminal window is unfocused. The notification is a
# compact summary: session name, duration, todo progress, and a snippet of the
# final assistant message.
# turn_complete = true
# Minimum turn duration (seconds) before notifying (default: 120)
# turn_complete_min_secs = 120
# Lower threshold (seconds) when the session has todos, since todos indicate
# task-style work worth reporting sooner (default: 30)
# turn_complete_todo_min_secs = 30
# Only notify while the terminal window is unfocused (default: true)
# turn_complete_only_when_unfocused = true
# macOS Notification Center sound played on completion (e.g. "Glass", "Ping",
# "Hero"). Empty string disables the sound. Ignored on non-macOS. (default: "Glass")
# turn_complete_sound = "Glass"

[hooks]
# Lifecycle hooks: external commands jcode runs at well-defined points so other
# programs can observe or gate agent behavior. Commands are parsed shell-style
# (quotes work) but executed directly, with JCODE_HOOK_* env vars describing
# the event:
#   JCODE_HOOK_EVENT       - "turn_start", "turn_end", "session_start",
#                            "session_end", "pre_tool", "post_tool"
#   JCODE_HOOK_SESSION_ID  - the session the event belongs to
#   JCODE_HOOK_CWD         - session working directory (also the hook's cwd)
#   JCODE_HOOK_PAYLOAD     - JSON mirror of all fields
# Hook processes get JCODE_HOOKS_DISABLED=1 so nested jcode calls don't recurse.
#
# All hooks except pre_tool are observers: detached, fire-and-forget, failures
# only logged. Env overrides: JCODE_HOOK_TURN_START, JCODE_HOOK_TURN_END,
# JCODE_HOOK_SESSION_START, JCODE_HOOK_SESSION_END, JCODE_HOOK_PRE_TOOL,
# JCODE_HOOK_POST_TOOL (set empty to disable a config hook).
#
# Runs when an agent turn begins, before the model starts generating and before
# the first pre_tool. Lets integrations detect the agent is working during the
# think/stream window before any tool call. Extra fields: JCODE_HOOK_MODEL,
# JCODE_HOOK_SOURCE ("chat"/"resume"/"ambient").
# turn_start = "~/bin/jcode-turn-start"
#
# Runs when an agent turn completes. Extra fields: JCODE_HOOK_STATUS
# ("ok"/"error"), JCODE_HOOK_DURATION_MS, JCODE_HOOK_MODEL,
# JCODE_HOOK_LAST_ASSISTANT_TEXT (first 4000 chars), JCODE_HOOK_ERROR.
# turn_end = "~/bin/jcode-turn-notify"
#
# Runs when a session becomes active. Extra: JCODE_HOOK_SOURCE
# ("create"/"attach"/"resume").
# session_start = ""
#
# Runs when a session closes normally. Extra: JCODE_HOOK_SOURCE ("close").
# session_end = ""
#
# Gate hook before every tool call. Receives JCODE_HOOK_TOOL_NAME and the tool
# input JSON on stdin (truncated copy in JCODE_HOOK_TOOL_INPUT). Exit 0 allows
# the call; exit 2 blocks it and stderr is shown to the model as the error;
# any other outcome (other exits, timeout, missing binary) fails open.
# pre_tool = "~/bin/jcode-tool-policy"
#
# Max milliseconds to wait for pre_tool before failing open (default: 5000).
# pre_tool_timeout_ms = 5000
#
# Runs after each tool call. Extra fields: JCODE_HOOK_TOOL_NAME,
# JCODE_HOOK_STATUS, JCODE_HOOK_DURATION_MS, JCODE_HOOK_OUTPUT_BYTES,
# JCODE_HOOK_ERROR.
# post_tool = ""

[ambient]
# Ambient mode: background agent that maintains your codebase
# Enable ambient mode (default: false)
enabled = false
# Provider override (default: auto-select based on available credentials)
# provider = "claude"
# Model override (default: provider's strongest)
# model = "claude-sonnet-4-20250514"
# Allow API key usage (default: false, only OAuth to avoid surprise costs)
allow_api_keys = false
# Daily token budget when using API keys (optional)
# api_daily_budget = 100000
# Minimum interval between cycles in minutes
min_interval_minutes = 5
# Maximum interval between cycles in minutes
max_interval_minutes = 120
# Pause ambient when user has active session
pause_on_active_session = true
# Enable proactive work (new features, refactoring) vs garden-only (lint, format, deps)
proactive_work = true
# Branch prefix for proactive work
work_branch_prefix = "ambient/"
# Show ambient cycle in a terminal window (default: true)
# visible = true

[gateway]
# Enable WebSocket gateway for iOS/web clients
enabled = false
# TCP port for gateway listener
port = 7643
# Bind address (0.0.0.0 for LAN/Tailscale reachability)
bind_addr = "0.0.0.0"

[power]
# Prevent the machine from suspending (idle/lid sleep) while any jcode session
# is actively streaming/processing. The display can still sleep; only system
# suspend is inhibited, and only for as long as work is in flight. (default: true)
# Set JCODE_DISABLE_POWER_INHIBIT=1 to force-disable regardless of this setting.
prevent_sleep_while_streaming = true

[safety]
# Notification settings for ambient mode events

# ntfy.sh push notifications (free, phone app: https://ntfy.sh)
# ntfy_topic = "jcode-ambient-your-secret-topic"
# ntfy_server = "https://ntfy.sh"

# Desktop notifications via notify-send (default: true)
desktop_notifications = true

# Email notifications via SMTP
# email_enabled = false
# email_to = "you@example.com"
# email_from = "jcode@example.com"
# email_smtp_host = "smtp.gmail.com"
# email_smtp_port = 587
# Password via env: JCODE_SMTP_PASSWORD (preferred) or config below
# email_password = ""

# IMAP for email replies (reply to ambient emails to send directives)
# email_reply_enabled = false
# email_imap_host = "imap.gmail.com"
# email_imap_port = 993

# Telegram notifications via Bot API (free, https://telegram.org)
# telegram_enabled = false
# telegram_bot_token = ""  # From @BotFather (prefer JCODE_TELEGRAM_BOT_TOKEN env var)
# telegram_chat_id = ""    # Your user/chat ID
# telegram_reply_enabled = false  # Reply to bot messages to send directives

# Discord notifications via Bot API (https://discord.com/developers)
# discord_enabled = false
# discord_bot_token = ""     # From Discord Developer Portal (prefer JCODE_DISCORD_BOT_TOKEN env var)
# discord_channel_id = ""    # Channel ID to post in
# discord_bot_user_id = ""   # Bot's user ID (for filtering own messages)
# discord_reply_enabled = false  # Messages in channel become agent directives

# Jade cloud relay (outbound-only long polling, disabled by default).
# Prefer environment variables for secrets:
# JCODE_JADE_RELAY_API_BASE, JCODE_JADE_RELAY_TOKEN, JCODE_JADE_RELAY_TOKEN_ID,
# JCODE_JADE_RELAY_USER_ID, JCODE_JADE_RELAY_SESSION_ID.
# jade_relay_enabled = false
# jade_relay_reply_enabled = false   # Deliver cloud prompts to one configured live session.
# jade_relay_launch_enabled = false  # Allow cloud device commands to open headed local sessions.
# jade_relay_launch_working_dir = "" # Optional default cwd for launched sessions.
	"#;

        // Substitute platform-specific defaults from the keybinding registry.
        let p = jcode_config_types::KeybindingPlatform::current();
        let effort_increase =
            jcode_config_types::default_binding("effort_increase", p).unwrap_or("alt+right");
        let effort_decrease =
            jcode_config_types::default_binding("effort_decrease", p).unwrap_or("alt+left");
        let default_content = default_content
            .replace("@EFFORT_INCREASE@", effort_increase)
            .replace("@EFFORT_DECREASE@", effort_decrease);

        std::fs::write(&path, default_content)?;
        Ok(path)
    }
}
