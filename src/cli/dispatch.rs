use anyhow::Result;
use std::process::{Command as ProcessCommand, Stdio};

use super::args::{
    AmbientCommand, Args, AuthCommand, Command, MemoryCommand, ModelCommand, ProviderCommand,
    RestartCommand, TranscriptModeArg,
};
use crate::{
    agent, auth, build, provider, provider_catalog, server, session, setup_hints, startup_profile,
    tui,
};

use super::{
    commands, debug, hot_exec, login, output, provider_init, selfdev, terminal, tui_launch,
};
use provider_init::ProviderChoice;

pub(crate) async fn run_main(mut args: Args) -> Result<()> {
    resolve_resume_arg(&mut args)?;

    match args.command {
        Some(Command::Serve) => {
            crate::env::set_var("JCODE_NON_INTERACTIVE", "1");
            let provider =
                provider_init::init_provider(&args.provider, args.model.as_deref()).await?;
            let server = server::Server::new(provider);
            server.run().await?;
        }
        Some(Command::Connect) => {
            tui_launch::run_client().await?;
        }
        Some(Command::Run {
            message,
            json,
            ndjson,
        }) => {
            commands::run_single_message_command(
                &args.provider,
                args.model.as_deref(),
                &message,
                json,
                ndjson,
            )
            .await?;
        }
        Some(Command::Login { account }) => {
            login::run_login(&args.provider, account.as_deref()).await?;
        }
        Some(Command::Repl) => {
            let (provider, registry) =
                provider_init::init_provider_and_registry(&args.provider, args.model.as_deref())
                    .await?;
            let mut agent = agent::Agent::new(provider, registry);
            agent.repl().await?;
        }
        Some(Command::Update) => {
            hot_exec::run_update()?;
        }
        Some(Command::Version { json }) => {
            commands::run_version_command(json)?;
        }
        Some(Command::Usage { json }) => {
            commands::run_usage_command(json).await?;
        }
        Some(Command::SelfDev { build }) => {
            selfdev::run_self_dev(build, args.resume).await?;
        }
        Some(Command::Debug {
            command,
            arg,
            session,
            socket,
            wait,
        }) => {
            debug::run_debug_command(&command, &arg, session, socket, wait).await?;
        }
        Some(Command::Auth(subcmd)) => match subcmd {
            AuthCommand::Status { json } => commands::run_auth_status_command(json)?,
        },
        Some(Command::Provider(subcmd)) => match subcmd {
            ProviderCommand::List { json } => {
                commands::run_provider_list_command(json)?;
            }
            ProviderCommand::Current { json } => {
                commands::run_provider_current_command(&args.provider, args.model.as_deref(), json)
                    .await?;
            }
        },
        Some(Command::Memory(subcmd)) => {
            commands::run_memory_command(map_memory_subcommand(subcmd))?;
        }
        Some(Command::Ambient(subcmd)) => {
            commands::run_ambient_command(map_ambient_subcommand(subcmd)).await?;
        }
        Some(Command::Pair { list, revoke }) => {
            commands::run_pair_command(list, revoke)?;
        }
        Some(Command::Permissions) => {
            tui::permissions::run_permissions()?;
        }
        Some(Command::Transcript {
            text,
            mode,
            session,
        }) => {
            commands::run_transcript_command(text, map_transcript_mode(mode), session).await?;
        }
        Some(Command::Dictate { r#type }) => {
            commands::run_dictate_command(r#type).await?;
        }
        Some(Command::SetupHotkey {
            listen_macos_hotkey,
        }) => {
            setup_hints::run_setup_hotkey(listen_macos_hotkey)?;
        }
        Some(Command::SetupLauncher) => {
            setup_hints::run_setup_launcher()?;
        }
        Some(Command::Browser { action }) => {
            commands::run_browser(&action).await?;
        }
        Some(Command::Replay {
            session,
            swarm,
            export,
            speed,
            timeline,
            auto_edit,
            video,
            cols,
            rows,
            fps,
            centered,
            no_centered,
        }) => {
            let centered_override = if centered {
                Some(true)
            } else if no_centered {
                Some(false)
            } else {
                None
            };
            tui_launch::run_replay_command(
                &session,
                swarm,
                export,
                auto_edit,
                speed,
                timeline.as_deref(),
                video.as_deref(),
                cols,
                rows,
                fps,
                centered_override,
            )
            .await?;
        }
        Some(Command::Model(subcmd)) => match subcmd {
            ModelCommand::List { json, verbose } => {
                commands::run_model_command(&args.provider, args.model.as_deref(), json, verbose)
                    .await?;
            }
        },
        Some(Command::AuthTest {
            login,
            all_configured,
            no_smoke,
            no_tool_smoke,
            prompt,
            json,
            output,
        }) => {
            commands::run_auth_test_command(
                &args.provider,
                args.model.as_deref(),
                login,
                all_configured,
                no_smoke,
                no_tool_smoke,
                prompt.as_deref(),
                json,
                output.as_deref(),
            )
            .await?;
        }
        Some(Command::Restart { action }) => match action {
            RestartCommand::Save { auto_restore } => {
                commands::run_restart_save_command(auto_restore).await?
            }
            RestartCommand::Restore => commands::run_restart_restore_command()?,
            RestartCommand::Status => commands::run_restart_status_command()?,
            RestartCommand::Clear => commands::run_restart_clear_command()?,
        },
        None => run_default_command(args).await?,
    }

    Ok(())
}

fn resolve_resume_arg(args: &mut Args) -> Result<()> {
    if let Some(ref resume_id) = args.resume {
        if resume_id.is_empty() {
            return tui_launch::list_sessions();
        }

        match session::find_session_by_name_or_id(resume_id) {
            Ok(full_id) => {
                args.resume = Some(full_id);
            }
            Err(e) => {
                eprintln!("Error: {}", e);
                if !output::quiet_enabled() {
                    eprintln!("\nUse `jcode --resume` to list available sessions.");
                }
                std::process::exit(1);
            }
        }
    }

    Ok(())
}

fn map_memory_subcommand(subcmd: MemoryCommand) -> commands::MemorySubcommand {
    match subcmd {
        MemoryCommand::List { scope, tag } => commands::MemorySubcommand::List { scope, tag },
        MemoryCommand::Search { query, semantic } => {
            commands::MemorySubcommand::Search { query, semantic }
        }
        MemoryCommand::Export { output, scope } => {
            commands::MemorySubcommand::Export { output, scope }
        }
        MemoryCommand::Import {
            input,
            scope,
            overwrite,
        } => commands::MemorySubcommand::Import {
            input,
            scope,
            overwrite,
        },
        MemoryCommand::Stats => commands::MemorySubcommand::Stats,
        MemoryCommand::ClearTest => commands::MemorySubcommand::ClearTest,
    }
}

fn map_ambient_subcommand(subcmd: AmbientCommand) -> commands::AmbientSubcommand {
    match subcmd {
        AmbientCommand::Status => commands::AmbientSubcommand::Status,
        AmbientCommand::Log => commands::AmbientSubcommand::Log,
        AmbientCommand::Trigger => commands::AmbientSubcommand::Trigger,
        AmbientCommand::Stop => commands::AmbientSubcommand::Stop,
        AmbientCommand::RunVisible => commands::AmbientSubcommand::RunVisible,
    }
}

fn map_transcript_mode(mode: TranscriptModeArg) -> crate::protocol::TranscriptMode {
    match mode {
        TranscriptModeArg::Insert => crate::protocol::TranscriptMode::Insert,
        TranscriptModeArg::Append => crate::protocol::TranscriptMode::Append,
        TranscriptModeArg::Replace => crate::protocol::TranscriptMode::Replace,
        TranscriptModeArg::Send => crate::protocol::TranscriptMode::Send,
    }
}

#[allow(deprecated)]
async fn run_default_command(args: Args) -> Result<()> {
    startup_profile::mark("run_main_none_branch");

    if args.resume.is_none() && commands::maybe_run_pending_restart_restore_on_startup().await? {
        return Ok(());
    }

    let startup_hints = setup_hints::maybe_show_setup_hints();
    startup_profile::mark("setup_hints");

    if args.resume.is_none() {
        terminal::show_crash_resume_hint();
    }
    startup_profile::mark("crash_resume_hint");

    let cwd = std::env::current_dir()?;
    let in_jcode_repo = build::is_jcode_repo(&cwd);
    startup_profile::mark("is_jcode_repo");
    let already_in_selfdev = crate::cli::selfdev::client_selfdev_requested();

    if in_jcode_repo && !already_in_selfdev && !args.standalone && !args.no_selfdev {
        output::stderr_info("📍 Detected jcode repository - enabling self-dev mode");
        output::stderr_info("   Using shared server with self-dev session mode");
        output::stderr_info("   (use --no-selfdev to disable auto-detection)");
        output::stderr_blank_line();

        crate::env::set_var(selfdev::CLIENT_SELFDEV_ENV, "1");
        crate::process_title::set_initial_title(&args);
    }

    if args.standalone {
        output::stderr_info(
            "\x1b[33m⚠️  Warning: --standalone is deprecated and will be removed in a future version.\x1b[0m",
        );
        output::stderr_info(
            "\x1b[33m   The default server/client mode now handles all use cases including self-dev.\x1b[0m\n",
        );
        let (provider, registry) =
            provider_init::init_provider_and_registry(&args.provider, args.model.as_deref())
                .await?;
        tui_launch::run_tui(
            provider,
            registry,
            args.resume,
            args.debug_socket,
            startup_hints,
        )
        .await?;
        return Ok(());
    }

    startup_profile::mark("client_mode_start");
    let mut server_running = server_is_running().await;
    startup_profile::mark("server_check");

    if !server_running {
        server_running = wait_for_existing_reload_server("client startup").await;
    }

    if !server_running && std::env::var("JCODE_RESUMING").is_ok() {
        server_running = wait_for_resuming_server(
            "client startup without reload marker",
            std::time::Duration::from_secs(5),
        )
        .await;
    }

    if server_running && (args.provider != ProviderChoice::Auto || args.model.is_some()) {
        output::stderr_info(
            "Server already running; provider/model flags only apply when starting a new server.",
        );
        output::stderr_info(format!(
            "Current server settings control `/model`. Restart server to apply: --provider {}{}",
            args.provider.as_arg_value(),
            args.model
                .as_ref()
                .map(|m| format!(" --model {}", m))
                .unwrap_or_default()
        ));
    }

    if !server_running {
        maybe_prompt_server_bootstrap_login(&args.provider).await?;
        spawn_server(&args.provider, args.model.as_deref()).await?;
    }

    startup_profile::mark("pre_tui_client");
    if std::env::var("JCODE_RESUMING").is_err() && server_running {
        output::stderr_info("Connecting to server...");
    }
    tui_launch::run_tui_client(args.resume, startup_hints, !server_running).await?;

    Ok(())
}

pub(crate) async fn server_is_running() -> bool {
    server_is_running_at(&server::socket_path()).await
}

async fn wait_for_existing_reload_server(context: &str) -> bool {
    if let Some(state) = server::recent_reload_state(std::time::Duration::from_secs(30)) {
        match state.phase {
            server::ReloadPhase::Starting => {
                crate::logging::info(&format!(
                    "Reload state=starting during {}; waiting for existing server to return",
                    context
                ));
                return wait_for_reloading_server().await;
            }
            server::ReloadPhase::Failed => {
                crate::logging::warn(&format!(
                    "Reload state=failed during {}: {}",
                    context,
                    state
                        .detail
                        .unwrap_or_else(|| "unknown reload failure".to_string())
                ));
            }
            server::ReloadPhase::SocketReady => {}
        }
    }

    false
}

pub(crate) async fn wait_for_resuming_server(context: &str, timeout: std::time::Duration) -> bool {
    let socket_path = server::socket_path();
    let start = std::time::Instant::now();
    let mut announced = false;

    while start.elapsed() < timeout {
        if server_is_running_at(&socket_path).await {
            crate::logging::info(&format!(
                "Server became available during resume wait for {} after {}ms",
                context,
                start.elapsed().as_millis()
            ));
            return true;
        }

        if !announced {
            crate::logging::info(&format!(
                "Server not ready during {}; waiting up to {}ms for a resumed/reloading server before spawning a replacement",
                context,
                timeout.as_millis()
            ));
            announced = true;
        }

        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }

    false
}

pub(crate) async fn wait_for_reloading_server() -> bool {
    match server::await_reload_handoff(&server::socket_path(), std::time::Duration::from_secs(30))
        .await
    {
        server::ReloadWaitStatus::Ready => true,
        server::ReloadWaitStatus::Failed(detail) => {
            crate::logging::warn(&format!(
                "Reload handoff failed while waiting for server: {}",
                detail.unwrap_or_else(|| "unknown reload failure".to_string())
            ));
            false
        }
        server::ReloadWaitStatus::Idle => false,
        server::ReloadWaitStatus::Waiting { .. } => false,
    }
}

async fn server_is_running_at(path: &std::path::Path) -> bool {
    server::is_server_ready(path).await || server::has_live_listener(path).await
}

#[cfg(unix)]
fn spawn_lock_path(socket_path: &std::path::Path) -> std::path::PathBuf {
    std::path::PathBuf::from(format!("{}.spawning", socket_path.display()))
}

#[cfg(unix)]
struct SpawnLockGuard {
    _file: std::fs::File,
    path: std::path::PathBuf,
}

#[cfg(unix)]
impl Drop for SpawnLockGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

#[cfg(unix)]
fn try_acquire_spawn_lock(path: &std::path::Path) -> Result<Option<SpawnLockGuard>> {
    use std::fs::OpenOptions;
    use std::os::fd::AsRawFd;

    let file = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(path)?;
    let fd = file.as_raw_fd();
    let ret = unsafe { libc::flock(fd, libc::LOCK_EX | libc::LOCK_NB) };
    if ret == 0 {
        Ok(Some(SpawnLockGuard {
            _file: file,
            path: path.to_path_buf(),
        }))
    } else {
        Ok(None)
    }
}

#[cfg(unix)]
async fn acquire_spawn_lock_or_wait(
    socket_path: &std::path::Path,
) -> Result<Option<SpawnLockGuard>> {
    let lock_path = spawn_lock_path(socket_path);
    let wait_start = std::time::Instant::now();
    let wait_timeout = std::time::Duration::from_secs(10);
    let mut announced_wait = false;

    loop {
        if let Some(lock) = try_acquire_spawn_lock(&lock_path)? {
            return Ok(Some(lock));
        }

        if server_is_running_at(socket_path).await {
            return Ok(None);
        }

        if !announced_wait {
            output::stderr_info("Another client is starting the server, waiting...");
            announced_wait = true;
        }

        if wait_start.elapsed() >= wait_timeout {
            anyhow::bail!(
                "Timed out waiting for another client to start server at {}",
                socket_path.display()
            );
        }

        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
}

pub(crate) async fn maybe_prompt_server_bootstrap_login(
    provider_choice: &ProviderChoice,
) -> Result<()> {
    startup_profile::mark("cred_check_start");
    let (has_claude, has_openai) = tokio::join!(
        tokio::task::spawn_blocking(|| auth::claude::load_credentials().is_ok()),
        tokio::task::spawn_blocking(|| auth::codex::load_credentials().is_ok()),
    );
    let has_claude = has_claude.unwrap_or(false);
    let has_openai = has_openai.unwrap_or(false);
    let has_openrouter = provider::openrouter::OpenRouterProvider::has_credentials();
    let has_copilot = auth::copilot::has_copilot_credentials();
    let has_api_key = std::env::var("ANTHROPIC_API_KEY").is_ok();
    startup_profile::mark("cred_check_done");

    if !has_claude
        && !has_openai
        && !has_openrouter
        && !has_copilot
        && !has_api_key
        && !auth::AuthStatus::has_any_untrusted_external_auth()
        && *provider_choice == ProviderChoice::Auto
    {
        let provider = provider_init::prompt_login_provider_selection(
            &provider_catalog::server_bootstrap_login_providers(),
            "No credentials found. Let's log in!\n\nChoose a provider:",
        )?;
        login::run_login_provider(provider, None).await?;
        provider_init::apply_login_provider_profile_env(provider);
        output::stderr_blank_line();
    }

    Ok(())
}

pub(crate) async fn spawn_server(
    provider_choice: &ProviderChoice,
    model: Option<&str>,
) -> Result<()> {
    let socket_path = server::socket_path();
    if server_is_running_at(&socket_path).await {
        startup_profile::mark("server_ready");
        return Ok(());
    }

    if wait_for_existing_reload_server("server spawn").await {
        startup_profile::mark("server_ready");
        return Ok(());
    }

    #[cfg(unix)]
    let _spawn_lock = acquire_spawn_lock_or_wait(&socket_path).await?;

    if server_is_running_at(&socket_path).await {
        startup_profile::mark("server_ready");
        return Ok(());
    }

    if wait_for_existing_reload_server("server spawn after lock").await {
        startup_profile::mark("server_ready");
        return Ok(());
    }

    startup_profile::mark("server_spawn_start");
    output::stderr_info("Starting server...");
    let client_requested_selfdev = selfdev::client_selfdev_requested();
    let exe = build::client_update_candidate(client_requested_selfdev)
        .map(|(path, _)| path)
        .or_else(|| std::env::current_exe().ok())
        .ok_or_else(|| anyhow::anyhow!("Could not determine executable path for server spawn"))?;
    let mut cmd = ProcessCommand::new(&exe);
    cmd.env_remove(selfdev::CLIENT_SELFDEV_ENV);
    if client_requested_selfdev {
        cmd.env("JCODE_DEBUG_CONTROL", "1");
    }
    cmd.arg("--provider").arg(provider_choice.as_arg_value());
    if let Some(model) = model {
        cmd.arg("--model").arg(model);
    }
    cmd.arg("serve")
        .stdout(Stdio::null())
        .stderr(Stdio::piped());

    #[cfg(unix)]
    {
        let _child = server::spawn_server_notify(&mut cmd).await?;
        startup_profile::mark("server_ready");
    }
    #[cfg(not(unix))]
    {
        cmd.spawn()?;
        let start = std::time::Instant::now();
        while start.elapsed() < std::time::Duration::from_millis(500) {
            if crate::transport::is_socket_path(&server::socket_path()) {
                if crate::transport::Stream::connect(server::socket_path())
                    .await
                    .is_ok()
                {
                    startup_profile::mark("server_ready");
                    break;
                }
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{Request, ServerEvent, encode_event};
    use crate::transport::Listener;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

    struct ReloadTestEnv {
        prev_socket: Option<std::ffi::OsString>,
        prev_runtime: Option<std::ffi::OsString>,
        socket_path: std::path::PathBuf,
    }

    impl ReloadTestEnv {
        fn new() -> Self {
            let temp = tempfile::tempdir().expect("tempdir");
            let socket_path = temp.path().join("jcode.sock");
            let prev_socket = std::env::var_os("JCODE_SOCKET");
            let prev_runtime = std::env::var_os("JCODE_RUNTIME_DIR");
            crate::server::set_socket_path(socket_path.to_str().expect("utf8 socket path"));
            crate::env::set_var("JCODE_RUNTIME_DIR", temp.path());
            // Keep tempdir alive for the duration of the test helper.
            let _ = temp.keep();
            Self {
                prev_socket,
                prev_runtime,
                socket_path,
            }
        }
    }

    impl Drop for ReloadTestEnv {
        fn drop(&mut self) {
            crate::server::clear_reload_marker();
            let _ = std::fs::remove_file(&self.socket_path);
            if let Some(prev_socket) = &self.prev_socket {
                crate::env::set_var("JCODE_SOCKET", prev_socket);
            } else {
                crate::env::remove_var("JCODE_SOCKET");
            }
            if let Some(prev_runtime) = &self.prev_runtime {
                crate::env::set_var("JCODE_RUNTIME_DIR", prev_runtime);
            } else {
                crate::env::remove_var("JCODE_RUNTIME_DIR");
            }
        }
    }

    #[cfg(unix)]
    #[test]
    fn spawn_lock_serializes_shared_server_bootstrap() {
        let temp = tempfile::tempdir().expect("tempdir");
        let socket_path = temp.path().join("jcode.sock");
        let lock_path = spawn_lock_path(&socket_path);

        let first = try_acquire_spawn_lock(&lock_path)
            .expect("acquire first lock")
            .expect("first lock should succeed");
        let second = try_acquire_spawn_lock(&lock_path).expect("acquire second lock");
        assert!(
            second.is_none(),
            "second lock should be held by first guard"
        );

        drop(first);

        let third = try_acquire_spawn_lock(&lock_path)
            .expect("acquire third lock")
            .expect("third lock should succeed after release");
        drop(third);

        assert!(
            !lock_path.exists(),
            "lock file should be cleaned up when the guard drops"
        );
    }

    #[tokio::test]
    async fn wait_for_existing_reload_server_uses_reloading_server_instead_of_spawning() {
        let _guard = crate::storage::lock_test_env();
        let env = ReloadTestEnv::new();
        crate::server::write_reload_state(
            "reload-test",
            "hash",
            crate::server::ReloadPhase::Starting,
            None,
        );

        let bind_path = env.socket_path.clone();
        let bind_task = tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            #[allow(unused_mut)]
            let mut listener = Listener::bind(&bind_path).expect("bind replacement listener");
            let (stream, _) = listener.accept().await.expect("accept ping probe");
            let (reader, mut writer) = stream.into_split();
            let mut reader = BufReader::new(reader);
            let mut line = String::new();
            let n = reader
                .read_line(&mut line)
                .await
                .expect("read ping request");
            assert!(n > 0, "expected ping request");
            let request: Request = serde_json::from_str(&line).expect("parse ping request");
            let id = match request {
                Request::Ping { id } => id,
                other => panic!("expected ping request, got {other:?}"),
            };
            let pong = encode_event(&ServerEvent::Pong { id });
            writer.write_all(pong.as_bytes()).await.expect("write pong");
        });

        assert!(wait_for_existing_reload_server("test").await);

        bind_task.await.expect("bind task");
    }

    #[tokio::test]
    async fn wait_for_existing_reload_server_returns_false_for_failed_reload() {
        let _guard = crate::storage::lock_test_env();
        let _env = ReloadTestEnv::new();
        crate::server::write_reload_state(
            "reload-test",
            "hash",
            crate::server::ReloadPhase::Failed,
            Some("boom".to_string()),
        );

        assert!(!wait_for_existing_reload_server("test").await);
    }

    #[tokio::test]
    async fn wait_for_resuming_server_detects_delayed_listener_without_marker() {
        let _guard = crate::storage::lock_test_env();
        let env = ReloadTestEnv::new();

        let bind_path = env.socket_path.clone();
        let bind_task = tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            #[allow(unused_mut)]
            let mut listener = Listener::bind(&bind_path).expect("bind delayed listener");
            let (stream, _) = listener.accept().await.expect("accept ping probe");
            let (reader, mut writer) = stream.into_split();
            let mut reader = BufReader::new(reader);
            let mut line = String::new();
            let n = reader
                .read_line(&mut line)
                .await
                .expect("read ping request");
            assert!(n > 0, "expected ping request");
            let request: Request = serde_json::from_str(&line).expect("parse ping request");
            let id = match request {
                Request::Ping { id } => id,
                other => panic!("expected ping request, got {other:?}"),
            };
            let pong = encode_event(&ServerEvent::Pong { id });
            writer.write_all(pong.as_bytes()).await.expect("write pong");
        });

        assert!(
            wait_for_resuming_server("test", std::time::Duration::from_secs(1)).await,
            "resume wait should detect a delayed server without requiring a reload marker"
        );

        bind_task.await.expect("bind task");
    }

    #[tokio::test]
    async fn wait_for_reloading_server_returns_false_when_idle() {
        let _guard = crate::storage::lock_test_env();
        let _env = ReloadTestEnv::new();

        assert!(!wait_for_reloading_server().await);
    }

    #[tokio::test]
    async fn wait_for_reloading_server_returns_false_when_reload_failed() {
        let _guard = crate::storage::lock_test_env();
        let _env = ReloadTestEnv::new();
        crate::server::write_reload_state(
            "reload-test",
            "hash",
            crate::server::ReloadPhase::Failed,
            Some("boom".to_string()),
        );

        assert!(!wait_for_reloading_server().await);
    }

    #[tokio::test]
    async fn wait_for_reloading_server_returns_true_for_live_listener() {
        let _guard = crate::storage::lock_test_env();
        let env = ReloadTestEnv::new();
        let _listener = Listener::bind(&env.socket_path).expect("bind listener");

        assert!(wait_for_reloading_server().await);
    }

    #[tokio::test]
    async fn server_is_running_at_treats_live_listener_as_running_without_pong() {
        let temp = tempfile::tempdir().expect("tempdir");
        let socket_path = temp.path().join("jcode.sock");

        let _listener = Listener::bind(&socket_path).expect("bind listener");

        assert!(
            server_is_running_at(&socket_path).await,
            "a live listener should prevent duplicate server spawns even if ping is slow or absent"
        );
    }
}
