use super::*;

fn fixture_path(dir: &tempfile::TempDir) -> PathBuf {
    dir.path().join("opencode.db")
}

#[test]
fn opencode_db_prefilter_preserves_unicode_case_matches() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = fixture_path(&dir);
    let f = fixture::Fixture::create(&path);
    for (id, text) in [
        ("ses_unicode", "Kubernetes deployment"),
        ("ses_ascii", "unrelated chatter"),
    ] {
        f.session(id, None, "Chat", "/tmp/c", "p", "m", 3_000_000, None)
            .message(id, id, "user", 100)
            .text(id, id, id, text);
    }
    drop(f);
    let ids = vec!["ses_unicode".to_string(), "ses_ascii".to_string()];
    let terms = vec!["kubernetes".to_string(), "deployment".to_string()];
    for include_tools in [false, true] {
        let matched = sessions_matching_terms(&path, &ids, &terms, 2, include_tools).unwrap();
        assert!(
            matched.contains("ses_unicode"),
            "Unicode case matches must survive the prefilter"
        );
        assert!(
            !matched.contains("ses_ascii"),
            "ASCII-only nonmatches should still be filtered"
        );
    }
}

#[test]
fn opencode_db_prefilter_preserves_escaped_unicode_tool_output() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = fixture_path(&dir);
    let f = fixture::Fixture::create(&path);
    f.session(
        "ses_tool", None, "Chat", "/tmp/c", "p", "m", 3_000_000, None,
    )
    .message("ses_tool", "msg", "assistant", 100)
    .part(
        "ses_tool",
        "msg",
        "prt",
        serde_json::json!({"type": "tool", "state": {"output": "Kubernetes"}}),
    );
    f.0.execute(
        "UPDATE part SET data = replace(data, ?1, ?2)",
        rusqlite::params!["K", r"\u212a"],
    )
    .unwrap();
    drop(f);
    let ids = vec!["ses_tool".to_string()];
    let terms = vec!["kubernetes".to_string()];
    assert!(
        sessions_matching_terms(&path, &ids, &terms, 1, true)
            .unwrap()
            .contains("ses_tool")
    );
    assert!(
        sessions_matching_terms(&path, &ids, &terms, 1, false)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn opencode_db_lists_top_level_unarchived_by_recency() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = fixture_path(&dir);
    drop(fixture::standard(&path));
    let sessions = list_sessions(&path, 50).unwrap();
    let ids: Vec<_> = sessions.iter().map(|s| s.id.as_str()).collect();
    assert_eq!(ids, vec!["ses_main", "ses_old"]);
    let main = &sessions[0];
    assert_eq!(main.provider_id.as_deref(), Some("anthropic"));
    assert_eq!(main.model_id.as_deref(), Some("claude-x"));
    assert_eq!(main.directory.as_deref(), Some("/tmp/oc-main"));
    assert_eq!(main.title.as_deref(), Some("Main task"));
    assert_eq!(main.message_count, 2);
    assert_eq!(main.updated_at.timestamp_millis(), 3_000_000);
    assert_eq!(list_sessions(&path, 1).unwrap().len(), 1);
}

#[test]
fn opencode_db_messages_keep_only_real_text_parts_in_order() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = fixture_path(&dir);
    drop(fixture::standard(&path));
    let messages = load_messages(&path, "ses_main", None).unwrap();
    let got: Vec<_> = messages
        .iter()
        .map(|m| (m.role.as_str(), m.text.as_str()))
        .collect();
    assert_eq!(
        got,
        vec![
            ("user", "hello opencode"),
            ("assistant", "hi from assistant")
        ]
    );
    let preview = load_messages(&path, "ses_main", Some(1)).unwrap();
    assert_eq!(preview.len(), 1);
    assert!(load_session(&path, "ses_main").unwrap().is_some());
    assert!(load_session(&path, "nope").unwrap().is_none());
}

#[test]
fn opencode_db_opens_wal_db_without_shm() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = fixture_path(&dir);
    {
        let f = fixture::standard(&path);
        f.0.pragma_update(None, "journal_mode", "WAL").unwrap();
    }
    let shm = dir.path().join("opencode.db-shm");
    let _ = std::fs::remove_file(&shm);
    let _ = std::fs::remove_file(dir.path().join("opencode.db-wal"));
    assert!(!shm.exists());
    let sessions = list_sessions(&path, 50).unwrap();
    assert_eq!(sessions.len(), 2);
    assert!(!shm.exists(), "reader must not create -shm");
}

// Copy only a checkpointed main file and its subsequently committed WAL.
// The detached owner directory has no live connection and deliberately no SHM.
fn committed_wal_without_shm(dir: &tempfile::TempDir) -> PathBuf {
    let source = dir.path().join("writer.db");
    let f = fixture::Fixture::create(&source);
    f.0.execute_batch("PRAGMA journal_mode=WAL; PRAGMA wal_autocheckpoint=0;")
        .unwrap();
    f.0.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")
        .unwrap();
    f.0.execute_batch("BEGIN;").unwrap();
    f.session("ses_wal", None, "WAL task", "/tmp/c", "p", "m", 3000, None)
        .message("ses_wal", "msg_wal", "user", 100)
        .text("ses_wal", "msg_wal", "prt_wal", "committed WAL text");
    f.0.execute_batch("COMMIT;").unwrap();
    let owner = dir.path().join("owner %?# 日本語");
    std::fs::create_dir(&owner).unwrap();
    let path = owner.join(DB_FILE_NAME);
    std::fs::copy(&source, &path).unwrap();
    std::fs::copy(
        dir.path().join("writer.db-wal"),
        owner.join("opencode.db-wal"),
    )
    .unwrap();
    assert!(
        std::fs::metadata(owner.join("opencode.db-wal"))
            .unwrap()
            .len()
            > 0
    );
    assert!(!owner.join("opencode.db-shm").exists());
    // Baseline witness: the old immutable path succeeds but silently loses data.
    let stale = Connection::open_with_flags(
        format!("file:{}?mode=ro&immutable=1", uri_escape(&path)),
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI,
    )
    .unwrap();
    assert_eq!(
        stale
            .query_row("SELECT COUNT(*) FROM session", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        0
    );
    assert_eq!(
        stale
            .query_row("SELECT COUNT(*) FROM message", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        0
    );
    path
}

fn owner_files(path: &Path) -> Vec<(std::ffi::OsString, Vec<u8>)> {
    let mut files: Vec<_> = std::fs::read_dir(path.parent().unwrap())
        .unwrap()
        .map(|entry| {
            let entry = entry.unwrap();
            (entry.file_name(), std::fs::read(entry.path()).unwrap())
        })
        .collect();
    files.sort_by(|a, b| a.0.cmp(&b.0));
    files
}

#[test]
fn opencode_db_public_readers_reject_committed_wal_without_shm() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = committed_wal_without_shm(&dir);
    let errors = [
        list_sessions(&path, 50).unwrap_err(),
        load_session(&path, "ses_wal").unwrap_err(),
        load_messages(&path, "ses_wal", None).unwrap_err(),
        load_search_messages(&path, "ses_wal", true).unwrap_err(),
        sessions_matching_terms(&path, &["ses_wal".into()], &["committed".into()], 1, false)
            .unwrap_err(),
    ];
    for error in errors {
        assert!(
            error
                .to_string()
                .contains("nonempty WAL prevents immutable reading"),
            "{error}"
        );
    }
}

#[test]
fn opencode_db_rejected_wal_read_leaves_owner_files_unchanged() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = committed_wal_without_shm(&dir);
    let before = owner_files(&path);
    let _ = list_sessions(&path, 50);
    let _ = load_session(&path, "ses_wal");
    let _ = load_messages(&path, "ses_wal", None);
    assert_eq!(owner_files(&path), before);
}

#[test]
fn opencode_db_checkpointed_wal_accepts_empty_sidecar_with_escaped_unicode_path() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("opencode %?# 日本語.db");
    let f = fixture::standard(&path);
    f.0.pragma_update(None, "journal_mode", "WAL").unwrap();
    drop(f);
    let mut wal = path.as_os_str().to_owned();
    wal.push("-wal");
    std::fs::write(Path::new(&wal), []).unwrap();
    let before = owner_files(&path);
    assert_eq!(list_sessions(&path, 50).unwrap().len(), 2);
    assert_eq!(load_messages(&path, "ses_main", None).unwrap().len(), 2);
    assert_eq!(owner_files(&path), before);
}

#[test]
fn opencode_db_public_importers_propagate_committed_wal_error_without_saving() {
    let _guard = crate::storage::lock_test_env();
    let dir = tempfile::TempDir::new().unwrap();
    struct HomeGuard(Option<std::ffi::OsString>);
    impl Drop for HomeGuard {
        fn drop(&mut self) {
            match self.0.take() {
                Some(value) => crate::env::set_var("JCODE_HOME", value),
                None => crate::env::remove_var("JCODE_HOME"),
            }
        }
    }
    let _home = HomeGuard(std::env::var_os("JCODE_HOME"));
    crate::env::set_var("JCODE_HOME", dir.path());
    let snapshot = committed_wal_without_shm(&dir);
    let path = db_path().unwrap();
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::copy(&snapshot, &path).unwrap();
    std::fs::copy(
        snapshot.parent().unwrap().join("opencode.db-wal"),
        path.parent().unwrap().join("opencode.db-wal"),
    )
    .unwrap();
    let before = owner_files(&path);

    for error in [
        crate::import::import_opencode_session("ses_wal").unwrap_err(),
        crate::import::import_external_resume_id("ses_wal").unwrap_err(),
    ] {
        assert!(
            error
                .to_string()
                .contains("nonempty WAL prevents immutable reading"),
            "{error}"
        );
    }
    assert_eq!(owner_files(&path), before);
    assert!(crate::session::Session::load("imported_opencode_ses_wal").is_err());
}

/// Live check against a copy of a real OpenCode database. Run with
/// `OPENCODE_DB_LIVE=/path/to/copy/opencode.db cargo test -p jcode-base --lib -- --ignored opencode_db_live`.
#[test]
#[ignore]
fn opencode_db_live_snapshot() {
    let Some(path) = std::env::var_os("OPENCODE_DB_LIVE").map(PathBuf::from) else {
        return;
    };
    let start = std::time::Instant::now();
    let sessions = list_sessions(&path, 10_000).unwrap();
    let list_ms = start.elapsed().as_secs_f64() * 1000.0;
    let start = std::time::Instant::now();
    let _ = list_sessions(&path, 100).unwrap();
    let list100_ms = start.elapsed().as_secs_f64() * 1000.0;
    let pick = sessions.iter().max_by_key(|s| s.message_count).unwrap();
    let start = std::time::Instant::now();
    let session = crate::import::import_opencode_db_session(&path, &pick.id).unwrap();
    let import_ms = start.elapsed().as_secs_f64() * 1000.0;
    let messages = load_messages(&path, &pick.id, None).unwrap();
    let with_text = messages
        .iter()
        .filter(|m| !m.text.trim().is_empty())
        .count();
    eprintln!(
        "LIVE listed={} list_ms={list_ms:.1} list100_ms={list100_ms:.1} session={} db_message_count={} loaded_user_assistant={} imported_messages={} with_text={} import_ms={import_ms:.1}",
        sessions.len(),
        pick.id,
        pick.message_count,
        messages.len(),
        session.messages.len(),
        with_text
    );
}
