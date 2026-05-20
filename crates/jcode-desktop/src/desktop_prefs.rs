use crate::workspace::{DEFAULT_SPACE_HOLD_TOGGLE_MS, DesktopPreferences, PanelSizePreset};
use anyhow::{Context, Result};
use serde_json::{Value, json};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

static PREFERENCES_TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

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
        space_hold_toggle_ms: value
            .get("space_hold_toggle_ms")
            .and_then(Value::as_u64)
            .unwrap_or(DEFAULT_SPACE_HOLD_TOGGLE_MS),
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
        "space_hold_toggle_ms": preferences.space_hold_toggle_ms,
    });
    let bytes = serde_json::to_vec_pretty(&value)?;
    let temp_path = unique_preferences_temp_path(&path);

    write_preferences_file(&temp_path, &bytes)
        .with_context(|| format!("failed to write {}", temp_path.display()))?;
    if let Err(error) = fs::rename(&temp_path, &path) {
        let _ = fs::remove_file(&temp_path);
        return Err(error).with_context(|| {
            format!(
                "failed to replace {} from {}",
                path.display(),
                temp_path.display()
            )
        });
    }
    if let Some(parent) = path.parent() {
        sync_preferences_parent(parent).with_context(|| {
            format!("failed to sync preferences directory {}", parent.display())
        })?;
    }
    Ok(())
}

fn unique_preferences_temp_path(path: &Path) -> PathBuf {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let file_name = path
        .file_name()
        .and_then(|file_name| file_name.to_str())
        .unwrap_or("desktop-state.json");
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    let counter = PREFERENCES_TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    parent.join(format!(
        ".{file_name}.{}.{}.{}.tmp",
        std::process::id(),
        nonce,
        counter
    ))
}

#[cfg(unix)]
fn sync_preferences_parent(parent: &Path) -> Result<()> {
    fs::File::open(parent)?.sync_all()?;
    Ok(())
}

#[cfg(not(unix))]
fn sync_preferences_parent(_parent: &Path) -> Result<()> {
    Ok(())
}

fn write_preferences_file(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut file = fs::File::create(path)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(())
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
        let _ = fs::remove_dir_all(&dir);
        let path = dir.join("state.json");
        unsafe {
            std::env::set_var("JCODE_DESKTOP_STATE", &path);
        }

        let preferences = DesktopPreferences {
            panel_size: PanelSizePreset::Half,
            focused_session_id: Some("session_cow".to_string()),
            workspace_lane: 2,
            space_hold_toggle_ms: 300,
        };
        save_preferences(&preferences)?;
        assert_eq!(load_preferences()?, Some(preferences));
        assert!(!path.with_extension("json.tmp").exists());
        assert!(
            fs::read_dir(&dir)?
                .filter_map(|entry| entry.ok())
                .all(|entry| !entry.file_name().to_string_lossy().ends_with(".tmp")),
            "preference save should not leave temp files behind"
        );

        unsafe {
            std::env::remove_var("JCODE_DESKTOP_STATE");
        }
        let _ = fs::remove_dir_all(dir);
        Ok(())
    }

    #[test]
    fn preference_temp_paths_are_unique_and_hidden_next_to_target() {
        let path = PathBuf::from("/tmp/jcode-desktop-prefs/state.json");
        let first = unique_preferences_temp_path(&path);
        let second = unique_preferences_temp_path(&path);

        assert_eq!(first.parent(), path.parent());
        assert_eq!(second.parent(), path.parent());
        assert_ne!(first, path.with_extension("json.tmp"));
        assert_ne!(first, second);
        assert!(
            first
                .file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with(".state.json.")
        );
        assert!(
            first
                .file_name()
                .unwrap()
                .to_string_lossy()
                .ends_with(".tmp")
        );
    }
}
