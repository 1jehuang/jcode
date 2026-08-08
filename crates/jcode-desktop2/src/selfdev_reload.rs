//! Graceful adoption of a newly built desktop2 binary.

use std::sync::atomic::{AtomicBool, Ordering};

static RELOAD_REQUESTED: AtomicBool = AtomicBool::new(false);

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

fn marker_path() -> Option<std::path::PathBuf> {
    Some(
        std::path::PathBuf::from(std::env::var_os("HOME")?).join(".jcode/selfdev/desktop2-current"),
    )
}

/// Start the activated build with this process's environment and working
/// directory. The caller exits the old event loop only after spawning works.
pub fn relaunch() -> anyhow::Result<()> {
    let marker = marker_path().ok_or_else(|| anyhow::anyhow!("HOME is not set"))?;
    let binary = std::fs::read_to_string(&marker)?.trim().to_owned();
    if binary.is_empty() {
        anyhow::bail!("desktop2 selfdev marker is empty: {}", marker.display());
    }
    std::process::Command::new(binary).spawn()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reload_flag_is_consumed_once() {
        RELOAD_REQUESTED.store(true, Ordering::Release);
        assert!(requested());
        assert!(!requested());
    }
}
