use serde::{Deserialize, Serialize};

pub mod keybindings;
pub use keybindings::{
    KEYBINDING_DEFAULTS, KeybindingDefault, KeybindingIssue, KeybindingIssueKind,
    KeybindingPlatform, KeybindingProvenance, PlatformDefault, default_binding, default_binding_or,
    keybinding_default, keybinding_defaults_report, validate_keybinding_defaults,
};

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

/// Session picker Enter action: "current-terminal" (default) or "new-terminal".
/// Ctrl+Enter performs the alternate action.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "kebab-case")]
pub enum SessionPickerResumeAction {
    NewTerminal,
    #[default]
    CurrentTerminal,
}

impl SessionPickerResumeAction {
    pub fn alternate(self) -> Self {
        match self {
            Self::NewTerminal => Self::CurrentTerminal,
            Self::CurrentTerminal => Self::NewTerminal,
        }
    }
}

/// How to display file diffs from edit/write tools.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DiffDisplayMode {
    /// Don't show diffs at all.
    Off,
    /// Show diffs inline in the chat (default).
    #[default]
    Inline,
    /// Show the full inline diff in the chat without preview truncation.
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
    /// Show diffs in a dedicated pinned pane.
    Pinned,
    /// Show full file with diff highlights in side panel, synced to scroll position.
    File,
}

impl DiffDisplayMode {
    pub fn is_inline(&self) -> bool {
        matches!(self, Self::Inline | Self::FullInline)
    }

    pub fn is_full_inline(&self) -> bool {
        matches!(self, Self::FullInline)
    }

    pub fn is_pinned(&self) -> bool {
        matches!(self, Self::Pinned)
    }

    pub fn is_file(&self) -> bool {
        matches!(self, Self::File)
    }

    pub fn has_side_pane(&self) -> bool {
        matches!(self, Self::Pinned | Self::File)
    }

    pub fn cycle(self) -> Self {
        match self {
            Self::Off => Self::Inline,
            Self::Inline => Self::FullInline,
            Self::FullInline => Self::Pinned,
            Self::Pinned => Self::File,
            Self::File => Self::Off,
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            Self::Off => "OFF",
            Self::Inline => "Inline",
            Self::FullInline => "Inline Full",
            Self::Pinned => "Pinned",
            Self::File => "File",
        }
    }
}

/// How to display mermaid diagrams.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DiagramDisplayMode {
    /// Don't show diagrams in dedicated widgets (only inline in messages).
    #[default]
    None,
    /// Show diagrams in info widget margins (opportunistic, if space available).
    Margin,
    /// Show diagrams in a dedicated pinned pane (forces space allocation).
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

/// How to display the model's reasoning/thinking content in the TUI.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ReasoningDisplayMode {
    /// Never display reasoning content.
    #[default]
    Off,
    /// Keep every reasoning trace in the transcript (classic behavior).
    Full,
    /// Show only the *current* reasoning live; collapse it once the model
    /// commits an assistant message or tool call, then show the next one.
    Current,
}

impl ReasoningDisplayMode {
    pub fn label(self) -> &'static str {
        match self {
            Self::Off => "Off",
            Self::Full => "Full",
            Self::Current => "Current",
        }
    }

    pub fn cycle(self) -> Self {
        match self {
            Self::Off => Self::Current,
            Self::Current => Self::Full,
            Self::Full => Self::Off,
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_lowercase().as_str() {
            "off" | "none" | "false" | "0" | "no" => Some(Self::Off),
            "full" | "all" | "true" | "1" | "yes" | "on" => Some(Self::Full),
            "current" | "live" | "ephemeral" | "collapse" => Some(Self::Current),
            _ => None,
        }
    }
}

/// Update channel: how aggressively to receive updates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum UpdateChannel {
    /// Only update from tagged GitHub Releases (default).
    #[default]
    Stable,
    /// Update from latest commit on main branch (bleeding edge).
    Main,
}

impl UpdateChannel {
    /// Parse a channel name, returning `None` for unknown values.
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "stable" | "release" => Some(Self::Stable),
            "main" | "nightly" | "edge" => Some(Self::Main),
            _ => None,
        }
    }
}

/// Config deserialization is deliberately lenient: an unknown or removed
/// channel name (e.g. a stale `update_channel = "manual"` left in
/// config.toml) falls back to the default channel instead of failing the
/// entire config parse. A strict enum here once made the freshly exec'd
/// server die during the reload handoff, leaving the handoff marker stuck
/// in `starting` and clients re-requesting the reload forever (issue #349).
impl<'de> Deserialize<'de> for UpdateChannel {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Ok(Self::parse(&value).unwrap_or_default())
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

/// Cross-provider failover behavior when the same input would be resent elsewhere.
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

    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "manual" => Some(Self::Manual),
            "countdown" | "auto" | "automatic" => Some(Self::Countdown),
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "kebab-case")]
pub enum NamedProviderType {
    #[serde(alias = "openai-compatible", alias = "openai_compatible")]
    #[default]
    OpenAiCompatible,
    OpenRouter,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum NamedProviderAuth {
    #[default]
    Bearer,
    Header,
    None,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(default)]
pub struct NamedProviderModelConfig {
    pub id: String,
    #[serde(
        default,
        alias = "context_limit",
        alias = "context-length",
        alias = "context-window",
        alias = "context_length",
        skip_serializing_if = "Option::is_none"
    )]
    pub context_window: Option<usize>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub input: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct NamedProviderConfig {
    #[serde(rename = "type")]
    pub provider_type: NamedProviderType,
    pub base_url: String,
    pub api: Option<String>,
    pub auth: NamedProviderAuth,
    pub auth_header: Option<String>,
    pub api_key_env: Option<String>,
    pub api_key: Option<String>,
    pub env_file: Option<String>,
    pub default_model: Option<String>,
    pub requires_api_key: Option<bool>,
    #[serde(default)]
    pub provider_routing: bool,
    #[serde(default)]
    pub model_catalog: bool,
    #[serde(default)]
    pub allow_provider_pinning: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub models: Vec<NamedProviderModelConfig>,
    /// Extra top-level JSON fields merged into every chat/completions request
    /// body sent to this provider. Lets users inject non-standard parameters
    /// some OpenAI-compatible backends require (e.g. NVIDIA NIM DeepSeek-V4
    /// needs `chat_template_kwargs = { thinking = true, reasoning_effort = "high" }`).
    /// Must be a JSON object; keys here override jcode-generated body fields.
    #[serde(default, alias = "extra-body", skip_serializing_if = "Option::is_none")]
    pub extra_body: Option<serde_json::Value>,
    /// Whether this endpoint accepts the DeepSeek-style top-level
    /// `reasoning_effort` request field (`/effort` support). When unset, jcode
    /// auto-detects it from the active model id (DeepSeek-family models
    /// support it regardless of which gateway serves them). Set `false` to
    /// suppress auto-detection for strict-schema endpoints.
    #[serde(
        default,
        alias = "supports-reasoning-effort",
        alias = "reasoning_effort",
        skip_serializing_if = "Option::is_none"
    )]
    pub supports_reasoning_effort: Option<bool>,
}

impl Default for NamedProviderConfig {
    fn default() -> Self {
        Self {
            provider_type: NamedProviderType::OpenAiCompatible,
            base_url: String::new(),
            api: None,
            auth: NamedProviderAuth::Bearer,
            auth_header: None,
            api_key_env: None,
            api_key: None,
            env_file: None,
            default_model: None,
            requires_api_key: None,
            provider_routing: false,
            model_catalog: false,
            allow_provider_pinning: false,
            models: Vec::new(),
            extra_body: None,
            supports_reasoning_effort: None,
        }
    }
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
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AgentsConfig {
    /// Optional default model override for spawned swarm/subagent sessions.
    ///
    /// Leave unset (or use `"inherit"` / `"coordinator"`) to have spawned swarm
    /// agents inherit the spawning coordinator's model. Set to a concrete model
    /// string only when you deliberately want every swarm worker pinned to a
    /// specific model regardless of which model spawned them.
    pub swarm_model: Option<String>,
    /// Default terminal mode for swarm-created agents.
    pub swarm_spawn_mode: SwarmSpawnMode,
    /// Maximum percentage (1-90) of the chat column height the inline swarm
    /// gallery band may occupy. Leave unset to use the built-in default (40%).
    /// Lower values keep more of the transcript visible; set near the minimum
    /// to effectively collapse the gallery to a thin strip.
    pub swarm_gallery_max_pct: Option<u8>,
    /// Optional default model override for the memory sidecar.
    pub memory_model: Option<String>,
    /// Whether memory should use the sidecar for relevance/extraction.
    ///
    /// Defaults to `true`: the LLM precision-judge path is the only memory mode
    /// that is reliably productive (injection precision ~1.0), so memory uses it
    /// by default. Set to `false` only to deliberately opt into the lower-
    /// precision no-LLM hybrid path. When sidecar mode is on but no LLM backend
    /// is reachable, the memory runtime goes dormant instead of degrading to the
    /// no-LLM path.
    #[serde(default = "default_memory_sidecar_enabled")]
    pub memory_sidecar_enabled: bool,
    /// Minimum turns between Mode-2 memory reranks (cadence floor). The
    /// expensive listwise LLM rerank runs at most once per this many turns;
    /// skipped turns fall back to hybrid-ordered surfacing. A topic change or
    /// the first turn always forces a rerank regardless of cadence. 0 or 1 =
    /// rerank every turn (no gating). Default 3.
    #[serde(default = "default_memory_rerank_cadence")]
    pub memory_rerank_cadence: usize,
    /// Number of independent LLM rerank "judges" to run per fired rerank. Their
    /// votes are combined and only memories meeting `memory_rerank_min_agree`
    /// agreement are injected. 1 = single judge (cheapest). 2 = two judges must
    /// agree, which lifts injection precision to ~1.0 with ~100% clean-rate on
    /// no-memory turns (offline adjudication), at 2 LLM calls per fired turn.
    #[serde(default = "default_memory_rerank_votes")]
    pub memory_rerank_votes: usize,
    /// Minimum judge agreement (of `memory_rerank_votes`) required to inject a
    /// memory. Clamped to 1..=votes. Higher = stricter precision, lower recall.
    #[serde(default = "default_memory_rerank_min_agree")]
    pub memory_rerank_min_agree: usize,
    /// Which embedding backend memory dense-retrieval uses: `"local"` (bundled
    /// all-MiniLM-L6-v2 ONNX, default, no network) or `"openai"` (remote
    /// OpenAI/openai-compatible `/v1/embeddings`, opt-in, requires an
    /// `OPENAI_API_KEY`). A keyless `"openai"` setting silently degrades to
    /// local. Env override: `JCODE_MEMORY_EMBEDDING_BACKEND`.
    #[serde(default = "default_memory_embedding_backend")]
    pub memory_embedding_backend: String,
    /// OpenAI embedding model name when `memory_embedding_backend = "openai"`.
    /// Unset = `text-embedding-3-small`. Env: `JCODE_MEMORY_EMBEDDING_MODEL`.
    #[serde(default)]
    pub memory_embedding_model: Option<String>,
    /// Optional override for the embeddings API base URL (no trailing slash),
    /// for OpenAI-compatible gateways. Unset = `https://api.openai.com/v1`.
    /// Env: `JCODE_MEMORY_EMBEDDING_BASE_URL`.
    #[serde(default)]
    pub memory_embedding_base_url: Option<String>,
    /// Optional override for the remote embedding dimensionality (vector-space
    /// metadata / sanity checks). Unset = inferred from the model name.
    #[serde(default)]
    pub memory_embedding_dim: Option<usize>,
    /// Maximum seconds a direct (blocking) `subagent` tool call will wait for the
    /// child session to produce its final answer before failing with a timeout
    /// error. Prevents a stuck/hung child turn from blocking the caller forever.
    /// `0` disables the bound (wait indefinitely). Default 600 (10 min).
    #[serde(default = "default_subagent_timeout_secs")]
    pub subagent_timeout_secs: u64,
}

fn default_subagent_timeout_secs() -> u64 {
    600
}

fn default_memory_embedding_backend() -> String {
    "local".to_string()
}

fn default_memory_sidecar_enabled() -> bool {
    true
}

fn default_memory_rerank_cadence() -> usize {
    3
}

fn default_memory_rerank_votes() -> usize {
    2
}

fn default_memory_rerank_min_agree() -> usize {
    2
}

impl Default for AgentsConfig {
    fn default() -> Self {
        Self {
            swarm_model: None,
            swarm_spawn_mode: SwarmSpawnMode::default(),
            swarm_gallery_max_pct: None,
            memory_model: None,
            memory_sidecar_enabled: default_memory_sidecar_enabled(),
            memory_rerank_cadence: default_memory_rerank_cadence(),
            memory_rerank_votes: default_memory_rerank_votes(),
            memory_rerank_min_agree: default_memory_rerank_min_agree(),
            memory_embedding_backend: default_memory_embedding_backend(),
            memory_embedding_model: None,
            memory_embedding_base_url: None,
            memory_embedding_dim: None,
            subagent_timeout_secs: default_subagent_timeout_secs(),
        }
    }
}

/// How swarm-created agents should be spawned.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum SwarmSpawnMode {
    /// Open a visible/headed terminal window. This preserves historical behavior.
    #[default]
    Visible,
    /// Create the worker in-process without opening a terminal window.
    Headless,
    /// Like headless (no terminal window), but the coordinator renders a live
    /// inline gallery viewport of each worker's streaming output.
    Inline,
    /// Try visible first and fall back to headless if a window cannot be opened.
    Auto,
}

impl SwarmSpawnMode {
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "visible" | "headed" => Some(Self::Visible),
            "headless" => Some(Self::Headless),
            "inline" => Some(Self::Inline),
            "auto" => Some(Self::Auto),
            _ => None,
        }
    }

    /// Canonical lowercase string for this mode (matches the config/env values).
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Visible => "visible",
            Self::Headless => "headless",
            Self::Inline => "inline",
            Self::Auto => "auto",
        }
    }
}

/// Terminal window/pane spawning configuration.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct TerminalConfig {
    /// External command that takes over headed session spawns (new terminal
    /// windows for swarm agents, resume-in-new-terminal, self-dev, restarts).
    ///
    /// When set, jcode runs `<spawn_hook> <jcode-binary> <args...>` instead of
    /// opening a terminal emulator itself, with `JCODE_SPAWN_*` metadata env
    /// vars describing the spawn (kind, session id, title, cwd, full command).
    /// This lets multiplexers and wrappers (tmux, kitty remote, zellij, herd
    /// runners, window managers) decide where and how the session appears.
    ///
    /// Example: `spawn_hook = "tmux new-window"` opens each headed spawn as a
    /// tmux window in the current server. If the hook fails to launch, jcode
    /// falls back to its built-in terminal detection.
    ///
    /// Env override: `JCODE_SPAWN_HOOK` (set empty to disable a config hook).
    pub spawn_hook: Option<String>,
    /// External command used to focus/raise an existing session window.
    ///
    /// When set, jcode runs the hook (instead of wmctrl/xdotool) whenever it
    /// wants to bring a session's window to the foreground, with
    /// `JCODE_FOCUS_SESSION_ID` and `JCODE_FOCUS_TITLE` env vars. Pair this
    /// with `spawn_hook` so wrappers that own placement (tmux, kitty remote,
    /// herd) also own focus (e.g. `tmux select-window`, Wayland compositor
    /// IPC like `niri msg`).
    ///
    /// Env override: `JCODE_FOCUS_HOOK` (set empty to disable a config hook).
    pub focus_hook: Option<String>,
    /// Terminal used by the macOS Cmd+; launch hotkey and in-app session spawns.
    ///
    /// One of: `ghostty`, `iterm2`, `wezterm`, `warp`, `alacritty`, `vscode`,
    /// `terminal` (Apple Terminal). When set, this is the source of truth for
    /// which terminal jcode launches into and is preferred over the legacy
    /// `~/.jcode/preferred_terminal.json` file. Re-run `jcode setup-hotkey`
    /// after changing it so the generated launcher script picks up the change.
    ///
    /// macOS only; ignored on other platforms.
    pub preferred: Option<String>,
}

/// Lifecycle hooks: external commands jcode runs at well-defined points.
///
/// Hook commands are parsed shell-style (quotes work) but executed directly,
/// with `JCODE_HOOK_*` env vars describing the event (`JCODE_HOOK_EVENT`,
/// `JCODE_HOOK_SESSION_ID`, `JCODE_HOOK_CWD`, event-specific fields, and a
/// `JCODE_HOOK_PAYLOAD` JSON mirror). Hook processes get
/// `JCODE_HOOKS_DISABLED=1` so nested jcode invocations don't recurse.
///
/// All hooks except `pre_tool` are observers: detached, fire-and-forget,
/// failures only logged. `pre_tool` is a gate: jcode waits for it and exit
/// code 2 blocks the tool call (stderr becomes the error shown to the model);
/// exit 0 allows; anything else fails open.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct HooksConfig {
    /// Runs when an agent turn begins (after the user message is added and
    /// before the model starts generating). Fires before the first `pre_tool`,
    /// so integrations can detect that the agent is actively working even while
    /// it is only thinking/streaming text. Fields: MODEL, SOURCE
    /// ("chat"/"resume"/"ambient"). Env override: JCODE_HOOK_TURN_START.
    pub turn_start: Option<String>,
    /// Runs when an agent turn completes.
    /// Fields: STATUS ("ok"/"error"), DURATION_MS, MODEL, LAST_ASSISTANT_TEXT.
    /// Env override: JCODE_HOOK_TURN_END.
    pub turn_end: Option<String>,
    /// Runs when a session becomes active (created or resumed).
    /// Fields: SOURCE ("create"/"resume").
    /// Env override: JCODE_HOOK_SESSION_START.
    pub session_start: Option<String>,
    /// Runs when a session closes normally.
    /// Env override: JCODE_HOOK_SESSION_END.
    pub session_end: Option<String>,
    /// Gate hook before each tool call. Receives TOOL_NAME and the tool input
    /// JSON on stdin (also truncated in TOOL_INPUT). Exit 0 allows, exit 2
    /// blocks (stderr is fed back to the model), anything else fails open.
    /// Env override: JCODE_HOOK_PRE_TOOL.
    pub pre_tool: Option<String>,
    /// Runs after each tool call completes.
    /// Fields: TOOL_NAME, STATUS ("ok"/"error"), DURATION_MS, OUTPUT_BYTES.
    /// Env override: JCODE_HOOK_POST_TOOL.
    pub post_tool: Option<String>,
    /// Max milliseconds to wait for the pre_tool gate before failing open
    /// (default: 5000). Env override: JCODE_HOOK_PRE_TOOL_TIMEOUT_MS.
    pub pre_tool_timeout_ms: u64,
}

impl Default for HooksConfig {
    fn default() -> Self {
        Self {
            turn_start: None,
            turn_end: None,
            session_start: None,
            session_end: None,
            pre_tool: None,
            post_tool: None,
            pre_tool_timeout_ms: 5000,
        }
    }
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
    /// Accept the post-error fallback offer: switch to the next best
    /// model/auth-method and resend the failed turn (default: "ctrl+y").
    pub fallback_switch: String,
    /// Effort increase key (default: "cmd+right" on macOS, "alt+right" elsewhere)
    pub effort_increase: String,
    /// Effort decrease key (default: "cmd+left" on macOS, "alt+left" elsewhere)
    pub effort_decrease: String,
    /// Centered mode toggle key (default: "alt+c")
    pub centered_toggle: String,
    /// Scroll to previous prompt key (default: "ctrl+[")
    pub scroll_prompt_up: String,
    /// Scroll to next prompt key (default: "ctrl+]")
    pub scroll_prompt_down: String,
    /// Scroll bookmark toggle key (default: "ctrl+g")
    pub scroll_bookmark: String,
    /// Scroll up fallback key (default: unset; Cmd+K moves up by prompt on macOS)
    pub scroll_up_fallback: String,
    /// Scroll down fallback key (default: unset; Cmd+J moves down by prompt on macOS)
    pub scroll_down_fallback: String,
    /// Workspace navigation left key (default: "alt+h")
    pub workspace_left: String,
    /// Workspace navigation down key (default: "alt+j")
    pub workspace_down: String,
    /// Workspace navigation up key (default: "alt+k")
    pub workspace_up: String,
    /// Workspace navigation right key (default: "alt+l")
    pub workspace_right: String,
    /// Toggle the side panel (default: "alt+m")
    pub side_panel_toggle: String,
    /// Toggle copy/selection mode (default: "alt+y")
    pub copy_selection_toggle: String,
    /// Toggle the diagram pane position (default: "alt+t")
    pub diagram_pane_toggle: String,
    /// Toggle typing scroll lock (default: "alt+s")
    pub typing_scroll_lock_toggle: String,
    /// Cycle inline diff display mode (default: "alt+g")
    pub diff_mode_cycle: String,
    /// Toggle the info widget (default: "alt+i")
    pub info_widget_toggle: String,
    /// Focus/unfocus the inline swarm panel for keyboard navigation (default:
    /// "alt+w"). Active only when `agents.swarm_spawn_mode = "inline"` and the
    /// session manages swarm agents.
    pub swarm_panel_focus: String,
    /// Spawn a fresh jcode session in a new terminal window (default: unbound).
    /// Example: "alt+enter".
    pub new_terminal: String,
    /// Open the `/resume` session picker (default: "cmd+b" on macOS, "alt+r"
    /// elsewhere). Set "" to disable.
    pub open_resume: String,
    /// Session picker Enter action: "current-terminal" (default) or "new-terminal".
    /// Ctrl+Enter performs the alternate action.
    pub session_picker_enter: SessionPickerResumeAction,
}

impl Default for KeybindingsConfig {
    fn default() -> Self {
        // Pull platform-appropriate defaults from the single source of truth in
        // `keybindings.rs`. This is where the macOS vs Windows/Linux split takes
        // effect: each field resolves to its own platform's default binding.
        let p = KeybindingPlatform::current();
        let get = |id: &str, fallback: &'static str| {
            default_binding(id, p).unwrap_or(fallback).to_string()
        };
        Self {
            scroll_up: get("scroll_up", "ctrl+k"),
            scroll_down: get("scroll_down", "ctrl+j"),
            scroll_page_up: get("scroll_page_up", "alt+u"),
            scroll_page_down: get("scroll_page_down", "alt+d"),
            model_switch_next: get("model_switch_next", "ctrl+tab"),
            model_switch_prev: get("model_switch_prev", "ctrl+shift+tab"),
            fallback_switch: get("fallback_switch", "ctrl+y"),
            effort_increase: get("effort_increase", "alt+right"),
            effort_decrease: get("effort_decrease", "alt+left"),
            centered_toggle: get("centered_toggle", "alt+c"),
            scroll_prompt_up: get("scroll_prompt_up", "ctrl+["),
            scroll_prompt_down: get("scroll_prompt_down", "ctrl+]"),
            scroll_bookmark: get("scroll_bookmark", "ctrl+g"),
            scroll_up_fallback: get("scroll_up_fallback", ""),
            scroll_down_fallback: get("scroll_down_fallback", ""),
            workspace_left: get("workspace_left", "alt+h"),
            workspace_down: get("workspace_down", "alt+j"),
            workspace_up: get("workspace_up", "alt+k"),
            workspace_right: get("workspace_right", "alt+l"),
            side_panel_toggle: get("side_panel_toggle", "alt+m"),
            copy_selection_toggle: get("copy_selection_toggle", "alt+y"),
            diagram_pane_toggle: get("diagram_pane_toggle", "alt+t"),
            typing_scroll_lock_toggle: get("typing_scroll_lock_toggle", "alt+s"),
            diff_mode_cycle: get("diff_mode_cycle", "alt+g"),
            info_widget_toggle: get("info_widget_toggle", "alt+i"),
            swarm_panel_focus: get("swarm_panel_focus", "alt+w"),
            new_terminal: get("new_terminal", ""),
            open_resume: get(
                "open_resume",
                if cfg!(target_os = "macos") {
                    "cmd+b"
                } else {
                    "alt+r"
                },
            ),
            session_picker_enter: SessionPickerResumeAction::CurrentTerminal,
        }
    }
}

/// How to display file diffs from edit/write tools
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
    /// Center all content (default: false)
    pub centered: bool,
    /// Show thinking/reasoning content by default (default: true)
    pub show_thinking: bool,
    /// How to display reasoning/thinking content (off/full/current).
    /// When unset, falls back to `show_thinking` (true => full, false => off).
    #[serde(default)]
    reasoning_display: Option<ReasoningDisplayMode>,
    /// How to display mermaid diagrams (none/margin/pinned, default: none).
    /// Mermaid rendering is temporarily disabled for users unless JCODE_ENABLE_MERMAID=1.
    pub diagram_mode: DiagramDisplayMode,
    /// Markdown block spacing style (compact/document, default: compact)
    pub markdown_spacing: MarkdownSpacingMode,
    /// Pin read images to side pane (default: true)
    pub pin_images: bool,
    /// Show idle animation before first prompt (default: true)
    pub idle_animation: bool,
    /// Briefly animate user prompt line when it enters viewport (default: true)
    pub prompt_entry_animation: bool,
    /// Disable specific animation variants by name (e.g. ["donut", "orbit_rings"])
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
    /// Render swarm/file-activity notifications in a compact single-line form
    /// instead of the full multi-line card with diff preview (default: false)
    pub compact_notifications: bool,
    /// Override the Alt/Option label shown in copy badges. Empty = auto (⌥ on macOS, Alt elsewhere).
    pub copy_badge_alt_label: String,
    /// Show the full agentgrep tool output inline in the transcript instead of
    /// just the one-line summary (default: false)
    #[serde(default)]
    pub show_agentgrep_output: bool,
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
            centered: false,
            show_thinking: true,
            reasoning_display: Some(ReasoningDisplayMode::Current),
            diagram_mode: DiagramDisplayMode::default(),
            markdown_spacing: MarkdownSpacingMode::default(),
            idle_animation: true,
            prompt_entry_animation: true,
            disabled_animations: Vec::new(),
            diff_line_wrap: true,
            performance: String::new(),
            animation_fps: 60,
            redraw_fps: 60,
            prompt_preview: true,
            compact_notifications: false,
            copy_badge_alt_label: String::new(),
            show_agentgrep_output: false,
            native_scrollbars: NativeScrollbarConfig::default(),
        }
    }
}

impl DisplayConfig {
    pub fn apply_legacy_compat(&mut self) {
        if let Some(show) = self.show_diffs.take() {
            self.diff_mode = if show {
                DiffDisplayMode::Inline
            } else {
                DiffDisplayMode::Off
            };
        }
    }

    /// Resolve the effective reasoning display mode. Prefers the explicit
    /// `reasoning_display` field, falling back to the legacy `show_thinking`
    /// boolean (true => Full, false => Off) when unset.
    pub fn reasoning_display(&self) -> ReasoningDisplayMode {
        self.reasoning_display.unwrap_or(if self.show_thinking {
            ReasoningDisplayMode::Full
        } else {
            ReasoningDisplayMode::Off
        })
    }

    /// Set the reasoning display mode and keep `show_thinking` in sync so the
    /// provider request path (which still keys off `show_thinking`) requests
    /// reasoning whenever any display mode is active.
    pub fn set_reasoning_display(&mut self, mode: ReasoningDisplayMode) {
        self.reasoning_display = Some(mode);
        self.show_thinking = !matches!(mode, ReasoningDisplayMode::Off);
    }

    /// Whether reasoning content should be generated/requested at all.
    pub fn reasoning_enabled(&self) -> bool {
        !matches!(self.reasoning_display(), ReasoningDisplayMode::Off)
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
    /// Persist auto-recalled memory injections into normal session history instead of sending
    /// them as request-only ephemeral suffix messages (default: false)
    pub persist_memory_injections: bool,
    /// Surface an in-chat system message whenever a request misses the KV cache
    /// for a harness-caused (avoidable) reason: the system prompt, tool set, or
    /// message prefix changed without the conversation legitimately growing.
    /// These should essentially never happen, so the notice acts as a loud alarm
    /// that something in the harness silently invalidated the prefix cache
    /// (default: true).
    pub kv_cache_miss_notices: bool,
    /// Update channel: "stable" (releases only) or "main" (latest commits)
    pub update_channel: UpdateChannel,
}

impl Default for FeatureConfig {
    fn default() -> Self {
        Self {
            memory: true,
            swarm: true,
            message_timestamps: true,
            persist_memory_injections: false,
            kv_cache_miss_notices: true,
            update_channel: UpdateChannel::default(),
        }
    }
}

/// Search engine used by the websearch tool.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash, Default)]
#[serde(rename_all = "lowercase")]
pub enum WebSearchEngine {
    /// DuckDuckGo HTML search, no API key required.
    #[default]
    Duckduckgo,
    /// Bing search. Uses the Bing API when configured, otherwise Bing HTML search.
    Bing,
    /// SearXNG metasearch instance (JSON API). Requires `searxng_url` (or the
    /// `JCODE_SEARXNG_URL` env var) to point at a SearXNG instance. Useful on
    /// hosts where DuckDuckGo/Bing block the request via TLS fingerprinting.
    Searxng,
}

impl WebSearchEngine {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Duckduckgo => "duckduckgo",
            Self::Bing => "bing",
            Self::Searxng => "searxng",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "duckduckgo" | "ddg" => Some(Self::Duckduckgo),
            "bing" => Some(Self::Bing),
            "searxng" | "searx" => Some(Self::Searxng),
            _ => None,
        }
    }
}

/// Configuration for the websearch tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct WebSearchConfig {
    /// Preferred engine when the tool input does not specify one.
    pub engine: WebSearchEngine,
    /// Keyless HTML engines to try after the preferred engine fails.
    pub fallback_engines: Vec<WebSearchEngine>,
    /// Optional Bing API key for primary Bing searches. Fallback Bing uses keyless HTML search.
    pub bing_api_key: Option<String>,
    /// Environment variable containing the Bing API key.
    pub bing_api_key_env: String,
    /// Bing market, e.g. "en-US" or "zh-CN".
    pub bing_market: String,
    /// Base URL of a SearXNG instance (e.g. "https://searx.example.org"), used
    /// by the `searxng` engine. When empty, the `searxng_url_env` variable is
    /// consulted instead.
    pub searxng_url: Option<String>,
    /// Environment variable containing the SearXNG base URL.
    pub searxng_url_env: String,
}

impl Default for WebSearchConfig {
    fn default() -> Self {
        Self {
            engine: WebSearchEngine::Duckduckgo,
            fallback_engines: vec![WebSearchEngine::Bing],
            bing_api_key: None,
            bing_api_key_env: "JCODE_BING_API_KEY".to_string(),
            bing_market: "en-US".to_string(),
            searxng_url: None,
            searxng_url_env: "JCODE_SEARXNG_URL".to_string(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ProviderConfig {
    /// Default model to use (e.g. "claude-opus-4-8", "copilot:claude-opus-4.6")
    pub default_model: Option<String>,
    /// Default provider to use (claude|openai|copilot|openrouter)
    pub default_provider: Option<String>,
    /// Reasoning effort for OpenAI Responses API (none|low|medium|high|xhigh)
    pub openai_reasoning_effort: Option<String>,
    /// Reasoning effort for Anthropic Messages API output_config (none|low|medium|high|xhigh; max aliases to strongest supported)
    pub anthropic_reasoning_effort: Option<String>,
    /// OpenAI transport mode (auto|websocket|https)
    pub openai_transport: Option<String>,
    /// OpenAI service tier override (priority|flex)
    pub openai_service_tier: Option<String>,
    /// OpenAI native compaction mode: "auto", "explicit", or "off".
    pub openai_native_compaction_mode: String,
    /// Token threshold at which OpenAI auto native compaction should trigger.
    pub openai_native_compaction_threshold_tokens: usize,
    /// Preserve provider-native reasoning/thinking items for future-turn context when supported.
    pub preserve_reasoning_context: bool,
    /// How to handle cross-provider failover when the same input would be resent elsewhere.
    pub cross_provider_failover: CrossProviderFailoverMode,
    /// Whether jcode should automatically try another account on the same provider
    /// before falling back to a different provider.
    pub same_provider_account_failover: bool,
    /// Copilot premium request mode: "normal", "one", or "zero"
    /// "zero" means all requests are free (no premium requests consumed)
    pub copilot_premium: Option<String>,
    /// Max seconds to wait for streaming data before timing out a request with
    /// no data received. Raise this for slow reasoning models (e.g. DeepSeek)
    /// that think silently for minutes before emitting tokens. Default: 180.
    /// Overridable per-launch via `JCODE_STREAM_IDLE_TIMEOUT_SECS`.
    pub stream_idle_timeout_secs: u64,
}

impl Default for ProviderConfig {
    fn default() -> Self {
        Self {
            default_model: None,
            default_provider: None,
            openai_reasoning_effort: Some("low".to_string()),
            anthropic_reasoning_effort: None,
            openai_transport: None,
            openai_service_tier: Some("priority".to_string()),
            openai_native_compaction_mode: "auto".to_string(),
            openai_native_compaction_threshold_tokens: 200_000,
            preserve_reasoning_context: true,
            cross_provider_failover: CrossProviderFailoverMode::Countdown,
            same_provider_account_failover: true,
            copilot_premium: None,
            stream_idle_timeout_secs: 180,
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

/// Desktop notification configuration for interactive sessions.
///
/// Unlike `[safety]` (ambient-mode ntfy/email/channel notifications), this
/// section controls lightweight local desktop notifications for the normal
/// interactive TUI, e.g. "agent finished a long turn".
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct NotificationsConfig {
    /// Send a desktop notification when an agent turn completes (default: true).
    /// Notifications fire only for long turns (see thresholds below) and, by
    /// default, only while the terminal window is unfocused.
    pub turn_complete: bool,
    /// Minimum turn duration, in seconds, before a completed turn notifies
    /// (default: 120).
    pub turn_complete_min_secs: u64,
    /// Lower duration threshold, in seconds, used when the session has todos
    /// recorded, since todos indicate longer task-style work (default: 30).
    pub turn_complete_todo_min_secs: u64,
    /// Only notify while the terminal window is unfocused (default: true).
    /// Requires a terminal that reports focus events (most modern terminals).
    pub turn_complete_only_when_unfocused: bool,
    /// macOS Notification Center sound name played on turn completion
    /// (e.g. "Glass", "Ping", "Hero"). Empty string disables the sound.
    /// Ignored on non-macOS platforms. Default: "Glass".
    pub turn_complete_sound: String,
}

impl Default for NotificationsConfig {
    fn default() -> Self {
        Self {
            turn_complete: true,
            turn_complete_min_secs: 120,
            turn_complete_todo_min_secs: 30,
            turn_complete_only_when_unfocused: true,
            turn_complete_sound: "Glass".to_string(),
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
    /// Enable the Jade cloud relay channel (remote control via cloud mailbox, default: false)
    pub jade_relay_enabled: bool,
    /// Jade relay API base URL (e.g. https://...lambda-url.us-east-1.on.aws/)
    pub jade_relay_api_base: Option<String>,
    /// Jade relay bearer token (prefer JCODE_JADE_RELAY_TOKEN env var)
    pub jade_relay_token: Option<String>,
    /// Jade relay token id header (x-jade-token-id), used for fast token lookup
    pub jade_relay_token_id: Option<String>,
    /// Jade relay user id (channel scope; defaults to the token's user when omitted)
    pub jade_relay_user_id: Option<String>,
    /// Jade relay session id to bind this laptop's listener to (the channel = user_id/session_id)
    pub jade_relay_session_id: Option<String>,
    /// Enable Jade relay prompt → agent directive feature (default: false)
    pub jade_relay_reply_enabled: bool,
    /// Enable Jade relay device launch commands that open headed local sessions (default: false)
    pub jade_relay_launch_enabled: bool,
    /// Default working directory for remotely launched headed sessions
    pub jade_relay_launch_working_dir: Option<String>,
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
            jade_relay_enabled: false,
            jade_relay_api_base: None,
            jade_relay_token: None,
            jade_relay_token_id: None,
            jade_relay_user_id: None,
            jade_relay_session_id: None,
            jade_relay_reply_enabled: false,
            jade_relay_launch_enabled: false,
            jade_relay_launch_working_dir: None,
        }
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

/// Power-management configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct PowerConfig {
    /// Prevent the machine from going to sleep (idle/lid suspend) while any
    /// jcode session is actively streaming/processing. The display is still
    /// allowed to sleep; only system suspend is inhibited. Default: true.
    ///
    /// Honored by the shared `jcode serve` daemon. The `JCODE_DISABLE_POWER_INHIBIT`
    /// environment variable forces this off regardless of the config value.
    pub prevent_sleep_while_streaming: bool,
}

impl Default for PowerConfig {
    fn default() -> Self {
        Self {
            prevent_sleep_while_streaming: true,
        }
    }
}

/// A single global launch hotkey: a chord plus the directory it opens jcode in.
///
/// `dir` is usually an absolute path, but a few sentinels keep dynamic targets
/// working without rewriting config on every launch:
/// - `$HOME` -> the user's home directory.
/// - `$LAST_DIR` -> the most recent non-home project directory jcode ran in.
/// - `$LAST_REPO` -> the most recent jcode repo (for self-dev).
///
/// `self_dev = true` opens the directory as a self-dev session (passes the
/// `self-dev` subcommand). `label` is an optional human name used in notices.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LaunchHotkeyEntry {
    /// jcode-style chord string, e.g. `cmd+;`, `cmd+[`, `cmd+shift+'`.
    pub chord: String,
    /// Directory to open (absolute path or a `$HOME`/`$LAST_DIR`/`$LAST_REPO`
    /// sentinel).
    pub dir: String,
    /// Optional short label (e.g. the repo's directory name) for notices.
    #[serde(default)]
    pub label: String,
    /// Open as a self-dev session instead of a normal session.
    #[serde(default)]
    pub self_dev: bool,
}

/// Configuration for the global "launch a new jcode" hotkeys (macOS).
///
/// When `entries` is empty, jcode uses its built-in defaults (`Cmd+;` -> home,
/// `Cmd+'` -> last project, `Cmd+Shift+'` -> self-dev). Auto-import can bake a
/// richer, per-repo mapping here once: the top repo on `Cmd+;`, home on
/// `Cmd+'`, and the next repos on `Cmd+[` / `Cmd+]` / `Cmd+\`. Once baked the
/// mapping is static and does not move around as the user's activity changes.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct LaunchHotkeysConfig {
    /// Whether the global launch hotkeys are installed at all. `None` means
    /// "not decided yet" (fall back to the legacy auto-install gating); `Some`
    /// is an explicit user/import choice.
    pub enabled: Option<bool>,
    /// Explicit chord -> directory mapping. Empty = use built-in defaults.
    pub entries: Vec<LaunchHotkeyEntry>,
    /// Set true once auto-import has populated `entries`, so we only bake the
    /// per-repo mapping a single time and never clobber later user edits.
    pub imported: bool,
}
