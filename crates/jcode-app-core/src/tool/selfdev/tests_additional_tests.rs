use super::*;

#[test]
fn build_lock_is_removed_on_drop_and_can_be_reacquired() {
    let _env_lock = crate::storage::lock_test_env();
    let temp = tempfile::tempdir().expect("temp jcode home");
    let _home = EnvVarGuard::set("JCODE_HOME", temp.path());
    let scope = format!("lock-drop-{}", std::process::id());
    let path = SelfDevTool::build_lock_path(&scope).expect("lock path");

    let first = SelfDevTool::try_acquire_build_lock(&scope)
        .expect("first lock attempt")
        .expect("first lock acquired");
    assert!(path.exists(), "lock file should exist while held");
    drop(first);
    assert!(!path.exists(), "lock file should be removed on drop");

    let second = SelfDevTool::try_acquire_build_lock(&scope)
        .expect("second lock attempt")
        .expect("lock should be reacquirable after drop");
    drop(second);
    assert!(!path.exists(), "reacquired lock should also clean up");
}
