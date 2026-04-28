use super::*;

#[test]
fn desired_nofile_soft_limit_only_raises_when_possible() {
    assert_eq!(desired_nofile_soft_limit(1024, 524_288, 8192), Some(8192));
    assert_eq!(desired_nofile_soft_limit(8192, 524_288, 8192), None);
    assert_eq!(desired_nofile_soft_limit(1024, 4096, 8192), Some(4096));
}

#[cfg(unix)]
#[test]
fn spawn_detached_creates_new_session() {
    use tempfile::NamedTempFile;

    let output = NamedTempFile::new().expect("temp file");
    let output_path = output.path().to_string_lossy().to_string();
    let parent_sid = unsafe { libc::getsid(0) };

    let mut cmd = std::process::Command::new("sh");
    cmd.arg("-c")
        .arg("ps -o sid= -p $$ > \"$JCODE_TEST_OUTPUT\"")
        .env("JCODE_TEST_OUTPUT", &output_path)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());

    let mut child = super::spawn_detached(&mut cmd).expect("spawn detached child");
    let status = child.wait().expect("wait for child");
    assert!(status.success(), "child should exit successfully");

    let child_sid = std::fs::read_to_string(&output_path)
        .expect("read child sid")
        .trim()
        .parse::<u32>()
        .expect("parse child sid");

    assert_eq!(
        child_sid,
        child.id(),
        "detached child should lead its own session"
    );
    assert_ne!(
        child_sid as i32, parent_sid,
        "detached child should not share parent session"
    );
}

#[cfg(windows)]
#[test]
fn is_process_running_reports_exited_children_as_stopped() {
    use std::process::{Command, Stdio};
    use std::time::Duration;

    let mut cmd = Command::new("cmd.exe");
    cmd.args(["/C", "ping -n 3 127.0.0.1 >NUL"])
        .stdout(Stdio::null())
        .stderr(Stdio::null());

    let mut child = cmd.spawn().expect("spawn child");
    let pid = child.id();
    assert!(
        super::is_process_running(pid),
        "child should initially be running"
    );

    let status = child.wait().expect("wait for child");
    assert!(status.success(), "child should exit successfully");
    std::thread::sleep(Duration::from_millis(100));

    assert!(
        !super::is_process_running(pid),
        "exited child should not be reported as running"
    );
}

#[cfg(windows)]
#[test]
fn spawn_replacement_process_returns_without_waiting_for_child_exit() {
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};

    let mut cmd = Command::new("cmd.exe");
    cmd.args(["/C", "ping -n 4 127.0.0.1 >NUL"])
        .stdout(Stdio::null())
        .stderr(Stdio::null());

    let start = Instant::now();
    let mut child = super::spawn_replacement_process(&mut cmd)
        .expect("spawn replacement process should succeed");
    let elapsed = start.elapsed();

    assert!(
        elapsed < Duration::from_secs(1),
        "replacement spawn should not block, took {:?}",
        elapsed
    );
    assert!(
        child.try_wait().expect("poll child status").is_none(),
        "replacement child should still be running immediately after spawn"
    );

    child.kill().ok();
    let _ = child.wait();
}

#[cfg(windows)]
#[test]
fn replace_executable_atomic_writes_new_contents_when_target_is_idle() {
    let temp = tempfile::tempdir().expect("tempdir");
    let src = temp.path().join("src.exe");
    let dst = temp.path().join("dst.exe");

    std::fs::write(&src, b"new contents").expect("write src");
    std::fs::write(&dst, b"old contents").expect("write dst");

    super::replace_executable_atomic(&src, &dst).expect("replace");

    let contents = std::fs::read(&dst).expect("read dst");
    assert_eq!(contents, b"new contents");
}

#[cfg(windows)]
#[test]
fn replace_executable_atomic_succeeds_when_target_is_locked_for_execution() {
    // Simulate a "running .exe": Windows opens the loaded image with
    // FILE_SHARE_READ | FILE_SHARE_DELETE — that combination rejects
    // unlink but permits rename (which is what our implementation
    // relies on).
    use std::os::windows::fs::OpenOptionsExt;
    // See windows-sys Win32::Storage::FileSystem. Hardcoded so the
    // test does not require pulling in that feature.
    const FILE_SHARE_READ: u32 = 0x0000_0001;
    const FILE_SHARE_DELETE: u32 = 0x0000_0004;

    let temp = tempfile::tempdir().expect("tempdir");
    let src = temp.path().join("src.exe");
    let dst = temp.path().join("dst.exe");

    std::fs::write(&src, b"new contents").expect("write src");
    std::fs::write(&dst, b"old contents").expect("write dst");

    let _locked = std::fs::OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_DELETE)
        .open(&dst)
        .expect("lock dst");

    super::replace_executable_atomic(&src, &dst).expect("replace");

    let contents = std::fs::read(&dst).expect("read dst");
    assert_eq!(contents, b"new contents");

    let aside_count = std::fs::read_dir(temp.path())
        .expect("read tempdir")
        .filter_map(|e| e.ok())
        .filter(|e| {
            e.file_name()
                .to_str()
                .map(|n| n.starts_with(".dst.exe.") && n.ends_with(".old"))
                .unwrap_or(false)
        })
        .count();
    assert_eq!(aside_count, 1, "expected exactly one .old sidecar");

    // Cleanup helper should remove the sidecar (drop the lock first so it can).
    drop(_locked);
    super::clean_stale_executable_replacements(temp.path());
    let aside_after = std::fs::read_dir(temp.path())
        .expect("read tempdir after cleanup")
        .filter_map(|e| e.ok())
        .filter(|e| {
            e.file_name()
                .to_str()
                .map(|n| n.ends_with(".old"))
                .unwrap_or(false)
        })
        .count();
    assert_eq!(aside_after, 0, "cleanup should remove .old sidecars");
}

#[cfg(unix)]
#[test]
fn replace_executable_atomic_replaces_target_on_unix() {
    let temp = tempfile::tempdir().expect("tempdir");
    let src = temp.path().join("src.bin");
    let dst = temp.path().join("dst.bin");

    std::fs::write(&src, b"new").expect("write src");
    std::fs::write(&dst, b"old").expect("write dst");

    super::replace_executable_atomic(&src, &dst).expect("replace");
    assert_eq!(std::fs::read(&dst).expect("read dst"), b"new");
}
