//! Mapping from parsed CLI arguments to an initial process title.
//!
//! This logic depends on the clap `Args`/`Command` types defined in `cli`, so
//! it lives in the CLI layer. The low-level title-setting primitives it uses
//! (`compact_process_title`, `session_name`, `set_title`) live in the
//! `process_title` core module.

use crate::cli::args::{AmbientCommand, Args, Command};
use crate::process_title::{compact_process_title, session_name, set_title};

pub(crate) fn initial_title(args: &Args) -> String {
    match &args.command {
        Some(Command::Serve { .. }) => "jcode:server".to_string(),
        Some(Command::Acp) => "jcode acp".to_string(),
        Some(Command::Server { .. }) => "jcode server".to_string(),
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
        Some(Command::Session(_)) => "jcode session".to_string(),
        Some(Command::Ambient(subcommand)) => match subcommand {
            AmbientCommand::RunVisible => "jcode ambient visible".to_string(),
            _ => "jcode ambient".to_string(),
        },
        Some(Command::Cloud(_)) => "jcode cloud".to_string(),
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
        Some(Command::ProviderTestCoverage { .. }) => "jcode provider-test-coverage".to_string(),
        Some(Command::ProviderDoctor { .. }) => "jcode provider-doctor".to_string(),
        Some(Command::AuthTest { .. }) => "jcode auth-test".to_string(),
        Some(Command::Restart { .. }) => "jcode restart".to_string(),
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
    use crate::storage::lock_test_env;
    use clap::Parser;

    const SELFDEV_ENV: &str = jcode_selfdev_types::CLIENT_SELFDEV_ENV;

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
