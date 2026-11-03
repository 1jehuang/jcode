//! Granular telemetry consent. Each category can be switched off on its own.
//!
//! The master `no_telemetry` marker (and the `JCODE_NO_TELEMETRY` /
//! `DO_NOT_TRACK` env vars) still disable everything. On top of that, each
//! non-content category has its own opt-out marker, and the content category
//! (prompts and transcripts) keeps its separate opt-in marker.

use super::{content_sharing_enabled, is_enabled, logging, set_content_sharing_enabled, storage};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TelemetryCategory {
    /// Prompt text and full conversation transcripts. Opt-in.
    Content,
    /// Session start/end, active days, installs, updates, turn metadata.
    Usage,
    /// Crash reports.
    Crashes,
    /// Feature usage: onboarding steps, discovery, todo stats, feedback.
    Features,
    /// Token and cost usage reports.
    Cost,
    /// Connected provider accounts (provider plus account email).
    Accounts,
}

impl TelemetryCategory {
    /// On-screen order. Content sits first.
    pub const ALL: [TelemetryCategory; 6] = [
        TelemetryCategory::Content,
        TelemetryCategory::Usage,
        TelemetryCategory::Crashes,
        TelemetryCategory::Features,
        TelemetryCategory::Cost,
        TelemetryCategory::Accounts,
    ];

    /// Short machine key used by `/telemetry <key> on|off` and marker files.
    pub fn key(self) -> &'static str {
        match self {
            TelemetryCategory::Content => "content",
            TelemetryCategory::Usage => "usage",
            TelemetryCategory::Crashes => "crashes",
            TelemetryCategory::Features => "features",
            TelemetryCategory::Cost => "cost",
            TelemetryCategory::Accounts => "accounts",
        }
    }

    pub fn from_key(key: &str) -> Option<Self> {
        match key {
            "content" | "prompts" | "transcripts" => Some(TelemetryCategory::Content),
            "usage" | "stats" => Some(TelemetryCategory::Usage),
            "crashes" | "crash" | "errors" => Some(TelemetryCategory::Crashes),
            "features" | "feature" => Some(TelemetryCategory::Features),
            "cost" | "tokens" => Some(TelemetryCategory::Cost),
            "accounts" | "account" | "email" => Some(TelemetryCategory::Accounts),
            _ => None,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            TelemetryCategory::Content => "Prompts and transcripts",
            TelemetryCategory::Usage => "Basic usage stats",
            TelemetryCategory::Crashes => "Crash reports",
            TelemetryCategory::Features => "Feature usage",
            TelemetryCategory::Cost => "Token and cost usage",
            TelemetryCategory::Accounts => "Connected accounts",
        }
    }

    pub fn caption(self) -> &'static str {
        match self {
            TelemetryCategory::Content => {
                "Prompts, responses, reasoning, code, and tool input/output"
            }
            TelemetryCategory::Usage => "Sessions, active days, version, installs and updates",
            TelemetryCategory::Crashes => "Lets us see and fix crashes",
            TelemetryCategory::Features => "Onboarding steps, commands and tools used, feedback",
            TelemetryCategory::Cost => "Model, provider, and token counts per request",
            TelemetryCategory::Accounts => "Which providers are connected, with account email",
        }
    }

    fn opt_out_marker(self) -> Option<std::path::PathBuf> {
        storage::jcode_dir()
            .ok()
            .map(|d| d.join(format!("no_telemetry_{}", self.key())))
    }

    /// Whether this category may be sent right now.
    pub fn enabled(self) -> bool {
        if !is_enabled() {
            return false;
        }
        match self {
            TelemetryCategory::Content => content_sharing_enabled(),
            _ => !self.opt_out_marker().is_some_and(|p| p.exists()),
        }
    }

    /// Persist one category, ignoring the master switch.
    fn persist_raw(self, enabled: bool) {
        if self == TelemetryCategory::Content {
            set_content_sharing_enabled(enabled);
            return;
        }
        let Some(path) = self.opt_out_marker() else {
            return;
        };
        let result = if enabled {
            match std::fs::remove_file(&path) {
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
                other => other,
            }
        } else {
            if let Some(parent) = path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            std::fs::write(&path, b"1")
        };
        if let Err(err) = result {
            logging::debug(&format!(
                "failed to persist telemetry category {}: {err}",
                self.key()
            ));
        }
    }

    /// Which category an outgoing payload's `event` belongs to. Unknown events
    /// fall under basic usage.
    pub fn for_event(event: &str) -> Self {
        match event {
            "transcript" => TelemetryCategory::Content,
            "session_crash" => TelemetryCategory::Crashes,
            "usage_report" => TelemetryCategory::Cost,
            "auth_success" | "oauth_account" => TelemetryCategory::Accounts,
            "onboarding_step" | "onboarding_trace" | "discovery" | "todo_session" | "feedback" => {
                TelemetryCategory::Features
            }
            _ => TelemetryCategory::Usage,
        }
    }
}

/// Current per-category state, in [`TelemetryCategory::ALL`] order.
pub fn category_settings() -> [bool; 6] {
    TelemetryCategory::ALL.map(TelemetryCategory::enabled)
}

/// Persist a full set of category choices (in [`TelemetryCategory::ALL`]
/// order). Everything off writes the master opt-out marker.
pub fn set_category_settings(settings: [bool; 6]) {
    if settings.iter().all(|on| !on) {
        for category in TelemetryCategory::ALL {
            category.persist_raw(false);
        }
        super::set_usage_telemetry_enabled(false);
        return;
    }
    super::set_usage_telemetry_enabled(true);
    for (category, on) in TelemetryCategory::ALL.into_iter().zip(settings) {
        category.persist_raw(on);
    }
}

/// Whether a payload may be delivered under the current category choices.
pub(crate) fn payload_allowed(payload: &serde_json::Value) -> bool {
    match payload.get("event").and_then(|v| v.as_str()) {
        // The opt-out notice is the one event sent on the way out.
        Some("telemetry_opt_out") => true,
        Some(event) => TelemetryCategory::for_event(event).enabled(),
        None => TelemetryCategory::Usage.enabled(),
    }
}
