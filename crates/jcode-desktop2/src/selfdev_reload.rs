//! Graceful adoption of a newly built desktop2 binary.

use std::sync::atomic::{AtomicBool, Ordering};

static RELOAD_REQUESTED: AtomicBool = AtomicBool::new(false);
const READY_ENV: &str = "JCODE_DESKTOP2_RELOAD_READY";
const READY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15);

pub struct Registration(Option<std::path::PathBuf>);

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
        libc::signal(libc::SIGUSR2, request_reload as libc::sighandler_t);
    }
}

/// Opt this process into future build broadcasts. Older desktop builds do not
/// register, so the first build that introduces reload support cannot
/// accidentally terminate them with a signal they do not handle.
pub fn register() -> Registration {
    let path = marker_path().and_then(|marker| {
        let dir = marker.parent()?.join("desktop2-instances");
        std::fs::create_dir_all(&dir).ok()?;
        let path = dir.join(std::process::id().to_string());
        std::fs::write(&path, b"ready\n").ok()?;
        Some(path)
    });
    Registration(path)
}

pub fn requested() -> bool {
    RELOAD_REQUESTED.swap(false, Ordering::AcqRel)
}

/// Ask the event loop to relaunch from the currently activated desktop build.
/// Using the same flag as the self-dev signal keeps geometry saving and graceful
/// replacement behavior identical for automatic and keyboard-triggered reloads.
pub fn request() {
    RELOAD_REQUESTED.store(true, Ordering::Release);
}

/// Tell the window which launched this process that our replacement surface is
/// alive. This deliberately happens after window and GPU creation, rather than
/// at process start, so a failed replacement never makes the visible window
/// disappear.
pub fn acknowledge_ready() {
    let Some(path) = std::env::var_os(READY_ENV).map(std::path::PathBuf::from) else {
        return;
    };
    if let Err(error) = std::fs::write(&path, b"ready\n") {
        eprintln!("desktop2 selfdev reload acknowledgement failed: {error}");
    }
}

fn marker_path() -> Option<std::path::PathBuf> {
    Some(
        std::path::PathBuf::from(std::env::var_os("HOME")?).join(".jcode/selfdev/desktop2-current"),
    )
}

/// Start the activated build with this process's environment and working
/// directory. The old window remains visible until the replacement has created
/// its own window and GPU surface. This avoids a reload looking like all desktop
/// windows were closed when startup takes a moment (or fails altogether).
pub fn relaunch() -> anyhow::Result<()> {
    let marker = marker_path().ok_or_else(|| anyhow::anyhow!("HOME is not set"))?;
    let binary = std::fs::read_to_string(&marker)?.trim().to_owned();
    if binary.is_empty() {
        anyhow::bail!("desktop2 selfdev marker is empty: {}", marker.display());
    }
    let ready = marker
        .parent()
        .expect("selfdev marker has a parent")
        .join(format!(
            ".desktop2-ready-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
    let mut child = std::process::Command::new(binary)
        .env(READY_ENV, &ready)
        .spawn()?;
    let deadline = std::time::Instant::now() + READY_TIMEOUT;
    loop {
        if ready.exists() {
            let _ = std::fs::remove_file(&ready);
            return Ok(());
        }
        if let Some(status) = child.try_wait()? {
            anyhow::bail!("replacement exited before opening a window ({status})");
        }
        if std::time::Instant::now() >= deadline {
            anyhow::bail!("replacement did not open a window within {READY_TIMEOUT:?}");
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
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
