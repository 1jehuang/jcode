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
fn signal_detached_process_group_terminates_descendant_tree() {
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};

    let temp = tempfile::tempdir().expect("temp dir");
    let ready_path = temp.path().join("child-ready.txt");
    let survived_path = temp.path().join("child-survived.txt");
    let child_script_path = temp.path().join("child.cmd");
    let parent_script_path = temp.path().join("parent.cmd");
    let child_script = concat!(
        "@echo off\r\n",
        "echo ready>\"%~dp0child-ready.txt\"\r\n",
        "ping -n 6 127.0.0.1 >NUL\r\n",
        "echo survived>\"%~dp0child-survived.txt\"\r\n"
    );
    let parent_script = concat!(
        "@echo off\r\n",
        "start \"\" /B cmd.exe /D /C \"\"%~dp0child.cmd\"\"\r\n",
        "ping -n 30 127.0.0.1 >NUL\r\n"
    );
    std::fs::write(&child_script_path, child_script).expect("write child command script");
    std::fs::write(&parent_script_path, parent_script).expect("write parent command script");
    let mut cmd = Command::new("cmd.exe");
    cmd.args(["/D", "/C"])
        .arg(&parent_script_path)
        .stdout(Stdio::null())
        .stderr(Stdio::null());

    let mut parent = super::spawn_detached(&mut cmd).expect("spawn detached process tree");
    let parent_pid = parent.id();
    let deadline = Instant::now() + Duration::from_secs(10);
    while !ready_path.exists() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(ready_path.exists(), "descendant should report ready");
    assert!(super::is_process_running(parent_pid));

    super::signal_detached_process_group(parent_pid, 0).expect("terminate process tree");
    let deadline = Instant::now() + Duration::from_secs(10);
    while super::is_process_running(parent_pid) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(50));
    }
    let _ = parent.wait();

    assert!(!super::is_process_running(parent_pid), "parent should stop");
    std::thread::sleep(Duration::from_secs(6));
    assert!(
        !survived_path.exists(),
        "descendant should not survive termination of the detached process tree"
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

/// Reads the `stat` field `ps` reports for `pid`, or `None` once the pid is gone.
#[cfg(unix)]
fn ps_process_state(pid: u32) -> Option<String> {
    let output = std::process::Command::new("ps")
        .args(["-o", "stat=", "-p", &pid.to_string()])
        .output()
        .expect("run ps to inspect process state");
    let state = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if state.is_empty() { None } else { Some(state) }
}

/// Regression test for the zombie-child leak: detached children that were
/// dropped without a `wait()` accumulated as `<defunct>` processes under the
/// long-lived server. `reap_detached` must remove the zombie entirely (not
/// merely stop reporting it as running).
#[cfg(unix)]
#[test]
fn reap_detached_removes_zombie_child() {
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};

    let mut cmd = Command::new("sh");
    cmd.arg("-c")
        .arg("exit 0")
        .stdout(Stdio::null())
        .stderr(Stdio::null());

    let child = spawn_detached(&mut cmd).expect("spawn detached child");
    let pid = child.id();

    // Control: nothing waits on the child, so it must linger as a zombie. This
    // also proves the observation method can actually see a zombie.
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut zombie_state: Option<String> = None;
    while Instant::now() < deadline {
        match ps_process_state(pid) {
            Some(state) if state.starts_with('Z') => {
                zombie_state = Some(state);
                break;
            }
            None => break,
            Some(_) => std::thread::sleep(Duration::from_millis(20)),
        }
    }
    let zombie_state = zombie_state.unwrap_or_else(|| {
        panic!("pid {pid} should become an unreaped zombie before reap_detached")
    });

    reap_detached(child);

    // Poll with a bounded timeout until the pid disappears completely. A
    // leftover `Z` state is exactly the bug this test guards against.
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut remaining_state: Option<String> = None;
    while Instant::now() < deadline {
        match ps_process_state(pid) {
            None => {
                remaining_state = None;
                break;
            }
            Some(state) => {
                remaining_state = Some(state);
                std::thread::sleep(Duration::from_millis(20));
            }
        }
    }
    assert!(
        remaining_state.is_none(),
        "reap_detached left pid {pid} behind (zombie before reap: {zombie_state}, \
         still present as: {remaining_state:?}); expected the pid to disappear"
    );

    // Belt and braces: no `<defunct>` child of this process may keep the pid.
    let pid_field = pid.to_string();
    let self_pid = std::process::id().to_string();
    let listing = Command::new("ps")
        .args(["-ax", "-o", "pid=,ppid=,stat="])
        .output()
        .expect("run ps to list processes");
    let listing = String::from_utf8_lossy(&listing.stdout);
    let leftover_zombie = listing.lines().any(|line| {
        let mut fields = line.split_whitespace();
        let line_pid = fields.next().unwrap_or_default();
        let line_ppid = fields.next().unwrap_or_default();
        let line_stat = fields.next().unwrap_or_default();
        line_pid == pid_field && line_ppid == self_pid && line_stat.starts_with('Z')
    });
    assert!(
        !leftover_zombie,
        "pid {self_pid} should have no <defunct> child with pid {pid_field}"
    );
}

/// `reap_detached` must hand the wait off to a background thread instead of
/// blocking the caller, otherwise spawning a long-lived detached child would
/// stall the tool loop that called it.
#[cfg(unix)]
#[test]
fn reap_detached_does_not_block_on_running_child() {
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};

    let mut cmd = Command::new("sh");
    cmd.arg("-c")
        .arg("sleep 5")
        .stdout(Stdio::null())
        .stderr(Stdio::null());

    let child = spawn_detached(&mut cmd).expect("spawn long-lived detached child");
    let pid = child.id();

    let start = Instant::now();
    reap_detached(child);
    let elapsed = start.elapsed();

    assert!(
        elapsed < Duration::from_secs(2),
        "reap_detached must not block on a running child, took {elapsed:?}"
    );
    assert!(
        is_process_running(pid),
        "the reaping thread must not stop the running child"
    );

    // Clean up; the reaping thread observes the exit.
    unsafe { libc::kill(pid as i32, libc::SIGKILL) };
}
