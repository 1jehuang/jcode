//! Native SSH attach: the TUI is local; its daemon and workspace are remote.

use anyhow::{Result, bail};

use super::args::{Args, Command};
use super::provider_init::ProviderChoice;

fn validate(args: &Args) -> Result<()> {
    match args.command {
        None | Some(Command::SelfDev { build: false }) => {}
        Some(Command::SelfDev { build: true }) => {
            bail!(
                "--ssh self-dev --build is not supported: run builds on the remote host, then reconnect"
            )
        }
        _ => bail!("--ssh supports the interactive client and self-dev only"),
    }
    if args.resume.as_deref().is_some_and(|id| {
        id.is_empty()
            || !id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
    }) {
        bail!(
            "--ssh --resume requires an explicit remote session ID; local session lookup is not used"
        )
    }
    if args.provider != ProviderChoice::Auto
        || args.model.is_some()
        || args.provider_profile.is_some()
        || args.tool_profile.is_some()
        || args.tools.is_some()
        || args.disabled_tools.is_some()
        || args.disable_base_tools
        || args.mcp_tools.is_some()
        || args.mcp_tools_token_threshold.is_some()
    {
        bail!(
            "provider/tool startup flags cannot configure an existing SSH server; use the remote /model command or configure the remote host"
        )
    }
    if args.onboarding_sim || args.update_sim {
        bail!("local onboarding/update simulators cannot run in an SSH session")
    }
    Ok(())
}

pub(crate) async fn run(args: Args) -> Result<()> {
    validate(&args)?;
    #[cfg(unix)]
    {
        run_unix(args).await
    }
    #[cfg(not(unix))]
    {
        let _ = args;
        bail!("native SSH TUI attach currently requires a Unix client")
    }
}

#[cfg(unix)]
async fn run_unix(args: Args) -> Result<()> {
    let host = args.ssh.as_deref().expect("SSH dispatch requires a host");
    let binary = args.ssh_binary.as_deref().unwrap_or("jcode");
    super::output::stderr_info(format!("Connecting local Jcode UI to {host} over SSH..."));
    let mut connection = super::ssh_transport::NativeSsh::connect_with_workspace(
        host,
        binary,
        args.ssh_server_socket.as_deref(),
        args.remote_working_dir.as_deref(),
    )
    .await?;
    let working_dir = connection.remote_working_dir().to_owned();
    crate::env::set_var("JCODE_SSH_REMOTE", host);
    crate::env::set_var("JCODE_SSH_BINARY", binary);
    crate::env::set_var("JCODE_SSH_WORKING_DIR", &working_dir);
    if let Some(socket) = args.ssh_server_socket.as_deref() {
        crate::env::set_var("JCODE_SSH_SERVER_SOCKET", socket);
    } else {
        crate::env::remove_var("JCODE_SSH_SERVER_SOCKET");
    }
    if matches!(args.command, Some(Command::SelfDev { .. })) {
        crate::env::set_var(super::selfdev::CLIENT_SELFDEV_ENV, "1");
    } else {
        crate::env::remove_var(super::selfdev::CLIENT_SELFDEV_ENV);
    }
    crate::server::set_socket_path(
        connection
            .socket_path()
            .to_str()
            .ok_or_else(|| anyhow::anyhow!("SSH adapter socket is not UTF-8"))?,
    );
    super::output::stderr_info(format!(
        "Remote: {} | workspace: {} | {}",
        connection.host(),
        working_dir,
        connection.handshake().version,
    ));
    // The guard remains alive for the whole UI, including reconnect attempts.
    // Each local connection gets its own owned SSH bridge to the shared daemon.
    use tokio::signal::unix::{SignalKind, signal};
    // Keep the transport guard outside the cancellable UI future and explicitly
    // reap its SSH children before returning to runtime shutdown.
    let mut hup = signal(SignalKind::hangup())?;
    let mut term = signal(SignalKind::terminate())?;
    let mut interrupt = signal(SignalKind::interrupt())?;
    let mut quit = signal(SignalKind::quit())?;
    let result = tokio::select! {
        result = super::tui_launch::run_tui_client(
            args.resume, None, false, true, Some(working_dir), false, false,
        ) => result,
        _ = hup.recv() => Ok(()),
        _ = term.recv() => Ok(()),
        _ = interrupt.recv() => Ok(()),
        _ = quit.recv() => Ok(()),
    };
    let cleanup = connection.close().await;
    result.and(cleanup)
}

fn quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

/// Return a remote-aware resume command, never a local session lookup.
pub(crate) fn resume_hint(session_id: &str) -> Option<String> {
    let host = std::env::var("JCODE_SSH_REMOTE").ok()?;
    let mut args = vec!["jcode".to_string(), "--ssh".to_string(), quote(&host)];
    for (flag, variable) in [
        ("--ssh-binary", "JCODE_SSH_BINARY"),
        ("--ssh-server-socket", "JCODE_SSH_SERVER_SOCKET"),
        ("--remote-working-dir", "JCODE_SSH_WORKING_DIR"),
    ] {
        if let Ok(value) = std::env::var(variable) {
            args.extend([flag.to_owned(), quote(&value)]);
        }
    }
    args.extend(["--resume".to_string(), quote(session_id)]);
    if super::selfdev::client_selfdev_requested() {
        args.push("self-dev".to_string());
    }
    Some(args.join(" "))
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[test]
    fn remote_modes_accept_explicit_remote_ids_without_local_lookup() {
        for argv in [
            vec!["jcode", "--ssh", "dev"],
            vec![
                "jcode",
                "--ssh",
                "dev",
                "--resume",
                "session_remote_123",
                "self-dev",
            ],
        ] {
            validate(&Args::try_parse_from(argv).unwrap()).unwrap();
        }
    }

    #[test]
    fn remote_modes_reject_local_only_operations_before_connecting() {
        for tail in [
            vec!["--resume"],
            vec!["self-dev", "--build"],
            vec!["run", "test"],
            vec!["--model", "local-model"],
            vec!["--onboarding-sim"],
            vec!["--tools", "bash"],
        ] {
            let mut argv = vec!["jcode", "--ssh", "dev"];
            argv.extend(tail);
            assert!(validate(&Args::try_parse_from(argv).unwrap()).is_err());
        }
    }

    #[test]
    fn resume_hint_retains_remote_identity_and_quotes_paths() {
        let _lock = crate::storage::lock_test_env();
        let names = [
            "JCODE_SSH_REMOTE",
            "JCODE_SSH_BINARY",
            "JCODE_SSH_WORKING_DIR",
            "JCODE_SSH_SERVER_SOCKET",
            super::super::selfdev::CLIENT_SELFDEV_ENV,
        ];
        let previous: Vec<_> = names.iter().map(std::env::var_os).collect();
        for name in names {
            crate::env::remove_var(name);
        }
        crate::env::set_var("JCODE_SSH_REMOTE", "dev");
        crate::env::set_var("JCODE_SSH_WORKING_DIR", "/srv/sam's repo");
        let hint = resume_hint("session_remote_1").unwrap();
        assert!(hint.contains("--ssh 'dev'"));
        assert!(hint.contains("'/srv/sam'\\''s repo'"));
        assert!(hint.contains("--resume 'session_remote_1'"));
        for (name, value) in names.into_iter().zip(previous) {
            match value {
                Some(value) => crate::env::set_var(name, value),
                None => crate::env::remove_var(name),
            }
        }
    }
}
