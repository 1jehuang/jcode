//! End-to-end PTY check, in a fresh process so crossterm's process-global
//! terminal reader cannot race other unit tests or inherit locked mutexes.

use super::*;
use std::io::{Read, Write};
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::process::CommandExt;
use std::process::{Command, Stdio};

async fn next_event(events: &mut EventStream) -> Event {
    use futures::StreamExt;
    tokio::time::timeout(Duration::from_secs(10), events.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap()
}

#[test]
fn pty_child() {
    if std::env::var_os("JCODE_TEST_THEME_PTY").is_none() {
        return;
    }
    crossterm::terminal::enable_raw_mode().unwrap();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        jcode_tui_style::set_theme_mode(ThemeMode::Dark);
        let mut events = EventStream::new();
        assert!(matches!(
            next_event(&mut events).await,
            Event::Resize(80, 24)
        ));
        assert_eq!(jcode_tui_style::theme_mode(), ThemeMode::Light);
        assert_eq!(
            next_event(&mut events).await,
            Event::Key(crossterm::event::KeyEvent::new(
                KeyCode::Char('x'),
                KeyModifiers::NONE
            ))
        );
        // The next reply is only sent when the periodic query appears, while
        // the process is otherwise idle. No user message or redraw is needed.
        assert!(matches!(
            next_event(&mut events).await,
            Event::Resize(80, 24)
        ));
        assert_eq!(jcode_tui_style::theme_mode(), ThemeMode::Dark);
        // Terminal paste must remain ordinary input, even when it contains a
        // string resembling a color report.
        assert_eq!(
            next_event(&mut events).await,
            Event::Paste("11;rgb:ff/ff/ff".to_string())
        );
    });
    println!("PTY_THEME_OK");
}

#[test]
fn live_queries_and_fragmented_color_replies_work_through_crossterm() {
    let mut master = -1;
    let mut slave = -1;
    let size = libc::winsize {
        ws_row: 24,
        ws_col: 80,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    // SAFETY: openpty initializes both descriptors, with no name or termios.
    assert_eq!(
        unsafe {
            libc::openpty(
                &mut master,
                &mut slave,
                std::ptr::null_mut(),
                std::ptr::null(),
                &size,
            )
        },
        0
    );
    // SAFETY: these descriptors are newly allocated and exclusively owned.
    let mut master = unsafe { std::fs::File::from_raw_fd(master) };
    let slave = unsafe { std::fs::File::from_raw_fd(slave) };
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args([
            "--exact",
            "tui::terminal_events::pty_tests::pty_child",
            "--nocapture",
        ])
        .env("JCODE_TEST_THEME_PTY", "1")
        .env("JCODE_THEME", "auto")
        .env("TERM", "xterm-256color")
        .stdin(Stdio::from(slave.try_clone().unwrap()))
        .stdout(Stdio::from(slave.try_clone().unwrap()))
        .stderr(Stdio::from(slave));
    // SAFETY: child-only controlling-terminal setup immediately before exec.
    // No allocation, locks, or runtime activity occurs in this closure.
    unsafe {
        command.pre_exec(|| {
            if libc::login_tty(libc::STDIN_FILENO) != 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let mut child = command.spawn().unwrap();
    let deadline = Instant::now() + Duration::from_secs(20);
    let mut output = Vec::new();
    let mut query_count = 0;
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if Instant::now() >= deadline {
            child.kill().unwrap();
            child.wait().unwrap();
            panic!("PTY test timed out: {}", String::from_utf8_lossy(&output));
        }
        let mut fd = libc::pollfd {
            fd: master.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        // SAFETY: fd describes the live master, the array length is one.
        if unsafe { libc::poll(&mut fd, 1, 50) } <= 0 || fd.revents & libc::POLLIN == 0 {
            continue;
        }
        let mut bytes = [0; 4096];
        match master.read(&mut bytes) {
            Ok(0) => continue,
            Ok(len) => output.extend_from_slice(&bytes[..len]),
            Err(error) if error.raw_os_error() == Some(libc::EIO) => continue,
            Err(error) => panic!("PTY read failed: {error}"),
        }
        let seen = output
            .windows(b"\x1b]11;?\x1b\\".len())
            .filter(|window| *window == b"\x1b]11;?\x1b\\")
            .count();
        if seen > query_count {
            query_count = seen;
            if seen == 1 {
                // Split every byte, including immediately after ESC. This
                // exercises crossterm's bare-Escape fallback on SSH fragments.
                for byte in b"\x1b]11;rgb:ffff/ffff/ffff\x1b\\x" {
                    master.write_all(&[*byte]).unwrap();
                    master.flush().unwrap();
                    std::thread::sleep(Duration::from_millis(20));
                }
            } else if seen == 2 {
                master
                    .write_all(b"\x1b]11;rgb:0000/0000/0000\x07\x1b[200~11;rgb:ff/ff/ff\x1b[201~")
                    .unwrap();
                master.flush().unwrap();
            }
        }
    };
    assert!(
        status.success(),
        "PTY child failed: {}",
        String::from_utf8_lossy(&output)
    );
    assert!(query_count >= 2, "periodic query was not emitted");
}
