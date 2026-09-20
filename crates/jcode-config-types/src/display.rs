//! `[display]` section of the config: TUI/CLI presentation settings.

use crate::{
    DiagramDisplayMode, DiffDisplayMode, LatexRenderingMode, MarkdownSpacingMode,
    NativeScrollbarConfig, OverscrollStatusMode, ReasoningDisplayMode, default_true,
};
use serde::{Deserialize, Serialize};

/// Display/UI configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct DisplayConfig {
    /// How to display file diffs (off/inline/full-inline/pinned/file, default: inline)
    #[serde(deserialize_with = "crate::serde_lenient::lenient_enum")]
    pub diff_mode: DiffDisplayMode,
    /// Legacy: "show_diffs = true/false" maps to diff_mode inline/off
    #[serde(default)]
    pub(crate) show_diffs: Option<bool>,
    /// Queue mode by default - wait until done before sending (default: false)
    pub queue_mode: bool,
    /// Automatically reload the remote server when a newer server binary is detected (default: true)
    pub auto_server_reload: bool,
    /// Capture mouse events (default: true). Enables scroll wheel but disables terminal selection.
    pub mouse_capture: bool,
    /// Enable debug socket for external control (default: false)
    pub debug_socket: bool,
    /// Render emoji in terminal-facing TUI and CLI output (default: true)
    pub emoji: bool,
    /// Center all content (default: false)
    pub centered: bool,
    /// Show thinking/reasoning content by default (default: true)
    pub show_thinking: bool,
    /// How to display reasoning/thinking content (off/full/current).
    /// When unset, falls back to `show_thinking` (true => full, false => off).
    #[serde(
        default,
        deserialize_with = "crate::serde_lenient::lenient_optional_enum"
    )]
    pub(crate) reasoning_display: Option<ReasoningDisplayMode>,
    /// How to display mermaid diagrams (none/margin/pinned, default: none).
    /// `none` still renders diagrams inline in the transcript via the inline
    /// image pipeline; `margin`/`pinned` add dedicated widget placements.
    #[serde(deserialize_with = "crate::serde_lenient::lenient_enum")]
    pub diagram_mode: DiagramDisplayMode,
    /// Markdown block spacing style (compact/document, default: compact)
    #[serde(deserialize_with = "crate::serde_lenient::lenient_enum")]
    pub markdown_spacing: MarkdownSpacingMode,
    /// LaTeX rendering style (none/unicode/image, default: image)
    #[serde(deserialize_with = "crate::serde_lenient::lenient_enum")]
    pub latex_rendering: LatexRenderingMode,
    /// Pin read images to side pane (default: true)
    pub pin_images: bool,
    /// Pin the full session todo list to the top of the chat transcript while
    /// it scrolls, like the sticky previous-prompt preview (default: true)
    #[serde(default = "default_true")]
    pub pin_todos: bool,
    /// Show idle animation before first prompt (default: false)
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
    /// Show up to the last three non-empty bash output lines beneath the tool
    /// summary (default: false).
    #[serde(default)]
    pub show_bash_output: bool,
    /// Show the dimmed technical detail (command, path, args) after the
    /// model-provided intent on tool rows (default: false). When off, rows
    /// that have an intent show only the intent; rows without an intent
    /// always fall back to the technical detail.
    #[serde(default)]
    pub tool_call_details: bool,
    /// Native terminal scrollbar configuration for scrollable panes
    pub native_scrollbars: NativeScrollbarConfig,
    /// Surface occasional "learn this keybinding" nudges when the user keeps
    /// performing an action the slow way (slash command) instead of using its
    /// configured shortcut (default: true). Set false to disable all such hints.
    #[serde(default = "default_true")]
    pub keybinding_hints: bool,
    /// Color theme: "auto" (detect terminal background), "dark", or "light".
    /// Auto queries the terminal's background color (OSC 11) at startup and
    /// adapts jcode's palette for light backgrounds. Default: auto.
    #[serde(default)]
    pub theme: String,
    /// Per-role color overrides, e.g. `user = "#8ab4f8"`. Any TUI color can be
    /// configured: the named roles are substituted directly, and ad hoc shades
    /// used by widgets follow the role they belong to. Run `/colors` to list
    /// roles and `/colors harmony` to score the result.
    #[serde(default)]
    pub colors: std::collections::BTreeMap<String, String>,
    /// Opt-in active sessions manager: pressing Left arrow on an empty input
    /// opens a picker scoped to live (open) sessions, showing which are still
    /// working and which are ready for input (default: false). The `/active`
    /// command works regardless of this setting.
    #[serde(default)]
    pub active_sessions_manager: bool,
    /// Include transcripts discovered from other agent CLIs (Claude Code,
    /// Codex, Pi, OpenCode, Cursor) in the session picker so they can be
    /// resumed or imported (default: true). Set false to show only jcode's own
    /// sessions (issue #674).
    #[serde(default = "default_true")]
    pub external_sessions: bool,
    /// Usage percentage wording: "left" (default), "used", or "elapsed".
    ///
    /// `elapsed` renders the consumed quota next to how much of the rolling
    /// window has already elapsed (`43%/91%`) instead of a reset countdown, so
    /// the two numbers can be read against each other: burning quota faster
    /// than the clock shows up as the first number outrunning the second.
    pub usage_display: String,
    /// When to show the overscroll status line below the input
    /// (off/on/overscroll, default: overscroll). "overscroll" is the elastic
    /// reveal when scrolling past the bottom, "on" keeps it always visible.
    #[serde(default, deserialize_with = "crate::serde_lenient::lenient_enum")]
    pub overscroll_status: OverscrollStatusMode,
}
impl Default for DisplayConfig {
    fn default() -> Self {
        Self {
            diff_mode: DiffDisplayMode::default(),
            show_diffs: None,
            pin_images: true,
            pin_todos: true,
            queue_mode: false,
            auto_server_reload: true,
            mouse_capture: true,
            debug_socket: false,
            emoji: true,
            centered: false,
            show_thinking: true,
            reasoning_display: Some(ReasoningDisplayMode::Full),
            diagram_mode: DiagramDisplayMode::default(),
            markdown_spacing: MarkdownSpacingMode::default(),
            latex_rendering: LatexRenderingMode::default(),
            idle_animation: false,
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
            show_bash_output: false,
            tool_call_details: false,
            native_scrollbars: NativeScrollbarConfig::default(),
            keybinding_hints: true,
            theme: String::new(),
            colors: std::collections::BTreeMap::new(),
            active_sessions_manager: false,
            external_sessions: true,
            usage_display: "left".to_string(),
            overscroll_status: OverscrollStatusMode::default(),
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

    /// Whether the user explicitly chose a reasoning display mode, as opposed
    /// to inheriting the legacy `show_thinking` fallback. Front-ends use this
    /// to apply their own default without overriding a deliberate choice.
    pub fn has_explicit_reasoning_display(&self) -> bool {
        self.reasoning_display.is_some()
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

    pub fn usage_display_used(&self) -> bool {
        self.usage_display.eq_ignore_ascii_case("used")
            || self.usage_display.eq_ignore_ascii_case("elapsed")
    }

    /// Whether the reset countdown should be replaced by the window's elapsed
    /// percentage, making consumed-vs-elapsed directly comparable.
    pub fn usage_display_elapsed(&self) -> bool {
        self.usage_display.eq_ignore_ascii_case("elapsed")
    }
}

#[cfg(test)]
mod tests {
    use super::DisplayConfig;
    use crate::ReasoningDisplayMode;

    #[test]
    fn thinking_is_shown_in_full_by_default() {
        let default = DisplayConfig::default();
        assert!(default.show_thinking);
        assert_eq!(default.reasoning_display(), ReasoningDisplayMode::Full);

        let missing: DisplayConfig = serde_json::from_str("{}").expect("display config");
        assert!(missing.show_thinking);
        assert_eq!(missing.reasoning_display(), ReasoningDisplayMode::Full);
    }

    #[test]
    fn todos_are_pinned_by_default_but_can_be_disabled() {
        assert!(DisplayConfig::default().pin_todos);

        let missing: DisplayConfig = serde_json::from_str("{}").expect("display config");
        assert!(missing.pin_todos);

        let disabled: DisplayConfig =
            serde_json::from_str(r#"{"pin_todos":false}"#).expect("display config");
        assert!(!disabled.pin_todos);
    }

    #[test]
    fn usage_percentage_wording_defaults_to_left_and_accepts_used() {
        assert_eq!(DisplayConfig::default().usage_display, "left");

        let used: DisplayConfig =
            serde_json::from_str(r#"{"usage_display":"used"}"#).expect("display config");
        assert!(used.usage_display_used());
    }

    #[test]
    fn usage_percentage_wording_accepts_elapsed_and_implies_used_wording() {
        let elapsed: DisplayConfig =
            serde_json::from_str(r#"{"usage_display":"elapsed"}"#).expect("display config");

        assert!(elapsed.usage_display_elapsed());
        // Elapsed pairs "consumed" against "elapsed", so the quota number must
        // read as consumed rather than remaining.
        assert!(elapsed.usage_display_used());

        let used: DisplayConfig =
            serde_json::from_str(r#"{"usage_display":"used"}"#).expect("display config");
        assert!(!used.usage_display_elapsed());
        assert!(!DisplayConfig::default().usage_display_elapsed());
    }
}
