//! Configuration file support for jcode
//!
//! Config is loaded from `~/.jcode/config.toml` (or `$JCODE_HOME/config.toml`)
//! Environment variables override config file settings.

pub use jcode_config_types::{
    AgentsConfig, AmbientConfig, AuthConfig, AutoJudgeConfig, AutoReviewConfig, CompactionConfig,
    CompactionMode, CrossProviderFailoverMode, DiagramDisplayMode, DiagramPanePosition,
    DiffDisplayMode, DisplayConfig, FeatureConfig, GatewayConfig, KeybindingsConfig,
    MarkdownSpacingMode, NamedProviderAuth, NamedProviderConfig, NamedProviderModelConfig,
    NamedProviderType, NativeScrollbarConfig, PowerConfig, ProviderConfig, ReasoningDisplayMode,
    SafetyConfig, SessionPickerResumeAction, SwarmSpawnMode, UpdateChannel, WebSearchConfig,
    WebSearchEngine,
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::sync::{LazyLock, RwLock};
use std::time::{Duration, Instant, SystemTime};

const CONFIG_CACHE_CHECK_INTERVAL: Duration = if cfg!(test) {
    Duration::ZERO
} else {
    Duration::from_millis(500)
};

const CONFIG_ENV_KEYS: &[&str] = &[
    "HOME",
    "JCODE_ACP_PROFILE",
    "JCODE_ACP_TOOL_PROFILE",
    "JCODE_AMBIENT_ENABLED",
    "JCODE_AMBIENT_MAX_INTERVAL",
    "JCODE_AMBIENT_MIN_INTERVAL",
    "JCODE_AMBIENT_MODEL",
    "JCODE_AMBIENT_PROACTIVE",
    "JCODE_AMBIENT_PROVIDER",
    "JCODE_AMBIENT_VISIBLE",
    "JCODE_ANIMATION_FPS",
    "JCODE_AUTOJUDGE_ENABLED",
    "JCODE_AUTOJUDGE_MODEL",
    "JCODE_AUTOREVIEW_ENABLED",
    "JCODE_AUTOREVIEW_MODEL",
    "JCODE_AUTO_SERVER_RELOAD",
    "JCODE_BING_API_KEY",
    "JCODE_BING_API_KEY_ENV",
    "JCODE_BING_MARKET",
    "JCODE_CENTERED_TOGGLE_KEY",
    "JCODE_CHAT_NATIVE_SCROLLBAR",
    "JCODE_COPY_BADGE_ALT_LABEL",
    "JCODE_COPY_SELECTION_TOGGLE_KEY",
    "JCODE_COPILOT_PREMIUM",
    "JCODE_CROSS_PROVIDER_FAILOVER",
    "JCODE_DEBUG_SOCKET",
    "JCODE_DICTATION_COMMAND",
    "JCODE_DICTATION_KEY",
    "JCODE_DICTATION_MODE",
    "JCODE_DICTATION_TIMEOUT_SECS",
    "JCODE_DIFF_LINE_WRAP",
    "JCODE_DIFF_MODE",
    "JCODE_DIFF_MODE_CYCLE_KEY",
    "JCODE_DIAGRAM_PANE_TOGGLE_KEY",
    "JCODE_DISABLE_BASE_TOOLS",
    "JCODE_DISABLED_ANIMATIONS",
    "JCODE_DISABLED_TOOLS",
    "JCODE_DISCORD_BOT_TOKEN",
    "JCODE_DISCORD_BOT_USER_ID",
    "JCODE_DISCORD_CHANNEL_ID",
    "JCODE_DISCORD_REPLY_ENABLED",
    "JCODE_DISPLAY_CENTERED",
    "JCODE_EFFORT_DECREASE_KEY",
    "JCODE_EFFORT_INCREASE_KEY",
    "JCODE_EMAIL_REPLY_ENABLED",
    "JCODE_EMAIL_TO",
    "JCODE_GATEWAY_BIND_ADDR",
    "JCODE_GATEWAY_ENABLED",
    "JCODE_GATEWAY_PORT",
    "JCODE_HOME",
    "JCODE_IDLE_ANIMATION",
    "JCODE_IMAP_HOST",
    "JCODE_INFO_WIDGET_TOGGLE_KEY",
    "JCODE_JADE_RELAY_API_BASE",
    "JCODE_JADE_RELAY_ENABLED",
    "JCODE_JADE_RELAY_LAUNCH_ENABLED",
    "JCODE_JADE_RELAY_LAUNCH_WORKING_DIR",
    "JCODE_JADE_RELAY_REPLY_ENABLED",
    "JCODE_JADE_RELAY_SESSION_ID",
    "JCODE_JADE_RELAY_TOKEN",
    "JCODE_JADE_RELAY_TOKEN_ID",
    "JCODE_JADE_RELAY_USER_ID",
    "JCODE_MARKDOWN_SPACING",
    "JCODE_MEMORY_ENABLED",
    "JCODE_MEMORY_MODEL",
    "JCODE_MEMORY_SIDECAR_ENABLED",
    "JCODE_PERSIST_MEMORY_INJECTIONS",
    "JCODE_MESSAGE_TIMESTAMPS",
    "JCODE_MODEL",
    "JCODE_MODEL_SWITCH_KEY",
    "JCODE_MODEL_SWITCH_PREV_KEY",
    "JCODE_MOUSE_CAPTURE",
    "JCODE_NTFY_SERVER",
    "JCODE_NTFY_TOPIC",
    "JCODE_OPENAI_NATIVE_COMPACTION_MODE",
    "JCODE_OPENAI_NATIVE_COMPACTION_THRESHOLD_TOKENS",
    "JCODE_OPENAI_REASONING_EFFORT",
    "JCODE_OPENAI_SERVICE_TIER",
    "JCODE_OPENAI_TRANSPORT",
    "JCODE_ANTHROPIC_REASONING_EFFORT",
    "JCODE_PRESERVE_REASONING_CONTEXT",
    "JCODE_PERFORMANCE",
    "JCODE_PIN_IMAGES",
    "JCODE_PREVENT_SLEEP_WHILE_STREAMING",
    "JCODE_PROVIDER",
    "JCODE_PROMPT_ENTRY_ANIMATION",
    "JCODE_QUEUE_MODE",
    "JCODE_REASONING_DISPLAY",
    "JCODE_REDRAW_FPS",
    "JCODE_SAME_PROVIDER_ACCOUNT_FAILOVER",
    "JCODE_SCROLL_BOOKMARK_KEY",
    "JCODE_SCROLL_DOWN_FALLBACK_KEY",
    "JCODE_SCROLL_DOWN_KEY",
    "JCODE_SCROLL_PAGE_DOWN_KEY",
    "JCODE_SCROLL_PAGE_UP_KEY",
    "JCODE_SCROLL_PROMPT_DOWN_KEY",
    "JCODE_SCROLL_PROMPT_UP_KEY",
    "JCODE_SCROLL_UP_FALLBACK_KEY",
    "JCODE_SCROLL_UP_KEY",
    "JCODE_SEARXNG_URL",
    "JCODE_SHOW_DIFFS",
    "JCODE_SHOW_THINKING",
    "JCODE_SIDE_PANEL_TOGGLE_KEY",
    "JCODE_SIDE_PANEL_NATIVE_SCROLLBAR",
    "JCODE_SMTP_PASSWORD",
    "JCODE_STREAM_IDLE_TIMEOUT_SECS",
    "JCODE_SWARM_ENABLED",
    "JCODE_SWARM_MODEL",
    "JCODE_SWARM_SPAWN_MODE",
    "JCODE_TELEGRAM_BOT_TOKEN",
    "JCODE_TELEGRAM_CHAT_ID",
    "JCODE_TELEGRAM_REPLY_ENABLED",
    "JCODE_TOOL_PROFILE",
    "JCODE_TOOLS",
    "JCODE_TRUSTED_EXTERNAL_AUTH_SOURCES",
    "JCODE_TYPING_SCROLL_LOCK_TOGGLE_KEY",
    "JCODE_UPDATE_CHANNEL",
    "JCODE_WEBSEARCH_ENGINE",
    "JCODE_WEBSEARCH_FALLBACK_ENGINES",
    "JCODE_WORKSPACE_DOWN_KEY",
    "JCODE_WORKSPACE_LEFT_KEY",
    "JCODE_WORKSPACE_RIGHT_KEY",
    "JCODE_WORKSPACE_UP_KEY",
    "XDG_CONFIG_HOME",
];

#[derive(Debug, Clone, PartialEq, Eq)]
struct ConfigCacheFingerprint {
    path: Option<PathBuf>,
    modified: Option<SystemTime>,
    len: Option<u64>,
    env: Vec<(String, String)>,
}

impl ConfigCacheFingerprint {
    fn current() -> Self {
        let path = Config::path();
        let metadata = path.as_ref().and_then(|path| std::fs::metadata(path).ok());
        Self {
            path,
            modified: metadata
                .as_ref()
                .and_then(|metadata| metadata.modified().ok()),
            len: metadata.as_ref().map(std::fs::Metadata::len),
            env: config_env_fingerprint(),
        }
    }
}

struct ConfigCache {
    config: &'static Config,
    fingerprint: ConfigCacheFingerprint,
    last_checked: Instant,
    force_reload: bool,
}

static CONFIG_CACHE: LazyLock<RwLock<ConfigCache>> = LazyLock::new(|| {
    let fingerprint = ConfigCacheFingerprint::current();
    RwLock::new(ConfigCache {
        config: leak_config(Config::load()),
        fingerprint,
        last_checked: Instant::now(),
        force_reload: false,
    })
});

fn leak_config(config: Config) -> &'static Config {
    Box::leak(Box::new(config))
}

/// Get the global config instance.
///
/// The returned reference is backed by a reloadable process cache. Calls check
/// the config file path/metadata and relevant environment overrides on a short
/// throttle, not every frame. When those inputs change, the next checked call
/// reloads config.toml and invalidates dependent auth/model caches. Older
/// references remain valid for the duration of any in-flight operation.
pub fn config() -> &'static Config {
    let now = Instant::now();
    if let Ok(cache) = CONFIG_CACHE.read()
        && !cache.force_reload
        && now.duration_since(cache.last_checked) < CONFIG_CACHE_CHECK_INTERVAL
    {
        return cache.config;
    }

    let mut reload_reason = None;
    let config = {
        let mut cache = CONFIG_CACHE
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        let now = Instant::now();
        if !cache.force_reload
            && now.duration_since(cache.last_checked) < CONFIG_CACHE_CHECK_INTERVAL
        {
            return cache.config;
        }

        let fingerprint = ConfigCacheFingerprint::current();
        cache.last_checked = now;
        if cache.force_reload || cache.fingerprint != fingerprint {
            reload_reason = Some(describe_config_reload(
                cache.force_reload,
                &cache.fingerprint,
                &fingerprint,
            ));
            cache.config = leak_config(Config::load());
            cache.fingerprint = fingerprint;
            cache.force_reload = false;
        }
        cache.config
    };

    if let Some(reason) = reload_reason {
        crate::logging::info(&format!("CONFIG_RELOAD {}", reason));
        notify_config_reloaded();
    }

    config
}

fn describe_config_reload(
    forced: bool,
    previous: &ConfigCacheFingerprint,
    next: &ConfigCacheFingerprint,
) -> String {
    let mut parts = Vec::new();
    if forced {
        parts.push("forced=true".to_string());
    }
    if previous.path != next.path {
        parts.push(format!(
            "path={:?}->{:?}",
            previous.path.as_ref().map(|p| p.display().to_string()),
            next.path.as_ref().map(|p| p.display().to_string())
        ));
    }
    if previous.modified != next.modified {
        parts.push("modified_changed=true".to_string());
    }
    if previous.len != next.len {
        parts.push(format!("len={:?}->{:?}", previous.len, next.len));
    }
    let env_changes = describe_env_changes(&previous.env, &next.env);
    if !env_changes.is_empty() {
        parts.push(format!("env=[{}]", env_changes.join(", ")));
    }
    if parts.is_empty() {
        "unchanged".to_string()
    } else {
        parts.join(" ")
    }
}

fn describe_env_changes(previous: &[(String, String)], next: &[(String, String)]) -> Vec<String> {
    let previous_map: BTreeMap<&str, &str> = previous
        .iter()
        .map(|(key, value)| (key.as_str(), value.as_str()))
        .collect();
    let next_map: BTreeMap<&str, &str> = next
        .iter()
        .map(|(key, value)| (key.as_str(), value.as_str()))
        .collect();
    let keys: BTreeSet<&str> = previous_map
        .keys()
        .chain(next_map.keys())
        .copied()
        .collect();

    keys.into_iter()
        .filter_map(|key| match (previous_map.get(key), next_map.get(key)) {
            (Some(previous), Some(next)) if previous != next => Some(format!(
                "{}:changed({}->{})",
                key,
                env_value_fingerprint(previous),
                env_value_fingerprint(next)
            )),
            (None, Some(next)) => Some(format!("{}:added({})", key, env_value_fingerprint(next))),
            (Some(previous), None) => Some(format!(
                "{}:removed({})",
                key,
                env_value_fingerprint(previous)
            )),
            _ => None,
        })
        .collect()
}

fn env_value_fingerprint(value: &str) -> String {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    value.hash(&mut hasher);
    format!("len:{} hash:{:016x}", value.len(), hasher.finish())
}

fn config_env_fingerprint() -> Vec<(String, String)> {
    let mut values = std::env::vars_os()
        .filter_map(|(key, value)| {
            let key = key.to_string_lossy().to_string();
            if CONFIG_ENV_KEYS.contains(&key.as_str()) {
                Some((key, value.to_string_lossy().to_string()))
            } else {
                None
            }
        })
        .collect::<Vec<_>>();
    values.sort_by(|left, right| left.0.cmp(&right.0));
    values
}

pub fn invalidate_config_cache() {
    let mut cache = CONFIG_CACHE
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    cache.force_reload = true;
    drop(cache);
    notify_config_reloaded();
}

fn notify_config_reloaded() {
    for listener in CONFIG_RELOAD_LISTENERS
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .iter()
    {
        listener();
    }
}

/// Listeners invoked after the config cache reloads.
///
/// Config is a foundational module, so instead of reaching up into higher-level
/// subsystems (auth cache, event bus) on reload, those subsystems register a
/// reaction here at startup. This keeps config free of upward dependencies and
/// breaks the config -> auth / config -> bus cycle edges.
static CONFIG_RELOAD_LISTENERS: LazyLock<RwLock<Vec<fn()>>> =
    LazyLock::new(|| RwLock::new(Vec::new()));

/// Register a callback to run after the config cache reloads.
///
/// Callbacks must be cheap and non-blocking; they run on whichever thread
/// triggers the reload. Intended to be called once per subsystem during
/// process startup.
pub fn on_config_reloaded(listener: fn()) {
    CONFIG_RELOAD_LISTENERS
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .push(listener);
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

    /// Web search tool configuration
    pub websearch: WebSearchConfig,

    /// Built-in tool exposure configuration
    pub tools: ToolConfig,

    /// Agent Client Protocol adapter configuration
    pub acp: AcpConfig,

    /// Auth trust / consent configuration
    pub auth: AuthConfig,

    /// Provider configuration
    pub provider: ProviderConfig,

    /// Named provider profiles, keyed by profile name.
    ///
    /// Example:
    /// [providers.my-gateway]
    /// type = "openai-compatible"
    /// base_url = "https://llm.example.com/v1"
    /// api_key_env = "MY_GATEWAY_API_KEY"
    pub providers: BTreeMap<String, NamedProviderConfig>,

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

    /// Power-management configuration (prevent sleep while streaming)
    pub power: PowerConfig,

    /// Auto-review configuration
    pub autoreview: AutoReviewConfig,

    /// Auto-judge configuration
    pub autojudge: AutoJudgeConfig,
}

/// Agent Client Protocol adapter configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AcpConfig {
    /// Client compatibility profile: "standard" (default), "extended", or "full".
    pub profile: String,
    /// Tool profile to request when `jcode acp` starts a daemon itself.
    pub tool_profile: String,
}

impl Default for AcpConfig {
    fn default() -> Self {
        Self {
            profile: "standard".to_string(),
            tool_profile: "acp".to_string(),
        }
    }
}

/// Controls which tools are sent to the model.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct ToolConfig {
    /// Tool profile: "full" (default), "acp", "minimal"/"lite", or "none".
    pub profile: String,
    /// Explicit allow-list. When set, only these tools are exposed.
    /// Use "*" or "all" to expose all tools, including default-disabled tools.
    pub enabled: Vec<String>,
    /// Tools to remove after applying profile/enabled.
    pub disabled: Vec<String>,
    /// Disable all built-in tools unless `enabled` is provided.
    pub disable_base_tools: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ToolSelection {
    pub allowed_tools: Option<HashSet<String>>,
    pub disabled_tools: HashSet<String>,
}

impl ToolConfig {
    const DEFAULT_DISABLED_TOOLS: &'static [&'static str] = &["gmail", "lsp"];

    pub fn selection(&self) -> ToolSelection {
        let mut allowed_tools = self.base_allowed_tools();
        let (explicit_enabled, enables_all_tools) = self.normalized_enabled_tools();
        let mut disabled_tools: HashSet<String> = self
            .disabled
            .iter()
            .map(|name| normalize_tool_name(name))
            .filter(|name| !name.is_empty())
            .collect();

        for name in Self::DEFAULT_DISABLED_TOOLS {
            let normalized = normalize_tool_name(name);
            if !enables_all_tools && !explicit_enabled.contains(&normalized) {
                disabled_tools.insert(normalized);
            }
        }

        if let Some(allowed) = allowed_tools.as_mut() {
            for name in &disabled_tools {
                allowed.remove(name);
            }
        }

        ToolSelection {
            allowed_tools,
            disabled_tools,
        }
    }

    pub fn allowed_tools(&self) -> Option<HashSet<String>> {
        self.selection().allowed_tools
    }

    pub fn apply_to_allowed_set(&self, allowed: &mut HashSet<String>) {
        let selection = self.selection();
        if let Some(global_allowed) = selection.allowed_tools {
            allowed.retain(|name| global_allowed.contains(name));
        }
        for disabled in selection.disabled_tools {
            allowed.remove(&disabled);
        }
    }

    fn base_allowed_tools(&self) -> Option<HashSet<String>> {
        let (explicit, enables_all_tools) = self.normalized_enabled_tools();

        let profile = self.profile.trim().to_ascii_lowercase();
        if enables_all_tools {
            None
        } else if !explicit.is_empty() {
            Some(explicit)
        } else if self.disable_base_tools || matches!(profile.as_str(), "none" | "off" | "disabled")
        {
            Some(HashSet::new())
        } else if matches!(profile.as_str(), "acp") {
            Some(
                [
                    "bash",
                    "read",
                    "write",
                    "edit",
                    "multiedit",
                    "apply_patch",
                    "patch",
                    "agentgrep",
                    "glob",
                    "grep",
                    "ls",
                    "batch",
                ]
                .into_iter()
                .map(|name| name.to_string())
                .collect(),
            )
        } else if matches!(profile.as_str(), "minimal" | "lite" | "small") {
            Some(
                [
                    "bash",
                    "read",
                    "write",
                    "edit",
                    "multiedit",
                    "apply_patch",
                    "patch",
                    "agentgrep",
                    "glob",
                    "grep",
                    "ls",
                ]
                .into_iter()
                .map(|name| name.to_string())
                .collect(),
            )
        } else {
            None
        }
    }

    fn normalized_enabled_tools(&self) -> (HashSet<String>, bool) {
        let mut enabled = HashSet::new();
        let mut enables_all_tools = false;

        for name in &self.enabled {
            let normalized = normalize_tool_name(name);
            if normalized.is_empty() {
                continue;
            }
            if normalized == "*" || normalized.eq_ignore_ascii_case("all") {
                enables_all_tools = true;
            } else {
                enabled.insert(normalized);
            }
        }

        (enabled, enables_all_tools)
    }
}

fn normalize_tool_name(name: &str) -> String {
    let trimmed = name.trim().trim_matches('"');
    jcode_tool_types::resolve_tool_name(trimmed).to_string()
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

mod config_file;
mod default_file;
mod display_summary;
mod env_overrides;

#[cfg(test)]
#[path = "config_tests.rs"]
mod tests;
