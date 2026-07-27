use crate::{desktop_log, desktop_session_events::BACKEND_EVENT_FORWARD_MAX_RAW_EVENTS};
use std::ffi::OsString;
use std::time::{Duration, Instant};

pub(crate) fn stream_e2e_benchmark_raw_events(args: &[String]) -> Option<usize> {
    args.iter().enumerate().find_map(|(index, arg)| {
        arg.strip_prefix("--stream-e2e-benchmark=")
            .and_then(|value| value.parse::<usize>().ok())
            .or_else(|| {
                (arg == "--stream-e2e-benchmark").then(|| {
                    args.get(index + 1)
                        .and_then(|value| value.parse::<usize>().ok())
                        .unwrap_or(BACKEND_EVENT_FORWARD_MAX_RAW_EVENTS * 6)
                })
            })
    })
}

pub(crate) fn env_flag_enabled(value: OsString) -> bool {
    let value = value.to_string_lossy();
    env_flag_text_enabled(&value)
}

fn env_flag_text_enabled(value: &str) -> bool {
    !matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "" | "0" | "false" | "off" | "no"
    )
}

pub(crate) fn desktop_frame_profile_mode() -> Option<String> {
    std::env::var("JCODE_DESKTOP_FRAME_PROFILE").ok()
}

pub(crate) fn parse_positive_duration_millis(value: &str) -> Option<Duration> {
    value
        .trim()
        .parse::<f64>()
        .ok()
        .filter(|value| value.is_finite() && *value > 0.0)
        .map(|ms| Duration::from_secs_f64(ms / 1000.0))
}

pub(crate) fn duration_millis_env(name: &str, default: Duration) -> Duration {
    std::env::var(name)
        .ok()
        .and_then(|value| parse_positive_duration_millis(&value))
        .unwrap_or(default)
}

pub(crate) fn desktop_frame_profile_enabled(mode: Option<&str>) -> bool {
    mode.is_some_and(env_flag_text_enabled)
}

pub(crate) fn desktop_frame_profile_log_all(mode: Option<&str>) -> bool {
    mode.is_some_and(|value| matches!(value.trim().to_ascii_lowercase().as_str(), "all" | "trace"))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum DesktopPlatform {
    Linux,
    Macos,
    Windows,
    Other,
}

fn current_desktop_platform() -> DesktopPlatform {
    if cfg!(target_os = "linux") {
        DesktopPlatform::Linux
    } else if cfg!(target_os = "macos") {
        DesktopPlatform::Macos
    } else if cfg!(windows) {
        DesktopPlatform::Windows
    } else {
        DesktopPlatform::Other
    }
}

pub(crate) fn desktop_platform_support_warning(platform: DesktopPlatform) -> Option<&'static str> {
    match platform {
        DesktopPlatform::Linux | DesktopPlatform::Macos => None,
        DesktopPlatform::Windows => Some(
            "Windows desktop support is experimental; terminal spawning, power inhibit, and GPU backend behavior may differ from Linux/macOS",
        ),
        DesktopPlatform::Other => Some(
            "this platform is not officially supported by jcode-desktop; startup will continue on a best-effort GPU backend",
        ),
    }
}

pub(crate) fn log_desktop_platform_support_warning() {
    if let Some(warning) = desktop_platform_support_warning(current_desktop_platform()) {
        desktop_log::warn(format_args!("jcode-desktop: {warning}"));
    }
}

#[derive(Clone, Copy)]
pub(crate) struct DesktopStartupTrace {
    started_at: Instant,
    enabled: bool,
}

impl DesktopStartupTrace {
    pub(crate) fn new(enabled: bool) -> Self {
        Self {
            started_at: Instant::now(),
            enabled,
        }
    }

    pub(crate) fn mark(&self, milestone: &str) {
        if self.enabled {
            eprintln!(
                "jcode-desktop startup +{:>7.2} ms  {milestone}",
                self.started_at.elapsed().as_secs_f64() * 1000.0
            );
            desktop_log::info(format_args!(
                "jcode-desktop: startup +{:>7.2} ms {milestone}",
                self.started_at.elapsed().as_secs_f64() * 1000.0
            ));
        }
    }
}

/// Default reasoning display mode the desktop requests from the server it
/// spawns. `current` shows live thinking and collapses it once a tool runs or
/// the answer commits, which matches how the desktop transcript renders.
pub(crate) const DESKTOP_DEFAULT_REASONING_DISPLAY: &str = "current";

/// Report the effective thinking-display mode for the desktop.
///
/// The desktop cannot depend on the config crates (dependency-boundary gate),
/// so this mirrors the server's precedence over the same inputs: an explicit
/// `display.reasoning_display` in config wins, then the `JCODE_REASONING_DISPLAY`
/// override, then the legacy `show_thinking` boolean, then the desktop default.
pub(crate) fn desktop_reasoning_display_mode() -> String {
    resolve_reasoning_display_mode(
        std::env::var("JCODE_REASONING_DISPLAY").ok().as_deref(),
        read_jcode_config_text().as_deref(),
    )
}

/// Pure precedence resolver, kept separate from IO so it can be tested without
/// mutating process-global environment state.
fn resolve_reasoning_display_mode(env_override: Option<&str>, config_text: Option<&str>) -> String {
    if let Some(mode) = env_override.and_then(normalized_reasoning_display_mode) {
        return mode.to_string();
    }
    config_text
        .and_then(configured_reasoning_display)
        .unwrap_or_else(|| DESKTOP_DEFAULT_REASONING_DISPLAY.to_string())
}

fn read_jcode_config_text() -> Option<String> {
    std::fs::read_to_string(jcode_home_dir()?.join("config.toml")).ok()
}

fn configured_reasoning_display(raw: &str) -> Option<String> {
    let mut explicit_mode = None;
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
            "reasoning_display" => {
                explicit_mode = normalized_reasoning_display_mode(value).map(str::to_string);
            }
            "show_thinking" => show_thinking = value.parse::<bool>().ok(),
            _ => {}
        }
    }
    // An explicit mode always wins; `show_thinking` is only the legacy fallback.
    explicit_mode
        .or_else(|| show_thinking.map(|enabled| if enabled { "full" } else { "off" }.to_string()))
}

fn normalized_reasoning_display_mode(value: &str) -> Option<&'static str> {
    match value.trim().to_ascii_lowercase().as_str() {
        "off" | "none" | "false" | "0" | "no" => Some("off"),
        "full" | "all" | "true" | "1" | "yes" | "on" => Some("full"),
        "current" | "live" | "ephemeral" | "collapse" => Some("current"),
        _ => None,
    }
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
mod reasoning_display_tests {
    use super::*;

    #[test]
    fn desktop_defaults_to_live_thinking_when_config_says_nothing() {
        assert_eq!(resolve_reasoning_display_mode(None, None), "current");
        assert_eq!(
            resolve_reasoning_display_mode(None, Some("[display]\nemoji = true\n")),
            "current"
        );
    }

    #[test]
    fn explicit_config_choice_beats_the_desktop_default() {
        assert_eq!(
            resolve_reasoning_display_mode(None, Some("reasoning_display = \"off\"\n")),
            "off"
        );
        assert_eq!(
            resolve_reasoning_display_mode(None, Some("reasoning_display = \"full\"\n")),
            "full"
        );
    }

    #[test]
    fn legacy_show_thinking_is_only_a_fallback() {
        // The shipped default config sets `show_thinking = false`, which is why
        // the desktop saw no reasoning at all before this change.
        assert_eq!(
            resolve_reasoning_display_mode(None, Some("show_thinking = false\n")),
            "off"
        );
        assert_eq!(
            resolve_reasoning_display_mode(
                None,
                Some("show_thinking = false\nreasoning_display = \"current\"\n"),
            ),
            "current",
            "an explicit mode must win over the legacy boolean"
        );
    }

    #[test]
    fn env_override_wins_and_comments_are_ignored() {
        assert_eq!(
            resolve_reasoning_display_mode(Some("full"), Some("reasoning_display = \"off\"\n")),
            "full"
        );
        assert_eq!(
            resolve_reasoning_display_mode(Some("bogus"), Some("reasoning_display = \"off\"\n")),
            "off",
            "an unparseable override should fall through, not reset to the default"
        );
        assert_eq!(
            resolve_reasoning_display_mode(None, Some("# reasoning_display = \"off\"\n")),
            "current",
            "commented-out settings must not be read as explicit choices"
        );
    }
}
