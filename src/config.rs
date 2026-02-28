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

/// Main configuration struct
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Config {
    /// Keybinding configuration
    pub keybindings: KeybindingsConfig,

    /// Display/UI configuration
    pub display: DisplayConfig,

    /// Feature toggles
    pub features: FeatureConfig,

    /// Provider configuration
    pub provider: ProviderConfig,

    /// Ambient mode configuration
    pub ambient: AmbientConfig,

    /// Safety / notification configuration
    pub safety: SafetyConfig,

    /// WebSocket gateway configuration (for iOS/web clients)
    pub gateway: GatewayConfig,
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
    /// Show diffs in a dedicated pinned pane
    Pinned,
}

impl DiffDisplayMode {
    pub fn is_off(&self) -> bool {
        matches!(self, DiffDisplayMode::Off)
    }

    pub fn is_inline(&self) -> bool {
        matches!(self, DiffDisplayMode::Inline)
    }

    pub fn is_pinned(&self) -> bool {
        matches!(self, DiffDisplayMode::Pinned)
    }

    pub fn cycle(self) -> Self {
        match self {
            DiffDisplayMode::Off => DiffDisplayMode::Inline,
            DiffDisplayMode::Inline => DiffDisplayMode::Pinned,
            DiffDisplayMode::Pinned => DiffDisplayMode::Off,
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            DiffDisplayMode::Off => "OFF",
            DiffDisplayMode::Inline => "Inline",
            DiffDisplayMode::Pinned => "Pinned",
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

/// Display/UI configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct DisplayConfig {
    /// How to display file diffs (off/inline/pinned, default: inline)
    pub diff_mode: DiffDisplayMode,
    /// Legacy: "show_diffs = true/false" maps to diff_mode inline/off
    #[serde(default)]
    show_diffs: Option<bool>,
    /// Queue mode by default - wait until done before sending (default: false)
    pub queue_mode: bool,
    /// Capture mouse events (default: false). Enables scroll wheel but disables terminal selection.
    pub mouse_capture: bool,
    /// Enable debug socket for external control (default: false)
    pub debug_socket: bool,
    /// Center all content (default: false)
    pub centered: bool,
    /// Show thinking/reasoning content by default (default: false)
    pub show_thinking: bool,
    /// How to display mermaid diagrams (none/margin/pinned, default: pinned)
    pub diagram_mode: DiagramDisplayMode,
    /// Pin read images to side pane (default: true)
    pub pin_images: bool,
    /// Show startup animation (default: false)
    pub startup_animation: bool,
    /// Show idle animation before first prompt (default: true)
    pub idle_animation: bool,
    /// Briefly animate user prompt line when it enters viewport (default: true)
    pub prompt_entry_animation: bool,
    /// Wrap long lines in the pinned diff pane (default: true)
    pub diff_line_wrap: bool,
    /// Performance tier override: auto/full/reduced/minimal (default: auto)
    pub performance: String,
}

impl Default for DisplayConfig {
    fn default() -> Self {
        Self {
            diff_mode: DiffDisplayMode::default(),
            show_diffs: None,
            pin_images: true,
            queue_mode: false,
            mouse_capture: true,
            debug_socket: false,
            centered: true,
            show_thinking: false,
            diagram_mode: DiagramDisplayMode::default(),
            startup_animation: false,
            idle_animation: true,
            prompt_entry_animation: true,
            diff_line_wrap: true,
            performance: String::new(),
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
pub enum UpdateChannel {
    /// Only update from tagged GitHub Releases (default)
    Stable,
    /// Update from latest commit on main branch (bleeding edge)
    Main,
}

impl Default for UpdateChannel {
    fn default() -> Self {
        Self::Stable
    }
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
}

impl Default for ProviderConfig {
    fn default() -> Self {
        Self {
            default_model: None,
            default_provider: None,
            openai_reasoning_effort: Some("xhigh".to_string()),
            openai_transport: None,
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
    /// Show ambient cycle in a terminal window (default: false)
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
            visible: false,
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

        // Display
        if let Ok(v) = std::env::var("JCODE_DIFF_MODE") {
            match v.to_lowercase().as_str() {
                "off" | "none" | "0" | "false" => self.display.diff_mode = DiffDisplayMode::Off,
                "inline" | "on" | "1" | "true" => self.display.diff_mode = DiffDisplayMode::Inline,
                "pinned" | "pin" => self.display.diff_mode = DiffDisplayMode::Pinned,
                _ => {}
            }
        } else if let Ok(v) = std::env::var("JCODE_SHOW_DIFFS") {
            if let Some(parsed) = parse_env_bool(&v) {
                self.display.diff_mode = if parsed {
                    DiffDisplayMode::Inline
                } else {
                    DiffDisplayMode::Off
                };
            }
        }
        if let Ok(v) = std::env::var("JCODE_PIN_IMAGES") {
            if let Some(parsed) = parse_env_bool(&v) {
                self.display.pin_images = parsed;
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
        if let Ok(v) = std::env::var("JCODE_PERFORMANCE") {
            let trimmed = v.trim().to_lowercase();
            if matches!(trimmed.as_str(), "auto" | "full" | "reduced" | "minimal") {
                self.display.performance = trimmed;
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
# Also supports Ctrl+1..9 for recency rank jumps (1 = most recent).
scroll_prompt_up = "ctrl+["
scroll_prompt_down = "ctrl+]"

# Scroll bookmark toggle (stash position, jump to bottom, press again to return)
scroll_bookmark = "ctrl+g"

# Optional fallback scroll bindings (useful on macOS terminals that forward Command)
scroll_up_fallback = "cmd+k"
scroll_down_fallback = "cmd+j"

[display]
# Diff display mode: "off", "inline" (default), or "pinned" (dedicated pane)
diff_mode = "inline"

# Pin read images to a side pane (default: true)
pin_images = true

# Wrap long lines in the pinned diff pane (default: true)
# Set to false for horizontal scrolling instead of wrapping
diff_line_wrap = true

# Queue mode: wait until assistant is done before sending next message
queue_mode = false

# Capture mouse events (enables scroll wheel; disables terminal text selection)
mouse_capture = true

# Enable debug socket for external control/testing (default: false)
debug_socket = false

# Show thinking/reasoning content (default: false)
show_thinking = false

# Show startup animation (default: false)
startup_animation = false

# Show idle animation before first prompt (default: true)
idle_animation = true

# Briefly animate a user prompt line when it enters the viewport (default: true)
prompt_entry_animation = true

# Performance tier: auto/full/reduced/minimal (default: auto)
# auto = detect system load, memory, terminal type, SSH
# full = all animations enabled
# reduced = skip startup/idle animations, keep spinners
# minimal = disable all animations, slower redraw rate
# performance = "auto"

[features]
# Memory: retrieval + extraction sidecar features
memory = true
# Swarm: multi-session coordination features
swarm = true
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
openai_reasoning_effort = "xhigh"
# OpenAI transport mode (auto|websocket|https)
# openai_transport = "auto"

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
# Show ambient cycle in a terminal window (default: false)
# visible = false

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

**Display:**
- Diff mode: {}
- Pin images: {}
- Diff line wrap: {}
- Queue mode: {}
- Mouse capture: {}
- Debug socket: {}
- Startup animation: {}
- Idle animation: {}
- Prompt entry animation: {}
- Performance tier: {}

**Features:**
- Memory: {}
- Swarm: {}
- Update channel: {}

**Provider:**
- Default model: {}
- Default provider: {}
- OpenAI reasoning effort: {}
- OpenAI transport: {}

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
            self.display.diff_mode.label(),
            self.display.pin_images,
            self.display.diff_line_wrap,
            self.display.queue_mode,
            self.display.mouse_capture,
            self.display.debug_socket,
            self.display.startup_animation,
            self.display.idle_animation,
            self.display.prompt_entry_animation,
            if self.display.performance.is_empty() {
                "auto"
            } else {
                &self.display.performance
            },
            self.features.memory,
            self.features.swarm,
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
