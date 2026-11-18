//! Bounded, passive desktop metadata. Never contains pixels or input events.
use serde::{Deserialize, Serialize};

pub const SCREEN_MAX_BYTES: usize = 16 * 1024;
pub const SCREEN_MAX_WINDOWS: usize = 32;
pub const SCREEN_MAX_OUTPUTS: usize = 16;
pub const SCREEN_LEASE_MS: u64 = 30_000;
pub const SCREEN_MIN_UPDATE_MS: u64 = 2_000;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum ScreenContextAction {
    Enable,
    Update { snapshot: ScreenSnapshot },
    Disable,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ScreenInteraction {
    Unknown,
    Active,
    Idle,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ScreenSnapshot {
    pub sequence: u64,
    pub observed_at_ms: u64,
    pub source: String,
    pub interaction: ScreenInteraction,
    pub focused_window: Option<u64>,
    pub windows: Vec<ScreenWindow>,
    pub outputs: Vec<ScreenOutput>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ScreenWindow {
    pub id: u64,
    pub app_id: Option<String>,
    pub title: Option<String>,
    pub workspace_id: Option<u64>,
    /// None means the compositor cannot establish visibility.
    pub visible: Option<bool>,
    /// Niri layout metadata, not a global screen rectangle.
    pub layout: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ScreenOutput {
    pub name: String,
    pub logical: Option<serde_json::Value>,
    pub scale: Option<f64>,
    pub transform: Option<String>,
}

impl ScreenSnapshot {
    /// Reject rather than silently truncating metadata at the trust boundary.
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.source != "niri" || self.sequence == 0 {
            return Err("Unsupported screen source or zero sequence");
        }
        if self.interaction != ScreenInteraction::Unknown {
            return Err("Niri window metadata cannot establish activity");
        }
        if self.windows.len() > SCREEN_MAX_WINDOWS || self.outputs.len() > SCREEN_MAX_OUTPUTS {
            return Err("Too many screen windows or outputs");
        }
        let bounded =
            |text: &str, max: usize| text.len() <= max && !text.chars().any(char::is_control);
        let mut ids = std::collections::HashSet::new();
        for window in &self.windows {
            if !ids.insert(window.id)
                || window.title.as_ref().is_some_and(|s| !bounded(s, 512))
                || window.app_id.as_ref().is_some_and(|s| !bounded(s, 128))
                || window.layout.as_ref().is_some_and(|v| !v.is_object())
            {
                return Err("Invalid window metadata");
            }
        }
        if self.focused_window.is_some_and(|id| !ids.contains(&id)) {
            return Err("Focused window absent from bounded snapshot");
        }
        for output in &self.outputs {
            if !bounded(&output.name, 128)
                || output.transform.as_ref().is_some_and(|s| !bounded(s, 64))
                || output.scale.is_some_and(|s| !s.is_finite() || s <= 0.0)
                || output.logical.as_ref().is_some_and(|v| !v.is_object())
            {
                return Err("Invalid output metadata");
            }
        }
        if serde_json::to_vec(self)
            .map_err(|_| "Invalid screen JSON")?
            .len()
            > SCREEN_MAX_BYTES
        {
            return Err("Screen snapshot exceeds 16 KiB");
        }
        Ok(())
    }
}
