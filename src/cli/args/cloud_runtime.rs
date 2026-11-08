use clap::Subcommand;
use std::path::PathBuf;

/// Managed runtime policy is host-wide, not session-specific.
#[derive(Subcommand, Debug)]
pub(crate) enum CloudRuntimeCommand {
    /// Show the installed runtime, running daemon, and update policy.
    Status,
    /// Upload a Linux x86_64 runtime package and select dev policy.
    /// Active daemons keep their executable until their next start, or until
    /// `--reload` hands the daemon off once no session is mid-turn.
    #[command(group(clap::ArgGroup::new("source").required(true).args(["artifact", "from_source"])))]
    Push {
        /// Compatible jcode-linux-x86_64.tar.gz package, not a laptop executable.
        #[arg(long)]
        artifact: Option<PathBuf>,
        /// Build a portable Linux package from this jcode checkout first.
        #[arg(long)]
        from_source: bool,
        /// After upload, hand the cloud daemon off to it once no session is mid-turn.
        #[arg(long)]
        reload: bool,
        /// Seconds to wait for running turns before giving up (with --reload).
        #[arg(long, default_value_t = 1800)]
        wait_secs: u64,
    },
    /// Hand the cloud daemon off to the selected runtime once no session is
    /// mid-turn. Daemons with live reload exec in place and keep clients
    /// attached. Older ones restart, and sessions resume on reconnect.
    Reload {
        /// Restart immediately even if turns are running (they are interrupted).
        #[arg(long)]
        force: bool,
        /// Seconds to wait for running turns before giving up.
        #[arg(long, default_value_t = 1800)]
        wait_secs: u64,
    },
    /// Return to automatic managed stable updates (may select an older release).
    Stable {
        /// Acknowledge replacing the host-wide dev/pinned selection.
        #[arg(long, required = true)]
        yes: bool,
    },
    /// Pin an exact published release instead of following automatic updates.
    Pin {
        /// Exact numeric release tag, for example v0.99.1.
        version: String,
        /// Acknowledge changing the host-wide runtime selection.
        #[arg(long, required = true)]
        yes: bool,
    },
    /// Check the managed channel now without restarting active daemons.
    Check,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::args::{Args, CloudCommand, Command};
    use clap::Parser;

    #[test]
    fn runtime_push_requires_an_explicit_artifact() {
        assert!(Args::try_parse_from(["jcode", "cloud", "runtime", "push"]).is_err());
        let parsed = Args::try_parse_from([
            "jcode",
            "cloud",
            "runtime",
            "push",
            "--artifact",
            "portable.tar.gz",
        ])
        .unwrap();
        assert!(matches!(
            parsed.command,
            Some(Command::Cloud(CloudCommand::Runtime {
                action: CloudRuntimeCommand::Push { artifact: Some(artifact), from_source: false, reload: false, .. }
            })) if artifact == std::path::Path::new("portable.tar.gz")
        ));
        assert!(
            Args::try_parse_from([
                "jcode",
                "cloud",
                "runtime",
                "push",
                "--artifact",
                "a.tar.gz",
                "--from-source"
            ])
            .is_err()
        );
        let parsed = Args::try_parse_from([
            "jcode",
            "cloud",
            "runtime",
            "push",
            "--from-source",
            "--reload",
        ])
        .unwrap();
        assert!(matches!(
            parsed.command,
            Some(Command::Cloud(CloudCommand::Runtime {
                action: CloudRuntimeCommand::Push {
                    artifact: None,
                    from_source: true,
                    reload: true,
                    wait_secs: 1800
                }
            }))
        ));
        assert!(Args::try_parse_from(["jcode", "cloud", "runtime", "reload", "--force"]).is_ok());
    }

    #[test]
    fn runtime_policy_switches_require_confirmation() {
        assert!(Args::try_parse_from(["jcode", "cloud", "runtime", "stable"]).is_err());
        assert!(Args::try_parse_from(["jcode", "cloud", "runtime", "pin", "v0.99.1"]).is_err());
        assert!(Args::try_parse_from(["jcode", "cloud", "runtime", "stable", "--yes"]).is_ok());
    }
}
