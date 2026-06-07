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
