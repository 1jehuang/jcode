//! Socket server and request handlers.

use crate::lid_override::LidOverride;
use anyhow::{Context, Result};
use jcode_base::companion_client::{
    CompanionRequest, CompanionResponse, PROTOCOL_VERSION, socket_path,
};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

/// The server re-reports activity every few seconds while a turn runs. If the
/// reports stop (server crashed or was killed), release the lid override
/// rather than holding it until the companion exits.
const POWER_LEASE: Duration = Duration::from_secs(30);
const BRIDGE_UPDATE_INTERVAL: Duration = Duration::from_secs(60 * 60);
const MAX_REQUEST_BYTES: usize = 64 * 1024;

#[derive(Debug, Clone)]
pub struct CompanionOptions {
    pub socket: PathBuf,
    /// Run the global push-to-talk listener (Linux).
    pub voice: bool,
    /// Periodically update an installed browser bridge.
    pub browser_updates: bool,
}

impl Default for CompanionOptions {
    fn default() -> Self {
        Self {
            socket: socket_path(),
            voice: true,
            browser_updates: true,
        }
    }
}

/// Shared state for request handlers.
pub struct Companion {
    lid: Mutex<LidState>,
    capabilities: Vec<String>,
}

struct LidState {
    lid: Option<LidOverride>,
    active_until: Option<Instant>,
}

impl Companion {
    fn new(capabilities: Vec<String>) -> Self {
        let mut lid = LidOverride::for_current_platform();
        // Repair settings a crashed earlier owner left behind.
        if let Some(lid) = lid.as_mut() {
            lid.recover_stale();
        }
        Self {
            lid: Mutex::new(LidState {
                lid,
                active_until: None,
            }),
            capabilities,
        }
    }

    pub async fn handle(&self, request: CompanionRequest) -> CompanionResponse {
        match request {
            CompanionRequest::Status => CompanionResponse {
                ok: true,
                message: format!("jcode companion {}", env!("CARGO_PKG_VERSION")),
                protocol: PROTOCOL_VERSION,
                capabilities: self.capabilities.clone(),
            },
            CompanionRequest::Open { target } => match open(&target).await {
                Ok(backend) => CompanionResponse::ok(backend),
                Err(error) => CompanionResponse::error(format!("{error:#}")),
            },
            CompanionRequest::Reveal { path } => {
                let kind = jcode_desktop_open::LocalTargetKind::of(&path);
                match jcode_desktop_open::reveal_target(&path, kind).await {
                    Ok((backend, _)) => CompanionResponse::ok(backend),
                    Err(error) => CompanionResponse::error(format!("{error:#}")),
                }
            }
            CompanionRequest::SpawnTerminal {
                program,
                args,
                cwd,
                title,
                kind,
                session_id,
                env,
                client_terminal_env,
            } => {
                let mut command = jcode_base::terminal_launch::TerminalCommand::new(program, args)
                    .client_terminal_env(client_terminal_env);
                if let Some(title) = title {
                    command = command.title(title);
                }
                if let Some(kind) = kind {
                    command = command.kind(kind);
                }
                if let Some(session_id) = session_id {
                    command = command.session_id(session_id);
                }
                for (key, value) in env {
                    command = command.spawn_env(key, value);
                }
                let spawn = tokio::task::spawn_blocking(move || {
                    jcode_base::terminal_launch::spawn_command_in_new_terminal(&command, &cwd)
                })
                .await;
                match spawn {
                    Ok(Ok(true)) => CompanionResponse::ok("terminal"),
                    Ok(Ok(false)) => CompanionResponse::error("no terminal emulator found"),
                    Ok(Err(error)) => CompanionResponse::error(format!("{error:#}")),
                    Err(error) => CompanionResponse::error(error.to_string()),
                }
            }
            CompanionRequest::PowerActivity { active } => {
                self.set_power_active(active, Instant::now());
                CompanionResponse::ok(if active { "engaged" } else { "released" })
            }
        }
    }

    fn set_power_active(&self, active: bool, now: Instant) {
        let mut state = self.lid.lock().unwrap_or_else(|p| p.into_inner());
        state.active_until = active.then(|| now + POWER_LEASE);
        if let Some(lid) = state.lid.as_mut() {
            lid.set_active(active);
        }
    }

    /// Release the override once the server stops renewing its lease.
    fn expire_power_lease(&self, now: Instant) {
        let mut state = self.lid.lock().unwrap_or_else(|p| p.into_inner());
        if state.active_until.is_some_and(|until| now >= until) {
            state.active_until = None;
            if let Some(lid) = state.lid.as_mut() {
                lid.set_active(false);
            }
            jcode_base::logging::info("companion: power lease expired, lid override released");
        }
    }

    fn release_power(&self) {
        let mut state = self.lid.lock().unwrap_or_else(|p| p.into_inner());
        state.active_until = None;
        if let Some(lid) = state.lid.as_mut() {
            lid.set_active(false);
        }
    }
}

async fn open(target: &str) -> Result<String> {
    let target = jcode_desktop_open::Target::parse(target)?;
    jcode_desktop_open::open_target(&target).await
}

/// Run the companion until SIGTERM/SIGINT. Fails if another companion already
/// owns the socket.
pub async fn run(options: CompanionOptions) -> Result<()> {
    // Terminal spawns from this process must use the local launcher, never
    // loop back into the companion socket.
    jcode_base::env::set_var("JCODE_COMPANION", "1");

    let listener = bind(&options.socket)?;
    // Windows named-pipe acceptance replaces the listening pipe instance.
    #[cfg(windows)]
    let mut listener = listener;
    let mut capabilities = vec![
        "open".to_string(),
        "reveal".to_string(),
        "spawn_terminal".to_string(),
    ];
    if cfg!(any(target_os = "macos", windows)) {
        capabilities.push("lid".to_string());
    }
    if options.voice && cfg!(target_os = "linux") {
        capabilities.push("voice".to_string());
    }
    let companion = Arc::new(Companion::new(capabilities));
    jcode_base::logging::info(&format!(
        "companion: listening on {}",
        options.socket.display()
    ));
    eprintln!("jcode companion listening on {}", options.socket.display());

    #[cfg(target_os = "linux")]
    if options.voice {
        crate::voice::spawn();
    }
    if options.browser_updates {
        tokio::spawn(browser_bridge_updates());
    }
    {
        let companion = companion.clone();
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(Duration::from_secs(5));
            loop {
                tick.tick().await;
                companion.expire_power_lease(Instant::now());
            }
        });
    }

    let accept = {
        let companion = companion.clone();
        async move {
            loop {
                let (stream, _) = listener.accept().await?;
                let companion = companion.clone();
                tokio::spawn(async move {
                    if let Err(error) = serve(stream, &companion).await {
                        jcode_base::logging::warn(&format!("companion: connection: {error:#}"));
                    }
                });
            }
            #[allow(unreachable_code)]
            Ok::<(), anyhow::Error>(())
        }
    };

    let result = tokio::select! {
        result = accept => result,
        result = shutdown_signal() => result,
    };
    companion.release_power();
    jcode_transport::remove_socket(&options.socket);
    result
}

fn bind(path: &Path) -> Result<jcode_transport::Listener> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).with_context(|| {
            format!(
                "failed to create companion socket directory {}",
                parent.display()
            )
        })?;
    }
    if jcode_transport::is_socket_path(path) {
        // A live companion answers. Only a stale socket file may be replaced.
        if jcode_base::companion_client::request_at(
            path,
            &CompanionRequest::Status,
            Duration::from_millis(500),
        )
        .is_ok()
        {
            anyhow::bail!("jcode companion is already running ({})", path.display());
        }
        jcode_transport::remove_socket(path);
    }
    #[cfg(unix)]
    {
        jcode_transport::Listener::bind(path)
            .with_context(|| format!("failed to bind {}", path.display()))
    }
    #[cfg(windows)]
    {
        jcode_transport::Listener::bind(path)
            .with_context(|| format!("failed to bind {}", path.display()))
    }
}

async fn serve(stream: jcode_transport::Stream, companion: &Companion) -> Result<()> {
    let (read, mut write) = stream.into_split();
    let mut reader = BufReader::new(read);
    let mut line = String::new();
    loop {
        line.clear();
        let read = reader.read_line(&mut line).await?;
        if read == 0 {
            return Ok(());
        }
        if line.len() > MAX_REQUEST_BYTES {
            anyhow::bail!("request too large");
        }
        let response = match serde_json::from_str::<CompanionRequest>(line.trim()) {
            Ok(request) => companion.handle(request).await,
            Err(error) => CompanionResponse::error(format!("invalid request: {error}")),
        };
        let mut reply = serde_json::to_string(&response)?;
        reply.push('\n');
        write.write_all(reply.as_bytes()).await?;
    }
}

async fn shutdown_signal() -> Result<()> {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        let mut term = signal(SignalKind::terminate()).context("install SIGTERM handler")?;
        tokio::select! {
            received = term.recv() => received.context("SIGTERM signal stream closed")?,
            result = tokio::signal::ctrl_c() => result.context("wait for Ctrl-C")?,
        }
        Ok(())
    }
    #[cfg(not(unix))]
    {
        tokio::signal::ctrl_c().await.context("wait for Ctrl-C")
    }
}

/// Keep an installed browser bridge current. Previously the browser tool did
/// this inline before an action, adding a GitHub round trip to agent turns.
async fn browser_bridge_updates() {
    let mut tick = tokio::time::interval(BRIDGE_UPDATE_INTERVAL);
    loop {
        tick.tick().await;
        let Some(kind) = jcode_base::browser::saved_browser_preference() else {
            continue;
        };
        match jcode_base::browser::update_bridge_if_newer(kind, false).await {
            Ok(update) => {
                if let Some(note) = update.describe() {
                    jcode_base::logging::info(&format!("companion: {note}"));
                }
            }
            Err(error) => jcode_base::logging::warn(&format!(
                "companion: browser bridge update check failed: {error:#}"
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn companion() -> Companion {
        Companion {
            lid: Mutex::new(LidState {
                lid: None,
                active_until: None,
            }),
            capabilities: vec!["open".into()],
        }
    }

    #[tokio::test]
    async fn status_reports_protocol_and_capabilities() {
        let response = companion().handle(CompanionRequest::Status).await;
        assert!(response.ok);
        assert_eq!(response.protocol, PROTOCOL_VERSION);
        assert_eq!(response.capabilities, vec!["open".to_string()]);
    }

    #[tokio::test]
    async fn open_rejects_missing_paths_without_launching() {
        let response = companion()
            .handle(CompanionRequest::Open {
                target: "/definitely/not/here/xyz".into(),
            })
            .await;
        assert!(!response.ok);
        assert!(response.message.contains("does not exist"));
    }

    #[test]
    fn power_lease_expires_without_renewal() {
        let companion = companion();
        let start = Instant::now();
        companion.set_power_active(true, start);
        companion.expire_power_lease(start + POWER_LEASE / 2);
        assert!(companion.lid.lock().unwrap().active_until.is_some());
        companion.expire_power_lease(start + POWER_LEASE);
        assert!(companion.lid.lock().unwrap().active_until.is_none());
        companion.set_power_active(true, start);
        companion.set_power_active(false, start);
        assert!(companion.lid.lock().unwrap().active_until.is_none());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn serves_requests_over_the_socket() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("companion.sock");
        let listener = bind(&path).unwrap();
        let companion = Arc::new(companion());
        let server = {
            let companion = companion.clone();
            tokio::spawn(async move {
                let (stream, _) = listener.accept().await.unwrap();
                serve(stream, &companion).await.unwrap();
            })
        };
        let client_path = path.clone();
        let response = tokio::task::spawn_blocking(move || {
            jcode_base::companion_client::request_at(
                &client_path,
                &CompanionRequest::Status,
                Duration::from_secs(2),
            )
        })
        .await
        .unwrap()
        .unwrap();
        assert!(response.ok);
        assert!(response.message.starts_with("jcode companion"));
        server.await.unwrap();
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn refuses_to_replace_a_live_companion() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("companion.sock");
        let listener = bind(&path).unwrap();
        let companion = Arc::new(companion());
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            serve(stream, &companion).await.unwrap();
        });
        let probe = path.clone();
        let error = tokio::task::spawn_blocking(move || bind(&probe).err())
            .await
            .unwrap()
            .expect("second bind must fail");
        assert!(error.to_string().contains("already running"));
        server.await.unwrap();
        // A stale socket file (no listener) is replaced.
        drop(bind(&path).unwrap());
    }
}
