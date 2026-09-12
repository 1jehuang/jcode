//! Runtime startup and private jcode instances.
//!
//! [`ensure_runtime`] attaches applications to the user's shared runtime.
//! [`launch_instance`] instead creates an isolated runtime directory and state
//! home which the returned owner tears down on drop.

use crate::errors::{Error, ErrorKind, Result};
#[cfg(unix)]
use serde_json::Value;
use std::collections::HashMap;
use std::ffi::{OsStr, OsString};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

const CREDENTIAL_FILES: &[&str] = &[
    "auth.json",
    "openai-auth.json",
    "antigravity_oauth.json",
    "gemini_oauth.json",
    "google_oauth.json",
    "google_credentials.json",
    "config.toml",
];

const EXTERNAL_CREDENTIAL_FILES: &[&str] = &[
    ".claude/.credentials.json",
    ".codex/auth.json",
    ".gemini/oauth_creds.json",
    ".cursor/auth.json",
    ".config/cursor/auth.json",
    "AppData/Roaming/Cursor/auth.json",
    ".config/Cursor/User/globalStorage/state.vscdb",
    ".config/cursor/User/globalStorage/state.vscdb",
    "Library/Application Support/Cursor/User/globalStorage/state.vscdb",
    "Library/Application Support/cursor/User/globalStorage/state.vscdb",
    "AppData/Roaming/Cursor/User/globalStorage/state.vscdb",
    "AppData/Roaming/cursor/User/globalStorage/state.vscdb",
    ".config/github-copilot/hosts.json",
    ".config/github-copilot/apps.json",
    ".copilot/config.json",
    ".hermes/auth.json",
    ".pi/agent/auth.json",
    ".openclaw/agent/auth.json",
    ".openclaw/credentials/oauth.json",
    ".local/share/opencode/auth.json",
];

/// Operator ownership mode for autonomous wake requests.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum WakeMode {
    #[default]
    Internal,
    External,
}

impl WakeMode {
    fn as_str(self) -> &'static str {
        match self {
            Self::Internal => "internal",
            Self::External => "external",
        }
    }
}

/// Options for starting a private jcode instance.
pub struct LaunchOptions {
    /// State directory for the instance. A temporary, drop-cleaned directory is
    /// used when omitted; an explicit directory is persistent.
    pub jcode_home: Option<PathBuf>,
    /// Working directory inherited by sessions created in the instance.
    pub working_dir: Option<PathBuf>,
    /// Share the user's provider login files. Enabled by default.
    pub inherit_logins: bool,
    /// jcode executable. Defaults to `jcode` on `$PATH`.
    pub binary: Option<PathBuf>,
    /// Extra environment variables for the private instance.
    pub env: HashMap<OsString, OsString>,
    /// Default model for spawned swarm workers, unless overridden per spawn.
    /// `inherit` defaults to the coordinator's model and auth route.
    /// This is applied as the operator-level
    /// `JCODE_SWARM_MODEL` override and takes precedence over `env`.
    pub swarm_model: Option<String>,
    /// Who executes autonomous wake requests. Applied as the operator-level
    /// `JCODE_WAKE_MODE` override and takes precedence over `env`.
    pub wake_mode: Option<WakeMode>,
    /// How long to wait for the API socket to accept connections.
    pub startup_timeout: Duration,
    /// Forward the bridge's stderr instead of capturing it for startup errors.
    pub inherit_stderr: bool,
    /// Maximum time spent removing a temporary home which is still being
    /// written by background work.
    pub cleanup_timeout: Duration,
    /// Client identity used by [`crate::JcodeClient::launch`].
    pub client_name: String,
    /// Request timeout used by [`crate::JcodeClient::launch`].
    pub request_timeout: Option<Duration>,
    /// Socket override used only by [`ensure_runtime`]. Kept here for backwards
    /// compatibility with the original shared-runtime launcher.
    pub api_socket: Option<PathBuf>,
    /// Shared-runtime startup timeout used only by [`ensure_runtime`].
    ///
    /// This retains the original Rust SDK field while `startup_timeout` matches
    /// private-instance launch terminology.
    pub start_timeout: Duration,
}

impl Default for LaunchOptions {
    fn default() -> Self {
        Self {
            jcode_home: None,
            working_dir: None,
            inherit_logins: true,
            binary: None,
            env: HashMap::new(),
            swarm_model: None,
            wake_mode: None,
            startup_timeout: Duration::from_secs(30),
            inherit_stderr: false,
            cleanup_timeout: Duration::from_secs(30),
            client_name: concat!("jcode-sdk-rs/", env!("CARGO_PKG_VERSION")).to_string(),
            request_timeout: Some(Duration::from_secs(30)),
            api_socket: None,
            start_timeout: Duration::from_secs(30),
        }
    }
}

/// A running private instance. Dropping it stops both bridge and daemon, then
/// removes an SDK-created temporary home. Caller-supplied homes are preserved.
pub struct LaunchedInstance {
    pub socket_path: PathBuf,
    pub jcode_home: PathBuf,
    runtime_dir: PathBuf,
    child: Mutex<Option<Child>>,
    ephemeral: bool,
    cleanup_timeout: Duration,
    stopped: AtomicBool,
}

impl LaunchedInstance {
    fn shutdown(&self) {
        if self.stopped.swap(true, Ordering::AcqRel) {
            return;
        }

        stop_instance_daemon(&self.jcode_home, &self.runtime_dir);
        if let Ok(mut slot) = self.child.lock()
            && let Some(mut child) = slot.take()
        {
            terminate_child(&mut child);
        }
        if self.ephemeral {
            remove_ephemeral_home(&self.jcode_home, self.cleanup_timeout);
        }
    }
}

impl Drop for LaunchedInstance {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// Start an isolated daemon and API bridge and return once its socket accepts.
pub fn launch_instance(options: &LaunchOptions) -> Result<LaunchedInstance> {
    let ephemeral = options.jcode_home.is_none();
    let jcode_home = match &options.jcode_home {
        Some(path) => {
            fs::create_dir_all(path).map_err(launch_io)?;
            path.clone()
        }
        None => tempfile::Builder::new()
            .prefix("jcode-sdk-instance-")
            .tempdir()
            .map_err(launch_io)?
            .keep(),
    };
    set_owner_only_dir(&jcode_home)?;

    let runtime_dir = jcode_home.join("run");
    fs::create_dir_all(&runtime_dir).map_err(launch_io)?;
    set_owner_only_dir(&runtime_dir)?;
    let socket_path = runtime_dir.join("jcode-api.sock");
    for stale in [
        "jcode-api.sock",
        "jcode.sock",
        "jcode-debug.sock",
        "jcode.sock.hash",
    ] {
        let _ = fs::remove_file(runtime_dir.join(stale));
    }

    let cleanup_on_error = || {
        if ephemeral {
            remove_ephemeral_home(&jcode_home, Duration::ZERO);
        }
    };
    if options.inherit_logins {
        if let Err(error) = inherit_credentials(&user_jcode_home(), &jcode_home) {
            cleanup_on_error();
            return Err(error);
        }
    }

    let binary = options
        .binary
        .clone()
        .unwrap_or_else(|| PathBuf::from("jcode"));
    let mut command = Command::new(&binary);
    command
        .args([OsStr::new("api-bridge"), OsStr::new("--api-socket")])
        .arg(&socket_path)
        .current_dir(
            options
                .working_dir
                .as_deref()
                .unwrap_or_else(|| Path::new(".")),
        )
        .env("JCODE_HOME", &jcode_home)
        .env("JCODE_RUNTIME_DIR", &runtime_dir)
        .env("JCODE_API_SOCKET", &socket_path)
        .env("JCODE_SOCKET", runtime_dir.join("jcode.sock"))
        .envs(options.env.iter())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(if options.inherit_stderr {
            Stdio::inherit()
        } else {
            Stdio::piped()
        });
    if let Some(swarm_model) = options.swarm_model.as_deref() {
        command.env("JCODE_SWARM_MODEL", swarm_model);
    }
    if let Some(wake_mode) = options.wake_mode {
        command.env("JCODE_WAKE_MODE", wake_mode.as_str());
    }

    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(cause) => {
            cleanup_on_error();
            let message = if cause.kind() == std::io::ErrorKind::NotFound {
                format!(
                    "could not run `{}`: jcode is not installed, or not on PATH. Install it from https://jcode.sh, or pass `binary` with its full path.",
                    binary.display()
                )
            } else {
                format!("could not run `{}`: {cause}", binary.display())
            };
            return Err(Error::new(ErrorKind::JcodeNotFound, message));
        }
    };
    let stderr = std::sync::Arc::new(Mutex::new(String::new()));
    let mut stderr_reader = child.stderr.take().map(|mut pipe| {
        let stderr = std::sync::Arc::clone(&stderr);
        std::thread::spawn(move || {
            let mut chunk = [0_u8; 1024];
            while let Ok(count) = pipe.read(&mut chunk) {
                if count == 0 {
                    break;
                }
                if let Ok(mut captured) = stderr.lock() {
                    captured.push_str(&String::from_utf8_lossy(&chunk[..count]));
                    if captured.len() > 4000 {
                        let mut keep_from = captured.len() - 4000;
                        while !captured.is_char_boundary(keep_from) {
                            keep_from += 1;
                        }
                        captured.drain(..keep_from);
                    }
                }
            }
        })
    });

    let deadline = Instant::now() + options.startup_timeout;
    while Instant::now() < deadline {
        if socket_accepts(&socket_path) {
            return Ok(LaunchedInstance {
                socket_path,
                jcode_home,
                runtime_dir,
                child: Mutex::new(Some(child)),
                ephemeral,
                cleanup_timeout: options.cleanup_timeout,
                stopped: AtomicBool::new(false),
            });
        }
        match child.try_wait() {
            Ok(Some(status)) => {
                if let Some(reader) = stderr_reader.take() {
                    let _ = reader.join();
                }
                let stderr = stderr.lock().map(|value| value.clone()).unwrap_or_default();
                cleanup_on_error();
                return Err(Error::new(
                    ErrorKind::StartupFailed,
                    format!(
                        "jcode exited during startup ({status}){}",
                        stderr_suffix(&stderr)
                    ),
                ));
            }
            Ok(None) => {}
            Err(cause) => {
                terminate_child(&mut child);
                cleanup_on_error();
                return Err(Error::new(ErrorKind::StartupFailed, cause.to_string()));
            }
        }
        std::thread::sleep(Duration::from_millis(50));
    }

    terminate_child(&mut child);
    if let Some(reader) = stderr_reader.take() {
        let _ = reader.join();
    }
    let stderr = stderr.lock().map(|value| value.clone()).unwrap_or_default();
    cleanup_on_error();
    Err(Error::new(
        ErrorKind::StartupTimeout,
        format!(
            "jcode did not create its API socket at {} within {:?}{}",
            socket_path.display(),
            options.startup_timeout,
            stderr_suffix(&stderr)
        ),
    ))
}

/// Resolve the user's normal jcode home before a private instance overrides it.
pub fn user_jcode_home() -> PathBuf {
    std::env::var_os("JCODE_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home_dir().join(".jcode"))
}

/// Share rotating login records with an instance and copy mutable config.
/// Returns the instance-relative paths which were inherited.
pub fn inherit_credentials(from_home: &Path, to_home: &Path) -> Result<Vec<PathBuf>> {
    fs::create_dir_all(to_home).map_err(launch_io)?;
    let metadata = fs::symlink_metadata(to_home).map_err(launch_io)?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(Error::new(
            ErrorKind::InvalidInstanceHome,
            format!(
                "instance home must be a real directory, not a link or file: {}",
                to_home.display()
            ),
        ));
    }
    if from_home.exists() && fs::canonicalize(from_home).ok() == fs::canonicalize(to_home).ok() {
        return Err(Error::new(
            ErrorKind::InvalidInstanceHome,
            "instance home must be different from the user's jcode home",
        ));
    }

    let mut inherited = Vec::new();
    for name in CREDENTIAL_FILES {
        let source = from_home.join(name);
        if !source.is_file() {
            continue;
        }
        let destination = to_home.join(name);
        if *name == "config.toml" {
            fs::copy(&source, &destination).map_err(launch_io)?;
            set_owner_only_file(&destination)?;
        } else {
            link_credential_file(&source, to_home, Path::new(name))?;
        }
        inherited.push(PathBuf::from(name));
    }

    // Provider env files live in the platform config directory. Link exact
    // files, never the directory, so cleanup cannot traverse into user data.
    if let Ok(entries) = fs::read_dir(user_app_config_dir()) {
        for entry in entries.flatten() {
            let name = entry.file_name();
            if !name.to_string_lossy().ends_with(".env") || !entry.path().is_file() {
                continue;
            }
            let relative = PathBuf::from("config").join("jcode").join(&name);
            link_credential_file(&entry.path(), to_home, &relative)?;
            inherited.push(relative);
        }
    }

    let home = home_dir();
    for relative in EXTERNAL_CREDENTIAL_FILES {
        let source = home.join(relative);
        if !source.is_file() {
            continue;
        }
        let instance_relative = PathBuf::from("external").join(relative);
        link_credential_file(&source, to_home, &instance_relative)?;
        inherited.push(instance_relative);
    }

    // OpenClaw's current credential store is per-agent. Enumerate only one
    // agent-id level and only the two filenames its loader recognizes.
    if let Ok(agents) = fs::read_dir(home.join(".openclaw/agents")) {
        for agent in agents.flatten().filter(|entry| entry.path().is_dir()) {
            for name in ["auth-profiles.json", "auth.json"] {
                let relative = PathBuf::from(".openclaw")
                    .join("agents")
                    .join(agent.file_name())
                    .join("agent")
                    .join(name);
                let source = home.join(&relative);
                if source.is_file() {
                    let instance_relative = PathBuf::from("external").join(&relative);
                    link_credential_file(&source, to_home, &instance_relative)?;
                    inherited.push(instance_relative);
                }
            }
        }
    }
    Ok(inherited)
}

fn link_credential_file(source: &Path, root: &Path, relative: &Path) -> Result<()> {
    let parent_relative = relative.parent().unwrap_or_else(|| Path::new(""));
    let parent = ensure_instance_directory(root, parent_relative)?;
    let destination = parent.join(relative.file_name().unwrap_or_default());
    let _ = fs::remove_file(&destination);
    link_file(source, &destination)
}

#[cfg(unix)]
fn link_file(source: &Path, destination: &Path) -> Result<()> {
    std::os::unix::fs::symlink(source, destination).map_err(launch_io)
}

#[cfg(windows)]
fn link_file(source: &Path, destination: &Path) -> Result<()> {
    // Preserve rotating credentials rather than silently creating a stale copy.
    // Windows may require Developer Mode or symlink privileges; report that OS
    // error to the caller when links are unavailable.
    std::os::windows::fs::symlink_file(source, destination).map_err(launch_io)
}

fn ensure_instance_directory(root: &Path, relative: &Path) -> Result<PathBuf> {
    if relative.is_absolute()
        || relative
            .components()
            .any(|part| matches!(part, std::path::Component::ParentDir))
    {
        return Err(Error::new(
            ErrorKind::InvalidInstanceHome,
            format!("unsafe instance path: {}", relative.display()),
        ));
    }
    let mut current = root.to_path_buf();
    for part in relative.components() {
        current.push(part);
        match fs::symlink_metadata(&current) {
            Ok(meta) if meta.file_type().is_symlink() => {
                fs::remove_file(&current).map_err(launch_io)?;
                fs::create_dir(&current).map_err(launch_io)?;
            }
            Ok(meta) if !meta.is_dir() => {
                return Err(Error::new(
                    ErrorKind::InvalidInstanceHome,
                    format!(
                        "instance credential path is not a directory: {}",
                        current.display()
                    ),
                ));
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                fs::create_dir(&current).map_err(launch_io)?;
            }
            Err(error) => return Err(launch_io(error)),
        }
        set_owner_only_dir(&current)?;
    }
    Ok(current)
}

/// Progress while a shared runtime comes up.
pub type Progress<'a> = dyn Fn(&str) + 'a;

/// Whether anything is listening on a socket path.
pub fn socket_accepts(path: &Path) -> bool {
    jcode_transport::is_socket_path(path)
}

/// Locate a sibling executable next to our own, falling back to `$PATH`.
fn sibling_exe(name: &str) -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.join(name)))
        .filter(|path| path.is_file())
        .unwrap_or_else(|| PathBuf::from(name))
}

/// Start the user's shared daemon and API bridge if necessary.
pub fn ensure_runtime(options: &LaunchOptions, progress: &Progress<'_>) -> Result<()> {
    let api = options
        .api_socket
        .clone()
        .unwrap_or_else(jcode_harness_api::api_socket_path);
    if socket_accepts(&api) {
        return Ok(());
    }
    let legacy = jcode_harness_api::legacy_socket_path();
    let jcode = options
        .binary
        .clone()
        .unwrap_or_else(|| sibling_exe("jcode"));
    if !socket_accepts(&legacy) {
        progress("starting jcode runtime...");
        spawn_detached(&jcode, &["serve"]).map_err(|error| {
            Error::new(
                ErrorKind::LaunchFailed,
                format!("could not start the jcode runtime: {error}"),
            )
        })?;
        wait_for_socket(&legacy, "jcode runtime", options.start_timeout)?;
    }
    progress("starting harness API bridge...");
    let standalone_bridge = sibling_exe("jcode-harness-api-bridge");
    let (bridge, bridge_args): (&Path, &[&str]) = if standalone_bridge.is_file() {
        (&standalone_bridge, &[])
    } else {
        // Current Jcode distributions host the bridge as a CLI subcommand so a
        // desktop bundle only needs to ship one companion executable. Keep the
        // standalone lookup above for compatibility with older installations.
        (&jcode, &["api-bridge"])
    };
    spawn_detached(bridge, bridge_args).map_err(|error| {
        Error::new(
            ErrorKind::LaunchFailed,
            format!("could not start the harness API bridge: {error}"),
        )
    })?;
    wait_for_socket(&api, "harness API bridge", options.start_timeout)
}

fn spawn_detached(program: &Path, args: &[&str]) -> std::io::Result<()> {
    Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map(drop)
}

/// Block until `path` accepts connections, or the timeout expires.
pub fn wait_for_socket(path: &Path, what: &str, timeout: Duration) -> Result<()> {
    let deadline = Instant::now() + timeout;
    // Runtime startup normally takes only a few milliseconds after the first
    // failed dial. A fixed 100ms sleep therefore imposed almost exactly 100ms
    // of artificial latency on every cold desktop launch. Start aggressively,
    // then back off so a genuinely broken runtime still costs little CPU while
    // we wait for the diagnostic timeout.
    let mut retry_delay = Duration::from_millis(2);
    const MAX_RETRY_DELAY: Duration = Duration::from_millis(25);
    while Instant::now() < deadline {
        if socket_accepts(path) {
            return Ok(());
        }
        std::thread::sleep(retry_delay.min(deadline.saturating_duration_since(Instant::now())));
        retry_delay = (retry_delay * 2).min(MAX_RETRY_DELAY);
    }
    Err(Error::new(
        ErrorKind::LaunchFailed,
        format!("timed out waiting for the {what} at {}", path.display()),
    ))
}

#[cfg(unix)]
fn read_daemon_pid(home: &Path, runtime_dir: &Path) -> Option<i32> {
    let raw = fs::read(home.join("servers.json")).ok()?;
    let registry: Value = serde_json::from_slice(&raw).ok()?;
    let socket = runtime_dir.join("jcode.sock");
    registry.as_object()?.values().find_map(|entry| {
        let entry = entry.as_object()?;
        if Path::new(entry.get("socket")?.as_str()?) != socket {
            return None;
        }
        let pid = i32::try_from(entry.get("pid")?.as_i64()?).ok()?;
        (pid > 1).then_some(pid)
    })
}

#[cfg(unix)]
fn stop_instance_daemon(home: &Path, runtime_dir: &Path) {
    let Some(pid) = read_daemon_pid(home, runtime_dir) else {
        return;
    };
    signal_process_group(pid, libc::SIGTERM);
    let deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < deadline {
        if !process_exists(pid) {
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    signal_process_group(pid, libc::SIGKILL);
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline && process_exists(pid) {
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[cfg(not(unix))]
fn stop_instance_daemon(_home: &Path, _runtime_dir: &Path) {}

#[cfg(unix)]
fn signal_process_group(pid: i32, signal: i32) {
    // SAFETY: `kill` does not retain pointers; both calls use validated pids.
    unsafe {
        if libc::kill(-pid, signal) != 0 {
            libc::kill(pid, signal);
        }
    }
}

#[cfg(unix)]
fn process_exists(pid: i32) -> bool {
    // SAFETY: signal 0 only probes whether the numeric pid exists.
    unsafe { libc::kill(pid, 0) == 0 }
}

#[cfg(unix)]
fn terminate_child(child: &mut Child) {
    if matches!(child.try_wait(), Ok(Some(_))) {
        return;
    }
    // SAFETY: the child id came from `std::process::Child` and is live here.
    unsafe {
        libc::kill(child.id() as i32, libc::SIGTERM);
    }
    let deadline = Instant::now() + Duration::from_secs(3);
    while Instant::now() < deadline {
        if matches!(child.try_wait(), Ok(Some(_))) {
            return;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    let _ = child.kill();
    let _ = child.wait();
}

#[cfg(not(unix))]
fn terminate_child(child: &mut Child) {
    if matches!(child.try_wait(), Ok(Some(_))) {
        return;
    }
    // std's Windows Child::kill uses TerminateProcess. There is no portable
    // graceful signal equivalent, so terminate immediately and reap it.
    let _ = child.kill();
    let _ = child.wait();
}

fn remove_ephemeral_home(home: &Path, timeout: Duration) {
    let Some(name) = home.file_name().and_then(OsStr::to_str) else {
        return;
    };
    if home.parent() != Some(std::env::temp_dir().as_path())
        || !name.starts_with("jcode-sdk-instance-")
    {
        return;
    }
    let deadline = Instant::now() + timeout;
    loop {
        let _ = fs::remove_dir_all(home);
        if !home.exists() || Instant::now() >= deadline {
            return;
        }
        std::thread::sleep(Duration::from_millis(250));
    }
}

fn stderr_suffix(stderr: &str) -> String {
    let trimmed = stderr.trim();
    if trimmed.is_empty() {
        String::new()
    } else {
        format!(":\n{trimmed}")
    }
}

fn home_dir() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

/// Resolve the user's platform jcode config directory.
pub fn user_app_config_dir() -> PathBuf {
    #[cfg(target_os = "macos")]
    {
        return home_dir().join("Library/Application Support/jcode");
    }
    #[cfg(target_os = "windows")]
    {
        return std::env::var_os("APPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(|| home_dir().join("AppData/Roaming"))
            .join("jcode");
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| home_dir().join(".config"))
            .join("jcode")
    }
}

#[cfg(unix)]
fn set_owner_only_dir(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700)).map_err(launch_io)
}

#[cfg(unix)]
fn set_owner_only_file(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).map_err(launch_io)
}

#[cfg(not(unix))]
fn set_owner_only_dir(_path: &Path) -> Result<()> {
    // Windows access is controlled by inherited ACLs. std does not expose an
    // owner-only ACL operation, and marking a path read-only is not equivalent.
    Ok(())
}

#[cfg(not(unix))]
fn set_owner_only_file(_path: &Path) -> Result<()> {
    Ok(())
}

fn launch_io(error: std::io::Error) -> Error {
    Error::new(ErrorKind::LaunchFailed, error.to_string())
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn swarm_model_reaches_runtime_and_overrides_generic_environment() {
        use std::os::unix::fs::PermissionsExt;

        let sandbox = tempfile::tempdir().expect("sandbox");
        let binary = sandbox.path().join("capture-env");
        let captured = sandbox.path().join("swarm-model.txt");
        fs::write(
            &binary,
            "#!/bin/sh\nprintf '%s' \"$JCODE_SWARM_MODEL\" > \"$CAPTURE_PATH\"\nexit 1\n",
        )
        .expect("write fake runtime");
        fs::set_permissions(&binary, fs::Permissions::from_mode(0o700))
            .expect("make fake runtime executable");

        let mut options = LaunchOptions {
            jcode_home: Some(sandbox.path().join("instance")),
            inherit_logins: false,
            binary: Some(binary),
            swarm_model: Some("inherit".to_string()),
            startup_timeout: Duration::from_secs(2),
            ..LaunchOptions::default()
        };
        options.env.insert(
            OsString::from("CAPTURE_PATH"),
            captured.as_os_str().to_owned(),
        );
        options.env.insert(
            OsString::from("JCODE_SWARM_MODEL"),
            OsString::from("unwanted-model"),
        );

        assert!(
            launch_instance(&options).is_err(),
            "fake runtime should fail startup"
        );
        assert_eq!(
            fs::read_to_string(captured).expect("captured model"),
            "inherit"
        );
    }

    #[cfg(unix)]
    #[test]
    fn wake_mode_reaches_runtime_and_overrides_generic_environment() {
        use std::os::unix::fs::PermissionsExt;

        let sandbox = tempfile::tempdir().expect("sandbox");
        let binary = sandbox.path().join("capture-env");
        let captured = sandbox.path().join("wake-mode.txt");
        fs::write(
            &binary,
            "#!/bin/sh\nprintf '%s' \"$JCODE_WAKE_MODE\" > \"$CAPTURE_PATH\"\nexit 1\n",
        )
        .unwrap();
        fs::set_permissions(&binary, fs::Permissions::from_mode(0o700)).unwrap();
        let mut options = LaunchOptions {
            jcode_home: Some(sandbox.path().join("instance")),
            inherit_logins: false,
            binary: Some(binary),
            wake_mode: Some(WakeMode::External),
            startup_timeout: Duration::from_secs(2),
            ..LaunchOptions::default()
        };
        options
            .env
            .insert("CAPTURE_PATH".into(), captured.as_os_str().to_owned());
        options
            .env
            .insert("JCODE_WAKE_MODE".into(), "internal".into());
        assert!(launch_instance(&options).is_err());
        assert_eq!(fs::read_to_string(captured).unwrap(), "external");
    }

    #[test]
    fn dropping_an_ephemeral_instance_stops_its_bridge_and_removes_its_home() {
        let home = tempfile::Builder::new()
            .prefix("jcode-sdk-instance-")
            .tempdir()
            .expect("temp home")
            .keep();
        let runtime_dir = home.join("run");
        fs::create_dir(&runtime_dir).expect("runtime dir");
        let child = Command::new("sleep")
            .arg("30")
            .spawn()
            .expect("sleep child");
        let pid = child.id() as i32;
        let instance = LaunchedInstance {
            socket_path: runtime_dir.join("jcode-api.sock"),
            jcode_home: home.clone(),
            runtime_dir,
            child: Mutex::new(Some(child)),
            ephemeral: true,
            cleanup_timeout: Duration::ZERO,
            stopped: AtomicBool::new(false),
        };

        drop(instance);

        assert!(!process_exists(pid), "owned bridge process survived Drop");
        assert!(!home.exists(), "ephemeral instance home survived Drop");
    }
}
