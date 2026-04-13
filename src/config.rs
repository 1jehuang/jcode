//! Configuration file support for jcode
//!
//! Config is loaded from `~/.jcode/config.toml` (or `$JCODE_HOME/config.toml`)
//! Environment variables override config file settings.

use crate::storage::jcode_dir;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::OnceLock;

static CONFIG: OnceLock<Config> = OnceLock::new();

/// Get the global config instance (loaded once on first access)
pub fn config() -> &'static Config {
    CONFIG.get_or_init(Config::load)
}

/// Compaction mode
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "lowercase")]
pub enum CompactionMode {
    /// Compact when context hits a fixed threshold (default)
    #[default]
    Reactive,
    /// Compact early based on predicted token growth rate
    Proactive,
    /// Compact based on semantic topic shifts and relevance scoring
    Semantic,
}

impl CompactionMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Reactive => "reactive",
            Self::Proactive => "proactive",
            Self::Semantic => "semantic",
        }
    }

    pub fn parse(input: &str) -> Option<Self> {
        match input.trim().to_ascii_lowercase().as_str() {
            "reactive" => Some(Self::Reactive),
            "proactive" => Some(Self::Proactive),
            "semantic" => Some(Self::Semantic),
            _ => None,
        }
    }
}

/// Compaction configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct CompactionConfig {
    /// Compaction mode: reactive (default), proactive, or semantic
    pub mode: CompactionMode,

    /// [proactive] Number of turns to look ahead when projecting token growth
    pub lookahead_turns: usize,

    /// [proactive] EWMA alpha for token growth smoothing (0.0-1.0, higher = more recency bias)
    pub ewma_alpha: f32,

    /// [proactive/semantic] Minimum context fill level before any proactive check fires (0.0-1.0)
    pub proactive_floor: f32,

    /// [proactive/semantic] Minimum number of token snapshots needed before proactive check
    pub min_samples: usize,

    /// [proactive/semantic] Number of stable turns (no growth) before suppressing proactive compact
    pub stall_window: usize,

    /// [proactive/semantic] Minimum turns between two compactions (cooldown)
    pub min_turns_between_compactions: usize,

    /// [semantic] Cosine similarity threshold below which a topic shift is detected (0.0-1.0)
    pub topic_shift_threshold: f32,

    /// [semantic] Cosine similarity above which a message is kept verbatim (0.0-1.0)
    pub relevance_keep_threshold: f32,

    /// [semantic] Number of recent turns to look at for building the "current goal" embedding
    pub goal_window_turns: usize,
}

impl Default for CompactionConfig {
    fn default() -> Self {
        Self {
            mode: CompactionMode::Reactive,
            lookahead_turns: 15,
            ewma_alpha: 0.3,
            proactive_floor: 0.40,
            min_samples: 3,
            stall_window: 5,
            min_turns_between_compactions: 10,
            topic_shift_threshold: 0.45,
            relevance_keep_threshold: 0.65,
            goal_window_turns: 5,
        }
    }
}

/// Main configuration struct
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Config {
    /// Keybinding configuration
    pub keybindings: KeybindingsConfig,

    /// External dictation / speech-to-text integration
    pub dictation: DictationConfig,

    /// Display/UI configuration
    pub display: DisplayConfig,

    /// Feature toggles
    pub features: FeatureConfig,

    /// Auth trust / consent configuration
    pub auth: AuthConfig,

    /// Provider configuration
    pub provider: ProviderConfig,

    /// Agent-specific model defaults
    pub agents: AgentsConfig,

    /// Ambient mode configuration
    pub ambient: AmbientConfig,

    /// Safety / notification configuration
    pub safety: SafetyConfig,

    /// WebSocket gateway configuration (for iOS/web clients)
    pub gateway: GatewayConfig,

    /// Compaction configuration
    pub compaction: CompactionConfig,

    /// Auto-review configuration
    pub autoreview: AutoReviewConfig,

    /// Auto-judge configuration
    pub autojudge: AutoJudgeConfig,
}

/// Remembered trust decisions for external auth sources managed by other tools.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct AuthConfig {
    /// External auth source ids that the user has approved jcode to read/use.
    pub trusted_external_sources: Vec<String>,
    /// Path-bound approvals for external auth sources managed by other tools.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub trusted_external_source_paths: Vec<String>,
}

/// Agent-specific model defaults.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct AgentsConfig {
    /// Optional default model override for spawned swarm/subagent sessions.
    pub swarm_model: Option<String>,
    /// Optional default model override for the memory sidecar.
    pub memory_model: Option<String>,
    /// Whether memory should use the sidecar for relevance/extraction.
    pub memory_sidecar_enabled: bool,
}

/// Automatic end-of-turn code review configuration.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct AutoReviewConfig {
    /// Enable autoreview by default for new/resumed sessions (default: false)
    pub enabled: bool,
    /// Optional model override for autoreview reviewer sessions.
    pub model: Option<String>,
}

/// Automatic end-of-turn execution judging configuration.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct AutoJudgeConfig {
    /// Enable autojudge by default for new/resumed sessions (default: false)
    pub enabled: bool,
    /// Optional model override for autojudge sessions.
    pub model: Option<String>,
}

/// Keybinding configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct KeybindingsConfig {
    /// Scroll up key (default: "ctrl+k")
    pub scroll_up: String,
    /// Scroll down key (default: "ctrl+j")
    pub scroll_down: String,
    /// Page up key (default: "alt+u")
    pub scroll_page_up: String,
    /// Page down key (default: "alt+d")
    pub scroll_page_down: String,
    /// Model switch next key (default: "ctrl+tab")
    pub model_switch_next: String,
    /// Model switch previous key (default: "ctrl+shift+tab")
    pub model_switch_prev: String,
    /// Effort increase key (default: "alt+right")
    pub effort_increase: String,
    /// Effort decrease key (default: "alt+left")
    pub effort_decrease: String,
    /// Centered mode toggle key (default: "alt+c")
    pub centered_toggle: String,
    /// Scroll to previous prompt key (default: "ctrl+[")
    pub scroll_prompt_up: String,
    /// Scroll to next prompt key (default: "ctrl+]")
    pub scroll_prompt_down: String,
    /// Scroll bookmark toggle key (default: "ctrl+g")
    pub scroll_bookmark: String,
    /// Scroll up fallback key (default: "cmd+k")
    pub scroll_up_fallback: String,
    /// Scroll down fallback key (default: "cmd+j")
    pub scroll_down_fallback: String,
    /// Workspace navigation left key (default: "alt+h")
    pub workspace_left: String,
    /// Workspace navigation down key (default: "alt+j")
    pub workspace_down: String,
    /// Workspace navigation up key (default: "alt+k")
    pub workspace_up: String,
    /// Workspace navigation right key (default: "alt+l")
    pub workspace_right: String,
}

impl Default for KeybindingsConfig {
    fn default() -> Self {
        Self {
            scroll_up: "ctrl+k".to_string(),
            scroll_down: "ctrl+j".to_string(),
            scroll_page_up: "alt+u".to_string(),
            scroll_page_down: "alt+d".to_string(),
            model_switch_next: "ctrl+tab".to_string(),
            model_switch_prev: "ctrl+shift+tab".to_string(),
            effort_increase: "alt+right".to_string(),
            effort_decrease: "alt+left".to_string(),
            centered_toggle: "alt+c".to_string(),
            scroll_prompt_up: "ctrl+[".to_string(),
            scroll_prompt_down: "ctrl+]".to_string(),
            scroll_bookmark: "ctrl+g".to_string(),
            scroll_up_fallback: "cmd+k".to_string(),
            scroll_down_fallback: "cmd+j".to_string(),
            workspace_left: "alt+h".to_string(),
            workspace_down: "alt+j".to_string(),
            workspace_up: "alt+k".to_string(),
            workspace_right: "alt+l".to_string(),
        }
    }
}

/// External dictation / speech-to-text integration.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct DictationConfig {
    /// Shell command to run. Must print the transcript to stdout.
    pub command: String,
    /// How to apply the resulting transcript.
    pub mode: crate::protocol::TranscriptMode,
    /// Optional in-app hotkey to trigger dictation.
    pub key: String,
    /// Maximum time to wait for the command to finish (0 = no timeout).
    pub timeout_secs: u64,
}

impl Default for DictationConfig {
    fn default() -> Self {
        Self {
            command: String::new(),
            mode: crate::protocol::TranscriptMode::Send,
            key: "off".to_string(),
            timeout_secs: 90,
        }
    }
}

/// How to display file diffs from edit/write tools
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DiffDisplayMode {
    /// Don't show diffs at all
    Off,
    /// Show diffs inline in the chat (default)
    #[default]
    Inline,
    /// Show the full inline diff in the chat without preview truncation
    #[serde(
        rename = "full-inline",
        alias = "full_inline",
        alias = "fullinline",
        alias = "inline-full",
        alias = "inline_full",
        alias = "inlinefull",
        alias = "full"
    )]
    FullInline,
    /// Show diffs in a dedicated pinned pane
    Pinned,
    /// Show full file with diff highlights in side panel, synced to scroll position
    File,
}

impl DiffDisplayMode {
    #[allow(dead_code)]
    pub fn is_off(&self) -> bool {
        matches!(self, DiffDisplayMode::Off)
    }

    pub fn is_inline(&self) -> bool {
        matches!(self, DiffDisplayMode::Inline | DiffDisplayMode::FullInline)
    }

    pub fn is_full_inline(&self) -> bool {
        matches!(self, DiffDisplayMode::FullInline)
    }

    pub fn is_pinned(&self) -> bool {
        matches!(self, DiffDisplayMode::Pinned)
    }

    pub fn is_file(&self) -> bool {
        matches!(self, DiffDisplayMode::File)
    }

    pub fn has_side_pane(&self) -> bool {
        matches!(self, DiffDisplayMode::Pinned | DiffDisplayMode::File)
    }

    pub fn cycle(self) -> Self {
        match self {
            DiffDisplayMode::Off => DiffDisplayMode::Inline,
            DiffDisplayMode::Inline => DiffDisplayMode::FullInline,
            DiffDisplayMode::FullInline => DiffDisplayMode::Pinned,
            DiffDisplayMode::Pinned => DiffDisplayMode::File,
            DiffDisplayMode::File => DiffDisplayMode::Off,
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            DiffDisplayMode::Off => "OFF",
            DiffDisplayMode::Inline => "Inline",
            DiffDisplayMode::FullInline => "Inline Full",
            DiffDisplayMode::Pinned => "Pinned",
            DiffDisplayMode::File => "File",
        }
    }
}

/// How to display mermaid diagrams
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DiagramDisplayMode {
    /// Don't show diagrams in dedicated widgets (only inline in messages)
    None,
    /// Show diagrams in info widget margins (opportunistic, if space available)
    Margin,
    /// Show diagrams in a dedicated pinned pane (forces space allocation)
    #[default]
    Pinned,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DiagramPanePosition {
    #[default]
    Side,
    Top,
}

/// How much vertical spacing to use when rendering markdown blocks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MarkdownSpacingMode {
    /// Compact chat/TUI-oriented spacing.
    #[default]
    Compact,
    /// Document-style spacing between top-level blocks.
    Document,
}

impl MarkdownSpacingMode {
    pub fn label(self) -> &'static str {
        match self {
            Self::Compact => "Compact",
            Self::Document => "Document",
        }
    }
}

/// Display/UI configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct NativeScrollbarConfig {
    /// Show a native terminal scrollbar in the chat viewport (default: true)
    pub chat: bool,
    /// Show a native terminal scrollbar in the side panel (default: true)
    pub side_panel: bool,
}

impl Default for NativeScrollbarConfig {
    fn default() -> Self {
        Self {
            chat: true,
            side_panel: true,
        }
    }
}

/// Display/UI configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct DisplayConfig {
    /// How to display file diffs (off/inline/full-inline/pinned/file, default: inline)
    pub diff_mode: DiffDisplayMode,
    /// Legacy: "show_diffs = true/false" maps to diff_mode inline/off
    #[serde(default)]
    show_diffs: Option<bool>,
    /// Queue mode by default - wait until done before sending (default: false)
    pub queue_mode: bool,
    /// Automatically reload the remote server when a newer server binary is detected (default: true)
    pub auto_server_reload: bool,
    /// Capture mouse events (default: true). Enables scroll wheel but disables terminal selection.
    pub mouse_capture: bool,
    /// Enable debug socket for external control (default: false)
    pub debug_socket: bool,
    /// Center all content (default: true)
    pub centered: bool,
    /// Show thinking/reasoning content by default (default: false)
    pub show_thinking: bool,
    /// How to display mermaid diagrams (none/margin/pinned, default: pinned)
    pub diagram_mode: DiagramDisplayMode,
    /// Markdown block spacing style (compact/document, default: compact)
    pub markdown_spacing: MarkdownSpacingMode,
    /// Pin read images to side pane (default: true)
    pub pin_images: bool,
    /// Show startup animation (default: false)
    pub startup_animation: bool,
    /// Show idle animation before first prompt (default: true)
    pub idle_animation: bool,
    /// Briefly animate user prompt line when it enters viewport (default: true)
    pub prompt_entry_animation: bool,
    /// Disable specific animation variants by name (e.g. ["mobius", "knot"])
    pub disabled_animations: Vec<String>,
    /// Wrap long lines in the pinned diff pane (default: true)
    pub diff_line_wrap: bool,
    /// Performance tier override: auto/full/reduced/minimal (default: auto)
    pub performance: String,
    /// FPS for animations (startup, idle donut): 1-120 (default: 60)
    pub animation_fps: u32,
    /// FPS for active redraw (processing, streaming): 1-120 (default: 30)
    pub redraw_fps: u32,
    /// Show a truncated preview of the previous prompt at the top when it scrolls out of view (default: true)
    pub prompt_preview: bool,
    /// Native terminal scrollbar configuration for scrollable panes
    pub native_scrollbars: NativeScrollbarConfig,
}

impl Default for DisplayConfig {
    fn default() -> Self {
        Self {
            diff_mode: DiffDisplayMode::default(),
            show_diffs: None,
            pin_images: true,
            queue_mode: false,
            auto_server_reload: true,
            mouse_capture: true,
            debug_socket: false,
            centered: true,
            show_thinking: false,
            diagram_mode: DiagramDisplayMode::default(),
            markdown_spacing: MarkdownSpacingMode::default(),
            startup_animation: false,
            idle_animation: true,
            prompt_entry_animation: true,
            disabled_animations: Vec::new(),
            diff_line_wrap: true,
            performance: String::new(),
            animation_fps: 60,
            redraw_fps: 60,
            prompt_preview: true,
            native_scrollbars: NativeScrollbarConfig::default(),
        }
    }
}

impl DisplayConfig {
    fn apply_legacy_compat(&mut self) {
        if let Some(show) = self.show_diffs.take() {
            self.diff_mode = if show {
                DiffDisplayMode::Inline
            } else {
                DiffDisplayMode::Off
            };
        }
    }
}

/// Update channel: how aggressively to receive updates
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[derive(Default)]
pub enum UpdateChannel {
    /// Only update from tagged GitHub Releases (default)
    #[default]
    Stable,
    /// Update from latest commit on main branch (bleeding edge)
    Main,
}

impl std::fmt::Display for UpdateChannel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Stable => write!(f, "stable"),
            Self::Main => write!(f, "main"),
        }
    }
}

/// Runtime feature toggles
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct FeatureConfig {
    /// Enable memory retrieval/extraction features (default: true)
    pub memory: bool,
    /// Enable swarm coordination features (default: true)
    pub swarm: bool,
    /// Inject timestamps into user messages and tool results sent to the model (default: true)
    pub message_timestamps: bool,
    /// Update channel: "stable" (releases only) or "main" (latest commits)
    pub update_channel: UpdateChannel,
}

impl Default for FeatureConfig {
    fn default() -> Self {
        Self {
            memory: true,
            swarm: true,
            message_timestamps: true,
            update_channel: UpdateChannel::default(),
        }
    }
}

/// Provider configuration
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum CrossProviderFailoverMode {
    /// Show a 3-second cancelable countdown, then resend on another provider.
    #[default]
    Countdown,
    /// Do not resend the prompt to another provider automatically.
    Manual,
}

impl CrossProviderFailoverMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Manual => "manual",
            Self::Countdown => "countdown",
        }
    }

    fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "manual" => Some(Self::Manual),
            "countdown" | "auto" | "automatic" => Some(Self::Countdown),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ProviderConfig {
    /// Default model to use (e.g. "claude-opus-4-6", "copilot:claude-opus-4.6")
    pub default_model: Option<String>,
    /// Default provider to use (claude|openai|copilot|openrouter)
    pub default_provider: Option<String>,
    /// Reasoning effort for OpenAI Responses API (none|low|medium|high|xhigh)
    pub openai_reasoning_effort: Option<String>,
    /// OpenAI transport mode (auto|websocket|https)
    pub openai_transport: Option<String>,
    /// OpenAI service tier override (priority|flex)
    pub openai_service_tier: Option<String>,
    /// OpenAI native compaction mode: "auto", "explicit", or "off".
    pub openai_native_compaction_mode: String,
    /// Token threshold at which OpenAI auto native compaction should trigger.
    pub openai_native_compaction_threshold_tokens: usize,
    /// How to handle cross-provider failover when the same input would be resent elsewhere.
    pub cross_provider_failover: CrossProviderFailoverMode,
    /// Whether jcode should automatically try another account on the same provider
    /// before falling back to a different provider.
    pub same_provider_account_failover: bool,
    /// Copilot premium request mode: "normal", "one", or "zero"
    /// "zero" means all requests are free (no premium requests consumed)
    pub copilot_premium: Option<String>,
    /// Custom system prompt to use instead of the default.
    pub system_prompt: Option<String>,
}

impl Default for ProviderConfig {
    fn default() -> Self {
        Self {
            default_model: None,
            default_provider: None,
            openai_reasoning_effort: Some("high".to_string()),
            openai_transport: None,
            openai_service_tier: None,
            openai_native_compaction_mode: "auto".to_string(),
            openai_native_compaction_threshold_tokens: 200_000,
            cross_provider_failover: CrossProviderFailoverMode::Countdown,
            same_provider_account_failover: true,
            copilot_premium: None,
            system_prompt: None,
        }
    }
}

/// Ambient mode configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AmbientConfig {
    /// Enable ambient mode (default: false)
    pub enabled: bool,
    /// Provider override (default: auto-select)
    pub provider: Option<String>,
    /// Model override (default: provider's strongest)
    pub model: Option<String>,
    /// Allow API key usage (default: false, only OAuth)
    pub allow_api_keys: bool,
    /// Daily token budget when using API keys
    pub api_daily_budget: Option<u64>,
    /// Minimum interval between cycles in minutes (default: 5)
    pub min_interval_minutes: u32,
    /// Maximum interval between cycles in minutes (default: 120)
    pub max_interval_minutes: u32,
    /// Pause ambient when user has active session (default: true)
    pub pause_on_active_session: bool,
    /// Enable proactive work vs garden-only (default: true)
    pub proactive_work: bool,
    /// Proactive work branch prefix (default: "ambient/")
    pub work_branch_prefix: String,
    /// Show ambient cycle in a terminal window (default: true)
    pub visible: bool,
}

impl Default for AmbientConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            provider: None,
            model: None,
            allow_api_keys: false,
            api_daily_budget: None,
            min_interval_minutes: 5,
            max_interval_minutes: 120,
            pause_on_active_session: true,
            proactive_work: true,
            work_branch_prefix: "ambient/".to_string(),
            visible: true,
        }
    }
}

/// Safety system & notification configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SafetyConfig {
    /// ntfy.sh topic name (required for push notifications)
    pub ntfy_topic: Option<String>,
    /// ntfy.sh server URL (default: https://ntfy.sh)
    pub ntfy_server: String,
    /// Enable desktop notifications via notify-send (default: true)
    pub desktop_notifications: bool,
    /// Enable email notifications (default: false)
    pub email_enabled: bool,
    /// Email recipient
    pub email_to: Option<String>,
    /// SMTP host (e.g. smtp.gmail.com)
    pub email_smtp_host: Option<String>,
    /// SMTP port (default: 587)
    pub email_smtp_port: u16,
    /// Email sender address
    pub email_from: Option<String>,
    /// SMTP password (prefer JCODE_SMTP_PASSWORD env var)
    pub email_password: Option<String>,
    /// IMAP host for receiving email replies (e.g. imap.gmail.com)
    pub email_imap_host: Option<String>,
    /// IMAP port (default: 993)
    pub email_imap_port: u16,
    /// Enable email reply → agent directive feature (default: false)
    pub email_reply_enabled: bool,
    /// Enable Telegram notifications (default: false)
    pub telegram_enabled: bool,
    /// Telegram bot token (from @BotFather)
    pub telegram_bot_token: Option<String>,
    /// Telegram chat ID to send messages to
    pub telegram_chat_id: Option<String>,
    /// Enable Telegram reply → agent directive feature (default: false)
    pub telegram_reply_enabled: bool,
    /// Enable Discord notifications (default: false)
    pub discord_enabled: bool,
    /// Discord bot token
    pub discord_bot_token: Option<String>,
    /// Discord channel ID to send messages to
    pub discord_channel_id: Option<String>,
    /// Discord bot user ID (for filtering own messages in polling)
    pub discord_bot_user_id: Option<String>,
    /// Enable Discord reply → agent directive feature (default: false)
    pub discord_reply_enabled: bool,
}

impl Default for SafetyConfig {
    fn default() -> Self {
        Self {
            ntfy_topic: None,
            ntfy_server: "https://ntfy.sh".to_string(),
            desktop_notifications: true,
            email_enabled: false,
            email_to: None,
            email_smtp_host: None,
            email_smtp_port: 587,
            email_from: None,
            email_password: None,
            email_imap_host: None,
            email_imap_port: 993,
            email_reply_enabled: false,
            telegram_enabled: false,
            telegram_bot_token: None,
            telegram_chat_id: None,
            telegram_reply_enabled: false,
            discord_enabled: false,
            discord_bot_token: None,
            discord_channel_id: None,
            discord_bot_user_id: None,
            discord_reply_enabled: false,
        }
    }
}

impl Config {
    /// Get the config file path
    pub fn path() -> Option<PathBuf> {
        jcode_dir().ok().map(|d| d.join("config.toml"))
    }

    /// Load config from file, with environment variable overrides
    pub fn load() -> Self {
        let mut config = Self::load_from_file().unwrap_or_default();
        config.apply_env_overrides();
        config
    }

    /// Load config from file only (no env overrides)
    fn load_from_file() -> Option<Self> {
        let path = Self::path()?;
        if !path.exists() {
            return None;
        }

        let content = std::fs::read_to_string(&path).ok()?;
        match toml::from_str::<Self>(&content) {
            Ok(mut config) => {
                config.display.apply_legacy_compat();
                Some(config)
            }
            Err(e) => {
                crate::logging::error(&format!("Failed to parse config file: {}", e));
                None
            }
        }
    }

    /// Apply environment variable overrides
    #[expect(
        clippy::collapsible_if,
        reason = "Environment override parsing is intentionally explicit and grouped by config area"
    )]
    fn apply_env_overrides(&mut self) {
        // Keybindings
        if let Ok(v) = std::env::var("JCODE_SCROLL_UP_KEY") {
            self.keybindings.scroll_up = v;
        }
        if let Ok(v) = std::env::var("JCODE_SCROLL_DOWN_KEY") {
            self.keybindings.scroll_down = v;
        }
        if let Ok(v) = std::env::var("JCODE_SCROLL_PAGE_UP_KEY") {
            self.keybindings.scroll_page_up = v;
        }
        if let Ok(v) = std::env::var("JCODE_SCROLL_PAGE_DOWN_KEY") {
            self.keybindings.scroll_page_down = v;
        }
        if let Ok(v) = std::env::var("JCODE_MODEL_SWITCH_KEY") {
            self.keybindings.model_switch_next = v;
        }
        if let Ok(v) = std::env::var("JCODE_MODEL_SWITCH_PREV_KEY") {
            self.keybindings.model_switch_prev = v;
        }
        if let Ok(v) = std::env::var("JCODE_EFFORT_INCREASE_KEY") {
            self.keybindings.effort_increase = v;
        }
        if let Ok(v) = std::env::var("JCODE_EFFORT_DECREASE_KEY") {
            self.keybindings.effort_decrease = v;
        }
        if let Ok(v) = std::env::var("JCODE_CENTERED_TOGGLE_KEY") {
            self.keybindings.centered_toggle = v;
        }
        if let Ok(v) = std::env::var("JCODE_SCROLL_PROMPT_UP_KEY") {
            self.keybindings.scroll_prompt_up = v;
        }
        if let Ok(v) = std::env::var("JCODE_SCROLL_PROMPT_DOWN_KEY") {
            self.keybindings.scroll_prompt_down = v;
        }
        if let Ok(v) = std::env::var("JCODE_SCROLL_BOOKMARK_KEY") {
            self.keybindings.scroll_bookmark = v;
        }
        if let Ok(v) = std::env::var("JCODE_SCROLL_UP_FALLBACK_KEY") {
            self.keybindings.scroll_up_fallback = v;
        }
        if let Ok(v) = std::env::var("JCODE_SCROLL_DOWN_FALLBACK_KEY") {
            self.keybindings.scroll_down_fallback = v;
        }
        if let Ok(v) = std::env::var("JCODE_WORKSPACE_LEFT_KEY") {
            self.keybindings.workspace_left = v;
        }
        if let Ok(v) = std::env::var("JCODE_WORKSPACE_DOWN_KEY") {
            self.keybindings.workspace_down = v;
        }
        if let Ok(v) = std::env::var("JCODE_WORKSPACE_UP_KEY") {
            self.keybindings.workspace_up = v;
        }
        if let Ok(v) = std::env::var("JCODE_WORKSPACE_RIGHT_KEY") {
            self.keybindings.workspace_right = v;
        }

        // Dictation
        if let Ok(v) = std::env::var("JCODE_DICTATION_COMMAND") {
            self.dictation.command = v;
        }
        if let Ok(v) = std::env::var("JCODE_DICTATION_MODE")
            && let Ok(mode) = toml::from_str::<crate::protocol::TranscriptMode>(&format!(
                "\"{}\"",
                v.trim().to_ascii_lowercase()
            ))
        {
            self.dictation.mode = mode;
        }
        if let Ok(v) = std::env::var("JCODE_DICTATION_KEY") {
            self.dictation.key = v;
        }
        if let Ok(v) = std::env::var("JCODE_DICTATION_TIMEOUT_SECS")
            && let Ok(parsed) = v.trim().parse::<u64>()
        {
            self.dictation.timeout_secs = parsed;
        }

        // Display
        if let Ok(v) = std::env::var("JCODE_DIFF_MODE") {
            match v.to_lowercase().as_str() {
                "off" | "none" | "0" | "false" => self.display.diff_mode = DiffDisplayMode::Off,
                "inline" | "on" | "1" | "true" => self.display.diff_mode = DiffDisplayMode::Inline,
                "full-inline" | "full_inline" | "fullinline" | "inline-full" | "inline_full"
                | "inlinefull" | "full" => {
                    self.display.diff_mode = DiffDisplayMode::FullInline;
                }
                "pinned" | "pin" => self.display.diff_mode = DiffDisplayMode::Pinned,
                "file" => self.display.diff_mode = DiffDisplayMode::File,
                _ => {}
            }
        } else if let Ok(v) = std::env::var("JCODE_SHOW_DIFFS")
            && let Some(parsed) = parse_env_bool(&v)
        {
            self.display.diff_mode = if parsed {
                DiffDisplayMode::Inline
            } else {
                DiffDisplayMode::Off
            };
        }
        if let Ok(v) = std::env::var("JCODE_PIN_IMAGES")
            && let Some(parsed) = parse_env_bool(&v)
        {
            self.display.pin_images = parsed;
        }
        if let Ok(v) = std::env::var("JCODE_DISPLAY_CENTERED") {
            if let Some(parsed) = parse_env_bool(&v) {
                self.display.centered = parsed;
            }
        }
        if let Ok(v) = std::env::var("JCODE_DIFF_LINE_WRAP") {
            if let Some(parsed) = parse_env_bool(&v) {
                self.display.diff_line_wrap = parsed;
            }
        }
        if let Ok(v) = std::env::var("JCODE_QUEUE_MODE") {
            if let Some(parsed) = parse_env_bool(&v) {
                self.display.queue_mode = parsed;
            }
        }
        if let Ok(v) = std::env::var("JCODE_AUTO_SERVER_RELOAD") {
            if let Some(parsed) = parse_env_bool(&v) {
                self.display.auto_server_reload = parsed;
            }
        }
        if let Ok(v) = std::env::var("JCODE_MOUSE_CAPTURE") {
            if let Some(parsed) = parse_env_bool(&v) {
                self.display.mouse_capture = parsed;
            }
        }
        if let Ok(v) = std::env::var("JCODE_DEBUG_SOCKET") {
            if let Some(parsed) = parse_env_bool(&v) {
                self.display.debug_socket = parsed;
            }
        }
        if let Ok(v) = std::env::var("JCODE_SHOW_THINKING") {
            if let Some(parsed) = parse_env_bool(&v) {
                self.display.show_thinking = parsed;
            }
        }
        if let Ok(v) = std::env::var("JCODE_MARKDOWN_SPACING") {
            match v.trim().to_lowercase().as_str() {
                "compact" => self.display.markdown_spacing = MarkdownSpacingMode::Compact,
                "document" | "doc" => {
                    self.display.markdown_spacing = MarkdownSpacingMode::Document;
                }
                _ => {}
            }
        }
        if let Ok(v) = std::env::var("JCODE_STARTUP_ANIMATION") {
            if let Some(parsed) = parse_env_bool(&v) {
                self.display.startup_animation = parsed;
            }
        }
        if let Ok(v) = std::env::var("JCODE_IDLE_ANIMATION") {
            if let Some(parsed) = parse_env_bool(&v) {
                self.display.idle_animation = parsed;
            }
        }
        if let Ok(v) = std::env::var("JCODE_PROMPT_ENTRY_ANIMATION") {
            if let Some(parsed) = parse_env_bool(&v) {
                self.display.prompt_entry_animation = parsed;
            }
        }
        if let Ok(v) = std::env::var("JCODE_DISABLED_ANIMATIONS") {
            self.display.disabled_animations = parse_env_list(&v);
        }
        if let Ok(v) = std::env::var("JCODE_PERFORMANCE") {
            let trimmed = v.trim().to_lowercase();
            if matches!(trimmed.as_str(), "auto" | "full" | "reduced" | "minimal") {
                self.display.performance = trimmed;
            }
        }
        if let Ok(v) = std::env::var("JCODE_ANIMATION_FPS") {
            if let Ok(fps) = v.trim().parse::<u32>() {
                self.display.animation_fps = fps.clamp(1, 120);
            }
        }
        if let Ok(v) = std::env::var("JCODE_REDRAW_FPS") {
            if let Ok(fps) = v.trim().parse::<u32>() {
                self.display.redraw_fps = fps.clamp(1, 120);
            }
        }
        if let Ok(v) = std::env::var("JCODE_CHAT_NATIVE_SCROLLBAR") {
            if let Some(parsed) = parse_env_bool(&v) {
                self.display.native_scrollbars.chat = parsed;
            }
        }
        if let Ok(v) = std::env::var("JCODE_SIDE_PANEL_NATIVE_SCROLLBAR") {
            if let Some(parsed) = parse_env_bool(&v) {
                self.display.native_scrollbars.side_panel = parsed;
            }
        }

        // Features
        if let Ok(v) = std::env::var("JCODE_MEMORY_ENABLED") {
            if let Some(parsed) = parse_env_bool(&v) {
                self.features.memory = parsed;
            }
        }
        if let Ok(v) = std::env::var("JCODE_SWARM_ENABLED") {
            if let Some(parsed) = parse_env_bool(&v) {
                self.features.swarm = parsed;
            }
        }
        if let Ok(v) = std::env::var("JCODE_MESSAGE_TIMESTAMPS") {
            if let Some(parsed) = parse_env_bool(&v) {
                self.features.message_timestamps = parsed;
            }
        }
        if let Ok(v) = std::env::var("JCODE_UPDATE_CHANNEL") {
            match v.trim().to_lowercase().as_str() {
                "main" | "nightly" | "edge" => {
                    self.features.update_channel = UpdateChannel::Main;
                }
                "stable" | "release" => {
                    self.features.update_channel = UpdateChannel::Stable;
                }
                _ => {}
            }
        }

        if let Ok(v) = std::env::var("JCODE_TRUSTED_EXTERNAL_AUTH_SOURCES") {
            let mut source_ids = Vec::new();
            let mut source_paths = Vec::new();
            for value in parse_env_list(&v) {
                let trimmed = value.trim();
                if trimmed.is_empty() {
                    continue;
                }
                if trimmed.contains('|') {
                    source_paths.push(trimmed.to_ascii_lowercase());
                } else {
                    source_ids.push(trimmed.to_ascii_lowercase());
                }
            }
            self.auth.trusted_external_sources = source_ids;
            self.auth.trusted_external_source_paths = source_paths;
        }

        // Autoreview
        if let Ok(v) = std::env::var("JCODE_AUTOREVIEW_ENABLED") {
            if let Some(parsed) = parse_env_bool(&v) {
                self.autoreview.enabled = parsed;
            }
        }
        if let Ok(v) = std::env::var("JCODE_AUTOREVIEW_MODEL") {
            let trimmed = v.trim();
            self.autoreview.model = if trimmed.is_empty() {
                None
            } else {
                Some(trimmed.to_string())
            };
        }

        // Autojudge
        if let Ok(v) = std::env::var("JCODE_AUTOJUDGE_ENABLED") {
            if let Some(parsed) = parse_env_bool(&v) {
                self.autojudge.enabled = parsed;
            }
        }
        if let Ok(v) = std::env::var("JCODE_AUTOJUDGE_MODEL") {
            let trimmed = v.trim();
            self.autojudge.model = if trimmed.is_empty() {
                None
            } else {
                Some(trimmed.to_string())
            };
        }

        // Ambient
        if let Ok(v) = std::env::var("JCODE_AMBIENT_ENABLED") {
            if let Some(parsed) = parse_env_bool(&v) {
                self.ambient.enabled = parsed;
            }
        }
        if let Ok(v) = std::env::var("JCODE_AMBIENT_PROVIDER") {
            self.ambient.provider = Some(v);
        }
        if let Ok(v) = std::env::var("JCODE_AMBIENT_MODEL") {
            self.ambient.model = Some(v);
        }
        if let Ok(v) = std::env::var("JCODE_AMBIENT_MIN_INTERVAL") {
            if let Ok(parsed) = v.trim().parse::<u32>() {
                self.ambient.min_interval_minutes = parsed;
            }
        }
        if let Ok(v) = std::env::var("JCODE_AMBIENT_MAX_INTERVAL") {
            if let Ok(parsed) = v.trim().parse::<u32>() {
                self.ambient.max_interval_minutes = parsed;
            }
        }
        if let Ok(v) = std::env::var("JCODE_AMBIENT_PROACTIVE") {
            if let Some(parsed) = parse_env_bool(&v) {
                self.ambient.proactive_work = parsed;
            }
        }

        // Safety / notifications
        if let Ok(v) = std::env::var("JCODE_NTFY_TOPIC") {
            self.safety.ntfy_topic = Some(v);
        }
        if let Ok(v) = std::env::var("JCODE_NTFY_SERVER") {
            self.safety.ntfy_server = v;
        }
        if let Ok(v) = std::env::var("JCODE_SMTP_PASSWORD") {
            self.safety.email_password = Some(v);
        }
        if let Ok(v) = std::env::var("JCODE_EMAIL_TO") {
            self.safety.email_to = Some(v);
            self.safety.email_enabled = true;
        }
        if let Ok(v) = std::env::var("JCODE_IMAP_HOST") {
            self.safety.email_imap_host = Some(v);
        }
        if let Ok(v) = std::env::var("JCODE_EMAIL_REPLY_ENABLED") {
            if let Some(parsed) = parse_env_bool(&v) {
                self.safety.email_reply_enabled = parsed;
            }
        }
        if let Ok(v) = std::env::var("JCODE_TELEGRAM_BOT_TOKEN") {
            self.safety.telegram_bot_token = Some(v);
            self.safety.telegram_enabled = true;
        }
        if let Ok(v) = std::env::var("JCODE_TELEGRAM_CHAT_ID") {
            self.safety.telegram_chat_id = Some(v);
        }
        if let Ok(v) = std::env::var("JCODE_TELEGRAM_REPLY_ENABLED") {
            if let Some(parsed) = parse_env_bool(&v) {
                self.safety.telegram_reply_enabled = parsed;
            }
        }
        if let Ok(v) = std::env::var("JCODE_DISCORD_BOT_TOKEN") {
            self.safety.discord_bot_token = Some(v);
            self.safety.discord_enabled = true;
        }
        if let Ok(v) = std::env::var("JCODE_DISCORD_CHANNEL_ID") {
            self.safety.discord_channel_id = Some(v);
        }
        if let Ok(v) = std::env::var("JCODE_DISCORD_BOT_USER_ID") {
            self.safety.discord_bot_user_id = Some(v);
        }
        if let Ok(v) = std::env::var("JCODE_DISCORD_REPLY_ENABLED") {
            if let Some(parsed) = parse_env_bool(&v) {
                self.safety.discord_reply_enabled = parsed;
            }
        }
        if let Ok(v) = std::env::var("JCODE_AMBIENT_VISIBLE") {
            if let Some(parsed) = parse_env_bool(&v) {
                self.ambient.visible = parsed;
            }
        }

        // Gateway (iOS/web)
        if let Ok(v) = std::env::var("JCODE_GATEWAY_ENABLED") {
            if let Some(parsed) = parse_env_bool(&v) {
                self.gateway.enabled = parsed;
            }
        }
        if let Ok(v) = std::env::var("JCODE_GATEWAY_PORT") {
            if let Ok(parsed) = v.trim().parse::<u16>() {
                self.gateway.port = parsed;
            }
        }
        if let Ok(v) = std::env::var("JCODE_GATEWAY_BIND_ADDR") {
            let trimmed = v.trim();
            if !trimmed.is_empty() {
                self.gateway.bind_addr = trimmed.to_string();
            }
        }

        // Provider
        if let Ok(v) = std::env::var("JCODE_MODEL") {
            self.provider.default_model = Some(v);
        }
        if let Ok(v) = std::env::var("JCODE_PROVIDER") {
            let trimmed = v.trim().to_lowercase();
            if !trimmed.is_empty() {
                self.provider.default_provider = Some(trimmed);
            }
        }
        if let Ok(v) = std::env::var("JCODE_OPENAI_REASONING_EFFORT") {
            let trimmed = v.trim().to_string();
            if !trimmed.is_empty() {
                self.provider.openai_reasoning_effort = Some(trimmed);
            }
        }
        if let Ok(v) = std::env::var("JCODE_OPENAI_TRANSPORT") {
            let trimmed = v.trim().to_string();
            if !trimmed.is_empty() {
                self.provider.openai_transport = Some(trimmed);
            }
        }
        if let Ok(v) = std::env::var("JCODE_OPENAI_SERVICE_TIER") {
            let trimmed = v.trim().to_string();
            if !trimmed.is_empty() {
                self.provider.openai_service_tier = Some(trimmed);
            }
        }
        if let Ok(v) = std::env::var("JCODE_OPENAI_NATIVE_COMPACTION_MODE") {
            let trimmed = v.trim().to_ascii_lowercase();
            if !trimmed.is_empty() {
                self.provider.openai_native_compaction_mode = trimmed;
            }
        }
        if let Ok(v) = std::env::var("JCODE_OPENAI_NATIVE_COMPACTION_THRESHOLD_TOKENS") {
            if let Ok(parsed) = v.trim().parse::<usize>() {
                if parsed > 0 {
                    self.provider.openai_native_compaction_threshold_tokens = parsed;
                }
            }
        }
        if let Ok(v) = std::env::var("JCODE_CROSS_PROVIDER_FAILOVER") {
            if let Some(mode) = CrossProviderFailoverMode::parse(&v) {
                self.provider.cross_provider_failover = mode;
            }
        }
        if let Ok(v) = std::env::var("JCODE_SAME_PROVIDER_ACCOUNT_FAILOVER") {
            if let Some(enabled) = parse_env_bool(&v) {
                self.provider.same_provider_account_failover = enabled;
            }
        }

        // Copilot premium mode: env var overrides config
        // If set in config but not in env, propagate config -> env
        if let Ok(v) = std::env::var("JCODE_COPILOT_PREMIUM") {
            self.provider.copilot_premium = Some(v);
        } else if let Some(ref mode) = self.provider.copilot_premium {
            let env_val = match mode.as_str() {
                "zero" | "0" => "0",
                "one" | "1" => "1",
                _ => "",
            };
            if !env_val.is_empty() {
                crate::env::set_var("JCODE_COPILOT_PREMIUM", env_val);
            }
        }
    }

    /// Save config to file
    pub fn save(&self) -> anyhow::Result<()> {
        let path = Self::path().ok_or_else(|| anyhow::anyhow!("No config path"))?;

        // Ensure parent directory exists
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let content = toml::to_string_pretty(self)?;
        std::fs::write(&path, content)?;
        Ok(())
    }

    /// Update the copilot premium mode in the config file.
    /// Reloads, patches, and saves so it doesn't clobber other fields.
    pub fn set_copilot_premium(mode: Option<&str>) -> anyhow::Result<()> {
        let mut cfg = Self::load();
        cfg.provider.copilot_premium = mode.map(|s| s.to_string());
        cfg.save()?;
        crate::logging::info(&format!(
            "Saved copilot_premium to config: {}",
            mode.unwrap_or("(none)")
        ));
        Ok(())
    }

    /// Update just the default model and provider in the config file.
    /// This reloads, patches, and saves so it doesn't clobber other fields.
    pub fn set_default_model(model: Option<&str>, provider: Option<&str>) -> anyhow::Result<()> {
        let mut cfg = Self::load();
        cfg.provider.default_model = model.map(|s| s.to_string());
        cfg.provider.default_provider = provider.map(|s| s.to_string());
        cfg.save()?;

        // Update the global singleton so current session reflects the change
        let global = CONFIG.get_or_init(|| cfg.clone());
        // CONFIG is a OnceLock so we can't mutate it directly, but the file is saved
        // and will take effect on next restart. For this session we log it.
        let _ = global; // suppress unused
        crate::logging::info(&format!(
            "Saved default model: {}, provider: {}",
            model.unwrap_or("(none)"),
            provider.unwrap_or("(auto)")
        ));
        Ok(())
    }

    /// Update just the default provider in the config file.
    pub fn set_default_provider(provider: Option<&str>) -> anyhow::Result<()> {
        let cfg = Self::load();
        Self::set_default_model(cfg.provider.default_model.as_deref(), provider)
    }

    /// Update just the default model in the config file.
    pub fn set_default_model_only(model: Option<&str>) -> anyhow::Result<()> {
        let cfg = Self::load();
        Self::set_default_model(model, cfg.provider.default_provider.as_deref())
    }

    /// Update the persisted OpenAI reasoning effort preference.
    pub fn set_openai_reasoning_effort(value: Option<&str>) -> anyhow::Result<()> {
        let mut cfg = Self::load();
        cfg.provider.openai_reasoning_effort = value.map(|s| s.to_string());
        cfg.save()?;
        crate::logging::info(&format!(
            "Saved openai_reasoning_effort to config: {}",
            value.unwrap_or("(none)")
        ));
        Ok(())
    }

    /// Update the persisted OpenAI transport preference.
    pub fn set_openai_transport(value: Option<&str>) -> anyhow::Result<()> {
        let mut cfg = Self::load();
        cfg.provider.openai_transport = value.map(|s| s.to_string());
        cfg.save()?;
        crate::logging::info(&format!(
            "Saved openai_transport to config: {}",
            value.unwrap_or("(none)")
        ));
        Ok(())
    }

    /// Update the persisted OpenAI service tier preference.
    pub fn set_openai_service_tier(value: Option<&str>) -> anyhow::Result<()> {
        let mut cfg = Self::load();
        cfg.provider.openai_service_tier = value.map(|s| s.to_string());
        cfg.save()?;
        crate::logging::info(&format!(
            "Saved openai_service_tier to config: {}",
            value.unwrap_or("(none)")
        ));
        Ok(())
    }

    /// Update the persisted default alignment preference.
    pub fn set_display_centered(centered: bool) -> anyhow::Result<()> {
        let mut cfg = Self::load();
        cfg.display.centered = centered;
        cfg.save()?;
        crate::logging::info(&format!("Saved display.centered to config: {}", centered));
        Ok(())
    }

    fn normalize_external_auth_source_id(source_id: &str) -> String {
        source_id.trim().to_ascii_lowercase()
    }

    fn trusted_external_auth_path_entry(
        source_id: &str,
        path: &std::path::Path,
    ) -> anyhow::Result<String> {
        let source_id = Self::normalize_external_auth_source_id(source_id);
        if source_id.is_empty() {
            anyhow::bail!("External auth source id cannot be empty");
        }
        let canonical = crate::storage::validate_external_auth_file(path)?;
        Ok(format!(
            "{}|{}",
            source_id,
            canonical.to_string_lossy().to_ascii_lowercase()
        ))
    }

    pub fn external_auth_source_allowed(source_id: &str) -> bool {
        let source_id = Self::normalize_external_auth_source_id(source_id);
        if source_id.is_empty() {
            return false;
        }

        let cfg = Self::load();
        cfg.auth
            .trusted_external_sources
            .iter()
            .any(|value| value.trim().eq_ignore_ascii_case(&source_id))
    }

    pub fn external_auth_source_allowed_for_path(source_id: &str, path: &std::path::Path) -> bool {
        let Ok(entry) = Self::trusted_external_auth_path_entry(source_id, path) else {
            return false;
        };

        let cfg = Self::load();
        cfg.auth
            .trusted_external_source_paths
            .iter()
            .any(|value| value.trim().eq_ignore_ascii_case(&entry))
    }

    /// Startup-sensitive variant that uses the process-cached config snapshot.
    ///
    /// This avoids reloading config.toml repeatedly during cold-start probes.
    pub fn external_auth_source_allowed_for_path_cached(
        source_id: &str,
        path: &std::path::Path,
    ) -> bool {
        let Ok(entry) = Self::trusted_external_auth_path_entry(source_id, path) else {
            return false;
        };

        config()
            .auth
            .trusted_external_source_paths
            .iter()
            .any(|value| value.trim().eq_ignore_ascii_case(&entry))
    }

    pub fn allow_external_auth_source(source_id: &str) -> anyhow::Result<()> {
        let source_id = Self::normalize_external_auth_source_id(source_id);
        if source_id.is_empty() {
            anyhow::bail!("External auth source id cannot be empty");
        }

        let mut cfg = Self::load();
        if !cfg
            .auth
            .trusted_external_sources
            .iter()
            .any(|value| value.trim().eq_ignore_ascii_case(&source_id))
        {
            cfg.auth.trusted_external_sources.push(source_id.clone());
            cfg.auth.trusted_external_sources.sort();
            cfg.auth.trusted_external_sources.dedup();
            cfg.save()?;
        }

        crate::logging::info(&format!(
            "Saved trusted external auth source to config: {}",
            source_id
        ));
        Ok(())
    }

    pub fn allow_external_auth_source_for_path(
        source_id: &str,
        path: &std::path::Path,
    ) -> anyhow::Result<()> {
        let entry = Self::trusted_external_auth_path_entry(source_id, path)?;
        let mut cfg = Self::load();
        if !cfg
            .auth
            .trusted_external_source_paths
            .iter()
            .any(|value| value.trim().eq_ignore_ascii_case(&entry))
        {
            cfg.auth.trusted_external_source_paths.push(entry.clone());
            cfg.auth.trusted_external_source_paths.sort();
            cfg.auth.trusted_external_source_paths.dedup();
            cfg.save()?;
        }
        crate::logging::info(&format!(
            "Saved trusted external auth source path: {}",
            entry
        ));
        Ok(())
    }

    pub fn revoke_external_auth_source_for_path(
        source_id: &str,
        path: &std::path::Path,
    ) -> anyhow::Result<()> {
        let entry = Self::trusted_external_auth_path_entry(source_id, path)?;
        let mut cfg = Self::load();
        let before = cfg.auth.trusted_external_source_paths.len();
        cfg.auth
            .trusted_external_source_paths
            .retain(|value| !value.trim().eq_ignore_ascii_case(&entry));
        if cfg.auth.trusted_external_source_paths.len() != before {
            cfg.save()?;
            crate::logging::info(&format!(
                "Removed trusted external auth source path: {}",
                entry
            ));
        }
        Ok(())
    }

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
effort_increase = "alt+right"
effort_decrease = "alt+left"

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
scroll_up_fallback = "cmd+k"
scroll_down_fallback = "cmd+j"

# Workspace navigation (Niri-style)
# Comma-separate multiple bindings to add aliases.
workspace_left = "alt+h"
workspace_down = "alt+j"
workspace_up = "alt+k"
workspace_right = "alt+l"

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

# Center all content by default (default: true)
centered = true

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

# Show thinking/reasoning content (default: false)
show_thinking = false

# Markdown spacing style: "compact" (chat/TUI) or "document" (docs-like)
# markdown_spacing = "compact"

# Show startup animation (default: false)
startup_animation = false

# Show idle animation before first prompt (default: true)
idle_animation = true

# Briefly animate a user prompt line when it enters the viewport (default: true)
prompt_entry_animation = true

# Disable specific animation variants by name.
# Examples: ["mobius"] or ["mobius", "knot", "three_rings"]
# Aliases: "mobius" also disables the idle knot; "three_rings" disables gyroscope.
# disabled_animations = []

# Performance tier: auto/full/reduced/minimal (default: auto)
# auto = detect system load, memory, terminal type, SSH, and apply extra caps for WSL/Windows Terminal
# full = all animations enabled
# reduced = skip startup/idle animations, keep spinners
# minimal = disable all animations, slower redraw rate
# performance = "auto"

# Animation FPS (startup animation, idle donut): 1-120 (default: 60)
# Runtime policy may cap this lower on slower environments such as WSL/Windows Terminal.
# Startup animation is skipped entirely if animation_fps < 20 (shows nothing instead of low-FPS jank)
# animation_fps = 60

# Active redraw FPS (processing, streaming, spinners): 1-120 (default: 60)
# Runtime policy may cap this lower on slower environments such as WSL/Windows Terminal.
# redraw_fps = 60

[features]
# Memory: retrieval + extraction sidecar features
memory = true
# Swarm: multi-session coordination features
swarm = true
# Inject timestamps into user messages and tool results sent to the model
message_timestamps = true
# Update channel: "stable" (releases only) or "main" (latest commits on push)
# Set to "main" for bleeding edge updates every time code is pushed
update_channel = "stable"

[provider]
# Default model (optional, uses provider default if not set)
# Set via /model picker with Ctrl+D to save as default
# default_model = "claude-opus-4-6"
# Default provider (optional: claude|openai|copilot|openrouter)
# When set, this provider is preferred on startup if available
# default_provider = "copilot"
# OpenAI reasoning effort (none|low|medium|high|xhigh)
openai_reasoning_effort = "high"
# OpenAI transport mode (auto|websocket|https)
# openai_transport = "auto"
# OpenAI service tier override (priority|flex)
# Set `priority` to match Codex /fast behavior (higher speed, higher usage)
# openai_service_tier = "priority"
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
"#;

        std::fs::write(&path, default_content)?;
        Ok(path)
    }

    /// Get config as a formatted string for display
    pub fn display_string(&self) -> String {
        let path = Self::path()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "unknown".to_string());

        format!(
            r#"**Configuration** (`{}`)

**Keybindings:**
- Scroll up: `{}`
- Scroll down: `{}`
- Scroll up fallback: `{}`
- Scroll down fallback: `{}`
- Page up: `{}`
- Page down: `{}`
- Model next: `{}`
- Model prev: `{}`
- Effort increase: `{}`
- Effort decrease: `{}`
- Centered toggle: `{}`
- Prompt up: `{}`
- Prompt down: `{}`
- Scroll bookmark: `{}`
- Workspace left: `{}`
- Workspace down: `{}`
- Workspace up: `{}`
- Workspace right: `{}`

**Dictation:**
- Command: `{}`
- Mode: `{}`
- Hotkey: `{}`
- Timeout: {}s

**Display:**
- Diff mode: {}
- Centered: {}
- Markdown spacing: {}
- Pin images: {}
- Diff line wrap: {}
- Queue mode: {}
- Auto server reload: {}
- Mouse capture: {}
- Debug socket: {}
- Startup animation: {}
- Idle animation: {}
- Prompt entry animation: {}
- Chat native scrollbar: {}
- Side panel native scrollbar: {}
- Disabled animations: {}
- Performance tier: {}
- Animation FPS: {}
- Redraw FPS: {}

**Features:**
- Memory: {}
- Swarm: {}
- Message timestamps: {}
- Update channel: {}

**Provider:**
- Default model: {}
- Default provider: {}
- OpenAI reasoning effort: {}
- OpenAI transport: {}
- OpenAI service tier: {}
- OpenAI native compaction: {}
- OpenAI native compaction threshold ratio: {:.2}
- Cross-provider failover: {}

**Agent models:**
- Swarm / subagent: {}
- Review: {}
- Judge: {}
- Memory: {}
- Ambient: {}

**Gateway:**
- Enabled: {}
- Bind address: {}:{}

**Ambient:**
- Enabled: {}
- Provider: {}
- Model: {}
- Interval: {}-{} minutes
- Pause on active session: {}
- Proactive work: {}
- Work branch prefix: `{}`
- Visible mode: {}

**Notifications:**
- ntfy.sh: {}
- Desktop: {}
- Email: {}
- Email replies: {}
- Telegram: {}
- Telegram replies: {}
- Discord: {}
- Discord replies: {}

*Edit the config file or set environment variables to customize.*
*Environment variables (e.g., `JCODE_SCROLL_UP_KEY`, `JCODE_GATEWAY_ENABLED`) override file settings.*"#,
            path,
            self.keybindings.scroll_up,
            self.keybindings.scroll_down,
            self.keybindings.scroll_up_fallback,
            self.keybindings.scroll_down_fallback,
            self.keybindings.scroll_page_up,
            self.keybindings.scroll_page_down,
            self.keybindings.model_switch_next,
            self.keybindings.model_switch_prev,
            self.keybindings.effort_increase,
            self.keybindings.effort_decrease,
            self.keybindings.centered_toggle,
            self.keybindings.scroll_prompt_up,
            self.keybindings.scroll_prompt_down,
            self.keybindings.scroll_bookmark,
            self.keybindings.workspace_left,
            self.keybindings.workspace_down,
            self.keybindings.workspace_up,
            self.keybindings.workspace_right,
            if self.dictation.command.trim().is_empty() {
                "(disabled)"
            } else {
                self.dictation.command.as_str()
            },
            match self.dictation.mode {
                crate::protocol::TranscriptMode::Insert => "insert",
                crate::protocol::TranscriptMode::Append => "append",
                crate::protocol::TranscriptMode::Replace => "replace",
                crate::protocol::TranscriptMode::Send => "send",
            },
            self.dictation.key,
            self.dictation.timeout_secs,
            self.display.diff_mode.label(),
            self.display.centered,
            self.display.markdown_spacing.label(),
            self.display.pin_images,
            self.display.diff_line_wrap,
            self.display.queue_mode,
            self.display.auto_server_reload,
            self.display.mouse_capture,
            self.display.debug_socket,
            self.display.startup_animation,
            self.display.idle_animation,
            self.display.prompt_entry_animation,
            self.display.native_scrollbars.chat,
            self.display.native_scrollbars.side_panel,
            if self.display.disabled_animations.is_empty() {
                "(none)".to_string()
            } else {
                self.display.disabled_animations.join(", ")
            },
            if self.display.performance.is_empty() {
                "auto"
            } else {
                &self.display.performance
            },
            self.display.animation_fps,
            self.display.redraw_fps,
            self.features.memory,
            self.features.swarm,
            self.features.message_timestamps,
            self.features.update_channel,
            self.provider
                .default_model
                .as_deref()
                .unwrap_or("(provider default)"),
            self.provider
                .default_provider
                .as_deref()
                .unwrap_or("(auto)"),
            self.provider
                .openai_reasoning_effort
                .as_deref()
                .unwrap_or("(provider default)"),
            self.provider
                .openai_transport
                .as_deref()
                .unwrap_or("(auto)"),
            self.provider
                .openai_service_tier
                .as_deref()
                .unwrap_or("(default)"),
            self.provider.openai_native_compaction_mode.as_str(),
            self.provider.openai_native_compaction_threshold_tokens,
            self.provider.cross_provider_failover.as_str(),
            self.agents
                .swarm_model
                .as_deref()
                .unwrap_or("(inherit current session)"),
            self.autoreview
                .model
                .as_deref()
                .unwrap_or("(inherit current session)"),
            self.autojudge
                .model
                .as_deref()
                .unwrap_or("(inherit current session)"),
            self.agents
                .memory_model
                .as_deref()
                .unwrap_or("(sidecar auto-select)"),
            self.ambient
                .model
                .as_deref()
                .unwrap_or("(provider default)"),
            self.gateway.enabled,
            self.gateway.bind_addr,
            self.gateway.port,
            self.ambient.enabled,
            self.ambient.provider.as_deref().unwrap_or("(auto)"),
            self.ambient
                .model
                .as_deref()
                .unwrap_or("(provider default)"),
            self.ambient.min_interval_minutes,
            self.ambient.max_interval_minutes,
            self.ambient.pause_on_active_session,
            self.ambient.proactive_work,
            self.ambient.work_branch_prefix,
            self.ambient.visible,
            self.safety
                .ntfy_topic
                .as_deref()
                .map(|t| format!("enabled (topic: {})", t))
                .unwrap_or_else(|| "disabled".to_string()),
            if self.safety.desktop_notifications {
                "enabled"
            } else {
                "disabled"
            },
            if self.safety.email_enabled {
                self.safety
                    .email_to
                    .as_deref()
                    .unwrap_or("enabled (no recipient)")
            } else {
                "disabled"
            },
            if self.safety.email_reply_enabled {
                self.safety
                    .email_imap_host
                    .as_deref()
                    .unwrap_or("enabled (no IMAP host)")
            } else {
                "disabled"
            },
            if self.safety.telegram_enabled {
                self.safety
                    .telegram_chat_id
                    .as_deref()
                    .unwrap_or("enabled (no chat_id)")
            } else {
                "disabled"
            },
            if self.safety.telegram_reply_enabled {
                "enabled"
            } else {
                "disabled"
            },
            if self.safety.discord_enabled {
                self.safety
                    .discord_channel_id
                    .as_deref()
                    .unwrap_or("enabled (no channel_id)")
            } else {
                "disabled"
            },
            if self.safety.discord_reply_enabled {
                "enabled"
            } else {
                "disabled"
            },
        )
    }
}

/// WebSocket gateway configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct GatewayConfig {
    /// Enable the WebSocket gateway (default: false)
    pub enabled: bool,
    /// TCP port to listen on (default: 7643)
    pub port: u16,
    /// Bind address (default: 0.0.0.0)
    pub bind_addr: String,
}

impl Default for GatewayConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            port: 7643,
            bind_addr: "0.0.0.0".to_string(),
        }
    }
}

fn parse_env_bool(raw: &str) -> Option<bool> {
    match raw.trim().to_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Some(true),
        "0" | "false" | "no" | "off" => Some(false),
        _ => None,
    }
}

fn parse_env_list(raw: &str) -> Vec<String> {
    raw.split([',', '\n'])
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .map(ToString::to_string)
        .collect()
}

#[cfg(test)]
#[path = "config_tests.rs"]
mod tests;
