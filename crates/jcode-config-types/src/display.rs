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
    /// How to display file diffs (off/inline/full-inline/file, default: inline)
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
    /// Show a compact duration badge (" · 2m 3s") on completed tool rows,
    /// colored by severity (issue #1453). Default: false.
    #[serde(default)]
    pub show_tool_duration: bool,
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
    /// Usage percentage wording: "left" (default) or "used".
    pub usage_display: String,
    /// Show when each completed tool call ran: an opt-in " · HH:MM:SS"
    /// stamp after the token count on tool transcript rows (default: false).
    /// Has no effect unless `show_tool_timestamp` is enabled.
    #[serde(default)]
    pub show_tool_timestamp: bool,
    /// Timezone for UI timestamps (tool row time stamps). "local" (default)
    /// uses the machine's timezone; otherwise a fixed offset like "UTC+3",
    /// "utc-5" or "UTC+05:30". Unknown values fall back to local.
    #[serde(default)]
    pub timestamp_tz: String,
    /// When to show the overscroll status line below the input
    /// (off/on/overscroll, default: on). "overscroll" is the elastic
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
            performance: String::new(),
            animation_fps: 60,
            redraw_fps: 60,
            prompt_preview: true,
            compact_notifications: false,
            copy_badge_alt_label: String::new(),
            show_agentgrep_output: false,
            show_bash_output: false,
            tool_call_details: false,
            show_tool_duration: false,
            native_scrollbars: NativeScrollbarConfig::default(),
            keybinding_hints: true,
            theme: String::new(),
            colors: std::collections::BTreeMap::new(),
            active_sessions_manager: false,
            external_sessions: true,
            usage_display: "left".to_string(),
            show_tool_timestamp: false,
            timestamp_tz: String::new(),
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
    }

    /// Fixed UTC offset (in seconds) for UI timestamps, when the user pinned
    /// one via `display.timestamp_tz = "UTC+3"`. `None` = use local time.
    /// Accepts "UTC+3", "utc-5", "UTC+0", "UTC+05:30" or a bare "3";
    /// anything else falls back to local so a typo never breaks rendering.
    pub fn timestamp_fixed_offset_secs(&self) -> Option<i32> {
        let raw = self.timestamp_tz.trim().to_ascii_lowercase();
        if raw.is_empty() || raw == "local" || raw == "system" {
            return None;
        }
        let body = raw.strip_prefix("utc").map(str::trim).unwrap_or(&raw);
        let body = body.strip_prefix(':').unwrap_or(body);
        let (sign, digits) = match body.strip_prefix('-') {
            Some(rest) => (-1i32, rest),
            None => (1i32, body.strip_prefix('+').unwrap_or(body)),
        };
        let (hours, minutes) = match digits.split_once(':') {
            Some((h, m)) => (h.trim(), m.trim()),
            None => (digits, "0"),
        };
        let hours: i32 = hours.parse().ok()?;
        let minutes: i32 = minutes.parse().ok()?;
        if !(0..=14).contains(&hours) || !(0..=59).contains(&minutes) {
            return None;
        }
        Some(sign * (hours * 3600 + minutes * 60))
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

    /// Issue #1453: the tool duration badge is strictly opt-in. Missing key
    /// means off; explicit true turns it on.
    #[test]
    fn tool_duration_badge_is_opt_in_and_defaults_off() {
        assert!(!DisplayConfig::default().show_tool_duration);

        let missing: DisplayConfig = serde_json::from_str("{}").expect("display config");
        assert!(!missing.show_tool_duration);

        let enabled: DisplayConfig =
            serde_json::from_str(r#"{"show_tool_duration":true}"#).expect("display config");
        assert!(enabled.show_tool_duration);
    }

    /// Issue #1454: the tool row time stamp is strictly opt-in. Missing key
    /// means off; explicit true turns it on.
    #[test]
    fn show_tool_timestamp_is_opt_in() {
        assert!(!DisplayConfig::default().show_tool_timestamp);

        let missing: DisplayConfig = serde_json::from_str("{}").expect("display config");
        assert!(!missing.show_tool_timestamp);

        let enabled: DisplayConfig =
            serde_json::from_str(r#"{"show_tool_timestamp":true}"#).expect("display config");
        assert!(enabled.show_tool_timestamp);
    }

    #[test]
    fn timestamp_tz_resolves_offsets_and_falls_back_to_local() {
        let parse = |value: &str| -> DisplayConfig {
            serde_json::from_str(&format!(r#"{{"timestamp_tz":"{value}"}}"#))
                .expect("display config")
        };

        // Empty/default and explicit local: no fixed offset.
        assert_eq!(DisplayConfig::default().timestamp_fixed_offset_secs(), None);
        assert_eq!(parse("local").timestamp_fixed_offset_secs(), None);
        assert_eq!(parse("system").timestamp_fixed_offset_secs(), None);

        // Offsets in seconds.
        assert_eq!(parse("UTC+3").timestamp_fixed_offset_secs(), Some(3 * 3600));
        assert_eq!(
            parse("utc-5").timestamp_fixed_offset_secs(),
            Some(-5 * 3600)
        );
        assert_eq!(parse("UTC+0").timestamp_fixed_offset_secs(), Some(0));
        assert_eq!(parse(" 3 ").timestamp_fixed_offset_secs(), Some(3 * 3600));
        assert_eq!(
            parse("UTC+05:30").timestamp_fixed_offset_secs(),
            Some(5 * 3600 + 30 * 60)
        );

        // Garbage falls back to local instead of breaking rendering.
        assert_eq!(parse("Moscow").timestamp_fixed_offset_secs(), None);
        assert_eq!(parse("UTC+99").timestamp_fixed_offset_secs(), None);

        // Boundary and malformed-offset handling: +14h is valid, +15h is not,
        // minutes must stay under 60, and sign-only or empty digits fail.
        assert_eq!(
            parse("UTC+14").timestamp_fixed_offset_secs(),
            Some(14 * 3600)
        );
        assert_eq!(
            parse("utc-14").timestamp_fixed_offset_secs(),
            Some(-14 * 3600)
        );
        assert_eq!(parse("UTC+15").timestamp_fixed_offset_secs(), None);
        assert_eq!(parse("UTC+3:60").timestamp_fixed_offset_secs(), None);
        assert_eq!(
            parse("UTC+3:30").timestamp_fixed_offset_secs(),
            Some(3 * 3600 + 30 * 60)
        );
        assert_eq!(parse("UTC-").timestamp_fixed_offset_secs(), None);
        assert_eq!(parse("UTC+abc").timestamp_fixed_offset_secs(), None);
    }
}
