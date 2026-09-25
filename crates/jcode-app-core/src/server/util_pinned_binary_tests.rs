use super::*;
use std::time::{Duration, SystemTime};

fn write_binary(path: &Path, mtime: SystemTime) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, vec![0u8; 8192]).unwrap();
    std::fs::File::options()
        .write(true)
        .open(path)
        .unwrap()
        .set_modified(mtime)
        .unwrap();
}

/// Reproduces the macOS failure: a server launched through a channel
/// symlink is still running the old image after the channel is promoted.
#[test]
fn promoted_channel_symlink_is_detected_only_with_a_pinned_identity() {
    let temp = tempfile::tempdir().unwrap();
    let old = temp.path().join("versions/old/jcode");
    let new = temp.path().join("versions/new/jcode");
    let channel = temp.path().join("shared-server/jcode");
    let t0 = SystemTime::now() - Duration::from_secs(3600);
    write_binary(&old, t0);
    write_binary(&new, t0 + Duration::from_secs(1800));
    std::fs::create_dir_all(channel.parent().unwrap()).unwrap();
    std::os::unix::fs::symlink(&old, &channel).unwrap();

    // Identity captured at process start (what `build::running_binary` pins).
    let pinned = build::resolve_binary_payload(&channel);
    assert_eq!(pinned, std::fs::canonicalize(&old).unwrap());

    // The channel is promoted while the old image keeps running.
    std::fs::remove_file(&channel).unwrap();
    std::os::unix::fs::symlink(&new, &channel).unwrap();
    let candidate = build::resolve_binary_payload(&channel);
    let candidate_mtime = binary_mtime(&candidate);

    assert!(
        newer_binary_available(
            binary_mtime(&pinned),
            Some(pinned.as_path()),
            [(candidate.clone(), candidate_mtime)],
        ),
        "the pinned identity must see the promoted build as newer"
    );

    // Re-canonicalizing the launch symlink now (the old behaviour) yields
    // the new build itself, so the update is invisible.
    let recanonicalized = build::resolve_binary_payload(&channel);
    assert!(!newer_binary_available(
        binary_mtime(&recanonicalized),
        Some(recanonicalized.as_path()),
        [(candidate, candidate_mtime)],
    ));
}

/// Run a fresh test process through the same channel link as the daemon, so
/// the process-global startup snapshot and the actual server decision are both
/// exercised rather than supplying a synthetic pinned path to the pure helper.
#[cfg(target_os = "macos")]
#[test]
fn promoted_channel_is_detected_through_startup_capture_and_server_decision() {
    const CHILD_ROOT: &str = "JCODE_PINNED_RELOAD_TEST_ROOT";
    if let Some(root) = std::env::var_os(CHILD_ROOT) {
        let root = PathBuf::from(root);
        let channel = root.join("builds/shared-server/jcode");
        assert_eq!(
            std::env::current_exe().unwrap(),
            channel,
            "the test must launch through the channel path to exercise macOS symlink identity"
        );
        let old = std::fs::canonicalize(&channel).unwrap();
        // Capture before promotion, as main.rs does before starting the server.
        build::capture_running_binary();
        assert_eq!(build::running_binary(), Some(old.clone()));
        let new = root.join("builds/versions/new/jcode");
        std::fs::remove_file(&channel).unwrap();
        std::os::unix::fs::symlink(&new, &channel).unwrap();
        assert_eq!(build::running_binary(), Some(old));
        assert!(
            server_has_newer_binary(),
            "promoted channel must signal an update"
        );
        assert_eq!(reload_exec_target(true).unwrap().0, channel);
        return;
    }

    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let old = root.join("builds/versions/old/jcode");
    let new = root.join("builds/versions/new/jcode");
    let channel = root.join("builds/shared-server/jcode");
    std::fs::create_dir_all(old.parent().unwrap()).unwrap();
    std::fs::copy(std::env::current_exe().unwrap(), &old).unwrap();
    let t0 = SystemTime::now() - Duration::from_secs(3600);
    std::fs::File::options()
        .write(true)
        .open(&old)
        .unwrap()
        .set_modified(t0)
        .unwrap();
    write_binary(&new, t0 + Duration::from_secs(1800));
    std::fs::create_dir_all(channel.parent().unwrap()).unwrap();
    std::os::unix::fs::symlink(&old, &channel).unwrap();

    let output = std::process::Command::new(&channel)
        .arg("promoted_channel_is_detected_through_startup_capture_and_server_decision")
        .arg("--nocapture")
        .env(CHILD_ROOT, root)
        .env("JCODE_HOME", root)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "channel-launched child failed: {}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("1 passed"));
}
