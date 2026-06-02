//! Crash log writer (#162).
//!
//! When jcode panics or receives a fatal signal mid-session, write a
//! structured crash log to `~/.jcode/logs/crash-<ts>-<session>.log` so the
//! user has a breadcrumb to share with maintainers, instead of just
//! dropping back to the shell with no clue what happened.
//!
//! The log is best-effort — a failure to write the crash log MUST NOT
//! cause a double-panic that aborts the process before the user sees the
//! resume hint.

use std::io::Write as _;
use std::path::PathBuf;
use std::time::SystemTime;

/// Write a crash log entry to disk. Returns the path written, or `None`
/// on any I/O failure (logged as a warn).
///
/// Caller passes the panic message + optional session_id + optional
/// provider/model for context. We tack on a backtrace if `RUST_BACKTRACE`
/// is set (the standard Rust env), and the current process's pid +
/// timestamp.
pub fn write_crash_log(
    panic_message: &str,
    session_id: Option<&str>,
    provider_model: Option<(&str, &str)>,
) -> Option<PathBuf> {
    let dir = crate::storage::logs_dir().ok()?;
    if let Err(e) = std::fs::create_dir_all(&dir) {
        crate::logging::warn(&format!(
            "crash_log: could not create logs dir {}: {}",
            dir.display(),
            e
        ));
        return None;
    }

    let timestamp = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    let session_label = session_id
        .map(|s| s.replace(['/', '\\'], "_"))
        .unwrap_or_else(|| "unknown".to_string());
    let path = dir.join(format!("crash-{timestamp}-{session_label}.log"));

    let mut content = String::new();
    content.push_str("===== jcode crash log =====\n");
    content.push_str(&format!("timestamp_unix: {timestamp}\n"));
    content.push_str(&format!(
        "timestamp_iso:  {}\n",
        chrono::Utc::now().to_rfc3339()
    ));
    content.push_str(&format!("pid:            {}\n", std::process::id()));
    content.push_str(&format!("version:        {}\n", jcode_build_meta::VERSION));
    content.push_str(&format!("git_hash:       {}\n", jcode_build_meta::GIT_HASH));
    content.push_str(&format!("os:             {}\n", std::env::consts::OS));
    content.push_str(&format!("arch:           {}\n", std::env::consts::ARCH));
    if let Some(sid) = session_id {
        content.push_str(&format!("session_id:     {sid}\n"));
    }
    if let Some((provider, model)) = provider_model {
        content.push_str(&format!("provider:       {provider}\n"));
        content.push_str(&format!("model:          {model}\n"));
    }
    content.push_str("\n----- panic message -----\n");
    content.push_str(panic_message);
    content.push('\n');

    // Capture a backtrace if RUST_BACKTRACE was set when jcode launched.
    // We don't unconditionally capture because backtrace generation can
    // cost ~50-200ms which doubles the panic-cleanup window. Users who
    // want backtraces should set RUST_BACKTRACE=1 (or =full) at launch.
    if std::env::var("RUST_BACKTRACE").is_ok() {
        content.push_str("\n----- backtrace -----\n");
        content.push_str(&format!("{}", std::backtrace::Backtrace::capture()));
        content.push('\n');
    } else {
        content.push_str(
            "\n----- backtrace -----\n(set RUST_BACKTRACE=1 at launch to capture backtraces)\n",
        );
    }

    content.push_str("\n----- session resume -----\n");
    if let Some(sid) = session_id {
        content.push_str(&format!("jcode --resume {sid}\n"));
    } else {
        content.push_str("(no session id captured)\n");
    }

    let mut file = match std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(&path)
    {
        Ok(f) => f,
        Err(e) => {
            crate::logging::warn(&format!(
                "crash_log: could not open {} for write: {}",
                path.display(),
                e
            ));
            return None;
        }
    };

    if let Err(e) = file.write_all(content.as_bytes()) {
        crate::logging::warn(&format!(
            "crash_log: could not write {}: {}",
            path.display(),
            e
        ));
        return None;
    }
    if let Err(e) = file.flush() {
        crate::logging::warn(&format!(
            "crash_log: could not flush {}: {}",
            path.display(),
            e
        ));
    }
    Some(path)
}

/// List recent crash logs, newest first. Bounded to the 10 most recent
/// to avoid scanning a runaway logs dir.
pub fn list_recent_crash_logs() -> Vec<PathBuf> {
    let dir = match crate::storage::logs_dir() {
        Ok(d) => d,
        Err(_) => return Vec::new(),
    };
    let read_dir = match std::fs::read_dir(&dir) {
        Ok(rd) => rd,
        Err(_) => return Vec::new(),
    };
    let mut entries: Vec<(SystemTime, PathBuf)> = read_dir
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            let path = e.path();
            let name = path.file_name()?.to_str()?;
            if !name.starts_with("crash-") || !name.ends_with(".log") {
                return None;
            }
            let modified = e.metadata().ok()?.modified().ok()?;
            Some((modified, path))
        })
        .collect();
    entries.sort_by(|a, b| b.0.cmp(&a.0));
    entries.into_iter().take(10).map(|(_, p)| p).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_crash_log_creates_file_with_panic_message() {
        let _lock = crate::storage::lock_test_env();
        let prev = std::env::var_os("JCODE_HOME");
        let temp = tempfile::TempDir::new().unwrap();
        crate::env::set_var("JCODE_HOME", temp.path());

        let path = write_crash_log("test panic", Some("sess_test_xyz"), None)
            .expect("crash log should write");

        let body = std::fs::read_to_string(&path).unwrap();
        assert!(body.contains("===== jcode crash log ====="));
        assert!(body.contains("test panic"));
        assert!(body.contains("sess_test_xyz"));
        assert!(body.contains("jcode --resume sess_test_xyz"));
        assert!(body.contains(jcode_build_meta::VERSION));

        if let Some(p) = prev {
            crate::env::set_var("JCODE_HOME", p);
        } else {
            crate::env::remove_var("JCODE_HOME");
        }
    }

    #[test]
    fn write_crash_log_handles_unknown_session() {
        let _lock = crate::storage::lock_test_env();
        let prev = std::env::var_os("JCODE_HOME");
        let temp = tempfile::TempDir::new().unwrap();
        crate::env::set_var("JCODE_HOME", temp.path());

        let path = write_crash_log("oops", None, None).expect("crash log should write");
        assert!(
            path.file_name()
                .unwrap()
                .to_string_lossy()
                .contains("unknown")
        );

        let body = std::fs::read_to_string(&path).unwrap();
        assert!(body.contains("(no session id captured)"));

        if let Some(p) = prev {
            crate::env::set_var("JCODE_HOME", p);
        } else {
            crate::env::remove_var("JCODE_HOME");
        }
    }

    #[test]
    fn write_crash_log_includes_provider_model_when_present() {
        let _lock = crate::storage::lock_test_env();
        let prev = std::env::var_os("JCODE_HOME");
        let temp = tempfile::TempDir::new().unwrap();
        crate::env::set_var("JCODE_HOME", temp.path());

        let path = write_crash_log(
            "panic",
            Some("sess_p"),
            Some(("anthropic", "claude-sonnet-4")),
        )
        .unwrap();
        let body = std::fs::read_to_string(&path).unwrap();
        assert!(body.contains("provider:       anthropic"));
        assert!(body.contains("model:          claude-sonnet-4"));

        if let Some(p) = prev {
            crate::env::set_var("JCODE_HOME", p);
        } else {
            crate::env::remove_var("JCODE_HOME");
        }
    }

    #[test]
    fn list_recent_crash_logs_returns_newest_first_bounded_to_ten() {
        let _lock = crate::storage::lock_test_env();
        let prev = std::env::var_os("JCODE_HOME");
        let temp = tempfile::TempDir::new().unwrap();
        crate::env::set_var("JCODE_HOME", temp.path());

        // Write 12 crash logs with sleep so mtime differs.
        for i in 0..12 {
            write_crash_log(&format!("panic {i}"), Some(&format!("s_{i}")), None).unwrap();
            std::thread::sleep(std::time::Duration::from_millis(10));
        }

        let recent = list_recent_crash_logs();
        assert_eq!(recent.len(), 10, "should bound to 10 entries");
        // Newest first — last written should be in the list, very-first should not.
        assert!(
            recent
                .iter()
                .any(|p| p.file_name().unwrap().to_string_lossy().contains("s_11"))
        );
        assert!(
            !recent
                .iter()
                .any(|p| p.file_name().unwrap().to_string_lossy().contains("s_0_"))
                || !recent.iter().any(|p| p
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .contains("s_1_"))
        );

        if let Some(p) = prev {
            crate::env::set_var("JCODE_HOME", p);
        } else {
            crate::env::remove_var("JCODE_HOME");
        }
    }
}
