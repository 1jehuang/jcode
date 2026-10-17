//! Small persisted UI preferences that survive restarts and session resumes.
//!
//! These are deliberately separate from the main config file: they capture
//! in-app toggles (like hiding inline images) that the user flips at runtime
//! and expects to stick, without editing `config.toml`.

use serde::{Deserialize, Serialize};

const UI_PREFS_FILE: &str = "ui_preferences.json";

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct UiPreferences {
    #[serde(default)]
    pub version: u8,
    /// Whether inline transcript images render expanded. `None` means the
    /// user never toggled it; default to visible.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inline_images_visible: Option<bool>,
}

fn prefs_path() -> Option<std::path::PathBuf> {
    crate::storage::app_config_dir()
        .ok()
        .map(|dir| dir.join(UI_PREFS_FILE))
}

pub(crate) fn load() -> UiPreferences {
    let Some(path) = prefs_path() else {
        return UiPreferences::default();
    };
    crate::storage::read_json::<UiPreferences>(&path).unwrap_or_default()
}

/// Persisted inline-image visibility, defaulting to visible.
pub(crate) fn inline_images_visible() -> bool {
    load().inline_images_visible.unwrap_or(true)
}

/// Persist the inline-image visibility toggle (load-modify-write so future
/// preference fields survive).
pub(crate) fn save_inline_images_visible(visible: bool) {
    let Some(path) = prefs_path() else {
        return;
    };
    let mut prefs = load();
    prefs.inline_images_visible = Some(visible);
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Err(error) = crate::storage::write_json(&path, &prefs) {
        crate::logging::info(&format!(
            "Failed to persist UI preferences {}: {}",
            path.display(),
            error
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inline_images_visibility_round_trips_through_disk() {
        let _guard = crate::storage::lock_test_env();
        let temp = tempfile::tempdir().expect("tempdir");
        let prev_home = std::env::var_os("KCODE_HOME");
        crate::env::set_var("KCODE_HOME", temp.path());

        // Default before any toggle: visible.
        assert!(inline_images_visible());

        save_inline_images_visible(false);
        assert!(!inline_images_visible(), "hidden state should persist");

        save_inline_images_visible(true);
        assert!(inline_images_visible(), "visible state should persist");

        if let Some(prev_home) = prev_home {
            crate::env::set_var("KCODE_HOME", prev_home);
        } else {
            crate::env::remove_var("KCODE_HOME");
        }
    }

    #[test]
    fn save_preserves_unknown_future_fields_via_load_modify_write() {
        let _guard = crate::storage::lock_test_env();
        let temp = tempfile::tempdir().expect("tempdir");
        let prev_home = std::env::var_os("KCODE_HOME");
        crate::env::set_var("KCODE_HOME", temp.path());

        save_inline_images_visible(false);
        let prefs = load();
        assert_eq!(prefs.inline_images_visible, Some(false));

        if let Some(prev_home) = prev_home {
            crate::env::set_var("KCODE_HOME", prev_home);
        } else {
            crate::env::remove_var("KCODE_HOME");
        }
    }
}
