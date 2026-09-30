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
