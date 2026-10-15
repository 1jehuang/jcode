use std::ffi::OsStr;
use std::path::Path;

/// Snapshot contents are build inputs even when no Git checkout is available.
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
