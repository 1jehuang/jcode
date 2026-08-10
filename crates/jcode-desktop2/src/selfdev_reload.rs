//! Stable-host activation of newly built desktop2 application workers.

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

static RELOAD_REQUESTED: AtomicBool = AtomicBool::new(false);
const WORKER_MARKER: &str = "desktop2-worker-current";

pub struct Registration(Option<PathBuf>);

impl Drop for Registration {
    fn drop(&mut self) {
        if let Some(path) = &self.0 {
            let _ = std::fs::remove_file(path);
        }
    }
}

#[cfg(unix)]
extern "C" fn request_reload(_: libc::c_int) {
    RELOAD_REQUESTED.store(true, Ordering::Release);
}

/// Install the tiny async-signal-safe handler used by selfdev builds.
pub fn install() {
    #[cfg(unix)]
    // SAFETY: the handler only performs an atomic store, which is
    // async-signal-safe. SIGUSR2 is reserved for desktop selfdev reloads.
    unsafe {
        libc::signal(
            libc::SIGUSR2,
            request_reload as *const () as libc::sighandler_t,
        );
    }
}

/// Opt this stable host into future worker-build broadcasts.
pub fn register(worker_build: u64) -> Registration {
    let path = selfdev_dir().and_then(|dir| {
        let instances = dir.join("desktop2-instances");
        std::fs::create_dir_all(&instances).ok()?;
        let path = instances.join(std::process::id().to_string());
        std::fs::write(
            &path,
            format!(
                "host_pid={}\nworker_build={worker_build:016x}\nworker=builtin\n",
                std::process::id()
            ),
        )
        .ok()?;
        Some(path)
    });
    Registration(path)
}

pub fn requested() -> bool {
    RELOAD_REQUESTED.swap(false, Ordering::AcqRel)
}

/// Ask the stable dispatcher to activate the currently published worker.
///
/// A dynamically loaded worker has its own copy of this module's statics, so a
/// direct atomic store would set the wrong generation. Raising the registered
/// process signal always reaches the permanent host's handler.
pub fn request() {
    #[cfg(unix)]
    // SAFETY: raise delivers a signal to this process. The installed handler is
    // async-signal-safe and performs only an atomic store.
    unsafe {
        libc::raise(libc::SIGUSR2);
    }
    #[cfg(not(unix))]
    RELOAD_REQUESTED.store(true, Ordering::Release);
}

pub fn worker_path() -> Result<PathBuf> {
    let marker = selfdev_dir()
        .context("HOME is not set")?
        .join(WORKER_MARKER);
    let raw = std::fs::read_to_string(&marker)
        .with_context(|| format!("read worker marker {}", marker.display()))?;
    let path = PathBuf::from(raw.trim());
    if path.as_os_str().is_empty() {
        anyhow::bail!("desktop2 worker marker is empty: {}", marker.display());
    }
    Ok(path)
}

/// Publish observable proof only after a callback from the new generation ran.
pub fn record_activation(path: &Path, build: u64) {
    let Some(dir) = selfdev_dir() else {
        return;
    };
    let marker = dir
        .join("desktop2-instances")
        .join(std::process::id().to_string());
    let contents = format!(
        "host_pid={}\nworker_build={build:016x}\nworker={}\n",
        std::process::id(),
        path.display()
    );
    if let Err(error) = std::fs::write(&marker, contents) {
        eprintln!("desktop2 worker activation marker failed: {error}");
    }
}

fn selfdev_dir() -> Option<PathBuf> {
    Some(PathBuf::from(std::env::var_os("HOME")?).join(".jcode/selfdev"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reload_flag_is_consumed_once() {
        request();
        assert!(requested());
        assert!(!requested());
    }
}
