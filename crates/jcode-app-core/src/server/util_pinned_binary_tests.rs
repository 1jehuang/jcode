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
