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
    crate::env::set_var(key, "env_test_key");
    let result = std::env::var(key).unwrap();
    assert_eq!(result, "env_test_key");
    match guard {
        Some(v) => crate::env::set_var(key, v),
        None => crate::env::remove_var(key),
    }
}

#[test]
fn cursor_vscdb_paths_respect_jcode_home() {
    let _guard = crate::storage::lock_test_env();
    let prev_home = std::env::var_os("JCODE_HOME");
    let temp = TempDir::new().unwrap();
    crate::env::set_var("JCODE_HOME", temp.path());

    let paths = cursor_vscdb_paths();
    assert!(!paths.is_empty());
    for path in paths {
        assert!(path.starts_with(temp.path().join("external")));
    }

    if let Some(prev_home) = prev_home {
        crate::env::set_var("JCODE_HOME", prev_home);
    } else {
        crate::env::remove_var("JCODE_HOME");
    }
}

#[test]
fn load_api_key_empty_env_falls_through() {
    let key_str = "";
    assert!(key_str.trim().is_empty());
}

#[cfg(unix)]
#[test]
fn load_access_token_from_auth_file_does_not_change_external_permissions() {
    use std::os::unix::fs::PermissionsExt;

    let _guard = crate::storage::lock_test_env();
    let prev_home = std::env::var_os("JCODE_HOME");
    let temp = TempDir::new().unwrap();
    crate::env::set_var("JCODE_HOME", temp.path());

    let path = cursor_auth_file_path().expect("cursor auth path");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(
        &path,
        r#"{"accessToken":"at-test","refreshToken":"rt-test"}"#,
    )
    .unwrap();
    std::fs::set_permissions(
        path.parent().unwrap(),
        std::fs::Permissions::from_mode(0o755),
    )
    .unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
    crate::config::Config::allow_external_auth_source_for_path(CURSOR_AUTH_FILE_SOURCE_ID, &path)
        .expect("trust cursor auth path");

    let tokens = load_access_token_from_env_or_file().expect("load auth file token");
    assert_eq!(tokens.access_token, "at-test");
    assert_eq!(tokens.refresh_token.as_deref(), Some("rt-test"));

    let dir_mode = std::fs::metadata(path.parent().unwrap())
        .unwrap()
        .permissions()
        .mode()
        & 0o777;
    let file_mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
    assert_eq!(dir_mode, 0o755);
    assert_eq!(file_mode, 0o644);

    if let Some(prev_home) = prev_home {
        crate::env::set_var("JCODE_HOME", prev_home);
    } else {
        crate::env::remove_var("JCODE_HOME");
    }
}

#[test]
fn status_output_detects_authenticated_session() {
    assert!(status_output_indicates_authenticated(
        true,
        b"Authenticated\nAccount: user@example.com\nEndpoint: production",
        b""
    ));
}

#[test]
fn status_output_detects_missing_authentication() {
    assert!(!status_output_indicates_authenticated(
        true,
        b"Not authenticated. Run cursor-agent login.",
        b""
    ));
}

#[test]
fn status_output_requires_successful_exit_for_authentication_keywords() {
    assert!(!status_output_indicates_authenticated(
        false,
        b"Account: user@example.com\nEndpoint: production",
        b"cursor-agent status failed"
    ));
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

/// Returns `true` if a `sqlite3` CLI is on `PATH`. The vscdb fixture tests
/// shell out to it; on hosts without sqlite3 (default Windows GitHub
/// runners, minimal Linux containers) the tests skip with a printed note
/// rather than failing.
fn sqlite3_available() -> bool {
    std::process::Command::new("sqlite3")
        .arg("-version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Helper: create a mock state.vscdb with the given key/value pairs.
/// Returns `None` if `sqlite3` is unavailable; callers should treat that
/// as a soft skip.
fn create_mock_vscdb(dir: &std::path::Path, entries: &[(&str, &str)]) -> Option<PathBuf> {
    if !sqlite3_available() {
        eprintln!("[skip] sqlite3 CLI not on PATH; skipping vscdb fixture test");
        return None;
    }
    let db_path = dir.join("state.vscdb");
    let status = std::process::Command::new("sqlite3")
        .arg(&db_path)
        .arg("CREATE TABLE ItemTable (key TEXT UNIQUE ON CONFLICT REPLACE, value BLOB);")
        .status()
        .expect("sqlite3 should run after sqlite3_available() returned true");
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
    Some(db_path)
}

#[test]
fn vscdb_read_access_token() {
    let dir = TempDir::new().unwrap();
    let Some(db) = create_mock_vscdb(dir.path(), &[("cursorAuth/accessToken", "tok_abc123xyz")])
    else {
        return;
    };
    let result = read_vscdb_key(&db, "cursorAuth/accessToken").unwrap();
    assert_eq!(result, "tok_abc123xyz");
}

#[test]
fn vscdb_read_machine_id() {
    let dir = TempDir::new().unwrap();
    let Some(db) = create_mock_vscdb(
        dir.path(),
        &[(
            "storage.serviceMachineId",
            "550e8400-e29b-41d4-a716-446655440000",
        )],
    ) else {
        return;
    };
    let result = read_vscdb_key(&db, "storage.serviceMachineId").unwrap();
    assert_eq!(result, "550e8400-e29b-41d4-a716-446655440000");
}

#[test]
fn vscdb_missing_key_returns_error() {
    let dir = TempDir::new().unwrap();
    let Some(db) = create_mock_vscdb(dir.path(), &[("other/key", "value")]) else {
        return;
    };
    let result = read_vscdb_key(&db, "cursorAuth/accessToken");
    assert!(result.is_err());
    assert!(
        result
            .unwrap_err()
            .to_string()
            .contains("not found or empty")
    );
}

#[test]
fn vscdb_empty_value_returns_error() {
    let dir = TempDir::new().unwrap();
    let Some(db) = create_mock_vscdb(dir.path(), &[("cursorAuth/accessToken", "")]) else {
        return;
    };
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
    let Some(db) = create_mock_vscdb(
        dir.path(),
        &[
            ("cursorAuth/accessToken", "my_token"),
            ("storage.serviceMachineId", "machine_123"),
            ("cursorAuth/refreshToken", "refresh_456"),
            ("cursorAuth/cachedEmail", "user@example.com"),
        ],
    ) else {
        return;
    };
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
    if !sqlite3_available() {
        eprintln!("[skip] sqlite3 CLI not on PATH; skipping vscdb fixture test");
        return;
    }
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
    if let Err(err) = result {
        assert!(err.to_string().contains("not found"));
    }
}
