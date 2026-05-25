use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;

const TEMP_SERVER_ENV: &str = "JCODE_TEMP_SERVER";
const SERVER_SCOPE_ENV: &str = "JCODE_SERVER_SCOPE";
const OWNER_PID_ENV: &str = "JCODE_SERVER_OWNER_PID";
const TEMP_IDLE_SECS_ENV: &str = "JCODE_TEMP_SERVER_IDLE_SECS";
const DEFAULT_TEMP_IDLE_SECS: u64 = 30 * 60;
const TEMP_SERVER_EXIT_CODE: i32 = super::EXIT_IDLE_TIMEOUT;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct TemporaryServerPolicy {
    pub(crate) owner_pid: Option<u32>,
    pub(crate) idle_timeout_secs: u64,
}

#[derive(Debug, Serialize, Deserialize)]
struct TemporaryServerMetadata {
    schema_version: u32,
    scope: String,
    pid: u32,
    ppid: Option<u32>,
    owner_pid: Option<u32>,
    started_at: String,
    socket_path: String,
    debug_socket_path: String,
    idle_timeout_secs: u64,
    argv: Vec<String>,
}

pub(crate) fn configure_temporary_server(owner_pid: Option<u32>, idle_timeout_secs: Option<u64>) {
    crate::env::set_var(TEMP_SERVER_ENV, "1");
    crate::env::set_var(SERVER_SCOPE_ENV, "temporary");
    if let Some(owner_pid) = owner_pid {
        crate::env::set_var(OWNER_PID_ENV, owner_pid.to_string());
    }
    if let Some(idle_timeout_secs) = idle_timeout_secs {
        crate::env::set_var(TEMP_IDLE_SECS_ENV, idle_timeout_secs.to_string());
    }
}

pub(crate) fn temporary_server_policy_from_env() -> Option<TemporaryServerPolicy> {
    if !temporary_server_env_enabled() {
        return None;
    }

    let owner_pid = std::env::var(OWNER_PID_ENV)
        .ok()
        .and_then(|value| value.parse::<u32>().ok())
        .filter(|pid| *pid > 0);
    let idle_timeout_secs = std::env::var(TEMP_IDLE_SECS_ENV)
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(DEFAULT_TEMP_IDLE_SECS);

    Some(TemporaryServerPolicy {
        owner_pid,
        idle_timeout_secs,
    })
}

fn temporary_server_env_enabled() -> bool {
    env_truthy(TEMP_SERVER_ENV)
        || std::env::var(SERVER_SCOPE_ENV)
            .ok()
            .map(|value| value.eq_ignore_ascii_case("temporary"))
            .unwrap_or(false)
}

fn env_truthy(name: &str) -> bool {
    std::env::var(name)
        .ok()
        .map(|value| matches!(value.as_str(), "1" | "true" | "yes" | "on"))
        .unwrap_or(false)
}

pub(crate) fn metadata_path(socket_path: &Path) -> PathBuf {
    let filename = socket_path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("jcode.sock");
    socket_path.with_file_name(format!("{filename}.server.json"))
}

pub(crate) fn write_temporary_metadata(
    socket_path: &Path,
    debug_socket_path: &Path,
    policy: &TemporaryServerPolicy,
) -> Option<PathBuf> {
    let path = metadata_path(socket_path);
    let metadata = TemporaryServerMetadata {
        schema_version: 1,
        scope: "temporary".to_string(),
        pid: std::process::id(),
        ppid: parent_pid(),
        owner_pid: policy.owner_pid,
        started_at: chrono::Utc::now().to_rfc3339(),
        socket_path: socket_path.display().to_string(),
        debug_socket_path: debug_socket_path.display().to_string(),
        idle_timeout_secs: policy.idle_timeout_secs,
        argv: std::env::args().collect(),
    };

    if let Some(parent) = path.parent()
        && let Err(error) = std::fs::create_dir_all(parent)
    {
        crate::logging::warn(&format!(
            "Failed to create temporary server metadata directory {}: {}",
            parent.display(),
            error
        ));
        return None;
    }

    match serde_json::to_vec_pretty(&metadata)
        .ok()
        .and_then(|bytes| std::fs::write(&path, bytes).ok().map(|_| ()))
    {
        Some(()) => Some(path),
        None => {
            crate::logging::warn(&format!(
                "Failed to write temporary server metadata {}",
                path.display()
            ));
            None
        }
    }
}

pub(crate) fn cleanup_temporary_metadata(socket_path: &Path) {
    let _ = std::fs::remove_file(metadata_path(socket_path));
}

pub(crate) fn spawn_temporary_lifecycle_monitor(
    client_count: Arc<RwLock<usize>>,
    socket_path: PathBuf,
    debug_socket_path: PathBuf,
    server_name: String,
    policy: TemporaryServerPolicy,
) {
    tokio::spawn(async move {
        let mut idle_since: Option<Instant> = None;
        let mut check_interval = tokio::time::interval(Duration::from_secs(10));

        loop {
            check_interval.tick().await;

            if let Some(owner_pid) = policy.owner_pid
                && owner_pid != std::process::id()
                && !process_alive(owner_pid)
            {
                crate::logging::info(&format!(
                    "Temporary server owner pid {} is gone. Shutting down.",
                    owner_pid
                ));
                shutdown_temporary_server(&server_name, &socket_path, &debug_socket_path).await;
            }

            let count = *client_count.read().await;
            if count == 0 {
                if idle_since.is_none() {
                    idle_since = Some(Instant::now());
                    crate::logging::info(&format!(
                        "Temporary server has no clients. It will exit after {} seconds idle.",
                        policy.idle_timeout_secs
                    ));
                }

                if let Some(since) = idle_since
                    && since.elapsed().as_secs() >= policy.idle_timeout_secs
                {
                    crate::logging::info(&format!(
                        "Temporary server idle for {} seconds. Shutting down.",
                        since.elapsed().as_secs()
                    ));
                    shutdown_temporary_server(&server_name, &socket_path, &debug_socket_path).await;
                }
            } else {
                if idle_since.is_some() {
                    crate::logging::info(
                        "Temporary server client connected. Idle timer cancelled.",
                    );
                }
                idle_since = None;
            }
        }
    });
}

async fn shutdown_temporary_server(
    server_name: &str,
    socket_path: &Path,
    debug_socket_path: &Path,
) -> ! {
    let _ = crate::registry::unregister_server(server_name).await;
    crate::transport::remove_socket(socket_path);
    crate::transport::remove_socket(debug_socket_path);
    cleanup_temporary_metadata(socket_path);
    std::process::exit(TEMP_SERVER_EXIT_CODE);
}

#[cfg(unix)]
fn parent_pid() -> Option<u32> {
    let ppid = unsafe { libc::getppid() };
    (ppid > 0).then_some(ppid as u32)
}

#[cfg(not(unix))]
fn parent_pid() -> Option<u32> {
    None
}

#[cfg(unix)]
pub(crate) fn process_alive(pid: u32) -> bool {
    if pid == 0 {
        return false;
    }

    let rc = unsafe { libc::kill(pid as libc::pid_t, 0) };
    if rc == 0 {
        return true;
    }

    matches!(
        std::io::Error::last_os_error().raw_os_error(),
        Some(libc::EPERM)
    )
}

#[cfg(not(unix))]
pub(crate) fn process_alive(_pid: u32) -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Mutex, OnceLock};

    static TEST_ENV_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

    struct EnvGuard {
        _lock: std::sync::MutexGuard<'static, ()>,
        entries: Vec<(&'static str, Option<std::ffi::OsString>)>,
    }

    impl EnvGuard {
        fn capture(names: &[&'static str]) -> Self {
            let _lock = TEST_ENV_LOCK
                .get_or_init(|| Mutex::new(()))
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let entries = names
                .iter()
                .map(|name| (*name, std::env::var_os(name)))
                .collect();
            Self { _lock, entries }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            for (name, value) in &self.entries {
                if let Some(value) = value {
                    crate::env::set_var(name, value);
                } else {
                    crate::env::remove_var(name);
                }
            }
        }
    }

    #[test]
    fn temporary_policy_requires_explicit_marker() {
        let _guard = EnvGuard::capture(&[TEMP_SERVER_ENV, SERVER_SCOPE_ENV, OWNER_PID_ENV]);
        crate::env::remove_var(TEMP_SERVER_ENV);
        crate::env::remove_var(SERVER_SCOPE_ENV);
        crate::env::set_var(OWNER_PID_ENV, "123");

        assert_eq!(temporary_server_policy_from_env(), None);
    }

    #[test]
    fn temporary_policy_reads_owner_and_timeout() {
        let _guard = EnvGuard::capture(&[
            TEMP_SERVER_ENV,
            SERVER_SCOPE_ENV,
            OWNER_PID_ENV,
            TEMP_IDLE_SECS_ENV,
        ]);
        crate::env::set_var(SERVER_SCOPE_ENV, "temporary");
        crate::env::set_var(OWNER_PID_ENV, "123");
        crate::env::set_var(TEMP_IDLE_SECS_ENV, "42");

        assert_eq!(
            temporary_server_policy_from_env(),
            Some(TemporaryServerPolicy {
                owner_pid: Some(123),
                idle_timeout_secs: 42,
            })
        );
    }

    #[test]
    fn temporary_metadata_path_is_socket_scoped() {
        assert_eq!(
            metadata_path(Path::new("/tmp/example/jcode.sock")),
            PathBuf::from("/tmp/example/jcode.sock.server.json")
        );
    }

    #[cfg(unix)]
    #[test]
    fn current_process_is_alive() {
        assert!(process_alive(std::process::id()));
        assert!(!process_alive(0));
    }
}
