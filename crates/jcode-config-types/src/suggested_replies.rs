use serde::{Deserialize, Serialize};

/// Two-stage cascading Suggested Replies with Default Action Focus configuration.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct SuggestedRepliesConfig {
    /// Enable contextual Suggested Replies in the chat TUI (default: true).
    #[serde(
        default = "default_suggested_replies_enabled",
        deserialize_with = "crate::serde_lenient::lenient_bool_true"
    )]
    pub enabled: bool,
    /// Jev SystemOne provider for Suggested Replies (`auto`, `laya`, `openjev`,
    /// `typesafe`, `openrouter`, `aimlapi`, or `jcode`). Env override:
    /// `JCODE_SUGGESTIONS_JEV_PROVIDER`.
    #[serde(
        default = "default_suggestions_jev_provider",
        alias = "jevProvider",
        alias = "provider"
    )]
    pub jev_provider: String,
    /// Switch between Jev-native fixed macro/micro pool (`false`, default) and
    /// generative OpenGF (`Sidecar`) candidate synthesis (`true`).
    #[serde(
        default,
        alias = "useOpenGf",
        deserialize_with = "crate::serde_lenient::lenient_bool_false"
    )]
    pub use_open_gf: bool,
    /// Maximum number of secondary suggestion badges displayed (clamped `1..=20`, default `3`).
    #[serde(
        default = "default_max_secondary_suggestions",
        alias = "maxSecondarySuggestions",
        alias = "max_suggestions",
        alias = "maxSuggestions",
        alias = "count",
        deserialize_with = "crate::serde_lenient::lenient_max_secondary_suggestions"
    )]
    pub max_secondary_suggestions: usize,
    /// Model confidence threshold (`0.0..=1.0`, default `0.55`) to promote the top
    /// candidate into the prominent Default Action control.
    #[serde(
        default = "default_suggested_replies_confidence_threshold",
        alias = "confidenceThreshold",
        deserialize_with = "crate::serde_lenient::lenient_confidence_threshold"
    )]
    pub confidence_threshold: f32,
    /// Display calibrated model confidence percentage (`(XX%)`) on every suggestion row
    /// (`Default Action` and each secondary suggestion `1..=N`, default `true`).
    #[serde(
        default = "default_suggested_replies_show_confidence",
        alias = "showConfidence",
        deserialize_with = "crate::serde_lenient::lenient_bool_true"
    )]
    pub show_confidence: bool,
    /// User-defined constant list of questions/prompts that are merged into the
    /// candidate pool and scored on every turn alongside dynamic candidates.
    #[serde(
        default,
        alias = "customPrompts",
        alias = "custom_questions",
        alias = "customQuestions",
        alias = "custom_candidates",
        alias = "pinned_prompts",
        deserialize_with = "crate::serde_lenient::lenient_string_list"
    )]
    pub custom_prompts: Vec<String>,
    /// Full-auto Explorer mode (`default: false`). When enabled and a Default Action
    /// reaches `confidence >= confidence_threshold`, automatically triggers a 500ms
    /// cancelable visual delay and dispatches the Default Action.
    #[serde(
        default,
        alias = "enableFullAutoExplorer",
        deserialize_with = "crate::serde_lenient::lenient_bool_false"
    )]
    pub enable_full_auto_explorer: bool,
    /// Automatic 6-lens subagent self-review (`default: false`). When enabled,
    /// spawns a 6-lens read-only review pass after code-changing turns and feeds
    /// any surfaced findings into Suggested Replies Default Action.
    #[serde(
        default,
        alias = "sixLensAutoReview",
        deserialize_with = "crate::serde_lenient::lenient_bool_false"
    )]
    pub six_lens_auto_review: bool,
}

fn default_suggested_replies_enabled() -> bool {
    true
}

fn default_suggestions_jev_provider() -> String {
    "auto".to_string()
}

fn default_max_secondary_suggestions() -> usize {
    3
}

fn default_suggested_replies_confidence_threshold() -> f32 {
    0.55
}

fn default_suggested_replies_show_confidence() -> bool {
    true
}

impl Default for SuggestedRepliesConfig {
    fn default() -> Self {
        Self {
            enabled: default_suggested_replies_enabled(),
            jev_provider: default_suggestions_jev_provider(),
            use_open_gf: false,
            max_secondary_suggestions: default_max_secondary_suggestions(),
            confidence_threshold: default_suggested_replies_confidence_threshold(),
            show_confidence: default_suggested_replies_show_confidence(),
            custom_prompts: Vec::new(),
            enable_full_auto_explorer: false,
            six_lens_auto_review: false,
        }
    }
}

impl SuggestedRepliesConfig {
    pub fn clamped_max_secondary(&self) -> usize {
        self.max_secondary_suggestions.clamp(1, 20)
    }

    pub fn clamped_confidence_threshold(&self) -> f32 {
        if self.confidence_threshold.is_finite() {
            self.confidence_threshold.clamp(0.0, 1.0)
        } else {
            0.55
        }
    }

    pub fn sanitized_custom_prompts(&self) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for item in &self.custom_prompts {
            let trimmed = item.trim();
            if trimmed.is_empty() {
                continue;
            }
            if !out
                .iter()
                .any(|existing| existing.eq_ignore_ascii_case(trimmed))
            {
                out.push(trimmed.to_string());
            }
        }
        out
    }
}
