use super::*;

fn repository() -> (tempfile::TempDir, tempfile::TempDir, RepoManifest) {
    let root = tempfile::tempdir().unwrap();
    let stage = tempfile::tempdir().unwrap();
    git(root.path(), &["init", "--quiet"]).unwrap();
    git(root.path(), &["config", "user.name", "Cloud Test"]).unwrap();
    git(
        root.path(),
        &["config", "user.email", "cloud@example.invalid"],
    )
    .unwrap();
    std::fs::write(root.path().join("file.txt"), "original\n").unwrap();
    git(root.path(), &["add", "."]).unwrap();
    git(root.path(), &["commit", "--quiet", "-m", "initial"]).unwrap();
    let repo = snapshot_repo(root.path(), "earlier-session", stage.path())
        .unwrap()
        .unwrap();
    git(
        root.path(),
        &[
            "update-ref",
            "refs/jcode-cloud/earlier-session/base",
            &repo.snapshot,
        ],
    )
    .unwrap();
    (root, stage, repo)
}

#[test]
fn cloud_conflict_identifies_earlier_session_and_preserves_changes() {
    let (root, stage, repo) = repository();
    std::fs::write(root.path().join("file.txt"), "valuable cloud work\n").unwrap();
    std::fs::write(root.path().join("untracked.txt"), "also valuable\n").unwrap();
    let head = git(root.path(), &["rev-parse", "HEAD"]).unwrap();
    let status = git(root.path(), &["status", "--porcelain"]).unwrap();
    let error = restore_repo(
        &repo,
        &stage.path().join("repo.bundle"),
        &stage.path().join("ignored.tar"),
        "new-session",
    )
    .unwrap_err()
    .to_string();
    assert!(
        error.contains("cloud return --session earlier-session"),
        "{error}"
    );
    assert!(error.contains("refusing to overwrite"), "{error}");
    assert_eq!(
        std::fs::read_to_string(root.path().join("file.txt")).unwrap(),
        "valuable cloud work\n"
    );
    assert_eq!(
        std::fs::read_to_string(root.path().join("untracked.txt")).unwrap(),
        "also valuable\n"
    );
    assert_eq!(git(root.path(), &["rev-parse", "HEAD"]).unwrap(), head);
    assert_eq!(
        git(root.path(), &["status", "--porcelain"]).unwrap(),
        status
    );
}

#[test]
fn cloud_restore_accepts_unchanged_snapshot_on_retry() {
    let (root, stage, _) = repository();
    std::fs::write(root.path().join("file.txt"), "local uncommitted work\n").unwrap();
    let repo = snapshot_repo(root.path(), "retry-session", stage.path())
        .unwrap()
        .unwrap();
    let restored = restore_repo(
        &repo,
        &stage.path().join("repo.bundle"),
        &stage.path().join("ignored.tar"),
        "retry-session",
    )
    .unwrap();
    assert_eq!(restored, repo.snapshot);
    assert_eq!(
        std::fs::read_to_string(root.path().join("file.txt")).unwrap(),
        "local uncommitted work\n"
    );
}

#[test]
fn cloud_restore_accepts_incremental_bundle_without_branch_advertisement() {
    let (root, stage, mut repo) = repository();
    let dest = tempfile::tempdir().unwrap();
    git(
        root.path(),
        &[
            "clone",
            "--quiet",
            "--no-local",
            repo.root.as_str(),
            dest.path().to_str().unwrap(),
        ],
    )
    .unwrap();
    let bundle = stage.path().join("incremental.bundle");
    let snapshot_ref = "refs/jcode-cloud/earlier-session/outgoing";
    git(
        root.path(),
        &[
            "bundle",
            "create",
            bundle.to_str().unwrap(),
            snapshot_ref,
            &format!("^{}", repo.head),
        ],
    )
    .unwrap();
    let heads = git(
        root.path(),
        &["bundle", "list-heads", bundle.to_str().unwrap()],
    )
    .unwrap();
    assert!(!heads.contains("refs/heads/"), "{heads}");
    repo.root = dest.path().to_string_lossy().into_owned();
    assert_eq!(
        restore_repo(
            &repo,
            &bundle,
            &stage.path().join("missing.tar"),
            "earlier-session"
        )
        .unwrap(),
        repo.snapshot
    );
    assert_eq!(git(dest.path(), &["rev-parse", "HEAD"]).unwrap(), repo.head);
    assert_eq!(git(dest.path(), &["status", "--porcelain"]).unwrap(), "");
}

#[test]
fn cloud_restore_rejects_missing_bundle_prerequisite_before_checkout_changes() {
    let (root, stage, mut repo) = repository();
    let dest = tempfile::tempdir().unwrap();
    git(dest.path(), &["init", "--quiet"]).unwrap();
    let bundle = stage.path().join("incremental.bundle");
    git(
        root.path(),
        &[
            "bundle",
            "create",
            bundle.to_str().unwrap(),
            "refs/jcode-cloud/earlier-session/outgoing",
            &format!("^{}", repo.head),
        ],
    )
    .unwrap();
    repo.root = dest.path().to_string_lossy().into_owned();
    assert!(
        restore_repo(
            &repo,
            &bundle,
            &stage.path().join("missing.tar"),
            "earlier-session"
        )
        .is_err()
    );
    assert!(git(dest.path(), &["rev-parse", "--verify", "HEAD"]).is_err());
    assert_eq!(
        git(dest.path(), &["for-each-ref", "--format=%(refname)"]).unwrap(),
        ""
    );
    assert!(!dest.path().join("file.txt").exists());
}
