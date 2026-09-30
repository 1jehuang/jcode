use super::*;

fn fixture_path(dir: &tempfile::TempDir) -> PathBuf {
    dir.path().join("opencode.db")
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
