//! PTY-level checks for secret input.
//!
//! The masking in [`super::read_secret_line`] only runs on the raw-mode TTY
//! branch, so an ordinary unit test cannot reach it: under `cargo test` stdin is
//! not a terminal and the function takes the piped-line fallback. That gap is
//! exactly how #660 happened, a prompt that echoed nothing at all and was
//! indistinguishable from a hung process.
//!
//! These tests open a real pty, drive the function on the far side, and assert
//! on the bytes the terminal received.

use std::io::{Read, Write};
use std::os::fd::FromRawFd;
use std::time::{Duration, Instant};

/// Read whatever the child has written, up to `timeout`, without blocking past
/// it. A pty master returns EIO rather than EOF once the slave closes.
fn drain(master: &mut std::fs::File, timeout: Duration) -> Vec<u8> {
    let deadline = Instant::now() + timeout;
    let mut out = Vec::new();
    let mut buf = [0u8; 4096];
    while Instant::now() < deadline {
        match master.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => out.extend_from_slice(&buf[..n]),
            Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(_) => break,
        }
    }
    out
}

/// Run `read_secret_line` in a forked child whose stdin/stderr are a pty slave,
/// type `typed` into it, and return (echoed_bytes, value_read_by_the_child).
fn read_secret_over_pty(typed: &str) -> (String, String) {
    // SAFETY: openpty/fork are the only way to give the function a real terminal.
    unsafe {
        let mut master_fd: libc::c_int = 0;
        let mut slave_fd: libc::c_int = 0;
        assert_eq!(
            libc::openpty(
                &mut master_fd,
                &mut slave_fd,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut()
            ),
            0,
            "openpty failed"
        );

        // Report the value back to the parent out-of-band, so the test can also
        // prove the masking did not corrupt what was read.
        let mut pipe_fds = [0 as libc::c_int; 2];
        assert_eq!(libc::pipe(pipe_fds.as_mut_ptr()), 0, "pipe failed");

        let pid = libc::fork();
        assert!(pid >= 0, "fork failed");
        if pid == 0 {
            libc::close(master_fd);
            libc::close(pipe_fds[0]);
            libc::dup2(slave_fd, libc::STDIN_FILENO);
            libc::dup2(slave_fd, libc::STDERR_FILENO);
            libc::close(slave_fd);

            // Hard deadline inside the child: a stuck read must not hang CI.
            libc::alarm(20);
            let value = super::read_secret_line().unwrap_or_default();
            let mut wr = std::fs::File::from_raw_fd(pipe_fds[1]);
            let _ = wr.write_all(value.as_bytes());
            let _ = wr.flush();
            libc::_exit(0);
        }

        libc::close(slave_fd);
        libc::close(pipe_fds[1]);
        let flags = libc::fcntl(master_fd, libc::F_GETFL);
        libc::fcntl(master_fd, libc::F_SETFL, flags | libc::O_NONBLOCK);

        let mut master = std::fs::File::from_raw_fd(master_fd);

        // Let the child enter raw mode before typing at it.
        std::thread::sleep(Duration::from_millis(150));
        let mut echoed = Vec::new();
        for ch in typed.chars() {
            let mut b = [0u8; 4];
            let s = ch.encode_utf8(&mut b);
            master.write_all(s.as_bytes()).expect("write to pty");
            master.flush().ok();
            echoed.extend_from_slice(&drain(&mut master, Duration::from_millis(60)));
        }
        master.write_all(b"\r").expect("write CR");
        master.flush().ok();
        let tail = drain(&mut master, Duration::from_millis(200));

        let mut rd = std::fs::File::from_raw_fd(pipe_fds[0]);
        let mut value = String::new();
        let _ = rd.read_to_string(&mut value);

        let mut status = 0;
        libc::waitpid(pid, &mut status, 0);

        let mut all = echoed.clone();
        all.extend_from_slice(&tail);
        (
            String::from_utf8_lossy(&all).into_owned(),
            value.trim().to_string(),
        )
    }
}

/// The #660 regression test. Before the fix this echoed **zero** bytes, which is
/// what made the prompt look like a hang; verified by reverting the echo and
/// watching this assertion fail with 0 masking characters.
#[test]
fn typing_a_secret_on_a_tty_echoes_a_mask_and_never_the_secret() {
    let secret = "AIzaSECRET12345";
    let (echoed, value) = read_secret_over_pty(secret);

    let stars = echoed.matches('*').count();
    assert!(
        stars >= secret.chars().count(),
        "expected one mask per typed char, got {stars} for {} chars; echoed={echoed:?}",
        secret.chars().count()
    );
    assert!(
        !echoed.contains(secret),
        "the secret must never reach the screen: {echoed:?}"
    );
    // Masking must not corrupt the value the caller receives.
    assert_eq!(value, secret, "read_secret_line returned the wrong value");
}

/// Backspace must erase a mask *and* a character, so the visible width keeps
/// matching the buffer.
#[test]
fn backspace_erases_one_mask_and_one_character() {
    let (echoed, value) = read_secret_over_pty("abc\u{7f}d");
    assert_eq!(value, "abd", "backspace should drop exactly one char");
    assert!(
        echoed.contains('\u{8}'),
        "expected a backspace erase sequence, got {echoed:?}"
    );
}
