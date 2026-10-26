//! Machine-wide MCP broker.
//!
//! [`SharedMcpPool`](super::pool::SharedMcpPool) only dedupes MCP servers
//! inside one jcode daemon. OpenRig seats and ad-hoc sessions each run their
//! own daemon, so N daemons x M servers still meant N x M child processes
//! (measured: ~350 processes and ~3 GB RSS for 12 servers on 20 daemons).
//!
//! The broker is one small process per jcode home. It owns a single upstream
//! child per distinct shared stdio server config and multiplexes every
//! daemon's requests onto it.
//!
//! Wire protocol, newline-delimited JSON over a unix socket
//! (`<jcode_dir>/mcp-broker.sock`, mode 0600):
//!
//! 1. client -> broker: `{"broker":1,"attach":{"name":..,"config":{..}}}`
//!    (or `{"broker":1,"status":true}` for a one-shot status report)
//! 2. broker -> client: `{"broker":1,"ok":true}` or
//!    `{"broker":1,"ok":false,"error":".."}`
//! 3. After a successful attach the stream carries plain MCP JSON-RPC. The
//!    broker rewrites request ids so concurrent clients never collide, answers
//!    `initialize` from the upstream's cached handshake, and swallows the
//!    per-client lifecycle messages that must reach an upstream only once.
//!
//! Upstreams are started in their own process group so `npx`/`uvx` wrappers
//! and their grandchildren are torn down together, and are stopped after they
//! have had no attached client for [`UPSTREAM_IDLE`]. The broker exits once it
//! has had nothing to do for [`BROKER_IDLE`]; the next client restarts it.

use super::pending::{self, PendingMap};
use super::protocol::{JsonRpcResponse, McpServerConfig};
use anyhow::{Context, Result};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::process::{Child, Command};
use tokio::sync::{Mutex, mpsc, oneshot};

const PROTOCOL_VERSION: u64 = 1;
const SOCKET_FILE: &str = "mcp-broker.sock";
const LOCK_FILE: &str = "mcp-broker.lock";

/// How long an upstream may sit with no attached client before it is stopped.
pub const UPSTREAM_IDLE: Duration = Duration::from_secs(10 * 60);
/// How long the broker may sit with no upstreams and no clients before exiting.
pub const BROKER_IDLE: Duration = Duration::from_secs(15 * 60);
const SWEEP_INTERVAL: Duration = Duration::from_secs(30);
/// Upstream `initialize` can include an `npx` download on first use.
const UPSTREAM_START_TIMEOUT: Duration = Duration::from_secs(180);
const KILL_GRACE: Duration = Duration::from_secs(2);

/// Env var that turns the broker off (`0`, `off`, `false`, `no`).
pub const BROKER_ENV: &str = "JCODE_MCP_BROKER";
/// Env var naming the binary to launch as `<bin> mcp-broker`.
pub const BROKER_BIN_ENV: &str = "JCODE_MCP_BROKER_BIN";

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

/// Whether shared stdio MCP servers should be routed through the broker.
pub fn broker_enabled() -> bool {
    !std::env::var(BROKER_ENV).is_ok_and(|value| {
        matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "0" | "off" | "false" | "no"
        )
    })
}

/// Socket path for the broker serving this jcode home.
pub fn socket_path() -> Result<PathBuf> {
    Ok(crate::storage::jcode_dir()?.join(SOCKET_FILE))
}

/// Key identifying one upstream: same name and same effective config share.
fn upstream_key(name: &str, config: &McpServerConfig) -> String {
    format!("{name}:{}", super::schema_cache::fingerprint_config(config))
}

fn json_line(value: &Value) -> String {
    let mut line = value.to_string();
    line.push('\n');
    line
}

fn error_response(id: &Value, code: i64, message: &str) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}})
}

// ---------------------------------------------------------------------------
// Broker server
// ---------------------------------------------------------------------------

enum Route {
    Client { conn: u64, original_id: Value },
    Internal(oneshot::Sender<Value>),
}

struct Upstream {
    name: String,
    pid: Option<u32>,
    child: Mutex<Option<Child>>,
    writer_tx: mpsc::Sender<String>,
    init_result: std::sync::RwLock<Value>,
    next_id: AtomicU64,
    routes: Mutex<HashMap<u64, Route>>,
    conns: Mutex<HashMap<u64, mpsc::UnboundedSender<String>>>,
    idle_since: std::sync::Mutex<Option<Instant>>,
    alive: AtomicBool,
    started_at: Instant,
}

impl Upstream {
    async fn spawn(name: &str, config: &McpServerConfig) -> Result<Arc<Self>> {
        let env = super::client::mcp_child_env(std::env::vars().collect(), &config.env);
        let mut command = Command::new(&config.command);
        command
            .args(&config.args)
            .env_clear()
            .envs(&env)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true);
        #[cfg(unix)]
        command.process_group(0);
        if let Some(home) = dirs::home_dir().filter(|dir| dir.is_dir()) {
            command.current_dir(home);
        }
        let mut child = command
            .spawn()
            .with_context(|| format!("Failed to spawn MCP server: {}", config.command))?;
        let pid = child.id();
        let stdin = child.stdin.take().context("No stdin")?;
        let stdout = child.stdout.take().context("No stdout")?;
        let stderr = child.stderr.take().context("No stderr")?;

        let (writer_tx, mut writer_rx) = mpsc::channel::<String>(256);
        let upstream = Arc::new(Self {
            name: name.to_string(),
            pid,
            child: Mutex::new(Some(child)),
            writer_tx,
            init_result: std::sync::RwLock::new(Value::Null),
            next_id: AtomicU64::new(1),
            routes: Mutex::new(HashMap::new()),
            conns: Mutex::new(HashMap::new()),
            idle_since: std::sync::Mutex::new(Some(Instant::now())),
            alive: AtomicBool::new(true),
            started_at: Instant::now(),
        });

        let stderr_name = name.to_string();
        tokio::spawn(async move {
            let mut lines = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                let line = line.trim();
                if !line.is_empty() {
                    crate::logging::warn(&format!("MCP broker [{stderr_name}] stderr: {line}"));
                }
            }
        });

        let mut stdin = stdin;
        tokio::spawn(async move {
            while let Some(message) = writer_rx.recv().await {
                if stdin.write_all(message.as_bytes()).await.is_err()
                    || stdin.flush().await.is_err()
                {
                    break;
                }
            }
        });

        let reader_upstream = Arc::clone(&upstream);
        tokio::spawn(async move {
            let mut lines = BufReader::new(stdout).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                reader_upstream.handle_upstream_line(&line).await;
            }
            reader_upstream.mark_dead().await;
        });

        let init = upstream
            .internal_request(
                "initialize",
                json!({
                    "protocolVersion": "2024-11-05",
                    "capabilities": {},
                    "clientInfo": {"name": "jcode-mcp-broker", "version": jcode_build_meta::pkg_version()},
                }),
            )
            .await;
        let init = match init {
            Ok(init) => init,
            Err(error) => {
                upstream.shutdown().await;
                return Err(error)
                    .with_context(|| format!("MCP server '{name}' failed to initialize"));
            }
        };
        if let Some(error) = init.get("error") {
            upstream.shutdown().await;
            anyhow::bail!("MCP server '{name}' rejected initialize: {error}");
        }
        *upstream
            .init_result
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) =
            init.get("result").cloned().unwrap_or(Value::Null);
        let _ = upstream
            .writer_tx
            .send(json_line(
                &json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
            ))
            .await;
        crate::logging::info(&format!(
            "MCP broker: started '{name}' (pid {pid:?}) in {}ms",
            upstream.started_at.elapsed().as_millis()
        ));
        Ok(upstream)
    }

    async fn internal_request(&self, method: &str, params: Value) -> Result<Value> {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let (tx, rx) = oneshot::channel();
        self.routes.lock().await.insert(id, Route::Internal(tx));
        let body = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
        self.writer_tx
            .send(json_line(&body))
            .await
            .context("MCP server stdin closed")?;
        match tokio::time::timeout(UPSTREAM_START_TIMEOUT, rx).await {
            Ok(Ok(value)) => Ok(value),
            Ok(Err(_)) => anyhow::bail!("MCP server exited before replying to {method}"),
            Err(_) => {
                self.routes.lock().await.remove(&id);
                anyhow::bail!(
                    "MCP server did not reply to {method} within {}s",
                    UPSTREAM_START_TIMEOUT.as_secs()
                )
            }
        }
    }

    async fn handle_upstream_line(&self, line: &str) {
        let Ok(mut message) = serde_json::from_str::<Value>(line) else {
            let line = line.trim();
            if !line.is_empty() {
                crate::logging::debug(&format!("MCP broker [{}] non-JSON: {line}", self.name));
            }
            return;
        };
        let has_method = message.get("method").is_some();
        let id = message.get("id").cloned();
        match (has_method, id) {
            // Response to one of our (rewritten) requests.
            (false, Some(id)) => {
                let Some(upstream_id) = id.as_u64() else {
                    return;
                };
                let route = self.routes.lock().await.remove(&upstream_id);
                match route {
                    Some(Route::Client { conn, original_id }) => {
                        message["id"] = original_id;
                        if let Some(tx) = self.conns.lock().await.get(&conn) {
                            let _ = tx.send(json_line(&message));
                        }
                    }
                    Some(Route::Internal(tx)) => {
                        let _ = tx.send(message);
                    }
                    None => {}
                }
            }
            // Server-to-client request (sampling, roots, elicitation). jcode
            // implements none of them, so answer instead of leaving the server
            // waiting forever.
            (true, Some(id)) => {
                let reply = error_response(&id, -32601, "Method not supported by jcode");
                let _ = self.writer_tx.send(json_line(&reply)).await;
            }
            // Server notifications (progress, list_changed). jcode ignores them.
            _ => {}
        }
    }

    async fn handle_client_line(&self, conn: u64, line: &str) {
        let Ok(mut message) = serde_json::from_str::<Value>(line) else {
            return;
        };
        let method = message
            .get("method")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let id = message.get("id").cloned().filter(|id| !id.is_null());
        let reply_to_client = |reply: Value| async move {
            if let Some(tx) = self.conns.lock().await.get(&conn) {
                let _ = tx.send(json_line(&reply));
            }
        };

        if method.is_empty() {
            // Client responses have nowhere to go: the broker answers every
            // server-to-client request itself.
            return;
        }
        match (method.as_str(), id) {
            ("initialize", Some(id)) => {
                let result = self
                    .init_result
                    .read()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .clone();
                reply_to_client(json!({"jsonrpc": "2.0", "id": id, "result": result})).await;
            }
            ("shutdown", Some(id)) => {
                reply_to_client(json!({"jsonrpc": "2.0", "id": id, "result": null})).await;
            }
            ("notifications/initialized" | "shutdown" | "exit", None) => {}
            (_, Some(original_id)) => {
                if !self.alive.load(Ordering::SeqCst) {
                    reply_to_client(error_response(&original_id, -32000, "MCP server exited"))
                        .await;
                    return;
                }
                let upstream_id = self.next_id.fetch_add(1, Ordering::SeqCst);
                self.routes.lock().await.insert(
                    upstream_id,
                    Route::Client {
                        conn,
                        original_id: original_id.clone(),
                    },
                );
                message["id"] = json!(upstream_id);
                if self.writer_tx.send(json_line(&message)).await.is_err() {
                    self.routes.lock().await.remove(&upstream_id);
                    reply_to_client(error_response(&original_id, -32000, "MCP server exited"))
                        .await;
                }
            }
            ("notifications/cancelled", None) => {
                // Cancellation names the client's request id; translate it so
                // one client can never cancel another client's request.
                let wanted = message.pointer("/params/requestId").cloned();
                let routes = self.routes.lock().await;
                let upstream_id = routes.iter().find_map(|(upstream_id, route)| match route {
                    Route::Client {
                        conn: owner,
                        original_id,
                    } if *owner == conn && Some(original_id) == wanted.as_ref() => {
                        Some(*upstream_id)
                    }
                    _ => None,
                });
                drop(routes);
                if let Some(upstream_id) = upstream_id {
                    message["params"]["requestId"] = json!(upstream_id);
                    let _ = self.writer_tx.send(json_line(&message)).await;
                }
            }
            (_, None) => {
                let _ = self.writer_tx.send(json_line(&message)).await;
            }
        }
    }

    async fn attach(&self, conn: u64, tx: mpsc::UnboundedSender<String>) {
        self.conns.lock().await.insert(conn, tx);
        *self
            .idle_since
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
    }

    async fn detach(&self, conn: u64) {
        let mut conns = self.conns.lock().await;
        conns.remove(&conn);
        let now_idle = conns.is_empty();
        drop(conns);
        self.routes.lock().await.retain(
            |_, route| !matches!(route, Route::Client { conn: owner, .. } if *owner == conn),
        );
        if now_idle {
            *self
                .idle_since
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(Instant::now());
        }
    }

    fn idle_for(&self) -> Option<Duration> {
        self.idle_since
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .map(|since| since.elapsed())
    }

    /// The upstream's stdout closed: nothing can answer anymore.
    async fn mark_dead(&self) {
        if self.alive.swap(false, Ordering::SeqCst) {
            crate::logging::warn(&format!("MCP broker: upstream '{}' exited", self.name));
        }
        // Dropping the senders ends every attached connection, so each client
        // sees EOF, fails its in-flight requests fast, and reconnects (which
        // starts a fresh upstream) on its next call.
        self.conns.lock().await.clear();
        self.routes.lock().await.clear();
        self.reap().await;
    }

    async fn reap(&self) {
        if let Some(mut child) = self.child.lock().await.take() {
            #[cfg(unix)]
            if let Some(pid) = self.pid {
                // Signal the whole group: `npx`/`uvx` wrappers leave the real
                // server as a grandchild that `child.kill()` alone would orphan.
                unsafe {
                    libc::killpg(pid as libc::pid_t, libc::SIGTERM);
                }
                if tokio::time::timeout(KILL_GRACE, child.wait())
                    .await
                    .is_err()
                {
                    unsafe {
                        libc::killpg(pid as libc::pid_t, libc::SIGKILL);
                    }
                }
            }
            let _ = child.kill().await;
            let _ = child.wait().await;
        }
    }

    async fn shutdown(&self) {
        self.alive.store(false, Ordering::SeqCst);
        self.conns.lock().await.clear();
        self.reap().await;
    }
}

/// One upstream per key, guarded so concurrent attaches start it only once.
type Slot = Arc<Mutex<Option<Arc<Upstream>>>>;

#[derive(Default)]
struct Broker {
    slots: Mutex<HashMap<String, Slot>>,
    next_conn: AtomicU64,
    active_conns: AtomicU64,
    idle_since: std::sync::Mutex<Option<Instant>>,
}

impl Broker {
    async fn upstream_for(&self, name: &str, config: &McpServerConfig) -> Result<Arc<Upstream>> {
        let key = upstream_key(name, config);
        let slot = {
            let mut slots = self.slots.lock().await;
            Arc::clone(slots.entry(key).or_default())
        };
        // Holding the slot lock while spawning makes concurrent attaches for
        // the same server wait for one start instead of racing N children.
        let mut slot = slot.lock().await;
        if let Some(upstream) = slot.as_ref()
            && upstream.alive.load(Ordering::SeqCst)
        {
            return Ok(Arc::clone(upstream));
        }
        let upstream = Upstream::spawn(name, config).await?;
        *slot = Some(Arc::clone(&upstream));
        Ok(upstream)
    }

    async fn status(&self) -> Value {
        let slots: Vec<_> = self.slots.lock().await.values().cloned().collect();
        let mut upstreams = Vec::new();
        for slot in slots {
            if let Some(upstream) = slot.lock().await.as_ref() {
                upstreams.push(json!({
                    "name": upstream.name,
                    "pid": upstream.pid,
                    "alive": upstream.alive.load(Ordering::SeqCst),
                    "clients": upstream.conns.lock().await.len(),
                    "uptime_secs": upstream.started_at.elapsed().as_secs(),
                    "idle_secs": upstream.idle_for().map(|idle| idle.as_secs()),
                }));
            }
        }
        json!({
            "broker": PROTOCOL_VERSION,
            "ok": true,
            "pid": std::process::id(),
            "version": jcode_build_meta::pkg_version(),
            "connections": self.active_conns.load(Ordering::SeqCst),
            "upstreams": upstreams,
        })
    }

    async fn handle_connection(self: Arc<Self>, stream: UnixStream) {
        let (read_half, mut write_half) = stream.into_split();
        let mut lines = BufReader::new(read_half).lines();
        let hello = match tokio::time::timeout(Duration::from_secs(10), lines.next_line()).await {
            Ok(Ok(Some(line))) => line,
            _ => return,
        };
        let hello: Value = match serde_json::from_str(&hello) {
            Ok(value) => value,
            Err(error) => {
                let _ = write_half
                    .write_all(json_line(&json!({"broker": PROTOCOL_VERSION, "ok": false, "error": format!("bad hello: {error}")})).as_bytes())
                    .await;
                return;
            }
        };
        if hello.get("status").and_then(Value::as_bool) == Some(true) {
            let _ = write_half
                .write_all(json_line(&self.status().await).as_bytes())
                .await;
            return;
        }
        let attach = hello.get("attach").cloned().unwrap_or(Value::Null);
        let name = attach
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let config = attach
            .get("config")
            .cloned()
            .and_then(|config| serde_json::from_value::<McpServerConfig>(config).ok());
        let (Some(config), false) = (config, name.is_empty()) else {
            let _ = write_half
                .write_all(json_line(&json!({"broker": PROTOCOL_VERSION, "ok": false, "error": "attach needs name and config"})).as_bytes())
                .await;
            return;
        };

        let upstream = match self.upstream_for(&name, &config).await {
            Ok(upstream) => upstream,
            Err(error) => {
                let _ = write_half
                    .write_all(json_line(&json!({"broker": PROTOCOL_VERSION, "ok": false, "error": format!("{error:#}")})).as_bytes())
                    .await;
                return;
            }
        };
        if write_half
            .write_all(json_line(&json!({"broker": PROTOCOL_VERSION, "ok": true})).as_bytes())
            .await
            .is_err()
        {
            return;
        }

        let conn = self.next_conn.fetch_add(1, Ordering::SeqCst) + 1;
        let (tx, mut rx) = mpsc::unbounded_channel::<String>();
        upstream.attach(conn, tx).await;
        self.active_conns.fetch_add(1, Ordering::SeqCst);
        *self.idle_since.lock().unwrap_or_else(|p| p.into_inner()) = None;

        let writer = tokio::spawn(async move {
            while let Some(line) = rx.recv().await {
                if write_half.write_all(line.as_bytes()).await.is_err() {
                    break;
                }
            }
            let _ = write_half.shutdown().await;
        });
        let reader_upstream = Arc::clone(&upstream);
        let mut reader = tokio::spawn(async move {
            while let Ok(Some(line)) = lines.next_line().await {
                reader_upstream.handle_client_line(conn, &line).await;
            }
        });
        // Either side ending closes the connection: the client hung up, or
        // the upstream died and dropped our sender so the writer finished.
        let mut writer = writer;
        tokio::select! {
            _ = &mut reader => { writer.abort(); }
            _ = &mut writer => { reader.abort(); }
        }

        upstream.detach(conn).await;
        if self.active_conns.fetch_sub(1, Ordering::SeqCst) == 1 {
            *self.idle_since.lock().unwrap_or_else(|p| p.into_inner()) = Some(Instant::now());
        }
    }

    /// Stop idle upstreams. Returns true when the broker itself should exit.
    async fn sweep(&self, upstream_idle: Duration, broker_idle: Duration) -> bool {
        let slots: Vec<_> = self
            .slots
            .lock()
            .await
            .iter()
            .map(|(k, v)| (k.clone(), Arc::clone(v)))
            .collect();
        let mut live = 0;
        for (key, slot) in slots {
            let mut slot = slot.lock().await;
            let stop = match slot.as_ref() {
                Some(upstream) if !upstream.alive.load(Ordering::SeqCst) => true,
                Some(upstream) => upstream
                    .idle_for()
                    .is_some_and(|idle| idle >= upstream_idle),
                None => true,
            };
            if stop {
                if let Some(upstream) = slot.take() {
                    crate::logging::info(&format!(
                        "MCP broker: stopping idle upstream '{}'",
                        upstream.name
                    ));
                    upstream.shutdown().await;
                }
                drop(slot);
                self.slots.lock().await.remove(&key);
            } else {
                live += 1;
            }
        }
        live == 0
            && self.active_conns.load(Ordering::SeqCst) == 0
            && self
                .idle_since
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .is_none_or(|since| since.elapsed() >= broker_idle)
    }

    async fn shutdown_all(&self) {
        let slots: Vec<_> = self.slots.lock().await.drain().map(|(_, v)| v).collect();
        for slot in slots {
            if let Some(upstream) = slot.lock().await.take() {
                upstream.shutdown().await;
            }
        }
    }
}

/// Hold an exclusive lock so only one broker serves a jcode home.
#[cfg(unix)]
fn acquire_singleton_lock(dir: &Path) -> Result<Option<std::fs::File>> {
    use std::os::unix::io::AsRawFd;
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(dir.join(LOCK_FILE))?;
    let rc = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    Ok((rc == 0).then_some(file))
}

/// Serve the broker on `socket`. Returns when idle or on SIGTERM/SIGINT.
#[cfg(unix)]
pub async fn serve(socket: &Path, upstream_idle: Duration, broker_idle: Duration) -> Result<()> {
    let dir = socket.parent().context("broker socket has no parent dir")?;
    std::fs::create_dir_all(dir)?;
    let Some(_lock) = acquire_singleton_lock(dir)? else {
        crate::logging::info("MCP broker: another broker holds the lock; exiting");
        return Ok(());
    };
    let _ = std::fs::remove_file(socket);
    let listener = UnixListener::bind(socket)
        .with_context(|| format!("bind MCP broker socket {}", socket.display()))?;
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(socket, std::fs::Permissions::from_mode(0o600));
    }
    crate::logging::info(&format!(
        "MCP broker: listening on {} (pid {})",
        socket.display(),
        std::process::id()
    ));

    let broker = Arc::new(Broker::default());
    *broker.idle_since.lock().unwrap_or_else(|p| p.into_inner()) = Some(Instant::now());
    let mut sweep = tokio::time::interval(
        SWEEP_INTERVAL
            .min(upstream_idle)
            .max(Duration::from_millis(50)),
    );
    let mut terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    loop {
        tokio::select! {
            accepted = listener.accept() => {
                if let Ok((stream, _)) = accepted {
                    tokio::spawn(Arc::clone(&broker).handle_connection(stream));
                }
            }
            _ = sweep.tick() => {
                // Another process replaced our socket: stop serving a path
                // nobody can reach any more.
                if !socket.exists() {
                    break;
                }
                if broker.sweep(upstream_idle, broker_idle).await {
                    crate::logging::info("MCP broker: idle; exiting");
                    break;
                }
            }
            _ = terminate.recv() => break,
            _ = tokio::signal::ctrl_c() => break,
        }
    }
    broker.shutdown_all().await;
    let _ = std::fs::remove_file(socket);
    Ok(())
}

/// Entry point for `jcode mcp-broker [--status]`.
#[cfg(unix)]
pub async fn run_cli(args: &[String]) -> Result<()> {
    let socket = socket_path()?;
    if args.iter().any(|arg| arg == "--status") {
        let status = request_status(&socket).await?;
        println!("{}", serde_json::to_string_pretty(&status)?);
        return Ok(());
    }
    crate::logging::init();
    serve(&socket, UPSTREAM_IDLE, BROKER_IDLE).await
}

/// Ask a running broker for its status report.
#[cfg(unix)]
pub async fn request_status(socket: &Path) -> Result<Value> {
    let stream = UnixStream::connect(socket)
        .await
        .with_context(|| format!("no MCP broker at {}", socket.display()))?;
    let (read_half, mut write_half) = stream.into_split();
    write_half
        .write_all(json_line(&json!({"broker": PROTOCOL_VERSION, "status": true})).as_bytes())
        .await?;
    let line = BufReader::new(read_half)
        .lines()
        .next_line()
        .await?
        .context("broker closed without a status reply")?;
    Ok(serde_json::from_str(&line)?)
}

// ---------------------------------------------------------------------------
// Client transport
// ---------------------------------------------------------------------------

/// Why a broker attach failed. Only `Unavailable` should fall back to
/// spawning the server directly: an `Upstream` failure would fail the same way.
#[derive(Debug)]
pub enum BrokerConnectError {
    Unavailable(anyhow::Error),
    Upstream(anyhow::Error),
}

impl std::fmt::Display for BrokerConnectError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unavailable(error) => write!(f, "MCP broker unavailable: {error:#}"),
            Self::Upstream(error) => write!(f, "{error:#}"),
        }
    }
}

impl std::error::Error for BrokerConnectError {}

struct BrokerConn {
    writer_tx: mpsc::Sender<String>,
    pending: PendingMap,
    alive: Arc<AtomicBool>,
}

/// One MCP server reached through the broker. Reconnects (and so restarts the
/// upstream through the broker) on the next request after the link drops.
pub struct BrokerTransport {
    name: String,
    config: McpServerConfig,
    socket: PathBuf,
    autostart: bool,
    conn: Mutex<Option<Arc<BrokerConn>>>,
}

impl BrokerTransport {
    pub async fn connect(
        name: String,
        config: &McpServerConfig,
        socket: PathBuf,
        autostart: bool,
    ) -> std::result::Result<Self, BrokerConnectError> {
        let transport = Self {
            name,
            config: config.clone(),
            socket,
            autostart,
            conn: Mutex::new(None),
        };
        let conn = transport.attach().await?;
        *transport.conn.lock().await = Some(conn);
        Ok(transport)
    }

    async fn attach(&self) -> std::result::Result<Arc<BrokerConn>, BrokerConnectError> {
        let stream = connect_or_start(&self.socket, self.autostart)
            .await
            .map_err(BrokerConnectError::Unavailable)?;
        let (read_half, mut write_half) = stream.into_split();
        let hello = json!({
            "broker": PROTOCOL_VERSION,
            "attach": {"name": self.name, "config": self.config},
        });
        write_half
            .write_all(json_line(&hello).as_bytes())
            .await
            .map_err(|error| BrokerConnectError::Unavailable(error.into()))?;
        let mut lines = BufReader::new(read_half).lines();
        let reply = tokio::time::timeout(
            UPSTREAM_START_TIMEOUT + Duration::from_secs(10),
            lines.next_line(),
        )
        .await
        .map_err(|_| BrokerConnectError::Unavailable(anyhow::anyhow!("attach timed out")))?
        .map_err(|error| BrokerConnectError::Unavailable(error.into()))?
        .ok_or_else(|| {
            BrokerConnectError::Unavailable(anyhow::anyhow!("broker closed during attach"))
        })?;
        let reply: Value = serde_json::from_str(&reply)
            .map_err(|error| BrokerConnectError::Unavailable(error.into()))?;
        if reply.get("ok").and_then(Value::as_bool) != Some(true) {
            let error = reply
                .get("error")
                .and_then(Value::as_str)
                .unwrap_or("broker refused attach")
                .to_string();
            return Err(BrokerConnectError::Upstream(anyhow::anyhow!(error)));
        }

        let pending = pending::new_pending();
        let alive = Arc::new(AtomicBool::new(true));
        let (writer_tx, mut writer_rx) = mpsc::channel::<String>(64);
        tokio::spawn(async move {
            while let Some(message) = writer_rx.recv().await {
                if write_half.write_all(message.as_bytes()).await.is_err() {
                    break;
                }
            }
        });
        let reader_pending = Arc::clone(&pending);
        let reader_alive = Arc::clone(&alive);
        let reader_name = self.name.clone();
        tokio::spawn(async move {
            while let Ok(Some(line)) = lines.next_line().await {
                if let Ok(response) = serde_json::from_str::<JsonRpcResponse>(&line) {
                    pending::resolve(&reader_pending, response).await;
                }
            }
            reader_alive.store(false, Ordering::SeqCst);
            crate::logging::debug(&format!("MCP [{reader_name}]: broker link closed"));
            pending::fail_all(&reader_pending).await;
        });
        Ok(Arc::new(BrokerConn {
            writer_tx,
            pending,
            alive,
        }))
    }

    async fn live_conn(&self) -> Result<Arc<BrokerConn>> {
        let mut slot = self.conn.lock().await;
        if let Some(conn) = slot.as_ref()
            && conn.alive.load(Ordering::SeqCst)
        {
            return Ok(Arc::clone(conn));
        }
        crate::logging::info(&format!("MCP [{}]: reattaching to broker", self.name));
        let conn = self
            .attach()
            .await
            .map_err(|error| anyhow::anyhow!("{error}"))?;
        *slot = Some(Arc::clone(&conn));
        Ok(conn)
    }

    pub async fn send(
        &self,
        body: &str,
        id: u64,
        expect_response: bool,
    ) -> Result<Option<JsonRpcResponse>> {
        let conn = self.live_conn().await?;
        let line = format!("{body}\n");
        if !expect_response {
            conn.writer_tx
                .send(line)
                .await
                .context("MCP broker link closed")?;
            return Ok(None);
        }
        let waiter = pending::PendingRequest::register(&conn.pending, id).await;
        if let Err(error) = conn.writer_tx.send(line).await {
            waiter.cancel().await;
            return Err(error).context("MCP broker link closed");
        }
        waiter
            .recv()
            .await
            .context("MCP broker link closed before the server replied")
            .map(Some)
    }

    pub async fn notify(&self, body: &str) -> Result<()> {
        self.send(body, 0, false).await.map(|_| ())
    }
}

/// Binary to launch as the broker, or `None` when this process is not a jcode
/// executable (test harnesses), in which case callers spawn servers directly.
fn broker_binary() -> Option<PathBuf> {
    if let Some(bin) = std::env::var_os(BROKER_BIN_ENV).filter(|bin| !bin.is_empty()) {
        // A configured binary that is not there cannot start a broker, so skip
        // straight to direct spawning instead of waiting for one to appear.
        return Some(PathBuf::from(bin)).filter(|bin| bin.is_file());
    }
    let exe = std::env::current_exe().ok()?;
    let file_name = exe.file_name()?.to_string_lossy().to_string();
    let in_test_deps = exe.components().any(|part| part.as_os_str() == "deps");
    (file_name.starts_with("jcode") && !in_test_deps).then_some(exe)
}

/// Whether this process can reach or start a broker at all.
pub fn broker_available() -> bool {
    broker_enabled() && broker_binary().is_some()
}

async fn connect_or_start(socket: &Path, autostart: bool) -> Result<UnixStream> {
    if let Ok(stream) = UnixStream::connect(socket).await {
        return Ok(stream);
    }
    if !autostart {
        anyhow::bail!("no MCP broker listening at {}", socket.display());
    }
    start_broker_process()?;
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        tokio::time::sleep(Duration::from_millis(50)).await;
        if let Ok(stream) = UnixStream::connect(socket).await {
            return Ok(stream);
        }
        if Instant::now() >= deadline {
            anyhow::bail!("MCP broker did not start listening at {}", socket.display());
        }
    }
}

/// Session-scoped variables that must not leak into a machine-wide process.
fn is_session_scoped_env(key: &str) -> bool {
    const KEEP: &[&str] = &["JCODE_HOME", BROKER_BIN_ENV];
    if KEEP.contains(&key) {
        return false;
    }
    key.starts_with("JCODE_") || key.starts_with("OPENRIG_") || key.starts_with("TMUX")
}

fn start_broker_process() -> Result<()> {
    let binary = broker_binary().context("no jcode binary available to run the MCP broker")?;
    // Launch through a shell that backgrounds the broker and exits at once, so
    // the broker is reparented to init. A daemon that spawned it directly
    // would be its parent forever, and after an exec-based reload the new
    // process image would never reap it: every broker exit would leave a
    // zombie behind.
    let mut command = std::process::Command::new("/bin/sh");
    command
        .arg("-c")
        .arg("\"$0\" mcp-broker </dev/null >/dev/null 2>&1 &")
        .arg(&binary)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    for (key, _) in std::env::vars_os() {
        if is_session_scoped_env(&key.to_string_lossy()) {
            command.env_remove(key);
        }
    }
    if let Some(home) = dirs::home_dir().filter(|dir| dir.is_dir()) {
        command.current_dir(home);
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // Own session: the broker must outlive the daemon that started it and
        // never receive that daemon's terminal signals.
        unsafe {
            command.pre_exec(|| {
                libc::setsid();
                Ok(())
            });
        }
    }
    let status = command
        .status()
        .with_context(|| format!("spawn MCP broker {}", binary.display()))?;
    anyhow::ensure!(status.success(), "MCP broker launcher exited with {status}");
    crate::logging::info(&format!("MCP: started broker {}", binary.display()));
    Ok(())
}

#[cfg(all(test, unix))]
#[path = "broker_tests.rs"]
mod tests;
