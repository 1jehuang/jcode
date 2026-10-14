//! Explain why a Chromium browser's bridge extension is not answering.
//!
//! "Chrome is running but the bridge is silent" used to produce one generic
//! hint, and re-running setup or repair could not change the outcome (#1743).
//! Chrome records each extension in its profile `Preferences` (or
//! `Secure Preferences`): whether it is installed, which folder an unpacked
//! copy loads from, and why it is disabled. Read that to say what is wrong.

use super::*;
use serde_json::Value;

/// What the profiles say about the bridge extension.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChromiumExtensionState {
    /// Not installed in any profile.
    NotInstalled { profiles: Vec<String> },
    /// Installed but turned off.
    Disabled { profile: String },
    /// Unpacked from a folder jcode does not update.
    OtherFolder { profile: String, path: String },
    /// Installed and enabled from jcode's folder.
    Installed { profile: String },
}

/// Classify one profile's extension record.
pub(super) fn classify_record(
    profile: &str,
    record: &Value,
    expected: &Path,
) -> ChromiumExtensionState {
    let disabled = record["disable_reasons"]
        .as_array()
        .is_some_and(|reasons| !reasons.is_empty())
        || record["disable_reasons"]
            .as_u64()
            .is_some_and(|bits| bits != 0)
        || record["state"].as_u64() == Some(0);
    if disabled {
        return ChromiumExtensionState::Disabled {
            profile: profile.to_string(),
        };
    }
    if let Some(path) = record["path"].as_str()
        && std::path::Path::new(path).is_absolute()
        && std::path::Path::new(path) != expected
    {
        return ChromiumExtensionState::OtherFolder {
            profile: profile.to_string(),
            path: path.to_string(),
        };
    }
    ChromiumExtensionState::Installed {
        profile: profile.to_string(),
    }
}

/// Profile directories listed in `Local State`, falling back to `Default`.
fn profile_dirs(user_data: &Path) -> Vec<String> {
    let listed = std::fs::read(user_data.join("Local State"))
        .ok()
        .and_then(|raw| serde_json::from_slice::<Value>(&raw).ok())
        .and_then(|state| {
            state["profile"]["info_cache"]
                .as_object()
                .map(|profiles| profiles.keys().cloned().collect::<Vec<_>>())
        })
        .unwrap_or_default();
    if listed.is_empty() {
        vec!["Default".to_string()]
    } else {
        listed
    }
}

fn extension_record(profile_dir: &Path) -> Option<Value> {
    ["Secure Preferences", "Preferences"]
        .iter()
        .find_map(|file| {
            let raw = std::fs::read(profile_dir.join(file)).ok()?;
            let prefs: Value = serde_json::from_slice(&raw).ok()?;
            let record = prefs["extensions"]["settings"][CHROMIUM_EXTENSION_ID].clone();
            record.is_object().then_some(record)
        })
}

/// Inspect the profiles under `user_data` (a Chromium user data directory).
pub(super) fn inspect_user_data(
    user_data: &Path,
    expected: &Path,
) -> Option<ChromiumExtensionState> {
    if !user_data.is_dir() {
        return None;
    }
    let profiles = profile_dirs(user_data);
    let mut best: Option<ChromiumExtensionState> = None;
    for profile in &profiles {
        let Some(record) = extension_record(&user_data.join(profile)) else {
            continue;
        };
        let state = classify_record(profile, &record, expected);
        // A healthy install anywhere wins; otherwise report the first problem.
        if matches!(state, ChromiumExtensionState::Installed { .. }) {
            return Some(state);
        }
        best.get_or_insert(state);
    }
    Some(best.unwrap_or(ChromiumExtensionState::NotInstalled { profiles }))
}

/// User data directory of `kind`: the parent of its NativeMessagingHosts dir.
fn user_data_dir(kind: BrowserKind) -> Option<PathBuf> {
    if kind.family() != BrowserFamily::Chromium || cfg!(windows) {
        return None;
    }
    kind.native_messaging_dirs()
        .first()
        .and_then(|dir| dir.parent())
        .map(Path::to_path_buf)
}

/// One-paragraph, actionable explanation for a silent Chromium bridge, or
/// `None` when the profiles do not explain it (or `kind` is not Chromium).
pub fn diagnose_silent_chromium_bridge(kind: BrowserKind) -> Option<String> {
    let expected = chromium_extension_dir();
    let state = inspect_user_data(&user_data_dir(kind)?, &expected)?;
    let name = kind.display_name();
    let page = kind.extensions_page();
    Some(match state {
        ChromiumExtensionState::NotInstalled { profiles } => format!(
            "The Browser Agent Bridge extension is not installed in {name} (checked profiles: {}). Open {page}, turn on Developer mode, click \"Load unpacked\", and choose {}.",
            profiles.join(", "),
            expected.display()
        ),
        ChromiumExtensionState::Disabled { profile } => format!(
            "The Browser Agent Bridge extension is installed in {name} (profile \"{profile}\") but turned off. Enable it in {page}."
        ),
        ChromiumExtensionState::OtherFolder { profile, path } => format!(
            "{name} (profile \"{profile}\") loads the Browser Agent Bridge extension from {path}, but jcode installs and updates it in {}. Setup and repair cannot fix a copy in another folder. In {page}, remove it and \"Load unpacked\" from {} instead.",
            expected.display(),
            expected.display()
        ),
        ChromiumExtensionState::Installed { profile } => format!(
            "The Browser Agent Bridge extension is installed and enabled in {name} (profile \"{profile}\") but is not answering. Click its reload button in {page}, or quit and reopen {name}. If it shows errors there, include them when reporting the problem."
        ),
    })
}

use std::path::Path;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn write_profile(user_data: &Path, profile: &str, record: Option<Value>) {
        let dir = user_data.join(profile);
        std::fs::create_dir_all(&dir).unwrap();
        let mut prefs = json!({"extensions": {"settings": {}}});
        if let Some(record) = record {
            prefs["extensions"]["settings"][CHROMIUM_EXTENSION_ID] = record;
        }
        std::fs::write(dir.join("Preferences"), prefs.to_string()).unwrap();
    }

    fn local_state(user_data: &Path, profiles: &[&str]) {
        let cache: serde_json::Map<String, Value> = profiles
            .iter()
            .map(|p| (p.to_string(), json!({})))
            .collect();
        std::fs::write(
            user_data.join("Local State"),
            json!({"profile": {"info_cache": cache}}).to_string(),
        )
        .unwrap();
    }

    #[test]
    fn reports_missing_disabled_other_folder_and_healthy_installs() {
        let temp = tempfile::tempdir().unwrap();
        let user_data = temp.path();
        let expected = Path::new("/home/u/.jcode/browser/extensions/chromium");

        local_state(user_data, &["Default"]);
        write_profile(user_data, "Default", None);
        assert!(matches!(
            inspect_user_data(user_data, expected),
            Some(ChromiumExtensionState::NotInstalled { .. })
        ));

        write_profile(
            user_data,
            "Default",
            Some(json!({"path": expected, "disable_reasons": [1]})),
        );
        assert!(matches!(
            inspect_user_data(user_data, expected),
            Some(ChromiumExtensionState::Disabled { .. })
        ));

        write_profile(
            user_data,
            "Default",
            Some(json!({"path": "/Users/u/Downloads/bridge", "disable_reasons": []})),
        );
        assert_eq!(
            inspect_user_data(user_data, expected),
            Some(ChromiumExtensionState::OtherFolder {
                profile: "Default".into(),
                path: "/Users/u/Downloads/bridge".into()
            })
        );

        // A healthy install in another profile wins over a problem elsewhere.
        local_state(user_data, &["Default", "Profile 1"]);
        write_profile(
            user_data,
            "Profile 1",
            Some(json!({"path": expected, "disable_reasons": []})),
        );
        assert_eq!(
            inspect_user_data(user_data, expected),
            Some(ChromiumExtensionState::Installed {
                profile: "Profile 1".into()
            })
        );
    }

    #[test]
    fn missing_user_data_dir_is_not_a_diagnosis() {
        let temp = tempfile::tempdir().unwrap();
        assert_eq!(
            inspect_user_data(&temp.path().join("nope"), Path::new("/x")),
            None
        );
    }

    #[test]
    fn legacy_bitmask_and_state_zero_count_as_disabled() {
        let expected = Path::new("/x");
        for record in [json!({"disable_reasons": 4}), json!({"state": 0})] {
            assert!(matches!(
                classify_record("Default", &record, expected),
                ChromiumExtensionState::Disabled { .. }
            ));
        }
    }
}
