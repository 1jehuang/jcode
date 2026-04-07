use crate::cli::args::{AmbientCommand, Args, Command};

const LINUX_PROCESS_TITLE_LIMIT: usize = 15;
const KILLALL_PROCESS_NAME: &str = "jcode";

fn compact_process_title(prefix: &str, name: Option<&str>) -> String {
    let mut title = prefix.to_string();
    if let Some(name) = name.filter(|name| !name.is_empty()) {
        let remaining = LINUX_PROCESS_TITLE_LIMIT.saturating_sub(title.len());
        if remaining > 0 {
            title.push_str(&name.chars().take(remaining).collect::<String>());
        }
    }
    title
}

fn session_name(session_id: &str) -> String {
    crate::id::extract_session_name(session_id)
        .map(|name| name.to_string())
        .unwrap_or_else(|| session_id.to_string())
}

pub(crate) fn set_title(title: impl AsRef<str>) {
    proctitle::set_title(title.as_ref());
    set_killall_process_name();
}

fn set_killall_process_name() {
    #[cfg(target_os = "linux")]
    unsafe {
        let mut name = [0u8; 16];
        let bytes = KILLALL_PROCESS_NAME.as_bytes();
        let len = bytes.len().min(name.len().saturating_sub(1));
        name[..len].copy_from_slice(&bytes[..len]);
        let _ = libc::prctl(libc::PR_SET_NAME, name.as_ptr(), 0, 0, 0);
    }
}

pub(crate) fn set_server_title(server_name: &str) {
    set_title(compact_process_title("jcode:s:", Some(server_name)));
}

pub(crate) fn set_client_generic_title(is_selfdev: bool) {
    let prefix = if is_selfdev {
        "jcode:selfdev"
    } else {
        "jcode:client"
    };
    set_title(compact_process_title(prefix, None));
}

pub(crate) fn set_client_session_title(session_id: &str, is_selfdev: bool) {
    set_client_display_title(&session_name(session_id), is_selfdev);
}

pub(crate) fn set_client_display_title(session_name: &str, is_selfdev: bool) {
    let prefix = if is_selfdev { "jcode:d:" } else { "jcode:c:" };
    set_title(compact_process_title(prefix, Some(session_name)));
}

pub(crate) fn set_client_remote_display_title(
    server_name: &str,
    session_name: &str,
    is_selfdev: bool,
) {
    if server_name.is_empty() || server_name.eq_ignore_ascii_case("jcode") {
        set_client_display_title(session_name, is_selfdev);
        return;
    }
    let prefix = if is_selfdev { "jcode:d:" } else { "jcode:c:" };
    set_title(format!("{prefix}{server_name}/{session_name}"));
}

pub(crate) fn initial_title(args: &Args) -> String {
    match &args.command {
        Some(Command::Serve) => "jcode:server".to_string(),
        Some(Command::Connect) => "jcode:client".to_string(),
        Some(Command::Run { .. }) => "jcode run".to_string(),
        Some(Command::Login { .. }) => "jcode login".to_string(),
        Some(Command::Repl) => "jcode repl".to_string(),
        Some(Command::Update) => "jcode update".to_string(),
        Some(Command::Version { .. }) => "jcode version".to_string(),
        Some(Command::Usage { .. }) => "jcode usage".to_string(),
        Some(Command::SelfDev { .. }) => "jcode:selfdev".to_string(),
        Some(Command::Debug { .. }) => "jcode debug".to_string(),
        Some(Command::Auth(_)) => "jcode auth".to_string(),
        Some(Command::Provider(_)) => "jcode provider".to_string(),
        Some(Command::Memory(_)) => "jcode memory".to_string(),
        Some(Command::Ambient(subcommand)) => match subcommand {
            AmbientCommand::RunVisible => "jcode ambient visible".to_string(),
            _ => "jcode ambient".to_string(),
        },
        Some(Command::Pair { .. }) => "jcode pair".to_string(),
        Some(Command::Permissions) => "jcode permissions".to_string(),
        Some(Command::Transcript { .. }) => "jcode transcript".to_string(),
        Some(Command::Dictate { .. }) => "jcode dictate".to_string(),
        Some(Command::SetupHotkey {
            listen_macos_hotkey,
        }) => {
            if *listen_macos_hotkey {
                "jcode hotkey listener".to_string()
            } else {
                "jcode hotkey setup".to_string()
            }
        }
        Some(Command::Browser { .. }) => "jcode browser".to_string(),
        Some(Command::Replay { .. }) => "jcode replay".to_string(),
        Some(Command::Model(_)) => "jcode model".to_string(),
        Some(Command::AuthTest { .. }) => "jcode auth-test".to_string(),
        Some(Command::SetupLauncher) => "jcode setup-launcher".to_string(),
        None => {
            if let Some(resume) = args.resume.as_deref().filter(|resume| !resume.is_empty()) {
                let prefix = if crate::cli::selfdev::client_selfdev_requested() {
                    "jcode:d:"
                } else {
                    "jcode:c:"
                };
                compact_process_title(prefix, Some(&session_name(resume)))
            } else if crate::cli::selfdev::client_selfdev_requested() {
                "jcode:selfdev".to_string()
            } else {
                "jcode:client".to_string()
            }
        }
    }
}

pub(crate) fn set_initial_title(args: &Args) {
    set_title(initial_title(args));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::args::Args;
    use crate::storage::lock_test_env;
    use clap::Parser;

    const SELFDEV_ENV: &str = crate::cli::selfdev::CLIENT_SELFDEV_ENV;

    fn with_selfdev_env_removed<T>(f: impl FnOnce() -> T) -> T {
        let _guard = lock_test_env();
        let previous = std::env::var_os(SELFDEV_ENV);
        crate::env::remove_var(SELFDEV_ENV);
        let result = f();
        if let Some(value) = previous {
            crate::env::set_var(SELFDEV_ENV, value);
        }
        result
    }

    #[test]
    fn initial_title_labels_server() {
        with_selfdev_env_removed(|| {
            let args = Args::parse_from(["jcode", "serve"]);
            assert_eq!(initial_title(&args), "jcode:server");
        });
    }

    #[test]
    fn initial_title_labels_resume_client_with_short_name() {
        with_selfdev_env_removed(|| {
            let args = Args::parse_from(["jcode", "--resume", "session_fox_123"]);
            assert_eq!(initial_title(&args), "jcode:c:fox");
        });
    }

    #[test]
    fn initial_title_labels_selfdev_command() {
        with_selfdev_env_removed(|| {
            let args = Args::parse_from(["jcode", "self-dev"]);
            assert_eq!(initial_title(&args), "jcode:selfdev");
        });
    }
}
