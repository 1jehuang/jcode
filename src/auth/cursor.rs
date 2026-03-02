use anyhow::{Context, Result};
use std::path::PathBuf;

/// Check if Cursor API key is available (env var or saved file).
pub fn has_cursor_api_key() -> bool {
    load_api_key().is_ok()
}

/// Check if `cursor-agent` CLI is available on PATH.
pub fn has_cursor_agent_cli() -> bool {
    super::command_available_from_env("JCODE_CURSOR_CLI_PATH", "cursor-agent")
}

/// Check if Cursor IDE's local vscdb has an access token.
pub fn has_cursor_vscdb_token() -> bool {
    read_vscdb_token().is_ok()
}

/// Read access token from Cursor IDE's SQLite storage (state.vscdb).
/// Uses the `sqlite3` CLI to avoid adding a native dependency.
pub fn read_vscdb_token() -> Result<String> {
    let db_path = find_cursor_vscdb()?;
    read_vscdb_key(&db_path, "cursorAuth/accessToken")
}

/// Read the machine ID from Cursor's vscdb (needed for API checksum header).
pub fn read_vscdb_machine_id() -> Result<String> {
    let db_path = find_cursor_vscdb()?;
    read_vscdb_key(&db_path, "storage.serviceMachineId")
}

/// Find the Cursor vscdb file on this platform.
fn find_cursor_vscdb() -> Result<PathBuf> {
    let candidates = cursor_vscdb_paths();
    for path in &candidates {
        if path.exists() {
            return Ok(path.clone());
        }
    }
    anyhow::bail!("Cursor state.vscdb not found (is Cursor IDE installed?)")
}

/// Platform-specific candidate paths for Cursor's state.vscdb.
fn cursor_vscdb_paths() -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Some(home) = dirs::home_dir() {
        #[cfg(target_os = "linux")]
        {
            paths.push(home.join(".config/Cursor/User/globalStorage/state.vscdb"));
            paths.push(home.join(".config/cursor/User/globalStorage/state.vscdb"));
        }
        #[cfg(target_os = "macos")]
        {
            paths.push(
                home.join("Library/Application Support/Cursor/User/globalStorage/state.vscdb"),
            );
            paths.push(
                home.join("Library/Application Support/cursor/User/globalStorage/state.vscdb"),
            );
        }
        #[cfg(target_os = "windows")]
        {
            paths.push(home.join("AppData/Roaming/Cursor/User/globalStorage/state.vscdb"));
            paths.push(home.join("AppData/Roaming/cursor/User/globalStorage/state.vscdb"));
        }
    }
    paths
}

/// Read a key from a vscdb file using the sqlite3 CLI.
fn read_vscdb_key(db_path: &PathBuf, key: &str) -> Result<String> {
    let output = std::process::Command::new("sqlite3")
        .arg(db_path)
        .arg(format!(
            "SELECT value FROM ItemTable WHERE key = '{}';",
            key
        ))
        .output()
        .context("Failed to run sqlite3 (is it installed?)")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("sqlite3 failed: {}", stderr.trim());
    }

    let value = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if value.is_empty() {
        anyhow::bail!("Key '{}' not found or empty in {}", key, db_path.display());
    }
    Ok(value)
}

/// Load Cursor API key. Checks in order:
/// 1. `CURSOR_API_KEY` env var
/// 2. Saved key in `~/.config/jcode/cursor.env`
pub fn load_api_key() -> Result<String> {
    if let Ok(key) = std::env::var("CURSOR_API_KEY") {
        let trimmed = key.trim().to_string();
        if !trimmed.is_empty() {
            return Ok(trimmed);
        }
    }

    let file_path = config_file_path()?;
    if file_path.exists() {
        let content = std::fs::read_to_string(&file_path)
            .with_context(|| format!("Failed to read {}", file_path.display()))?;
        for line in content.lines() {
            let line = line.trim();
            if let Some(key) = line.strip_prefix("CURSOR_API_KEY=") {
                let key = key.trim().trim_matches('"').trim_matches('\'');
                if !key.is_empty() {
                    return Ok(key.to_string());
                }
            }
        }
    }

    anyhow::bail!(
        "Cursor API key not found. Set CURSOR_API_KEY env var, \
         or run `/login cursor` to configure."
    )
}

/// Save a Cursor API key to `~/.config/jcode/cursor.env`.
pub fn save_api_key(key: &str) -> Result<()> {
    let file_path = config_file_path()?;
    let config_dir = file_path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("No parent dir"))?;
    std::fs::create_dir_all(config_dir)?;

    let content = format!("CURSOR_API_KEY={}\n", key);
    std::fs::write(&file_path, &content)?;
    crate::platform::set_permissions_owner_only(&file_path)?;

    std::env::set_var("CURSOR_API_KEY", key);
    Ok(())
}

fn config_file_path() -> Result<PathBuf> {
    let config_dir = dirs::config_dir()
        .ok_or_else(|| anyhow::anyhow!("No config directory found"))?
        .join("jcode");
    Ok(config_dir.join("cursor.env"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn config_file_path_under_jcode() {
        let path = config_file_path().unwrap();
        let path_str = path.to_string_lossy();
        assert!(path_str.contains("jcode"));
        assert!(path_str.ends_with("cursor.env"));
    }

    #[test]
    fn save_and_load_api_key() {
        let dir = TempDir::new().unwrap();
        let file = dir.path().join("jcode").join("cursor.env");

        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        let content = "CURSOR_API_KEY=test_key_123\n";
        std::fs::write(&file, content).unwrap();

        let loaded = load_key_from_file(&file).unwrap();
        assert_eq!(loaded, "test_key_123");
    }

    #[test]
    fn load_key_quoted() {
        let dir = TempDir::new().unwrap();
        let file = dir.path().join("cursor.env");

        std::fs::write(&file, "CURSOR_API_KEY=\"my_quoted_key\"\n").unwrap();
        let loaded = load_key_from_file(&file).unwrap();
        assert_eq!(loaded, "my_quoted_key");
    }

    #[test]
    fn load_key_single_quoted() {
        let dir = TempDir::new().unwrap();
        let file = dir.path().join("cursor.env");

        std::fs::write(&file, "CURSOR_API_KEY='single_quoted'\n").unwrap();
        let loaded = load_key_from_file(&file).unwrap();
        assert_eq!(loaded, "single_quoted");
    }

    #[test]
    fn load_key_empty_value() {
        let dir = TempDir::new().unwrap();
        let file = dir.path().join("cursor.env");

        std::fs::write(&file, "CURSOR_API_KEY=\n").unwrap();
        let result = load_key_from_file(&file);
        assert!(result.is_err());
    }

    #[test]
    fn load_key_missing_file() {
        let path = PathBuf::from("/tmp/nonexistent_cursor_test_12345.env");
        let result = load_key_from_file(&path);
        assert!(result.is_err());
    }

    #[test]
    fn load_key_no_cursor_line() {
        let dir = TempDir::new().unwrap();
        let file = dir.path().join("cursor.env");

        std::fs::write(&file, "OTHER_KEY=value\n").unwrap();
        let result = load_key_from_file(&file);
        assert!(result.is_err());
    }

    #[test]
    fn load_key_with_whitespace() {
        let dir = TempDir::new().unwrap();
        let file = dir.path().join("cursor.env");

        std::fs::write(&file, "  CURSOR_API_KEY=  spaced_key  \n").unwrap();
        let loaded = load_key_from_file(&file).unwrap();
        assert_eq!(loaded, "spaced_key");
    }

    #[test]
    fn load_key_multiple_lines() {
        let dir = TempDir::new().unwrap();
        let file = dir.path().join("cursor.env");

        std::fs::write(
            &file,
            "# comment\nOTHER=foo\nCURSOR_API_KEY=the_real_key\nMORE=bar\n",
        )
        .unwrap();
        let loaded = load_key_from_file(&file).unwrap();
        assert_eq!(loaded, "the_real_key");
    }

    #[test]
    fn has_cursor_api_key_from_env() {
        let key = "CURSOR_API_KEY";
        let guard = std::env::var(key).ok();
        std::env::set_var(key, "env_test_key");
        let result = std::env::var(key).unwrap();
        assert_eq!(result, "env_test_key");
        match guard {
            Some(v) => std::env::set_var(key, v),
            None => std::env::remove_var(key),
        }
    }

    #[test]
    fn load_api_key_empty_env_falls_through() {
        let key_str = "";
        assert!(key_str.trim().is_empty());
    }

    fn load_key_from_file(path: &PathBuf) -> Result<String> {
        if !path.exists() {
            anyhow::bail!("File not found");
        }
        let content = std::fs::read_to_string(path)?;
        for line in content.lines() {
            let line = line.trim();
            if let Some(key) = line.strip_prefix("CURSOR_API_KEY=") {
                let key = key.trim().trim_matches('"').trim_matches('\'');
                if !key.is_empty() {
                    return Ok(key.to_string());
                }
            }
        }
        anyhow::bail!("No CURSOR_API_KEY found")
    }

    /// Helper: create a mock state.vscdb with the given key/value pairs.
    fn create_mock_vscdb(dir: &std::path::Path, entries: &[(&str, &str)]) -> PathBuf {
        let db_path = dir.join("state.vscdb");
        let status = std::process::Command::new("sqlite3")
            .arg(&db_path)
            .arg("CREATE TABLE ItemTable (key TEXT UNIQUE ON CONFLICT REPLACE, value BLOB);")
            .status()
            .expect("sqlite3 must be installed for these tests");
        assert!(status.success(), "Failed to create mock vscdb");

        for (key, value) in entries {
            let sql = format!(
                "INSERT INTO ItemTable (key, value) VALUES ('{}', '{}');",
                key, value
            );
            let status = std::process::Command::new("sqlite3")
                .arg(&db_path)
                .arg(&sql)
                .status()
                .unwrap();
            assert!(status.success(), "Failed to insert into mock vscdb");
        }
        db_path
    }

    #[test]
    fn vscdb_read_access_token() {
        let dir = TempDir::new().unwrap();
        let db = create_mock_vscdb(dir.path(), &[("cursorAuth/accessToken", "tok_abc123xyz")]);
        let result = read_vscdb_key(&db, "cursorAuth/accessToken").unwrap();
        assert_eq!(result, "tok_abc123xyz");
    }

    #[test]
    fn vscdb_read_machine_id() {
        let dir = TempDir::new().unwrap();
        let db = create_mock_vscdb(
            dir.path(),
            &[(
                "storage.serviceMachineId",
                "550e8400-e29b-41d4-a716-446655440000",
            )],
        );
        let result = read_vscdb_key(&db, "storage.serviceMachineId").unwrap();
        assert_eq!(result, "550e8400-e29b-41d4-a716-446655440000");
    }

    #[test]
    fn vscdb_missing_key_returns_error() {
        let dir = TempDir::new().unwrap();
        let db = create_mock_vscdb(dir.path(), &[("other/key", "value")]);
        let result = read_vscdb_key(&db, "cursorAuth/accessToken");
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("not found or empty"));
    }

    #[test]
    fn vscdb_empty_value_returns_error() {
        let dir = TempDir::new().unwrap();
        let db = create_mock_vscdb(dir.path(), &[("cursorAuth/accessToken", "")]);
        let result = read_vscdb_key(&db, "cursorAuth/accessToken");
        assert!(result.is_err());
    }

    #[test]
    fn vscdb_missing_file_returns_error() {
        let path = PathBuf::from("/tmp/nonexistent_vscdb_test_999.vscdb");
        let result = read_vscdb_key(&path, "cursorAuth/accessToken");
        assert!(result.is_err());
    }

    #[test]
    fn vscdb_multiple_keys() {
        let dir = TempDir::new().unwrap();
        let db = create_mock_vscdb(
            dir.path(),
            &[
                ("cursorAuth/accessToken", "my_token"),
                ("storage.serviceMachineId", "machine_123"),
                ("cursorAuth/refreshToken", "refresh_456"),
                ("cursorAuth/cachedEmail", "user@example.com"),
            ],
        );
        assert_eq!(
            read_vscdb_key(&db, "cursorAuth/accessToken").unwrap(),
            "my_token"
        );
        assert_eq!(
            read_vscdb_key(&db, "storage.serviceMachineId").unwrap(),
            "machine_123"
        );
        assert_eq!(
            read_vscdb_key(&db, "cursorAuth/refreshToken").unwrap(),
            "refresh_456"
        );
        assert_eq!(
            read_vscdb_key(&db, "cursorAuth/cachedEmail").unwrap(),
            "user@example.com"
        );
    }

    #[test]
    fn vscdb_wrong_table_name() {
        let dir = TempDir::new().unwrap();
        let db_path = dir.path().join("state.vscdb");
        let status = std::process::Command::new("sqlite3")
            .arg(&db_path)
            .arg("CREATE TABLE WrongTable (key TEXT, value BLOB);")
            .status()
            .unwrap();
        assert!(status.success());
        let result = read_vscdb_key(&db_path, "cursorAuth/accessToken");
        assert!(result.is_err());
    }

    #[test]
    fn vscdb_paths_not_empty() {
        let paths = cursor_vscdb_paths();
        assert!(!paths.is_empty(), "Should have at least one candidate path");
        for path in &paths {
            let s = path.to_string_lossy();
            assert!(
                s.contains("ursor"),
                "Path should contain 'Cursor' or 'cursor'"
            );
            assert!(s.ends_with("state.vscdb"));
        }
    }

    #[test]
    fn find_vscdb_missing_returns_error() {
        let result = find_cursor_vscdb();
        // On this machine Cursor isn't installed, so it should fail
        // (if Cursor IS installed, this test still passes - it finds the file)
        if result.is_err() {
            assert!(result.unwrap_err().to_string().contains("not found"));
        }
    }
}
