use crate::workspace::{DesktopPreferences, PanelSizePreset};
use anyhow::{Context, Result};
use serde_json::{Value, json};
use std::fs;
use std::path::PathBuf;

pub fn load_preferences() -> Result<Option<DesktopPreferences>> {
    let path = preferences_path()?;
    if !path.exists() {
        return Ok(None);
    }

    let raw =
        fs::read_to_string(&path).with_context(|| format!("failed to read {}", path.display()))?;
    let value: Value = serde_json::from_str(&raw)
        .with_context(|| format!("failed to parse {}", path.display()))?;
    Ok(Some(DesktopPreferences {
        panel_size: value
            .get("panel_size")
            .and_then(Value::as_str)
            .and_then(PanelSizePreset::from_storage_key)
            .unwrap_or(PanelSizePreset::Quarter),
        focused_session_id: value
            .get("focused_session_id")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned),
        workspace_lane: value
            .get("workspace_lane")
            .and_then(Value::as_i64)
            .and_then(|lane| i32::try_from(lane).ok())
            .unwrap_or_default(),
    }))
}

pub fn save_preferences(preferences: &DesktopPreferences) -> Result<()> {
    let path = preferences_path()?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }

    let value = json!({
        "panel_size": preferences.panel_size.storage_key(),
        "focused_session_id": preferences.focused_session_id,
        "workspace_lane": preferences.workspace_lane,
    });
    fs::write(&path, serde_json::to_vec_pretty(&value)?)
        .with_context(|| format!("failed to write {}", path.display()))
}

fn preferences_path() -> Result<PathBuf> {
    if let Ok(path) = std::env::var("JCODE_DESKTOP_STATE") {
        return Ok(PathBuf::from(path));
    }

    if let Ok(path) = std::env::var("JCODE_HOME") {
        return Ok(PathBuf::from(path).join("config/jcode/desktop-state.json"));
    }

    if let Ok(path) = std::env::var("XDG_CONFIG_HOME") {
        return Ok(PathBuf::from(path).join("jcode/desktop-state.json"));
    }

    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .context("HOME is not set")?;
    Ok(home.join(".config/jcode/desktop-state.json"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Mutex, OnceLock};

    fn env_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    #[test]
    fn saves_and_loads_preferences() -> Result<()> {
        let Ok(_guard) = env_lock().lock() else {
            anyhow::bail!("desktop prefs test env lock poisoned");
        };
        let dir =
            std::env::temp_dir().join(format!("jcode-desktop-prefs-test-{}", std::process::id()));
        let path = dir.join("state.json");
        unsafe {
            std::env::set_var("JCODE_DESKTOP_STATE", &path);
        }

        let preferences = DesktopPreferences {
            panel_size: PanelSizePreset::Half,
            focused_session_id: Some("session_cow".to_string()),
            workspace_lane: 2,
        };
        save_preferences(&preferences)?;
        assert_eq!(load_preferences()?, Some(preferences));

        unsafe {
            std::env::remove_var("JCODE_DESKTOP_STATE");
        }
        let _ = fs::remove_dir_all(dir);
        Ok(())
    }
}
