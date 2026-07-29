//! How much of the model's thinking the transcript keeps.
//!
//! The daemon always streams reasoning (whether a client renders it is a
//! presentation choice), so this is desktop2's side of that contract. Three
//! modes, matching the TUI and desktop1 so one config key means one thing
//! everywhere:
//!
//! - `off`: thinking never lands in the transcript.
//! - `full`: every thought is kept, in order (classic behavior).
//! - `current`: only the thinking happening *right now* is on screen, and only
//!   its current paragraph. A committed answer or a tool call ends the thought
//!   and it leaves the page. This is what makes a long turn readable: the
//!   transcript is the work, not a wall of superseded thinking.
//!
//! Precedence mirrors the server's over the same inputs, because desktop2
//! cannot depend on the config crates (dependency-boundary gate):
//! `JCODE_REASONING_DISPLAY`, then `display.reasoning_display` in
//! `~/.jcode/config.toml`, then the legacy `show_thinking` boolean, then the
//! desktop default.

/// What the transcript does with streamed reasoning.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ReasoningMode {
    Off,
    Full,
    /// Show only the live thought, trimmed to its current paragraph.
    #[default]
    Current,
}

impl ReasoningMode {
    pub fn label(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Full => "full",
            Self::Current => "current",
        }
    }

    /// The same spellings the config crate accepts, so a value that works in
    /// the TUI is not silently ignored here.
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "off" | "none" | "false" | "0" | "no" => Some(Self::Off),
            "full" | "all" | "true" | "1" | "yes" | "on" => Some(Self::Full),
            "current" | "live" | "ephemeral" | "collapse" => Some(Self::Current),
            _ => None,
        }
    }

    /// Cycle order for the runtime toggle: the useful pair (`current` and
    /// `full`) is one keypress apart, and `off` is reachable from either.
    pub fn cycle(self) -> Self {
        match self {
            Self::Off => Self::Current,
            Self::Current => Self::Full,
            Self::Full => Self::Off,
        }
    }

    /// The effective mode for this process.
    pub fn from_env() -> Self {
        Self::resolve(
            std::env::var("JCODE_REASONING_DISPLAY").ok().as_deref(),
            read_config_text().as_deref(),
        )
    }

    /// Pure precedence resolver, kept apart from IO so it is testable without
    /// mutating process-global environment state.
    pub fn resolve(env_override: Option<&str>, config_text: Option<&str>) -> Self {
        if let Some(mode) = env_override.and_then(Self::parse) {
            return mode;
        }
        config_text
            .and_then(configured_mode)
            .unwrap_or(Self::Current)
    }
}

fn read_config_text() -> Option<String> {
    std::fs::read_to_string(jcode_home_dir()?.join("config.toml")).ok()
}

/// Scan `config.toml` for the two keys that decide this. A real TOML parse
/// would mean a dependency for two scalars; the keys are unique enough in the
/// file that a line scan reads them correctly, and an unparsable value simply
/// falls through to the default rather than failing the window's startup.
fn configured_mode(raw: &str) -> Option<ReasoningMode> {
    let mut explicit = None;
    let mut show_thinking = None;
    for line in raw.lines() {
        let line = line.trim();
        if line.starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let value = value
            .split('#')
            .next()
            .unwrap_or_default()
            .trim()
            .trim_matches('"')
            .trim_matches('\'');
        match key.trim() {
            "reasoning_display" => explicit = ReasoningMode::parse(value),
            "show_thinking" => show_thinking = value.parse::<bool>().ok(),
            _ => {}
        }
    }
    // An explicit mode always wins; `show_thinking` is only the legacy fallback.
    explicit.or_else(|| {
        show_thinking.map(|on| {
            if on {
                ReasoningMode::Full
            } else {
                ReasoningMode::Off
            }
        })
    })
}

fn jcode_home_dir() -> Option<std::path::PathBuf> {
    match std::env::var_os("JCODE_HOME") {
        Some(path) => Some(std::path::PathBuf::from(path)),
        None => std::env::var_os("HOME")
            .map(std::path::PathBuf::from)
            .map(|home| home.join(".jcode")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_is_live_thinking() {
        assert_eq!(ReasoningMode::resolve(None, None), ReasoningMode::Current);
        assert_eq!(
            ReasoningMode::resolve(None, Some("[display]\nemoji = true\n")),
            ReasoningMode::Current
        );
    }

    #[test]
    fn an_explicit_config_mode_is_honored() {
        assert_eq!(
            ReasoningMode::resolve(None, Some("[display]\nreasoning_display = \"full\"\n")),
            ReasoningMode::Full
        );
        assert_eq!(
            ReasoningMode::resolve(None, Some("[display]\nreasoning_display = \"off\"\n")),
            ReasoningMode::Off
        );
    }

    /// The env override is how a single window opts out without editing the
    /// config every other session.
    #[test]
    fn the_env_override_beats_config() {
        assert_eq!(
            ReasoningMode::resolve(
                Some("current"),
                Some("[display]\nreasoning_display = \"full\"\n")
            ),
            ReasoningMode::Current
        );
    }

    #[test]
    fn the_legacy_boolean_is_only_a_fallback() {
        assert_eq!(
            ReasoningMode::resolve(None, Some("[display]\nshow_thinking = false\n")),
            ReasoningMode::Off
        );
        assert_eq!(
            ReasoningMode::resolve(
                None,
                Some("[display]\nshow_thinking = false\nreasoning_display = \"current\"\n")
            ),
            ReasoningMode::Current
        );
    }

    #[test]
    fn the_cycle_visits_every_mode() {
        let mut mode = ReasoningMode::Current;
        let mut seen = vec![mode];
        for _ in 0..2 {
            mode = mode.cycle();
            seen.push(mode);
        }
        assert_eq!(
            seen,
            vec![
                ReasoningMode::Current,
                ReasoningMode::Full,
                ReasoningMode::Off
            ]
        );
        assert_eq!(mode.cycle(), ReasoningMode::Current);
    }
}
