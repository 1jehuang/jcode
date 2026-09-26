use super::*;
#[cfg(target_os = "linux")]
use std::process::{Command, Stdio};

#[test]
fn test_own_process_not_reading_stdin() {
    let pid = std::process::id();
    let state = is_waiting_for_stdin(pid);
    assert_ne!(state, StdinState::Reading);
}

#[test]
fn test_nonexistent_pid() {
    let state = is_waiting_for_stdin(u32::MAX);
    assert_ne!(state, StdinState::Reading);
}

#[cfg(target_os = "linux")]
#[test]
fn test_blocked_process_detected() {
    let mut child = Command::new("cat")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .spawn()
        .expect("failed to spawn cat");

    let pid = child.id();
    std::thread::sleep(std::time::Duration::from_millis(200));

    let state = linux::check_process_tree(pid);

    child.kill().ok();
    child.wait().ok();

    assert_eq!(
        state,
        StdinState::Reading,
        "cat should be waiting for stdin"
    );
}

#[cfg(target_os = "linux")]
#[test]
fn test_running_process_not_reading() {
    let mut child = Command::new("sleep")
        .arg("10")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .spawn()
        .expect("failed to spawn sleep");

    let pid = child.id();
    std::thread::sleep(std::time::Duration::from_millis(100));

    let state = linux::check(pid);

    child.kill().ok();
    child.wait().ok();

    assert_eq!(
        state,
        StdinState::NotReading,
        "sleep should not be reading stdin"
    );
}

#[cfg(target_os = "linux")]
#[test]
fn test_child_process_tree_detection() {
    // bash -c "cat" spawns bash which spawns cat - cat is the one reading stdin
    let mut child = Command::new("bash")
        .arg("-c")
        .arg("cat")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .spawn()
        .expect("failed to spawn bash");

    let pid = child.id();
    std::thread::sleep(std::time::Duration::from_millis(300));

    // The bash process itself may not be reading, but its child (cat) should be
    let state = linux::check_process_tree(pid);

    child.kill().ok();
    child.wait().ok();

    assert_eq!(
        state,
        StdinState::Reading,
        "child cat should be detected via process tree"
    );
}

#[cfg(target_os = "linux")]
#[test]
fn test_grandchild_process_tree_detection() {
    // Wrapper chain: an outer bash spawns an inner `bash -c cat`, so the actual
    // stdin reader (`cat`) is a GRANDCHILD of the tracked pid. The intermediate
    // bash is not itself reading stdin, so detection requires recursing past
    // direct children (issue #373). A trailing `; true` keeps each bash from
    // exec-optimizing itself away so the nesting (outer bash -> inner bash ->
    // cat) is preserved.
    let mut child = Command::new("bash")
        .arg("-c")
        .arg("bash -c 'cat; true'; true")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .spawn()
        .expect("failed to spawn bash");

    let pid = child.id();
    std::thread::sleep(std::time::Duration::from_millis(400));

    let state = linux::check_process_tree(pid);

    child.kill().ok();
    child.wait().ok();

    assert_eq!(
        state,
        StdinState::Reading,
        "grandchild cat should be detected via recursive process-tree walk"
    );
}

#[cfg(target_os = "linux")]
#[test]
fn test_direct_children_lists_immediate_children() {
    // Spawn a parent shell that itself spawns a long-lived child (`sleep`).
    // `direct_children` should report the immediate child PID(s) without
    // scanning all of /proc.
    // Use a compound command so bash does NOT exec-optimize itself away and
    // actually stays alive as the parent of a `sleep` child.
    let mut child = Command::new("bash")
        .arg("-c")
        .arg("sleep 5; true")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .spawn()
        .expect("failed to spawn bash");

    let pid = child.id();
    std::thread::sleep(std::time::Duration::from_millis(200));

    let kids = linux::direct_children(pid);

    // Verify parentage BEFORE killing the parent, otherwise the child
    // reparents to init (ppid 1) and the check races.
    let mut all_parented_by_pid = !kids.is_empty();
    for kid in &kids {
        let status = std::fs::read_to_string(format!("/proc/{}/status", kid)).unwrap_or_default();
        let ppid = status
            .lines()
            .find_map(|l| l.strip_prefix("PPid:\t"))
            .and_then(|v| v.trim().parse::<u32>().ok());
        if ppid != Some(pid) {
            all_parented_by_pid = false;
        }
    }

    child.kill().ok();
    child.wait().ok();

    assert!(
        !kids.is_empty(),
        "bash should have at least one direct child (the sleep)"
    );
    assert!(
        all_parented_by_pid,
        "every reported child should be parented by {pid}; got {kids:?}"
    );
}

#[cfg(target_os = "linux")]
#[test]
fn test_process_that_reads_then_exits() {
    use std::io::Write;

    let mut child = Command::new("head")
        .arg("-n1")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .spawn()
        .expect("failed to spawn head");

    let pid = child.id();
    std::thread::sleep(std::time::Duration::from_millis(200));

    // Should be reading initially
    let state_before = linux::check(pid);

    // Write a line - head should read it and exit
    if let Some(ref mut stdin) = child.stdin {
        stdin.write_all(b"hello\n").ok();
        stdin.flush().ok();
    }

    // Wait for exit
    let status = child.wait().expect("failed to wait");

    // After exit, checking the pid should not report Reading
    let state_after = is_waiting_for_stdin(pid);

    assert_eq!(
        state_before,
        StdinState::Reading,
        "head should be reading before input"
    );
    assert_ne!(
        state_after,
        StdinState::Reading,
        "head should not be reading after exit"
    );
    assert!(status.success(), "head should exit successfully");
}

#[cfg(target_os = "linux")]
#[test]
fn test_process_with_closed_stdin_not_reading() {
    // Spawn a process with stdin completely closed (null)
    let mut child = Command::new("cat")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .spawn()
        .expect("failed to spawn cat");

    let pid = child.id();
    // cat with /dev/null as stdin should read EOF immediately and exit
    std::thread::sleep(std::time::Duration::from_millis(200));

    let state = is_waiting_for_stdin(pid);

    child.kill().ok();
    child.wait().ok();

    // cat with /dev/null gets EOF immediately, should not be stuck reading
    assert_ne!(state, StdinState::Reading);
}

#[cfg(target_os = "linux")]
#[test]
fn test_multiple_sequential_reads() {
    use std::io::Write;

    // Use a program that reads multiple lines
    let mut child = Command::new("head")
        .arg("-n2")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .spawn()
        .expect("failed to spawn head");

    let pid = child.id();
    std::thread::sleep(std::time::Duration::from_millis(200));

    // Should be reading first line
    let state1 = linux::check(pid);
    assert_eq!(
        state1,
        StdinState::Reading,
        "should be waiting for first line"
    );

    // Send first line
    if let Some(ref mut stdin) = child.stdin {
        stdin.write_all(b"line1\n").ok();
        stdin.flush().ok();
    }
    std::thread::sleep(std::time::Duration::from_millis(100));

    // Should be reading second line
    let state2 = linux::check(pid);
    assert_eq!(
        state2,
        StdinState::Reading,
        "should be waiting for second line"
    );

    // Send second line
    if let Some(ref mut stdin) = child.stdin {
        stdin.write_all(b"line2\n").ok();
        stdin.flush().ok();
    }

    let status = child.wait().expect("failed to wait");
    assert!(status.success());
}

#[cfg(target_os = "linux")]
#[test]
fn direct_children_of_childless_process_does_not_scan_proc() {
    // Regression test for issue #392 A1 (second occurrence): a childless
    // process must return an empty list via /proc/<pid>/task/<tid>/children
    // without falling back to the whole-/proc scan. We can't observe syscalls
    // here, but we can assert the interface itself reports readable-and-empty
    // for a leaf process we control, which is the branch condition the fix
    // keys on.
    let mut child = std::process::Command::new("sleep")
        .arg("5")
        .spawn()
        .expect("spawn sleep");
    let pid = child.id();

    // `sleep` spawns no children. The children interface must be readable so
    // direct_children() returns empty WITHOUT the proc-scan fallback.
    let path = format!("/proc/{}/task/{}/children", pid, pid);
    let readable = std::fs::read_to_string(&path).is_ok();
    let children = super::linux::direct_children(pid);

    let _ = child.kill();
    let _ = child.wait();

    assert!(
        readable,
        "kernel lacks CONFIG_PROC_CHILDREN; fallback scan is expected on this system"
    );
    assert!(
        children.is_empty(),
        "sleep should have no children, got {children:?}"
    );
}
