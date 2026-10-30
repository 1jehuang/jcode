//! Native SSH attach: the TUI is local; its daemon and workspace are remote.

use anyhow::{Context, Result, bail};

use super::args::{Args, Command};
use super::provider_init::ProviderChoice;

static CLIENT_RELOAD: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);

pub(crate) fn stash_client_reload(session_id: String) {
    *CLIENT_RELOAD
        .lock()
        .unwrap_or_else(|error| error.into_inner()) = Some(session_id);
}

fn client_reload_command(
    executable: &std::path::Path,
    args: &Args,
    session_id: &str,
    working_dir: &str,
) -> std::process::Command {
    let mut command = std::process::Command::new(executable);
    command
        .arg("--ssh")
        .arg(args.ssh.as_deref().expect("SSH reload target"));
    if let Some(binary) = args.ssh_binary.as_deref() {
        command.arg("--ssh-binary").arg(binary);
    }
    if let Some(socket) = args.ssh_server_socket.as_deref() {
        command.arg("--ssh-server-socket").arg(socket);
    }
    command.arg("--remote-working-dir").arg(working_dir);
    command.arg("--resume").arg(session_id).arg("--no-update");
    if matches!(args.command, Some(Command::SelfDev { .. })) {
        command.arg("self-dev");
    }
    // The previous terminal was fully restored and the adapter socket died
    // with its SSH guard. The fresh process must create both again.
    command.env_remove("JCODE_RESUMING");
    command
}

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
    if let Some(id) = super::cloud_ephemeral::host_id(host) {
        let id = id.to_owned();
        tokio::task::spawn_blocking(move || super::cloud_ephemeral::wait_ready(&id))
            .await
            .context("wait for ephemeral cloud machine")??;
    }
    super::output::stderr_info(format!("Connecting local Jcode UI to {host} over SSH..."));
    let mut connection = super::ssh_transport::NativeSsh::connect_with_workspace(
        host,
        binary,
        args.ssh_server_socket.as_deref(),
        args.remote_working_dir.as_deref(),
    )
    .await?;
    let working_dir = connection.remote_working_dir().to_owned();
    // The TUI stays local. Preserve its workspace separately from the VM's,
    // and share the exact managed-host config with read-only metadata probes.
    let explicit_source = std::env::var_os("JCODE_SSH_LOCAL_WORKING_DIR").is_some();
    crate::env::set_var(
        "JCODE_SSH_LOCAL_IS_SOURCE",
        if explicit_source { "1" } else { "0" },
    );
    if let Some(local_dir) = std::env::var_os("JCODE_SSH_LOCAL_WORKING_DIR")
        .map(std::path::PathBuf::from)
        .or_else(|| std::env::current_dir().ok())
    {
        crate::env::set_var("JCODE_SSH_LOCAL_WORKING_DIR", local_dir);
    }
    if let Some(id) = super::cloud_ephemeral::host_id(host) {
        crate::env::set_var(
            "JCODE_SSH_CONFIG",
            super::cloud_ephemeral::ssh_config_path(id)?,
        );
    } else if super::cloud_managed::is_managed(host) {
        let _ = super::cloud_managed::ssh_args()?;
        crate::env::set_var("JCODE_SSH_CONFIG", super::cloud_managed::ssh_config_path()?);
    } else {
        crate::env::remove_var("JCODE_SSH_CONFIG");
    }
    crate::env::set_var("JCODE_SSH_REMOTE", host);
    crate::env::set_var("JCODE_SSH_BINARY", binary);
    crate::env::set_var("JCODE_SSH_WORKING_DIR", &working_dir);
    // The local migration lease is the source of truth for the last transfer.
    // Do not reuse a timestamp from a previous attachment or another host.
    let lease = args
        .resume
        .as_deref()
        .and_then(crate::storage::read_session_lease);
    if let Some(timestamp) = last_transfer_for_host(host, lease.as_ref()) {
        crate::env::set_var("JCODE_SSH_LAST_TRANSFER_AT", timestamp);
    } else {
        crate::env::remove_var("JCODE_SSH_LAST_TRANSFER_AT");
    }
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
            args.resume.clone(), None, false, true, Some(working_dir.clone()), false, false,
        ) => result,
        _ = hup.recv() => Ok(()),
        _ = term.recv() => Ok(()),
        _ = interrupt.recv() => Ok(()),
        _ = quit.recv() => Ok(()),
    };
    let cleanup = connection.close().await;
    let result = result.and(cleanup);
    if result.is_ok()
        && let Some(handoff) = super::cloud_move::take_stashed_handoff()
    {
        return super::cloud_move::exec_handoff(handoff);
    }
    if result.is_ok() {
        let reload = CLIENT_RELOAD
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .take();
        if let Some(session_id) = reload {
            let is_selfdev = matches!(args.command, Some(Command::SelfDev { .. }));
            let executable = crate::build::preferred_reload_candidate(is_selfdev)
                .map(|(path, _)| path)
                .unwrap_or(std::env::current_exe()?);
            let mut command = client_reload_command(&executable, &args, &session_id, &working_dir);
            let error = crate::platform::replace_process(&mut command);
            return Err(anyhow::anyhow!("Failed to reload SSH client: {error}"));
        }
    }
    result
}

fn quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn last_transfer_for_host<'a>(
    host: &str,
    lease: Option<&'a crate::storage::SessionLease>,
) -> Option<&'a str> {
    let lease = lease?;
    if lease.away_host.as_deref() != Some(host)
        || chrono::DateTime::parse_from_rfc3339(&lease.updated_at).is_err()
    {
        return None;
    }
    Some(&lease.updated_at)
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
    fn ssh_client_reload_preserves_remote_transport_and_workspace() {
        let args = Args::try_parse_from([
            "jcode",
            "--ssh",
            "dev",
            "--ssh-binary",
            "/opt/jcode",
            "--ssh-server-socket",
            "/run/remote.sock",
            "--resume",
            "session_old",
            "self-dev",
        ])
        .unwrap();
        let command = client_reload_command(
            std::path::Path::new("/local/new-jcode"),
            &args,
            "session_remote",
            "/vm/project space",
        );
        let argv = command
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        let parsed = Args::try_parse_from(std::iter::once("jcode".to_owned()).chain(argv)).unwrap();
        validate(&parsed).unwrap();
        assert_eq!(command.get_program(), "/local/new-jcode");
        assert_eq!(parsed.ssh.as_deref(), Some("dev"));
        assert_eq!(parsed.ssh_binary.as_deref(), Some("/opt/jcode"));
        assert_eq!(
            parsed.ssh_server_socket.as_deref(),
            Some("/run/remote.sock")
        );
        assert_eq!(
            parsed.remote_working_dir.as_deref(),
            Some("/vm/project space")
        );
        assert_eq!(parsed.resume.as_deref(), Some("session_remote"));
        assert!(matches!(
            parsed.command,
            Some(Command::SelfDev { build: false })
        ));
        assert!(
            command
                .get_envs()
                .any(|(key, value)| key == "JCODE_RESUMING" && value.is_none())
        );
    }

    #[test]
    fn transfer_timestamp_requires_matching_host_and_valid_lease() {
        let mut lease = crate::storage::SessionLease {
            session_id: "session_cloud_widget".into(),
            epoch: 1,
            away_host: Some("cloud-box".into()),
            repo_root: Some("/repo".into()),
            last_sync_snapshot: None,
            last_sync_head: None,
            updated_at: "2026-10-20T07:30:00Z".into(),
        };
        assert_eq!(
            last_transfer_for_host("cloud-box", Some(&lease)),
            Some("2026-10-20T07:30:00Z")
        );
        assert_eq!(last_transfer_for_host("other-box", Some(&lease)), None);
        assert_eq!(last_transfer_for_host("cloud-box", None), None);
        lease.away_host = None;
        assert_eq!(last_transfer_for_host("cloud-box", Some(&lease)), None);
        lease.away_host = Some("cloud-box".into());
        lease.updated_at = "not-a-timestamp".into();
        assert_eq!(last_transfer_for_host("cloud-box", Some(&lease)), None);
    }

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
