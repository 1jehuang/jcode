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

use anyhow::Result;
use std::future::Future;
use std::pin::Pin;
use std::sync::OnceLock;

type ServerSpawner =
    Box<dyn Fn() -> Pin<Box<dyn Future<Output = Result<()>> + Send>> + Send + Sync>;

static DEFAULT_SERVER_SPAWNER: OnceLock<ServerSpawner> = OnceLock::new();

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
