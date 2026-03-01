use anyhow::Result;
use serde::de::DeserializeOwned;
use serde::Serialize;
use std::io::Write;
use std::path::{Path, PathBuf};

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
    std::env::temp_dir()
}

pub fn jcode_dir() -> Result<PathBuf> {
    if let Ok(path) = std::env::var("JCODE_HOME") {
        return Ok(PathBuf::from(path));
    }

    let home = dirs::home_dir().ok_or_else(|| anyhow::anyhow!("No home directory"))?;
    Ok(home.join(".jcode"))
}

pub fn ensure_dir(path: &Path) -> Result<()> {
    if !path.exists() {
        std::fs::create_dir_all(path)?;
    }
    Ok(())
}

pub fn write_json<T: Serialize + ?Sized>(path: &Path, value: &T) -> Result<()> {
    if let Some(parent) = path.parent() {
        ensure_dir(parent)?;
    }

    let tmp_path = path.with_extension("tmp");
    let file = std::fs::File::create(&tmp_path)?;
    let mut writer = std::io::BufWriter::new(file);
    serde_json::to_writer(&mut writer, value)?;
    let file = writer.into_inner().map_err(|e| anyhow::anyhow!("flush failed: {}", e))?;
    file.sync_all()?;

    if path.exists() {
        let bak_path = path.with_extension("bak");
        let _ = std::fs::rename(path, &bak_path);
    }

    std::fs::rename(&tmp_path, path)?;

    #[cfg(unix)]
    if let Some(parent) = path.parent() {
        if let Ok(dir) = std::fs::File::open(parent) {
            let _ = dir.sync_all();
        }
    }

    Ok(())
}

pub fn read_json<T: DeserializeOwned>(path: &Path) -> Result<T> {
    let data = std::fs::read_to_string(path)?;
    match serde_json::from_str(&data) {
        Ok(val) => Ok(val),
        Err(e) => {
            let bak_path = path.with_extension("bak");
            if bak_path.exists() {
                crate::logging::warn(&format!(
                    "Corrupt JSON at {}, trying backup: {}",
                    path.display(),
                    e
                ));
                let bak_data = std::fs::read_to_string(&bak_path)?;
                match serde_json::from_str(&bak_data) {
                    Ok(val) => {
                        crate::logging::info(&format!(
                            "Recovered from backup: {}",
                            bak_path.display()
                        ));
                        let _ = std::fs::copy(&bak_path, path);
                        Ok(val)
                    }
                    Err(bak_err) => {
                        Err(anyhow::anyhow!(
                            "Corrupt JSON at {} ({}), backup also corrupt ({})",
                            path.display(),
                            e,
                            bak_err
                        ))
                    }
                }
            } else {
                Err(anyhow::anyhow!(
                    "Corrupt JSON at {}: {}",
                    path.display(),
                    e
                ))
            }
        }
    }
}
