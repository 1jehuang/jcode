//! Shared-server lifecycle hooks usable from lower layers.
//!
//! The actual "spawn a shared jcode server" logic lives in the CLI command
//! layer (`cli::dispatch::spawn_server`) because it depends on CLI types like
//! `ProviderChoice` and on argument-driven bootstrap. Lower layers such as the
//! TUI reconnect loop still need to (a) check whether a shared server is
//! reachable and (b) request a replacement server when a reload stalls.
//!
//! To avoid a `tui -> cli` dependency, the CLI registers a default spawner here
//! at startup (mirroring the `register_permission_notifier` /
//! `register_api_key_fallback_resolver` inversion pattern). Consumers call
//! [`is_running`] and [`spawn_default_server`] without knowing about `cli`.
//!
//! The same inversion carries the *owned* (ephemeral, process-scoped) spawner
//! used by headless one-shot `jcode run`: see
//! [`register_on_demand_server_spawner`] and [`ensure_on_demand_server`].

use anyhow::Result;
use std::future::Future;
use std::pin::Pin;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};

type ServerSpawner =
    Box<dyn Fn() -> Pin<Box<dyn Future<Output = Result<()>> + Send>> + Send + Sync>;

static DEFAULT_SERVER_SPAWNER: OnceLock<ServerSpawner> = OnceLock::new();
static ON_DEMAND_SERVER_SPAWNER: OnceLock<ServerSpawner> = OnceLock::new();
static ON_DEMAND_SERVER_LOCK: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();
static ON_DEMAND_SERVER_SPAWN_FAILED: AtomicBool = AtomicBool::new(false);

/// Register the default shared-server spawner.
///
/// Called once at startup by the CLI layer, which owns the provider-bootstrap
/// logic. Subsequent calls are ignored.
pub fn register_default_server_spawner(spawner: ServerSpawner) {
    let _ = DEFAULT_SERVER_SPAWNER.set(spawner);
}

/// Returns true if a shared server is currently reachable on the default socket.
pub async fn is_running() -> bool {
    let socket = crate::server::socket_path();
    crate::server::is_server_ready(&socket).await || crate::server::has_live_listener(&socket).await
}

/// Spawn a replacement shared server using the registered default spawner.
///
/// Returns an error if no spawner has been registered (e.g. in a context that
/// never initialized the CLI startup hooks).
pub async fn spawn_default_server() -> Result<()> {
    match DEFAULT_SERVER_SPAWNER.get() {
        Some(spawner) => spawner().await,
        None => anyhow::bail!("no default server spawner registered"),
    }
}

/// Register a spawner this process may use to start a server for itself.
///
/// Headless one-shot `jcode run` sessions host their agent in-process, so no
/// server exists for them at all: `--socket X` only selects the path tools
/// will dial, and nothing ever binds `X` (issue #1748). The `run` path
/// registers a spawner here so a tool that genuinely needs a server (today:
/// `swarm`) can start one on demand, instead of failing with "jcode server is
/// not running". Whether that server is the shared daemon or an ephemeral one
/// is the CLI's decision; this layer only knows that one can be started.
///
/// Only the one-shot `run` path registers this. Interactive and client
/// contexts deliberately do not: their agents already execute inside a
/// long-lived server, and a spawn triggered from deep inside a tool would race
/// the client's own reconnect/bootstrap logic.
pub fn register_on_demand_server_spawner(spawner: ServerSpawner) {
    let _ = ON_DEMAND_SERVER_SPAWNER.set(spawner);
}

/// True when this process can start a server for itself on demand.
pub fn on_demand_server_spawner_registered() -> bool {
    ON_DEMAND_SERVER_SPAWNER.get().is_some()
}

/// Make a server reachable on the socket this process dials, starting one with
/// the registered spawner if needed.
///
/// Idempotent and safe to call from concurrent tool invocations: callers are
/// serialized, and a caller that finds a live server returns without spawning.
/// A failed spawn is latched so a run that cannot start a server pays the
/// startup cost once rather than on every swarm call.
pub async fn ensure_on_demand_server() -> Result<()> {
    let Some(spawner) = ON_DEMAND_SERVER_SPAWNER.get() else {
        anyhow::bail!("no on-demand server spawner registered");
    };

    let _guard = ON_DEMAND_SERVER_LOCK
        .get_or_init(|| tokio::sync::Mutex::new(()))
        .lock()
        .await;

    if is_running().await {
        return Ok(());
    }

    if ON_DEMAND_SERVER_SPAWN_FAILED.load(Ordering::Relaxed) {
        anyhow::bail!("an earlier attempt to start a server for this run already failed");
    }

    match spawner().await {
        Ok(()) => Ok(()),
        Err(error) => {
            ON_DEMAND_SERVER_SPAWN_FAILED.store(true, Ordering::Relaxed);
            Err(error)
        }
    }
}
