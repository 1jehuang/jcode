//! Exit-mode idle hibernation waiter.
//!
//! When `session.hibernation_mode = "exit"`, an idle client re-execs itself
//! with `JCODE_HIBERNATE_WAIT=1` and its normal `--resume` arguments. The
//! process is intercepted in `main` before the Tokio runtime or any jcode
//! state exists, so it holds only a few hundred KiB of private memory. The
//! last frame stays on screen (the terminal is not restored across the exec).
//! The first byte of input wakes it, and it re-execs the same arguments
//! without the marker, resuming the session from the daemon.

pub const WAIT_ENV: &str = "JCODE_HIBERNATE_WAIT";

pub fn is_invocation() -> bool {
    std::env::var_os(WAIT_ENV).is_some()
}

/// Build the waiter command for `session_id` from the current invocation.
pub fn waiter_command(session_id: &str) -> std::io::Result<std::process::Command> {
    let exe = std::env::current_exe()?;
    let mut cmd = std::process::Command::new(exe);
    if jcode_selfdev_types::client_selfdev_requested() {
        cmd.arg("self-dev");
    }
    cmd.arg("--resume")
        .arg(session_id)
        .env(WAIT_ENV, "1")
        .env("JCODE_RESUMING", "1")
        .current_dir(std::env::current_dir()?);
    Ok(cmd)
}

pub fn run() -> anyhow::Result<()> {
    draw_notice();
    wait_for_input();
    let mut args = std::env::args_os();
    let exe = std::env::current_exe()?;
    let _argv0 = args.next();
    let mut cmd = std::process::Command::new(&exe);
    cmd.args(args)
        .env_remove(WAIT_ENV)
        .env("JCODE_RELOAD_FAST_START", "1");
    let err = crate::platform::replace_process(&mut cmd);
    Err(anyhow::anyhow!("failed to resume after hibernation: {err}"))
}

fn draw_notice() {
    use std::io::Write as _;
    let rows = crossterm::terminal::size()
        .map(|(_, rows)| rows)
        .unwrap_or(1);
    // Save cursor, move to the bottom row, clear it, dim text, restore cursor.
    let mut out = std::io::stdout().lock();
    let _ = write!(
        out,
        "\x1b7\x1b[{rows};1H\x1b[2K\x1b[2m\u{1f4a4} Hibernated \u{b7} client exited to free memory \u{b7} press any key to resume\x1b[0m\x1b8"
    );
    let _ = out.flush();
}

fn wait_for_input() {
    // Raw mode is inherited from the exited client. Ensure it anyway so a
    // single key wakes us instead of waiting for Enter.
    let _ = crossterm::terminal::enable_raw_mode();
    #[cfg(unix)]
    {
        let mut byte = [0u8; 1];
        loop {
            let read = unsafe { libc::read(0, byte.as_mut_ptr().cast(), 1) };
            if read != 0
                && !(read < 0
                    && std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted)
            {
                break;
            }
            if read == 0 {
                // stdin closed (terminal went away): nothing to resume into.
                std::process::exit(0);
            }
        }
        // Drop the rest of an escape sequence so it is not typed into the draft.
        std::thread::sleep(std::time::Duration::from_millis(30));
        unsafe {
            libc::tcflush(0, libc::TCIFLUSH);
        }
    }
    #[cfg(not(unix))]
    {
        let _ = crossterm::event::read();
    }
}
