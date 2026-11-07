#[tokio::test]
async fn launch_target_stays_pinned_while_provider_initialization_is_pending() {
    let dir = tempfile::tempdir().expect("tempdir");
    let old = dir.path().join("old");
    let new = dir.path().join("new");
    std::fs::write(&old, b"old").unwrap();
    std::fs::write(&new, b"new").unwrap();
    let link = dir.path().join("channel");
    std::os::unix::fs::symlink(&old, &link).unwrap();
    let cache = std::sync::OnceLock::new();
    super::pin_launch_path(&cache, || Some(link.clone()));

    // The entry-point capture must precede a provider future's first poll.
    let provider_initialization = async {
        tokio::task::yield_now().await;
        std::fs::remove_file(&link).unwrap();
        std::os::unix::fs::symlink(&new, &link).unwrap();
    };
    provider_initialization.await;

    assert_eq!(
        super::pin_launch_path(&cache, || Some(link)),
        Some(std::fs::canonicalize(old).unwrap())
    );
}

#[test]
fn failed_launch_identity_is_not_replaced_by_a_later_path() {
    let cache = std::sync::OnceLock::new();
    super::pin_launch_path(&cache, || None);
    assert_eq!(
        super::pin_launch_path(&cache, || Some("later".into())),
        None
    );
}

/// The server is started through a channel link; installing a new build
/// repoints the link. The pinned path must stay on the binary the server
/// was started from, so the new build counts as newer and reload runs.
#[test]
fn launch_path_pinned_before_the_channel_link_moves() {
    let dir = tempfile::tempdir().expect("tempdir");
    let old = dir.path().join("versions/old/jcode");
    let new = dir.path().join("versions/new/jcode");
    for binary in [&old, &new] {
        std::fs::create_dir_all(binary.parent().unwrap()).unwrap();
        std::fs::write(binary, b"bin").unwrap();
    }
    let link = dir.path().join("stable/jcode");
    std::fs::create_dir_all(link.parent().unwrap()).unwrap();
    std::os::unix::fs::symlink(&old, &link).unwrap();

    // Server start: what running_server_binary pins.
    let pinned = super::resolve_launch_path(link.clone());
    // An install repoints the channel.
    std::fs::remove_file(&link).unwrap();
    std::os::unix::fs::symlink(&new, &link).unwrap();

    assert_eq!(pinned, std::fs::canonicalize(&old).unwrap());
    // Re-resolving the launch path now (the old behavior) lands on the new
    // build, which is why reload thought nothing was newer.
    assert_eq!(
        super::resolve_launch_path(link),
        std::fs::canonicalize(&new).unwrap()
    );
}
