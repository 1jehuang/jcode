use clap::Subcommand;

/// Disposable machines, separate from the account's persistent cloud host.
#[derive(Subcommand, Debug)]
pub(crate) enum CloudEphemeralCommand {
    /// Create an isolated machine that is automatically destroyed at its deadline.
    Create {
        /// Hard lifetime in seconds, including startup time (5 minutes to 24 hours).
        #[arg(long, default_value_t = 3600, value_parser = clap::value_parser!(u32).range(300..=86400))]
        ttl_seconds: u32,
        /// Reuse this UUID when retrying a create request after a network failure.
        #[arg(long)]
        request_id: Option<String>,
        /// Optional short label for the run (letters, digits, hyphen, underscore).
        #[arg(long)]
        name: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// List your ephemeral machines. Does not wake or create any machine.
    List {
        #[arg(long)]
        json: bool,
    },
    /// Inspect one ephemeral machine without authorizing SSH.
    Status {
        id: String,
        #[arg(long)]
        json: bool,
    },
    /// Irreversibly destroy an ephemeral machine and its disk.
    Terminate {
        id: String,
        /// Explicitly acknowledge destruction of this machine's files.
        #[arg(long, required = true)]
        yes: bool,
        #[arg(long)]
        json: bool,
    },
    /// Attach the real local Jcode TUI to the remote machine, without tmux.
    Attach {
        id: String,
        /// Exact remote Jcode binary to use.
        #[arg(long, default_value = "/opt/jcode-cloud/jcode")]
        remote_binary: String,
        #[arg(long)]
        workdir: Option<String>,
        #[arg(long)]
        server_socket: Option<String>,
        #[arg(long)]
        session: Option<String>,
    },
    /// Run a command remotely. stdin/stdout are preserved for scripts and uploads.
    Exec {
        id: String,
        #[arg(last = true, required = true, num_args = 1..)]
        command: Vec<String>,
    },
    /// Internal SSH ProxyCommand for one ephemeral machine.
    #[command(hide = true)]
    Proxy { id: String },
}

#[cfg(test)]
mod tests {
    use crate::cli::args::{Args, CloudCommand, CloudEphemeralCommand, Command};
    use clap::Parser;

    #[test]
    fn ephemeral_cli_enforces_ttl_and_destructive_confirmation() {
        let args = Args::try_parse_from([
            "jcode",
            "cloud",
            "ephemeral",
            "create",
            "--ttl-seconds",
            "600",
            "--name",
            "bench-1",
            "--json",
        ])
        .unwrap();
        assert!(matches!(
            args.command,
            Some(Command::Cloud(CloudCommand::Ephemeral {
                action: CloudEphemeralCommand::Create {
                    ttl_seconds: 600,
                    json: true,
                    ..
                }
            }))
        ));
        for args in [
            vec![
                "jcode",
                "cloud",
                "ephemeral",
                "create",
                "--ttl-seconds",
                "299",
            ],
            vec![
                "jcode",
                "cloud",
                "ephemeral",
                "create",
                "--ttl-seconds",
                "86401",
            ],
            vec![
                "jcode",
                "cloud",
                "ephemeral",
                "terminate",
                "cm_0123456789abcdef0123456789abcdef",
            ],
        ] {
            assert!(Args::try_parse_from(args).is_err());
        }
        Args::try_parse_from([
            "jcode",
            "cloud",
            "ephemeral",
            "terminate",
            "cm_0123456789abcdef0123456789abcdef",
            "--yes",
        ])
        .unwrap();
    }

    #[test]
    fn ephemeral_attach_and_exec_do_not_steal_global_flags() {
        let id = "cm_0123456789abcdef0123456789abcdef";
        let args = Args::try_parse_from([
            "jcode",
            "cloud",
            "ephemeral",
            "attach",
            id,
            "--workdir",
            "/home/ec2-user/bench",
            "--server-socket",
            "/run/user/1000/bench.sock",
            "--session",
            "test",
            "--remote-binary",
            "/opt/bench/jcode",
        ])
        .unwrap();
        assert!(matches!(
            args.command,
            Some(Command::Cloud(CloudCommand::Ephemeral {
                action: CloudEphemeralCommand::Attach {
                    workdir: Some(_),
                    server_socket: Some(_),
                    session: Some(_),
                    ..
                }
            }))
        ));
        let args = Args::try_parse_from([
            "jcode",
            "cloud",
            "ephemeral",
            "exec",
            id,
            "--",
            "printf",
            "--help",
        ])
        .unwrap();
        assert!(
            matches!(args.command, Some(Command::Cloud(CloudCommand::Ephemeral {
            action: CloudEphemeralCommand::Exec { command, .. }
        })) if command == ["printf", "--help"])
        );
    }
}
