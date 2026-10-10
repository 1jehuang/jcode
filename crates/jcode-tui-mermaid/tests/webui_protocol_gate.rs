//! Process-level regression test for the HERDR_WEBUI mermaid/math protocol
//! gate.
//!
//! The global PICKER is a process-global OnceLock, so env-based protocol
//! inference can only be exercised faithfully by initializing it in a fresh
//! process carrying the exact environment herdr-webui's builtin backend
//! exports into panes: HERDR_WEBUI=1, TERM_PROGRAM=ghostty, the pane's TERM,
//! and KITTY_WINDOW_ID scrubbed. The parent spawns itself as a child with
//! that environment; the child calls init_picker() and asserts the picker
//! stays on Halfblocks (halfblock text art the browser can actually draw)
//! instead of Kitty, whose unicode-placeholder placements leak through as
//! U+10EEEE garbage.
//!
//! A control child with the identical environment minus HERDR_WEBUI proves
//! the same TERM_PROGRAM=ghostty hint selects Kitty without the gate, so the
//! assertion exercises the gate itself rather than an accident of the
//! environment. The fast-path children pin JCODE_MERMAID_PICKER_PROBE off and
//! scrub KITTY_WINDOW_ID/LC_TERMINAL/HERDR_WEBUI so the outcome is
//! deterministic no matter where the suite runs (including inside a webui
//! pane). Separate probe-path children opt into the real stdio probe and run
//! behind a controlled pseudo terminal that answers the upstream Kitty query.

#[cfg(unix)]
use std::io;
#[cfg(unix)]
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
#[cfg(unix)]
use std::process::Stdio;
use std::process::{Command, Output};
#[cfg(unix)]
use std::time::{Duration, Instant};

const CHILD_MODE_VAR: &str = "JCODE_TEST_WEBUI_GATE_CHILD";
const TEST_NAME: &str = "herdr_webui_gate_picks_halfblocks_and_control_picks_kitty";
#[cfg(unix)]
const PROBE_TEST_NAME: &str = "successful_native_probe_respects_herdr_webui_gate";
#[cfg(unix)]
const KITTY_PROBE_RESPONSE: &str = "\x1b_Gi=31;OK\x1b\\\x1b[6;7;14t\x1b[0n";

fn spawn_gate_child(mode: &'static str) -> Output {
    let exe = std::env::current_exe().expect("test binary path");
    let mut command = Command::new(exe);
    command
        .arg("--exact")
        .arg(TEST_NAME)
        .arg("--nocapture")
        .env(CHILD_MODE_VAR, mode)
        // Pin the exact pane environment the builtin webui backend exports.
        .env("TERM", "xterm-256color")
        .env("TERM_PROGRAM", "ghostty")
        .env_remove("KITTY_WINDOW_ID")
        .env_remove("LC_TERMINAL")
        // The probe is opt-in and queries stdio, which is piped here; pin the
        // default Fast init mode so the child is deterministic.
        .env_remove("JCODE_MERMAID_PICKER_PROBE");
    match mode {
        "webui" => {
            command.env("HERDR_WEBUI", "1");
        }
        "control" => {
            command.env_remove("HERDR_WEBUI");
        }
        other => panic!("unknown gate child mode: {other}"),
    }
    command.output().expect("spawn HERDR_WEBUI gate child")
}

fn require_gate_child(mode: &'static str, expected: &str) {
    let output = spawn_gate_child(mode);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "{mode} child failed with {:?}\nstdout:\n{stdout}\nstderr:\n{stderr}",
        output.status.code()
    );
    let marker = format!("GATE_RESULT={expected}");
    assert!(
        stdout.contains(&marker),
        "{mode} child did not report {marker}\nstdout:\n{stdout}"
    );
}

#[cfg(unix)]
struct PtyPair {
    master: OwnedFd,
    slave: OwnedFd,
}

#[cfg(unix)]
fn open_pty_pair() -> io::Result<PtyPair> {
    let mut master: RawFd = -1;
    let mut slave: RawFd = -1;
    // SAFETY: openpty initializes the two fd out-parameters on success. The
    // null termios/winsize pointers request platform defaults, which is the
    // same setup a terminal emulator would provide for this probe.
    let result = unsafe {
        libc::openpty(
            &mut master,
            &mut slave,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    };
    if result == -1 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: openpty returned owned file descriptors and we wrap each exactly
    // once so they close automatically.
    let master = unsafe { OwnedFd::from_raw_fd(master) };
    let slave = unsafe { OwnedFd::from_raw_fd(slave) };
    Ok(PtyPair { master, slave })
}

#[cfg(unix)]
fn dup_fd(fd: RawFd) -> io::Result<OwnedFd> {
    // SAFETY: dup does not take ownership of fd and returns a new owned fd on
    // success.
    let duplicated = unsafe { libc::dup(fd) };
    if duplicated == -1 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: duplicated is a fresh owned fd from dup.
    Ok(unsafe { OwnedFd::from_raw_fd(duplicated) })
}

#[cfg(unix)]
fn spawn_probe_child(mode: &'static str) -> io::Result<(std::process::Child, OwnedFd)> {
    let exe = std::env::current_exe().expect("test binary path");
    let pty = open_pty_pair()?;
    let slave_raw = pty.slave.as_raw_fd();
    let stdin = dup_fd(slave_raw)?;
    let stdout = dup_fd(slave_raw)?;
    let stderr = dup_fd(slave_raw)?;
    let mut command = Command::new(exe);
    command
        .arg("--exact")
        .arg(PROBE_TEST_NAME)
        .arg("--nocapture")
        .stdin(Stdio::from(stdin))
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(stderr))
        .env(CHILD_MODE_VAR, mode)
        // Keep env detection neutral so the final protocol comes from the
        // controlled probe response, not from inherited terminal hints.
        .env("TERM", "xterm-256color")
        .env_remove("TERM_PROGRAM")
        .env_remove("KITTY_WINDOW_ID")
        .env_remove("LC_TERMINAL")
        .env("JCODE_MERMAID_PICKER_PROBE", "1");
    match mode {
        "webui_probe" => {
            command.env("HERDR_WEBUI", "1");
        }
        "control_probe" => {
            command.env_remove("HERDR_WEBUI");
        }
        other => panic!("unknown probe child mode: {other}"),
    }
    let child = command.spawn()?;
    Ok((child, pty.master))
}

#[cfg(unix)]
fn read_probe_child(mut child: std::process::Child, master: OwnedFd) -> io::Result<String> {
    let master_fd = master.as_raw_fd();
    let mut output = Vec::new();
    let mut answered_probe = false;
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        let remaining = deadline.saturating_duration_since(Instant::now());
        let timeout_ms = remaining.as_millis().min(i32::MAX as u128) as i32;
        let mut poll_fd = libc::pollfd {
            fd: master_fd,
            events: libc::POLLIN | libc::POLLHUP | libc::POLLERR,
            revents: 0,
        };
        // SAFETY: poll_fd points to one valid poll descriptor and the timeout
        // is bounded by the child deadline.
        let poll_result = unsafe { libc::poll(&mut poll_fd, 1, timeout_ms) };
        if poll_result == -1 {
            let err = io::Error::last_os_error();
            if err.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            let _ = child.kill();
            let _ = child.wait();
            return Err(err);
        }
        if poll_result == 0 {
            break;
        }

        let mut buf = [0_u8; 256];
        // SAFETY: master_fd remains valid while master is alive and buf is a
        // valid writable byte slice.
        let read = unsafe { libc::read(master_fd, buf.as_mut_ptr().cast(), buf.len()) };
        if read > 0 {
            output.extend_from_slice(&buf[..read as usize]);
            if !answered_probe && output.windows(b"\x1b[5n".len()).any(|w| w == b"\x1b[5n") {
                // SAFETY: master_fd remains valid and the response buffer is a
                // valid readable byte slice. Writing to the pty master is how a
                // terminal emulator delivers probe responses to child stdin.
                let written = unsafe {
                    libc::write(
                        master_fd,
                        KITTY_PROBE_RESPONSE.as_ptr().cast(),
                        KITTY_PROBE_RESPONSE.len(),
                    )
                };
                assert_eq!(written, KITTY_PROBE_RESPONSE.len() as isize);
                answered_probe = true;
            }
        } else if read == 0 {
            break;
        } else {
            let err = io::Error::last_os_error();
            if err.kind() != io::ErrorKind::Interrupted {
                break;
            }
        }
        if child.try_wait()?.is_some() {
            break;
        }
    }

    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                format!(
                    "probe child exceeded the 10 second deadline; answered_probe={answered_probe}; pty output={:?}",
                    String::from_utf8_lossy(&output)
                ),
            ));
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    let output = String::from_utf8_lossy(&output).into_owned();
    assert!(
        status.success(),
        "probe child failed with {:?}\npty output:\n{output}",
        status.code()
    );
    assert!(
        answered_probe,
        "probe child never sent stdio query\npty output:\n{output}"
    );
    Ok(output)
}

#[cfg(unix)]
fn require_probe_child(mode: &'static str, expected: &str) {
    let (child, master) = spawn_probe_child(mode).expect("spawn HERDR_WEBUI probe child");
    let output = read_probe_child(child, master).expect("read HERDR_WEBUI probe child");
    let marker = format!("GATE_RESULT={expected}");
    assert!(
        output.contains(&marker),
        "{mode} child did not report {marker}\npty output:\n{output}"
    );
}

#[test]
fn herdr_webui_gate_picks_halfblocks_and_control_picks_kitty() {
    // Child mode: initialize the picker under the inherited environment and
    // report the resulting protocol; the parent asserts on the marker.
    if let Ok(mode) = std::env::var(CHILD_MODE_VAR) {
        let expected = match mode.as_str() {
            "webui" => "Some(Halfblocks)",
            "control" => "Some(Kitty)",
            other => panic!("unknown gate child mode: {other}"),
        };
        jcode_tui_mermaid::init_picker();
        let actual = format!("{:?}", jcode_tui_mermaid::protocol_type());
        println!("GATE_RESULT={actual}");
        assert_eq!(actual, expected, "{mode} child protocol mismatch");
        return;
    }

    require_gate_child("webui", "Some(Halfblocks)");
    require_gate_child("control", "Some(Kitty)");
}

#[cfg(unix)]
#[test]
fn successful_native_probe_respects_herdr_webui_gate() {
    // Child mode: initialize the picker with JCODE_MERMAID_PICKER_PROBE=1
    // while the parent pty answers the real ratatui-image Kitty/cell-size/status
    // protocol. This covers the successful native-probe path, not just the
    // default environment fast path.
    if let Ok(mode) = std::env::var(CHILD_MODE_VAR) {
        let expected = match mode.as_str() {
            "webui_probe" => "Some(Halfblocks)",
            "control_probe" => "Some(Kitty)",
            other => panic!("unknown probe child mode: {other}"),
        };
        jcode_tui_mermaid::init_picker();
        let actual = format!("{:?}", jcode_tui_mermaid::protocol_type());
        println!("GATE_RESULT={actual}");
        assert_eq!(actual, expected, "{mode} child protocol mismatch");
        return;
    }

    require_probe_child("webui_probe", "Some(Halfblocks)");
    require_probe_child("control_probe", "Some(Kitty)");
}
