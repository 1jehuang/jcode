//! Find the session of the running always-on Jarvis client (Linux).
//!
//! An always-on Jarvis TUI (`jcode --jarvis-always-on`) owns the microphone
//! lease in `$XDG_RUNTIME_DIR/jcode-jarvis` and rewrites `<pid>.json` at least
//! once a second. Its session is the one the client registered for its PID.
//! Stale files from crashed clients are skipped: the PID must be alive, still
//! an always-on Jarvis process, and the file recently written.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// Jarvis rewrites its status at least every second while it runs.
const FRESH: Duration = Duration::from_secs(10);

/// A live always-on Jarvis client.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct JarvisTarget {
    pub pid: u32,
    pub session: String,
}

pub(crate) fn status_dir() -> Option<PathBuf> {
    std::env::var_os("XDG_RUNTIME_DIR").map(|dir| PathBuf::from(dir).join("jcode-jarvis"))
}

/// Ask Jarvis `pid` to act on recently heard speech (it consumes `<pid>.wake`).
pub(crate) fn request_wake(pid: u32) -> std::io::Result<()> {
    use std::os::unix::fs::OpenOptionsExt;
    let dir = status_dir().ok_or_else(|| std::io::Error::other("XDG_RUNTIME_DIR unset"))?;
    std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .mode(0o600)
        .custom_flags(0o400000) // O_NOFOLLOW on Linux
        .open(dir.join(format!("{pid}.wake")))
        .map(drop)
}

/// The most recently active always-on Jarvis client with a registered session.
pub(crate) fn live_jarvis() -> Option<JarvisTarget> {
    find(
        &status_dir()?,
        SystemTime::now(),
        is_always_on_jarvis,
        jcode_base::client_focus::registered_client_session,
    )
}

fn find(
    dir: &Path,
    now: SystemTime,
    alive: impl Fn(u32) -> bool,
    session_of: impl Fn(u32) -> Option<String>,
) -> Option<JarvisTarget> {
    let mut best: Option<(SystemTime, JarvisTarget)> = None;
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) => {
            if error.kind() != std::io::ErrorKind::NotFound {
                jcode_base::logging::debug(&format!(
                    "cannot scan Jarvis status {}: {error}",
                    dir.display()
                ));
            }
            return None;
        }
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
            continue;
        }
        let Some(stem) = path.file_stem().and_then(|stem| stem.to_str()) else {
            continue;
        };
        let Ok(pid) = stem.parse::<u32>() else {
            continue;
        };
        let Ok(modified) = entry.metadata().and_then(|meta| meta.modified()) else {
            continue;
        };
        // Future timestamps are treated as fresh after a clock adjustment.
        if now.duration_since(modified).is_ok_and(|age| age > FRESH) {
            continue;
        }
        if best.as_ref().is_some_and(|(at, _)| *at >= modified) || !alive(pid) {
            continue;
        }
        let Some(session) = session_of(pid) else {
            continue;
        };
        best = Some((modified, JarvisTarget { pid, session }));
    }
    best.map(|(_, target)| target)
}

/// Guards against PID reuse: the process must be a Jcode always-on client.
fn is_always_on_jarvis(pid: u32) -> bool {
    std::fs::read(format!("/proc/{pid}/cmdline")).is_ok_and(|cmdline| {
        cmdline
            .split(|byte| *byte == 0)
            .any(|arg| arg == b"--jarvis-always-on")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn touch(dir: &Path, name: &str, age: Duration) {
        let path = dir.join(name);
        std::fs::write(&path, b"{}").unwrap();
        let file = std::fs::File::options().write(true).open(&path).unwrap();
        file.set_modified(SystemTime::now() - age).unwrap();
    }

    #[test]
    fn picks_freshest_live_registered_jarvis() {
        let dir = tempfile::tempdir().unwrap();
        touch(dir.path(), "100.json", Duration::from_secs(2));
        touch(dir.path(), "200.json", Duration::from_secs(1));
        // Newest, but its process is gone.
        touch(dir.path(), "300.json", Duration::ZERO);
        // Long dead, never rewritten.
        touch(dir.path(), "400.json", Duration::from_secs(3600));
        touch(dir.path(), "500.tmp", Duration::ZERO);
        touch(dir.path(), "capture.lock", Duration::ZERO);
        let found = find(
            dir.path(),
            SystemTime::now(),
            |pid| pid != 300,
            |pid| Some(format!("session_{pid}")),
        );
        assert_eq!(
            found,
            Some(JarvisTarget {
                pid: 200,
                session: "session_200".into()
            })
        );
    }

    #[test]
    fn no_session_or_no_live_client_finds_nothing() {
        let dir = tempfile::tempdir().unwrap();
        touch(dir.path(), "100.json", Duration::ZERO);
        assert_eq!(
            find(dir.path(), SystemTime::now(), |_| true, |_| None),
            None
        );
        assert_eq!(
            find(
                dir.path(),
                SystemTime::now(),
                |_| false,
                |pid| Some(pid.to_string())
            ),
            None
        );
        assert_eq!(
            find(
                &dir.path().join("missing"),
                SystemTime::now(),
                |_| true,
                |_| None
            ),
            None
        );
    }

    #[test]
    fn this_test_process_is_not_jarvis() {
        assert!(!is_always_on_jarvis(std::process::id()));
    }
}
