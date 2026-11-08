//! Start `jcode companion` in the background when a local TUI client launches
//! and none is running, so global voice, opening and terminal spawns work
//! without a manual service. Controlled by `[companion] autostart`.

use std::time::Duration;

/// Whether this client should try to start a companion.
fn should_autostart(
    enabled: bool,
    native_ssh: bool,
    in_test: bool,
    env_off: bool,
    is_companion: bool,
) -> bool {
    enabled && !native_ssh && !in_test && !env_off && !is_companion
}

/// Fire-and-forget: checks for a live companion off the UI thread and spawns
/// a detached one if missing. Concurrent clients are safe: the companion
/// refuses to start when another one already answers on the socket.
pub fn spawn_if_needed(native_ssh: bool) {
    let enabled = crate::config::config().companion.autostart;
    let env_off = std::env::var("JCODE_COMPANION_AUTOSTART").as_deref() == Ok("0");
    if !should_autostart(
        enabled,
        native_ssh,
        crate::auth::running_in_test_harness(),
        env_off,
        std::env::var_os("JCODE_COMPANION").is_some(),
    ) {
        return;
    }
    let _ = std::thread::Builder::new()
        .name("jcode-companion-autostart".into())
        .spawn(|| {
            if crate::companion_client::is_running() {
                return;
            }
            if let Err(error) = spawn_detached() {
                crate::logging::warn(&format!("companion autostart failed: {error:#}"));
            } else {
                crate::logging::info("companion autostart: launched jcode companion");
                // Give it a moment, so the first open/spawn request finds it.
                std::thread::sleep(Duration::from_millis(200));
            }
        });
}

fn spawn_detached() -> anyhow::Result<()> {
    use std::process::{Command, Stdio};
    let exe = std::env::current_exe()?;
    let mut command = Command::new(exe);
    command
        .arg("companion")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    // Outlive this client: own session, not killed with the terminal.
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        command.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP);
    }
    command.spawn()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::should_autostart;

    #[test]
    fn autostart_respects_config_ssh_tests_env_and_self() {
        assert!(should_autostart(true, false, false, false, false));
        assert!(!should_autostart(false, false, false, false, false));
        assert!(!should_autostart(true, true, false, false, false));
        assert!(!should_autostart(true, false, true, false, false));
        assert!(!should_autostart(true, false, false, true, false));
        assert!(!should_autostart(true, false, false, false, true));
    }
}
