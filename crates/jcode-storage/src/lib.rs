use anyhow::Result;
use serde::Serialize;
use serde::de::DeserializeOwned;
use std::io::Write;
use std::path::{Path, PathBuf};

mod active_pids;
pub use active_pids::{
    active_pids_dir, active_session_ids, find_active_session_id_by_pid, register_active_pid,
    unregister_active_pid,
};

/// Platform-aware runtime directory for sockets and ephemeral state.
///
/// - Linux: `$XDG_RUNTIME_DIR` (typically `/run/user/<uid>`)
/// - macOS: `$TMPDIR` (per-user, e.g. `/var/folders/xx/.../T/`)
/// - Fallback: `std::env::temp_dir()`
///
/// Can be overridden with `$JCODE_RUNTIME_DIR`.
pub fn runtime_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("JCODE_RUNTIME_DIR") {
        return PathBuf::from(dir);
    }
    if let Ok(dir) = std::env::var("XDG_RUNTIME_DIR") {
        return PathBuf::from(dir);
    }
    #[cfg(target_os = "macos")]
    {
        if let Ok(dir) = std::env::var("TMPDIR") {
            return PathBuf::from(dir);
        }
    }

    let dir = fallback_runtime_dir();
    ensure_private_runtime_dir(&dir);
    dir
}

fn fallback_runtime_dir() -> PathBuf {
    std::env::temp_dir().join(format!("jcode-{}", runtime_user_discriminator()))
}

#[cfg(unix)]
fn runtime_user_discriminator() -> String {
    unsafe { libc::geteuid() }.to_string()
}

#[cfg(not(unix))]
fn runtime_user_discriminator() -> String {
    let raw = std::env::var("USERNAME")
        .or_else(|_| std::env::var("USER"))
        .unwrap_or_else(|_| "user".to_string());
    let sanitized: String = raw
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_'))
        .take(64)
        .collect();
    if sanitized.is_empty() {
        "user".to_string()
    } else {
        sanitized
    }
}

fn ensure_private_runtime_dir(path: &Path) {
    let _ = std::fs::create_dir_all(path);
    #[cfg(unix)]
    {
        let _ = jcode_core::fs::set_directory_permissions_owner_only(path);
    }
}

pub fn jcode_dir() -> Result<PathBuf> {
    if let Ok(path) = std::env::var("JCODE_HOME") {
        return Ok(PathBuf::from(path));
    }

    // Issue #123: opt-in XDG Base Directory Specification support.
    //
    // When `JCODE_USE_XDG=1` is set in the environment (truthy), resolve the
    // jcode home to `$XDG_DATA_HOME/jcode` (or `$HOME/.local/share/jcode` if
    // XDG_DATA_HOME is unset). This keeps the default unchanged for existing
    // users who already have `~/.jcode/`, while letting XDG-strict
    // distributions and dotfile managers move jcode out of `$HOME` root.
    //
    // Non-truthy / unset → keep the legacy `~/.jcode` location for full
    // backwards compatibility.
    if jcode_use_xdg_enabled() {
        let xdg_data = std::env::var("XDG_DATA_HOME")
            .ok()
            .filter(|s| !s.trim().is_empty())
            .map(PathBuf::from)
            .or_else(|| dirs::home_dir().map(|h| h.join(".local").join("share")))
            .ok_or_else(|| anyhow::anyhow!("No XDG_DATA_HOME and no $HOME"))?;
        return Ok(xdg_data.join("jcode"));
    }

    let home = dirs::home_dir().ok_or_else(|| anyhow::anyhow!("No home directory"))?;
    Ok(home.join(".jcode"))
}

/// Whether the user has opted into XDG Base Directory Specification paths
/// via `JCODE_USE_XDG=1` (or any truthy value). See `jcode_dir` for details.
fn jcode_use_xdg_enabled() -> bool {
    matches!(
        std::env::var("JCODE_USE_XDG")
            .ok()
            .as_deref()
            .map(str::trim)
            .map(str::to_ascii_lowercase)
            .as_deref(),
        Some("1") | Some("true") | Some("yes") | Some("on")
    )
}

pub fn logs_dir() -> Result<PathBuf> {
    Ok(jcode_dir()?.join("logs"))
}

/// Resolve jcode's app-owned config directory.
///
/// Default location is the platform config dir + `jcode` (for example
/// `~/.config/jcode` on Linux). When `JCODE_HOME` is set, sandbox this under
/// `$JCODE_HOME/config/jcode` so self-dev/tests do not leak into the user's
/// real config directory.
pub fn app_config_dir() -> Result<PathBuf> {
    if let Ok(path) = std::env::var("JCODE_HOME") {
        return Ok(PathBuf::from(path).join("config").join("jcode"));
    }

    let config_dir =
        dirs::config_dir().ok_or_else(|| anyhow::anyhow!("No config directory found"))?;
    Ok(config_dir.join("jcode"))
}

/// Resolve a path under the user's home directory, but sandbox it under
/// `$JCODE_HOME/external/` when `JCODE_HOME` is set.
///
/// This keeps external provider auth files isolated during tests and sandboxed
/// runs without changing default on-disk locations for normal users.
pub fn user_home_path(relative: impl AsRef<Path>) -> Result<PathBuf> {
    let relative = relative.as_ref();
    if relative.is_absolute() {
        anyhow::bail!(
            "user_home_path expects a relative path, got {}",
            relative.display()
        );
    }

    if let Ok(path) = std::env::var("JCODE_HOME") {
        return Ok(PathBuf::from(path).join("external").join(relative));
    }

    let home = dirs::home_dir().ok_or_else(|| anyhow::anyhow!("No home directory"))?;
    Ok(home.join(relative))
}

/// Best-effort startup hardening for local config dirs that may store credentials.
///
/// This intentionally ignores failures so startup does not fail on exotic
/// filesystems, but it narrows exposure on typical Unix systems.
pub fn harden_user_config_permissions() {
    if let Some(config_dir) = dirs::config_dir() {
        let jcode_config_dir = config_dir.join("jcode");
        if jcode_config_dir.exists() {
            let _ = jcode_core::fs::set_directory_permissions_owner_only(&jcode_config_dir);
        }
    }

    if let Ok(jcode_home) = jcode_dir()
        && jcode_home.exists()
    {
        let _ = jcode_core::fs::set_directory_permissions_owner_only(&jcode_home);
    }
}

/// Best-effort hardening for a secret-bearing file and its parent directory.
///
/// This is used before reading credential files so legacy permissive modes can
/// be tightened opportunistically.
pub fn harden_secret_file_permissions(path: &Path) {
    if let Some(parent) = path.parent() {
        let _ = jcode_core::fs::set_directory_permissions_owner_only(parent);
    }
    if path.exists() {
        let _ = jcode_core::fs::set_permissions_owner_only(path);
    }
}

/// Validate an external auth file managed by another tool before reading it.
///
/// jcode intentionally avoids mutating these files. We also reject obvious risky
/// cases like symlinks so a remembered trust decision stays bound to a real file
/// path rather than an arbitrary redirect.
pub fn validate_external_auth_file(path: &Path) -> Result<PathBuf> {
    let metadata = std::fs::symlink_metadata(path).map_err(|e| {
        anyhow::anyhow!(
            "Failed to inspect external auth file {}: {}",
            path.display(),
            e
        )
    })?;
    if metadata.file_type().is_symlink() {
        anyhow::bail!(
            "Refusing to read external auth file via symlink: {}",
            path.display()
        );
    }
    if !metadata.is_file() {
        anyhow::bail!(
            "External auth path is not a regular file: {}",
            path.display()
        );
    }
    std::fs::canonicalize(path).map_err(|e| {
        anyhow::anyhow!(
            "Failed to canonicalize external auth file {}: {}",
            path.display(),
            e
        )
    })
}

pub fn ensure_dir(path: &Path) -> Result<()> {
    if !path.exists() {
        std::fs::create_dir_all(path)?;
        jcode_core::fs::set_directory_permissions_owner_only(path)?;
    }
    Ok(())
}

pub fn write_text_secret(path: &Path, content: &str) -> Result<()> {
    write_bytes_inner(path, content.as_bytes(), true)?;
    if let Some(parent) = path.parent() {
        jcode_core::fs::set_directory_permissions_owner_only(parent)?;
    }
    jcode_core::fs::set_permissions_owner_only(path)?;
    Ok(())
}

pub fn upsert_env_file_value(path: &Path, env_key: &str, value: Option<&str>) -> Result<()> {
    let existing = std::fs::read_to_string(path).unwrap_or_default();
    let prefix = format!("{}=", env_key);

    let mut lines = Vec::new();
    let mut replaced = false;
    for line in existing.lines() {
        if line.starts_with(&prefix) {
            replaced = true;
            if let Some(value) = value {
                lines.push(format!("{}={}", env_key, value));
            }
        } else {
            lines.push(line.to_string());
        }
    }

    if !replaced && let Some(value) = value {
        lines.push(format!("{}={}", env_key, value));
    }

    let mut content = lines.join("\n");
    if !content.is_empty() {
        content.push('\n');
    }
    write_text_secret(path, &content)
}

pub fn write_json<T: Serialize + ?Sized>(path: &Path, value: &T) -> Result<()> {
    write_json_inner(path, value, true)
}

pub fn write_json_secret<T: Serialize + ?Sized>(path: &Path, value: &T) -> Result<()> {
    write_json_inner(path, value, true)?;
    if let Some(parent) = path.parent() {
        jcode_core::fs::set_directory_permissions_owner_only(parent)?;
    }
    jcode_core::fs::set_permissions_owner_only(path)?;
    Ok(())
}

/// Fast JSON write: atomic rename but no fsync. Good for frequent saves where
/// durability on power loss is not critical (e.g., session saves during tool execution).
/// Data is still safe against process crashes (atomic rename protects against partial writes).
pub fn write_json_fast<T: Serialize + ?Sized>(path: &Path, value: &T) -> Result<()> {
    write_json_inner(path, value, false)
}

fn write_json_inner<T: Serialize + ?Sized>(path: &Path, value: &T, durable: bool) -> Result<()> {
    let bytes = serde_json::to_vec(value)?;
    write_bytes_inner(path, &bytes, durable)
}

fn write_bytes_inner(path: &Path, bytes: &[u8], durable: bool) -> Result<()> {
    if let Some(parent) = path.parent() {
        ensure_dir(parent)?;
    }

    let pid = std::process::id();
    let nonce: u64 = rand::random();
    let tmp_path = path.with_extension(format!("tmp.{}.{}", pid, nonce));

    let result = (|| -> Result<()> {
        let file = std::fs::File::create(&tmp_path)?;
        let mut writer = std::io::BufWriter::new(file);
        writer.write_all(bytes)?;
        let file = writer
            .into_inner()
            .map_err(|e| anyhow::anyhow!("flush failed: {}", e))?;

        if durable {
            file.sync_all()?;
        }

        if path.exists() {
            let bak_path = path.with_extension("bak");
            let _ = std::fs::rename(path, &bak_path);
        }

        std::fs::rename(&tmp_path, path)?;

        #[cfg(unix)]
        if durable
            && let Some(parent) = path.parent()
            && let Ok(dir) = std::fs::File::open(parent)
        {
            let _ = dir.sync_all();
        }

        Ok(())
    })();

    if result.is_err() {
        let _ = std::fs::remove_file(&tmp_path);
    }

    result
}

pub enum StorageRecoveryEvent<'a> {
    CorruptPrimary {
        path: &'a Path,
        error: &'a serde_json::Error,
    },
    RecoveredFromBackup {
        backup_path: &'a Path,
    },
}

pub fn read_json<T: DeserializeOwned>(path: &Path) -> Result<T> {
    read_json_with_recovery_handler(path, |event| match event {
        StorageRecoveryEvent::CorruptPrimary { path, error } => {
            eprintln!(
                "Corrupt JSON at {}, trying backup: {}",
                path.display(),
                error
            );
        }
        StorageRecoveryEvent::RecoveredFromBackup { backup_path } => {
            eprintln!("Recovered from backup: {}", backup_path.display());
        }
    })
}

pub fn read_json_with_recovery_handler<T, F>(path: &Path, mut on_recovery: F) -> Result<T>
where
    T: DeserializeOwned,
    F: FnMut(StorageRecoveryEvent<'_>),
{
    let data = std::fs::read_to_string(path)?;
    match serde_json::from_str(&data) {
        Ok(val) => Ok(val),
        Err(e) => {
            let bak_path = path.with_extension("bak");
            if bak_path.exists() {
                on_recovery(StorageRecoveryEvent::CorruptPrimary { path, error: &e });
                let bak_data = std::fs::read_to_string(&bak_path)?;
                match serde_json::from_str(&bak_data) {
                    Ok(val) => {
                        on_recovery(StorageRecoveryEvent::RecoveredFromBackup {
                            backup_path: &bak_path,
                        });
                        let _ = std::fs::copy(&bak_path, path);
                        Ok(val)
                    }
                    Err(bak_err) => Err(anyhow::anyhow!(
                        "Corrupt JSON at {} ({}), backup also corrupt ({})",
                        path.display(),
                        e,
                        bak_err
                    )),
                }
            } else {
                Err(anyhow::anyhow!("Corrupt JSON at {}: {}", path.display(), e))
            }
        }
    }
}

/// Fast append of a single JSON value followed by a newline.
/// Intended for append-only journals where per-write fsync is not required.
pub fn append_json_line_fast<T: Serialize + ?Sized>(path: &Path, value: &T) -> Result<()> {
    if let Some(parent) = path.parent() {
        ensure_dir(parent)?;
    }

    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    serde_json::to_writer(&mut file, value)?;
    file.write_all(b"\n")?;
    file.flush()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// Local mutex — these tests mutate process env vars and would race with
    /// each other if run in parallel.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    fn save(keys: &[&'static str]) -> Vec<(&'static str, Option<std::ffi::OsString>)> {
        keys.iter().map(|k| (*k, std::env::var_os(k))).collect()
    }

    fn restore(saved: Vec<(&'static str, Option<std::ffi::OsString>)>) {
        for (k, v) in saved {
            unsafe {
                match v {
                    Some(val) => std::env::set_var(k, val),
                    None => std::env::remove_var(k),
                }
            }
        }
    }

    #[test]
    fn jcode_dir_legacy_default_when_xdg_disabled() {
        let _guard = ENV_LOCK.lock().unwrap();
        let saved = save(&["JCODE_HOME", "JCODE_USE_XDG"]);
        unsafe {
            std::env::remove_var("JCODE_HOME");
            std::env::remove_var("JCODE_USE_XDG");
        }
        let dir = jcode_dir().expect("legacy path");
        assert!(
            dir.ends_with(".jcode"),
            "expected ~/.jcode legacy path, got {}",
            dir.display()
        );
        restore(saved);
    }

    #[test]
    fn jcode_dir_uses_xdg_data_home_when_opt_in() {
        let _guard = ENV_LOCK.lock().unwrap();
        let saved = save(&["JCODE_HOME", "JCODE_USE_XDG", "XDG_DATA_HOME"]);
        unsafe {
            std::env::remove_var("JCODE_HOME");
            std::env::set_var("JCODE_USE_XDG", "1");
            std::env::set_var("XDG_DATA_HOME", "/tmp/test-xdg-data");
        }
        let dir = jcode_dir().expect("xdg path");
        assert_eq!(dir, PathBuf::from("/tmp/test-xdg-data/jcode"));
        restore(saved);
    }

    #[test]
    fn jcode_dir_xdg_falls_back_to_local_share() {
        let _guard = ENV_LOCK.lock().unwrap();
        let saved = save(&["JCODE_HOME", "JCODE_USE_XDG", "XDG_DATA_HOME"]);
        unsafe {
            std::env::remove_var("JCODE_HOME");
            std::env::set_var("JCODE_USE_XDG", "true");
            std::env::remove_var("XDG_DATA_HOME");
        }
        let dir = jcode_dir().expect("xdg fallback");
        // Should end with `.local/share/jcode` regardless of platform's
        // home dir.
        let suffix: PathBuf = [".local", "share", "jcode"].iter().collect();
        assert!(
            dir.ends_with(&suffix),
            "expected ~/.local/share/jcode, got {}",
            dir.display()
        );
        restore(saved);
    }

    #[test]
    fn jcode_dir_jcode_home_overrides_everything() {
        let _guard = ENV_LOCK.lock().unwrap();
        let saved = save(&["JCODE_HOME", "JCODE_USE_XDG", "XDG_DATA_HOME"]);
        unsafe {
            std::env::set_var("JCODE_HOME", "/tmp/forced-home");
            std::env::set_var("JCODE_USE_XDG", "1");
            std::env::set_var("XDG_DATA_HOME", "/tmp/should-be-ignored");
        }
        let dir = jcode_dir().expect("jcode_home");
        assert_eq!(dir, PathBuf::from("/tmp/forced-home"));
        restore(saved);
    }

    #[test]
    fn jcode_use_xdg_enabled_recognizes_truthy_values() {
        let _guard = ENV_LOCK.lock().unwrap();
        let saved = save(&["JCODE_USE_XDG"]);
        for truthy in ["1", "true", "TRUE", "yes", "On"] {
            unsafe {
                std::env::set_var("JCODE_USE_XDG", truthy);
            }
            assert!(jcode_use_xdg_enabled(), "{truthy:?} should be truthy");
        }
        for falsy in ["0", "false", "no", "off", "", "  "] {
            unsafe {
                std::env::set_var("JCODE_USE_XDG", falsy);
            }
            assert!(!jcode_use_xdg_enabled(), "{falsy:?} should be falsy");
        }
        restore(saved);
    }
}
