/// Claude Code OAuth beta headers used by the Anthropic transport.
pub const ANTHROPIC_OAUTH_BETA_HEADERS: &str = "claude-code-20250219,oauth-2025-04-20,interleaved-thinking-2025-05-14,context-management-2025-06-27,prompt-caching-scope-2026-01-05,advisor-tool-2026-03-01,advanced-tool-use-2025-11-20,effort-2025-11-24";

/// Claude Code OAuth beta headers with Anthropic's explicit 1M context beta.
pub const ANTHROPIC_OAUTH_BETA_HEADERS_1M: &str = "claude-code-20250219,oauth-2025-04-20,interleaved-thinking-2025-05-14,context-management-2025-06-27,prompt-caching-scope-2026-01-05,advisor-tool-2026-03-01,advanced-tool-use-2025-11-20,effort-2025-11-24,context-1m-2025-08-07";

/// How a Claude model exposes its 1M-token long-context window.
///
/// These classifications were verified against the live Anthropic API on a
/// Claude subscription (raw 250K-token requests): the catalog's
/// `max_input_tokens` field is not a reliable signal because it over-advertises
/// 1M for models that are still hard-capped at 200K (e.g. `claude-sonnet-4-5`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AnthropicContextMode {
    /// 1M input window available by default, no beta header or `[1m]` opt-in
    /// needed (e.g. `claude-opus-4-8`, `claude-opus-4-7`).
    Native1M,
    /// 200K by default; 1M available as an opt-in via the `context-1m` beta
    /// header (the `[1m]` suffix), which may require usage credits
    /// (e.g. `claude-opus-4-6`, `claude-sonnet-4-6`).
    OptIn1M,
    /// 200K input window, with no 1M path (e.g. `claude-opus-4-5`,
    /// `claude-sonnet-4-5`, `claude-haiku-4-5`).
    Standard,
}

impl AnthropicContextMode {
    /// The default context window (in tokens) for this mode, i.e. what a request
    /// gets without opting in to the 1M beta.
    pub fn default_context_window(self) -> usize {
        match self {
            AnthropicContextMode::Native1M => 1_000_000,
            AnthropicContextMode::OptIn1M | AnthropicContextMode::Standard => 200_000,
        }
    }

    /// The context window (in tokens) when the 1M long-context path is engaged
    /// (the `[1m]` suffix). For `Standard` models there is no 1M path, so this is
    /// the same as the default.
    pub fn long_context_window(self) -> usize {
        match self {
            AnthropicContextMode::Native1M => 1_000_000,
            // Anthropic's opt-in beta advertises a 1,048,576-token window.
            AnthropicContextMode::OptIn1M => 1_048_576,
            AnthropicContextMode::Standard => 200_000,
        }
    }

    /// Whether this model has any 1M long-context path at all (native or opt-in).
    pub fn has_1m_window(self) -> bool {
        !matches!(self, AnthropicContextMode::Standard)
    }

    /// Whether jcode should surface a distinct `[1m]` picker alias for this model.
    /// Only opt-in models benefit, native-1M models already use 1M by default so
    /// a `[1m]` alias would be a redundant duplicate.
    pub fn exposes_1m_alias(self) -> bool {
        matches!(self, AnthropicContextMode::OptIn1M)
    }
}

/// Classify how a Claude model exposes long context. Accepts both canonical
/// (`claude-opus-4-8`) and dotted (`claude-opus-4.8`) forms, with or without a
/// trailing `[1m]` suffix.
pub fn anthropic_context_mode(model: &str) -> AnthropicContextMode {
    let base = anthropic_strip_1m_suffix(model.trim()).to_ascii_lowercase();

    // Native 1M (default, no opt-in): Opus 5, Opus 4.8 and 4.7, Sonnet 5, Fable 5.
    // Sonnet 5 supports the 1M window by default (1M is both the default and
    // the maximum; there is no smaller context variant).
    if base.starts_with("claude-opus-5")
        || base.starts_with("claude-opus-4-8")
        || base.starts_with("claude-opus-4.8")
        || base.starts_with("claude-opus-4-7")
        || base.starts_with("claude-opus-4.7")
        || base.starts_with("claude-sonnet-5")
        || base.starts_with("claude-fable-5")
    {
        return AnthropicContextMode::Native1M;
    }

    // Opt-in 1M via the context-1m beta: Opus 4.6 and Sonnet 4.6.
    if base.starts_with("claude-opus-4-6")
        || base.starts_with("claude-opus-4.6")
        || base.starts_with("claude-sonnet-4-6")
        || base.starts_with("claude-sonnet-4.6")
    {
        return AnthropicContextMode::OptIn1M;
    }

    AnthropicContextMode::Standard
}

/// Check if a model name explicitly requests 1M context via suffix
/// (for example `claude-opus-4-6[1m]`).
pub fn anthropic_is_1m_model(model: &str) -> bool {
    model.ends_with("[1m]")
}

/// Maximum output tokens Anthropic's synchronous Messages API accepts for a
/// model, per the published model comparison table.
///
/// This matters more than it looks. Adaptive-thinking models spend their output
/// budget on thinking *and* the visible tool call, so a budget that is too
/// small truncates mid-tool-call and silently ends an agent turn. jcode used a
/// flat 32K default for every Claude model, which cut long agentic turns on
/// models that actually allow 128K.
pub fn anthropic_max_output_tokens(model: &str) -> u32 {
    let base = anthropic_strip_1m_suffix(model.trim()).to_ascii_lowercase();

    // Opus 5, Opus 4.6-4.8, Sonnet 5, Sonnet 4.6, and Fable/Mythos 5 all
    // advertise 128K max output on the synchronous Messages API.
    const LARGE_OUTPUT_PREFIXES: &[&str] = &[
        "claude-opus-5",
        "claude-opus-4-8",
        "claude-opus-4.8",
        "claude-opus-4-7",
        "claude-opus-4.7",
        "claude-opus-4-6",
        "claude-opus-4.6",
        "claude-sonnet-5",
        "claude-sonnet-4-6",
        "claude-sonnet-4.6",
        "claude-fable-5",
        "claude-fable",
        "claude-mythos",
    ];
    if LARGE_OUTPUT_PREFIXES
        .iter()
        .any(|prefix| base.starts_with(prefix))
    {
        return 128_000;
    }

    // Haiku 4.5 tops out at 64K.
    if base.starts_with("claude-haiku-4-5") || base.starts_with("claude-haiku-4.5") {
        return 64_000;
    }

    // Older/unknown generations keep the conservative 32K jcode has always used.
    32_768
}

/// Check if a model explicitly requests 1M context via the `[1m]` suffix.
pub fn anthropic_effectively_1m(model: &str) -> bool {
    anthropic_is_1m_model(model)
}

/// Strip the `[1m]` suffix to get the actual API model name.
pub fn anthropic_strip_1m_suffix(model: &str) -> &str {
    crate::model_id::strip_long_context_suffix(model)
}

/// Get the OAuth beta header value appropriate for the model.
pub fn anthropic_oauth_beta_headers(model: &str) -> &'static str {
    if anthropic_is_1m_model(model) {
        ANTHROPIC_OAUTH_BETA_HEADERS_1M
    } else {
        ANTHROPIC_OAUTH_BETA_HEADERS
    }
}

/// How a Claude model exposes reasoning effort and thinking on the live
/// Messages API.
///
/// This is the single source of truth shared by the Anthropic runtime (request
/// building, `set_reasoning_effort` validation) and the TUI effort cycler, so
/// new models cannot drift between the two.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AnthropicReasoningCaps {
    /// Accepts `output_config: {effort}`.
    pub output_effort: bool,
    /// Accepts `thinking: {type: adaptive}`.
    pub adaptive_thinking: bool,
    /// Needs `thinking: {type: enabled, budget_tokens}` (manual budgets).
    pub manual_thinking: bool,
    /// Accepts the `xhigh` effort level.
    pub xhigh_effort: bool,
    /// Accepts the `max` effort level.
    pub max_effort: bool,
}

impl AnthropicReasoningCaps {
    /// Full modern ladder: `output_config` effort low..xhigh/max + adaptive thinking.
    const FULL: Self = Self {
        output_effort: true,
        adaptive_thinking: true,
        manual_thinking: false,
        xhigh_effort: true,
        max_effort: true,
    };
    /// `output_config` effort + adaptive thinking, but no `xhigh` level.
    const EFFORT_NO_XHIGH: Self = Self {
        output_effort: true,
        adaptive_thinking: true,
        manual_thinking: false,
        xhigh_effort: false,
        max_effort: true,
    };
    /// `output_config` effort with manual thinking budgets (Opus 4.5).
    const MANUAL_WITH_EFFORT: Self = Self {
        output_effort: true,
        adaptive_thinking: false,
        manual_thinking: true,
        xhigh_effort: false,
        max_effort: false,
    };
    /// Manual thinking budgets only (Claude 3.7 Sonnet).
    const MANUAL_ONLY: Self = Self {
        output_effort: false,
        adaptive_thinking: false,
        manual_thinking: true,
        xhigh_effort: false,
        max_effort: false,
    };
    const NONE: Self = Self {
        output_effort: false,
        adaptive_thinking: false,
        manual_thinking: false,
        xhigh_effort: false,
        max_effort: false,
    };

    /// Whether any reasoning-effort control is available at all.
    pub fn supports_reasoning_effort(self) -> bool {
        self.output_effort || self.manual_thinking
    }
}

/// Normalize a Claude id for capability matching: lowercase, `[1m]` and
/// `-YYYYMMDD` date suffixes stripped, dotted versions (`4.6`) dashed (`4-6`).
fn normalized_claude_caps_key(model: &str) -> String {
    let base = anthropic_strip_1m_suffix(model.trim())
        .to_ascii_lowercase()
        .replace('.', "-");
    crate::model_id::strip_date_suffix(&base).to_string()
}

/// Parse `(family, version)` from a normalized Claude id. Handles both
/// version-last (`claude-sonnet-4-6`) and version-first (`claude-3-7-sonnet`)
/// forms. A single version number means `.0` (`claude-sonnet-5` -> 5.0).
fn parse_claude_family_version(base: &str) -> (Option<&str>, Option<(u32, u32)>) {
    let mut family = None;
    let mut nums: Vec<u32> = Vec::new();
    for segment in base.split('-') {
        if segment == "claude" {
            continue;
        }
        if let Ok(num) = segment.parse::<u32>() {
            if nums.len() < 2 {
                nums.push(num);
            }
        } else if family.is_none() && segment.chars().all(|c| c.is_ascii_alphabetic()) {
            family = Some(segment);
        }
    }
    let version = match nums.as_slice() {
        [] => None,
        [major] => Some((*major, 0)),
        [major, minor, ..] => Some((*major, *minor)),
    };
    (family, version)
}

/// Reasoning-effort capabilities for a Claude model.
///
/// Known generations are pinned to what the live API accepts (verified live
/// 2026-07-01 for Fable 5 / Opus 4.x, 2026-07-07 for Sonnet 5). Unknown
/// *future* generations (version 5+ in any family) optimistically default to
/// the full ladder: the Anthropic runtime self-heals by stripping the
/// reasoning fields and retrying if a model rejects them, so optimism degrades
/// gracefully while pessimism silently disables effort until someone probes
/// the model and updates a table.
pub fn anthropic_reasoning_caps(model: &str) -> AnthropicReasoningCaps {
    let base = normalized_claude_caps_key(model);
    if !base.starts_with("claude") {
        return AnthropicReasoningCaps::NONE;
    }
    if base.contains("mythos") {
        return AnthropicReasoningCaps::EFFORT_NO_XHIGH;
    }
    let (family, version) = parse_claude_family_version(&base);
    let Some(version) = version else {
        return AnthropicReasoningCaps::NONE;
    };
    match family {
        Some("opus") => {
            if version >= (4, 7) {
                AnthropicReasoningCaps::FULL
            } else if version == (4, 6) {
                AnthropicReasoningCaps::EFFORT_NO_XHIGH
            } else if version == (4, 5) {
                AnthropicReasoningCaps::MANUAL_WITH_EFFORT
            } else {
                AnthropicReasoningCaps::NONE
            }
        }
        Some("sonnet") => {
            if version >= (5, 0) {
                AnthropicReasoningCaps::FULL
            } else if version == (4, 6) {
                AnthropicReasoningCaps::EFFORT_NO_XHIGH
            } else if version == (3, 7) {
                AnthropicReasoningCaps::MANUAL_ONLY
            } else {
                AnthropicReasoningCaps::NONE
            }
        }
        // Optimistic default for new generations (Fable 5, Haiku 5, future
        // families): assume the modern full ladder from version 5 on.
        _ => {
            if version >= (5, 0) {
                AnthropicReasoningCaps::FULL
            } else {
                AnthropicReasoningCaps::NONE
            }
        }
    }
}

pub fn anthropic_map_tool_name_for_oauth(name: &str) -> String {
    match name {
        "bash" => "Bash",
        "read" => "Read",
        "write" => "Write",
        "edit" => "Edit",
        "glob" => "Glob",
        "grep" => "Grep",
        "subagent" => "Agent",
        "schedule" => "ScheduleWakeup",
        "skill_manage" => "Skill",
        _ => name,
    }
    .to_string()
}

pub fn anthropic_map_tool_name_from_oauth(name: &str) -> String {
    match name {
        "Bash" => "bash",
        "Read" => "read",
        "Write" => "write",
        "Edit" => "edit",
        "Glob" => "glob",
        "Grep" => "grep",
        "Agent" => "subagent",
        "ScheduleWakeup" => "schedule",
        "Skill" => "skill_manage",
        _ => name,
    }
    .to_string()
}

pub fn anthropic_stainless_arch() -> &'static str {
    match std::env::consts::ARCH {
        "x86_64" => "x64",
        "aarch64" => "arm64",
        other => other,
    }
}

pub fn anthropic_stainless_os() -> &'static str {
    match std::env::consts::OS {
        "linux" => "Linux",
        "macos" => "MacOS",
        "windows" => "Windows",
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ALL_CLAUDE_MODELS;

    #[test]
    fn model_suffix_helpers_require_explicit_1m_suffix() {
        assert!(!anthropic_effectively_1m("claude-opus-4-6"));
        assert!(anthropic_effectively_1m("claude-opus-4-6[1m]"));
        assert_eq!(
            anthropic_strip_1m_suffix("claude-opus-4-6[1m]"),
            "claude-opus-4-6"
        );
    }

    #[test]
    fn oauth_beta_headers_follow_1m_suffix() {
        assert_eq!(
            anthropic_oauth_beta_headers("claude-opus-4-6"),
            ANTHROPIC_OAUTH_BETA_HEADERS
        );
        assert_eq!(
            anthropic_oauth_beta_headers("claude-opus-4-6[1m]"),
            ANTHROPIC_OAUTH_BETA_HEADERS_1M
        );
    }

    #[test]
    fn oauth_tool_name_mapping_is_reversible_for_known_tools() {
        for (local, oauth) in [
            ("bash", "Bash"),
            ("read", "Read"),
            ("subagent", "Agent"),
            ("schedule", "ScheduleWakeup"),
            ("skill_manage", "Skill"),
        ] {
            assert_eq!(anthropic_map_tool_name_for_oauth(local), oauth);
            assert_eq!(anthropic_map_tool_name_from_oauth(oauth), local);
        }
        assert_eq!(anthropic_map_tool_name_for_oauth("custom"), "custom");
    }

    #[test]
    fn stainless_labels_are_non_empty() {
        assert!(!anthropic_stainless_arch().is_empty());
        assert!(!anthropic_stainless_os().is_empty());
    }

    #[test]
    fn reasoning_caps_match_live_verified_generations() {
        // Full ladder: Fable 5 (live 2026-07-01), Sonnet 5 (live 2026-07-07),
        // Opus 5 (live 2026-07-24), Opus 4.7/4.8.
        for model in [
            "claude-fable-5",
            "claude-sonnet-5",
            "claude-opus-5",
            "claude-opus-4-8",
            "claude-opus-4-7",
        ] {
            let caps = anthropic_reasoning_caps(model);
            assert!(caps.output_effort, "{model} should support output effort");
            assert!(caps.adaptive_thinking, "{model} should be adaptive");
            assert!(caps.xhigh_effort, "{model} should support xhigh");
            assert!(caps.max_effort, "{model} should support max");
            assert!(!caps.manual_thinking);
        }

        // Effort without xhigh: Opus/Sonnet 4.6, Mythos.
        for model in ["claude-opus-4-6", "claude-sonnet-4-6", "claude-mythos"] {
            let caps = anthropic_reasoning_caps(model);
            assert!(caps.output_effort, "{model} should support output effort");
            assert!(caps.adaptive_thinking);
            assert!(!caps.xhigh_effort, "{model} has no xhigh");
            assert!(caps.max_effort, "{model} still supports max");
        }

        // Manual thinking generations.
        let opus_4_5 = anthropic_reasoning_caps("claude-opus-4-5");
        assert!(opus_4_5.output_effort && opus_4_5.manual_thinking);
        assert!(!opus_4_5.adaptive_thinking && !opus_4_5.xhigh_effort && !opus_4_5.max_effort);
        let sonnet_3_7 = anthropic_reasoning_caps("claude-3-7-sonnet");
        assert!(sonnet_3_7.manual_thinking && !sonnet_3_7.output_effort);
        assert_eq!(
            anthropic_reasoning_caps("claude-sonnet-3-7"),
            sonnet_3_7,
            "version-first and version-last forms must match"
        );

        // No reasoning-effort support.
        for model in [
            "claude-sonnet-4-5",
            "claude-haiku-4-5",
            "claude-opus-4-1",
            "claude-3-5-haiku",
            "gpt-5.5",
        ] {
            assert!(
                !anthropic_reasoning_caps(model).supports_reasoning_effort(),
                "{model} should not support effort"
            );
        }
    }

    #[test]
    fn max_output_tokens_match_published_model_limits() {
        // 128K-output generations, including dotted and [1m]/dated aliases.
        for model in [
            "claude-opus-5",
            "claude-opus-4-8",
            "claude-opus-4.8",
            "claude-opus-4-7",
            "claude-opus-4-6[1m]",
            "claude-sonnet-5",
            "claude-sonnet-4-6",
            "claude-fable-5",
            "Claude-Opus-5-20260724",
        ] {
            assert_eq!(
                anthropic_max_output_tokens(model),
                128_000,
                "{model} should allow 128K output"
            );
        }

        // Haiku 4.5 is a 64K-output model.
        assert_eq!(anthropic_max_output_tokens("claude-haiku-4-5"), 64_000);
        assert_eq!(
            anthropic_max_output_tokens("claude-haiku-4-5-20251001"),
            64_000
        );

        // Older generations keep the conservative legacy budget.
        for model in [
            "claude-opus-4-5",
            "claude-sonnet-4-5",
            "claude-sonnet-4-20250514",
            "claude-instant",
        ] {
            assert_eq!(
                anthropic_max_output_tokens(model),
                32_768,
                "{model} should keep the conservative default"
            );
        }
    }

    #[test]
    fn max_output_tokens_never_undercut_the_legacy_default() {
        // Regression guard: a per-model budget must never be *smaller* than the
        // flat 32K default jcode shipped before, or turns that used to fit would
        // start truncating.
        for model in ALL_CLAUDE_MODELS {
            assert!(
                anthropic_max_output_tokens(model) >= 32_768,
                "{model} regressed below the legacy 32K output budget"
            );
        }
    }

    #[test]
    fn reasoning_caps_normalize_suffixes_and_dots() {
        let base = anthropic_reasoning_caps("claude-sonnet-5");
        assert_eq!(anthropic_reasoning_caps("claude-sonnet-5[1m]"), base);
        assert_eq!(anthropic_reasoning_caps("claude-sonnet-5-20260701"), base);
        assert_eq!(anthropic_reasoning_caps("Claude-Sonnet-5"), base);
        assert_eq!(
            anthropic_reasoning_caps("claude-opus-4.6"),
            anthropic_reasoning_caps("claude-opus-4-6")
        );
    }

    #[test]
    fn reasoning_caps_are_optimistic_for_future_generations() {
        // New 5.x+ models default to the full ladder (the runtime self-heals
        // on 400 by stripping reasoning fields), instead of silently
        // disabling effort until someone probes them.
        for model in [
            "claude-sonnet-5-1",
            "claude-sonnet-6",
            "claude-opus-5",
            "claude-haiku-5",
            "claude-fable-6",
            "claude-nova-5",
        ] {
            let caps = anthropic_reasoning_caps(model);
            assert_eq!(
                caps,
                anthropic_reasoning_caps("claude-fable-5"),
                "{model} should default to the full modern ladder"
            );
        }
        // But old/unversioned ids stay conservative.
        assert!(!anthropic_reasoning_caps("claude-haiku-4-5").supports_reasoning_effort());
        assert!(!anthropic_reasoning_caps("claude-instant").supports_reasoning_effort());
    }
}
