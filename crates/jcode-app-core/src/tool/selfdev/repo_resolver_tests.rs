use super::*;

/// The reload must publish from the same checkout the build used. A primary
/// repo derived from the running binary's compile location (another worktree)
/// used to win, which published a stale binary from that worktree.
#[test]
fn reload_repo_resolver_prefers_session_working_dir_over_binary_origin() {
    let session_repo = create_repo_fixture();
    let other_worktree = create_repo_fixture();
    let nested = session_repo.path().join("crates");
    std::fs::create_dir_all(&nested).expect("nested dir");

    let resolved = reload::resolve_selfdev_reload_repo_dir_from(
        Some(other_worktree.path().to_path_buf()),
        Some(&nested),
    );
    assert_eq!(resolved.as_deref(), Some(session_repo.path()));
}
