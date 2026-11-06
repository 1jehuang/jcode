use clap::Subcommand;
use std::path::PathBuf;

/// Managed runtime policy is host-wide, not session-specific.
#[derive(Subcommand, Debug)]
pub(crate) enum CloudRuntimeCommand {
    /// Show the installed runtime, running daemon, and update policy.
    Status,
    /// Upload an explicit Linux x86_64 runtime package and select dev policy.
    /// Active daemons keep their executable until their next normal start.
    Push {
        /// Compatible jcode-linux-x86_64.tar.gz package, not a laptop executable.
        #[arg(long)]
        artifact: PathBuf,
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
                action: CloudRuntimeCommand::Push { artifact }
            })) if artifact == std::path::Path::new("portable.tar.gz")
        ));
    }

    #[test]
    fn runtime_policy_switches_require_confirmation() {
        assert!(Args::try_parse_from(["jcode", "cloud", "runtime", "stable"]).is_err());
        assert!(Args::try_parse_from(["jcode", "cloud", "runtime", "pin", "v0.99.1"]).is_err());
        assert!(Args::try_parse_from(["jcode", "cloud", "runtime", "stable", "--yes"]).is_ok());
    }
}
