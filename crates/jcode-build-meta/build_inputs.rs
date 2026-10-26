use std::ffi::OsStr;
use std::path::Path;

/// Accept release identities without accepting arbitrary environment text.
pub fn release_semver(value: &str) -> Option<String> {
    let value = value.trim().strip_prefix('v').unwrap_or(value.trim());
    let (core, preview) = match value.split_once('-') {
        Some((core, suffix)) => (core, Some(suffix.strip_prefix("preview.")?)),
        None => (value, None),
    };
    let components: Vec<_> = core.split('.').collect();
    if components.len() != 3 || components.iter().any(|part| !numeric_component(part)) {
        return None;
    }
    if let Some(number) = preview {
        if core != "1.0.0" || !numeric_component(number) || number == "0" {
            return None;
        }
    }
    Some(value.to_string())
}

fn numeric_component(value: &str) -> bool {
    !value.is_empty()
        && value.bytes().all(|byte| byte.is_ascii_digit())
        && (value == "0" || !value.starts_with('0'))
        && value.parse::<u64>().is_ok()
}

/// Snapshot contents are build inputs even when no Git checkout is available.
#[allow(dead_code)] // Used by build.rs, but not by the runtime identity parser.
pub fn metadata_rerun_inputs(path: Option<&OsStr>) -> Vec<String> {
    let mut inputs = vec![
        "cargo:rerun-if-env-changed=JCODE_BUILD_METADATA_FILE".to_owned(),
        "cargo:rerun-if-env-changed=JCODE_BUILD_CHANGELOG_RAW".to_owned(),
        "cargo:rerun-if-changed=build_inputs.rs".to_owned(),
    ];
    if let Some(path) = path {
        inputs.push(format!(
            "cargo:rerun-if-changed={}",
            Path::new(path).display()
        ));
    }
    inputs
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn release_identity_accepts_stable_and_v1_previews() {
        for value in ["1.2.3", "1.0.0-preview.1", "1.0.0-preview.123"] {
            assert_eq!(release_semver(value).as_deref(), Some(value));
        }
        assert_eq!(
            release_semver(" v1.0.0-preview.2 ").as_deref(),
            Some("1.0.0-preview.2")
        );
        for value in [
            "1.2",
            "1.2.3.4",
            "1.0.0-preview.0",
            "1.0.0-preview.01",
            "1.0.0-preview.1-dev",
            "1.1.0-preview.1",
            "01.0.0",
            "1.0.0\nINJECT=1",
        ] {
            assert_eq!(release_semver(value), None, "{value}");
        }
    }

    #[test]
    fn no_snapshot_still_tracks_metadata_environment() {
        let inputs = metadata_rerun_inputs(None);
        assert!(inputs.contains(&"cargo:rerun-if-env-changed=JCODE_BUILD_METADATA_FILE".into()));
        assert!(inputs.contains(&"cargo:rerun-if-env-changed=JCODE_BUILD_CHANGELOG_RAW".into()));
        assert_eq!(inputs.len(), 3);
    }

    #[test]
    fn snapshot_contents_are_tracked_without_git() {
        let inputs = metadata_rerun_inputs(Some(OsStr::new(".jcode-release-metadata/build.txt")));
        assert!(
            inputs.contains(&"cargo:rerun-if-changed=.jcode-release-metadata/build.txt".into())
        );
        assert_eq!(inputs.len(), 4);
    }
}
