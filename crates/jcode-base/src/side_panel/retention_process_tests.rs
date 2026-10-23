use super::*;
use crate::side_panel::side_panel_tests::EnvVarGuard;

#[test]
#[ignore = "invoked only by the isolated cross-process lock regression"]
fn side_panel_process_lock_worker() {
    let ready = std::env::var_os("JCODE_PANEL_READY_FILE").expect("synthetic ready path");
    std::fs::write(ready, "ready").unwrap();
    write_markdown_page("process", "child", None, "child body", true).unwrap();
}

#[test]
fn side_panel_mutation_waits_for_other_process_lock() {
    let _env = crate::storage::lock_test_env();
    let temp = tempfile::tempdir().unwrap();
    let _home = EnvVarGuard::set_path("JCODE_HOME", temp.path());
    let dir = temp.path().join("side_panel");
    std::fs::create_dir_all(&dir).unwrap();
    let gate = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(dir.join(".lock"))
        .unwrap();
    gate.lock().unwrap();
    let ready = temp.path().join("ready");
    let mut child = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--ignored",
            "--exact",
            "side_panel::retention_process_tests::side_panel_process_lock_worker",
            "--test-threads=1",
        ])
        .env("JCODE_PANEL_READY_FILE", &ready)
        .stdout(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while !ready.exists() && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert!(ready.exists(), "child reached public mutation API");
    std::thread::sleep(std::time::Duration::from_millis(50));
    assert!(
        child.try_wait().unwrap().is_none(),
        "external lock blocks mutation"
    );
    gate.unlock().unwrap();
    assert!(child.wait().unwrap().success());
    assert_eq!(
        snapshot_for_session("process").unwrap().pages[0].content,
        "child body"
    );
}
