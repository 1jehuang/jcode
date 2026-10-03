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
