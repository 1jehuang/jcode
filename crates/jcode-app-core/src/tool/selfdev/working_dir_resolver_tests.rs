use super::*;

#[test]
fn reload_repo_resolver_uses_working_dir_when_primary_detection_fails() {
    let repo = create_repo_fixture();
    let nested = repo.path().join("crates").join("jcode-build-support");
    std::fs::create_dir_all(&nested).expect("nested dir");

    let resolved = reload::resolve_selfdev_reload_repo_dir_from(None, Some(&nested));
    assert_eq!(resolved.as_deref(), Some(repo.path()));
}
