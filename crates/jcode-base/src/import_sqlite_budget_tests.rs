//! Disposable SQLite fixtures exercise the real public importer, not a copied
//! normalizer. No test opens an installed OpenCode database.
use super::*;
use crate::opencode_db::fixture::Fixture;

struct HomeGuard(Option<std::ffi::OsString>);
impl HomeGuard {
    fn set(path: &Path) -> Self {
        let old = std::env::var_os("JCODE_HOME");
        crate::env::set_var("JCODE_HOME", path);
        Self(old)
    }
}
impl Drop for HomeGuard {
    fn drop(&mut self) {
        match self.0.take() {
            Some(old) => crate::env::set_var("JCODE_HOME", old),
            None => crate::env::remove_var("JCODE_HOME"),
        }
    }
}

fn texts(session: &Session) -> Vec<String> {
    session
        .messages
        .iter()
        .map(|m| match &m.content[0] {
            ContentBlock::Text { text, .. } => text.clone(),
            _ => panic!("import must contain text only"),
        })
        .collect()
}

fn fixture(path: &Path) -> Fixture {
    let f = Fixture::create(path);
    f.session(
        "budget",
        None,
        "Budget task",
        "/fixture/project",
        "provider",
        "model",
        9000,
        None,
    );
    f
}

fn add(f: &Fixture, index: usize, text: &str) {
    let id = format!("m{index:05}");
    f.message(
        "budget",
        &id,
        if index.is_multiple_of(2) {
            "user"
        } else {
            "assistant"
        },
        index as i64,
    )
    .text("budget", &id, &format!("p{index:05}"), text);
}

fn notice(omitted: usize) -> String {
    format!(
        "[Imported session: {omitted} older messages were omitted for a fast, provider-safe resume. Use session_search to inspect earlier external history.]"
    )
}

#[test]
fn sqlite_budget_count_retains_newest_usable_tail() {
    let _lock = crate::storage::lock_test_env();
    let temp = tempfile::TempDir::new().unwrap();
    let _home = HomeGuard::set(temp.path());
    let path = temp.path().join("fixture/opencode.db");
    let f = fixture(&path);
    for i in 0..1000 {
        add(&f, i, &format!("turn {i}"));
    }
    // Latest unusable rows must not consume the 160 usable-message budget.
    f.message("budget", "tool_only", "assistant", 2000).part(
        "budget",
        "tool_only",
        "tool",
        serde_json::json!({"type":"tool"}),
    );
    add(&f, 2001, "\u{2003}\n\t");
    f.message("budget", "synthetic", "assistant", 2002).part(
        "budget",
        "synthetic",
        "synthetic_part",
        serde_json::json!({"type":"text", "text":"ignored", "synthetic":true}),
    );
    drop(f);
    let imported = import_opencode_db_session(&path, "budget").unwrap();
    let mut expected = vec![notice(840)];
    expected.extend((840..1000).map(|i| format!("turn {i}")));
    assert_eq!(texts(&imported), expected);
}

#[test]
fn sqlite_budget_exact_message_boundary_has_no_notice() {
    let _lock = crate::storage::lock_test_env();
    let temp = tempfile::TempDir::new().unwrap();
    let _home = HomeGuard::set(temp.path());
    let path = temp.path().join("fixture/opencode.db");
    let f = fixture(&path);
    for i in 0..IMPORT_HISTORY_MAX_MESSAGES {
        add(&f, i, "x");
    }
    drop(f);
    assert_eq!(
        texts(&import_opencode_db_session(&path, "budget").unwrap()),
        vec!["x"; IMPORT_HISTORY_MAX_MESSAGES]
    );
}

#[test]
fn sqlite_budget_exact_utf8_byte_boundary_retains_eight_blocks() {
    let _lock = crate::storage::lock_test_env();
    let temp = tempfile::TempDir::new().unwrap();
    let _home = HomeGuard::set(temp.path());
    let path = temp.path().join("fixture/opencode.db");
    let f = fixture(&path);
    let block = "🦀".repeat(IMPORT_BLOCK_MAX_TEXT_BYTES / 4);
    for i in 0..8 {
        add(&f, i, &block);
    }
    drop(f);
    assert_eq!(
        texts(&import_opencode_db_session(&path, "budget").unwrap()),
        vec![block; 8]
    );
}

#[test]
fn sqlite_budget_one_byte_over_history_keeps_contiguous_tail() {
    let _lock = crate::storage::lock_test_env();
    let temp = tempfile::TempDir::new().unwrap();
    let _home = HomeGuard::set(temp.path());
    let path = temp.path().join("fixture/opencode.db");
    let f = fixture(&path);
    add(&f, 0, "x");
    let block = "界".repeat(10922) + "ab";
    for i in 1..9 {
        add(&f, i, &block);
    }
    drop(f);
    let mut expected = vec![notice(1)];
    expected.extend(vec![block; 8]);
    assert_eq!(
        texts(&import_opencode_db_session(&path, "budget").unwrap()),
        expected
    );
}

#[test]
fn sqlite_budget_huge_unicode_parts_preserve_exact_truncation() {
    let _lock = crate::storage::lock_test_env();
    let temp = tempfile::TempDir::new().unwrap();
    let _home = HomeGuard::set(temp.path());
    let path = temp.path().join("fixture/opencode.db");
    let f = fixture(&path);
    let huge = "\u{2003}".repeat(12000) + &"🦀界".repeat(300000) + "  ";
    f.message("budget", "m", "user", 1)
        .text("budget", "m", "p0", &huge)
        .text("budget", "m", "p1", "\u{2003}\t\n")
        .text("budget", "m", "p2", "tail");
    drop(f);
    let original = format!("{}\ntail", huge).trim().to_string();
    assert_eq!(
        texts(&import_opencode_db_session(&path, "budget").unwrap()),
        vec![truncate_import_text(original, IMPORT_BLOCK_MAX_TEXT_BYTES)]
    );
}

#[test]
fn sqlite_budget_block_one_byte_over_preserves_marker() {
    let _lock = crate::storage::lock_test_env();
    let temp = tempfile::TempDir::new().unwrap();
    let _home = HomeGuard::set(temp.path());
    let path = temp.path().join("fixture/opencode.db");
    let f = fixture(&path);
    let text = "x".repeat(IMPORT_BLOCK_MAX_TEXT_BYTES + 1);
    add(&f, 0, &text);
    drop(f);
    assert_eq!(
        texts(&import_opencode_db_session(&path, "budget").unwrap()),
        vec![truncate_import_text(text, IMPORT_BLOCK_MAX_TEXT_BYTES)]
    );
}

#[test]
fn sqlite_budget_metadata_fallback_includes_omitted_tool_only_turn() {
    let _lock = crate::storage::lock_test_env();
    let temp = tempfile::TempDir::new().unwrap();
    let _home = HomeGuard::set(temp.path());
    let path = temp.path().join("fixture/opencode.db");
    let f = fixture(&path);
    f.0.execute("UPDATE session SET model=NULL WHERE id='budget'", [])
        .unwrap();
    f.message("budget", "old_tool", "assistant", -1);
    f.0.execute("UPDATE message SET data=?1 WHERE id='old_tool'", [serde_json::json!({"role":"assistant", "providerID":"old-provider", "modelID":"old-model"}).to_string()]).unwrap();
    for i in 0..200 {
        add(&f, i, "turn");
    }
    drop(f);
    let before_save = Utc::now();
    let imported = import_opencode_db_session(&path, "budget").unwrap();
    let after_save = Utc::now();
    assert_eq!(
        (
            imported.provider_key.as_deref(),
            imported.model.as_deref(),
            imported.title.as_deref(),
            imported.working_dir.as_deref(),
            imported.created_at.timestamp_millis(),
            imported.last_active_at.map(|time| time.timestamp_millis()),
            imported.provider_session_id.as_deref()
        ),
        (
            Some("old-provider"),
            Some("old-model"),
            Some("Budget task"),
            Some("/fixture/project"),
            8000,
            Some(9000),
            Some("budget")
        )
    );
    // Session::save_inner deliberately refreshes updated_at at write time.
    // The external update timestamp remains in last_active_at instead.
    let persisted = Session::load(&imported.id).unwrap();
    assert!(
        imported.updated_at >= before_save
            && imported.updated_at <= after_save
            && persisted.updated_at == imported.updated_at
    );
    let mut expected = vec![notice(40)];
    expected.extend(vec!["turn".to_string(); IMPORT_HISTORY_MAX_MESSAGES]);
    assert_eq!(texts(&imported), expected);
}

#[test]
fn sqlite_budget_public_import_rejects_nonempty_wal_without_shm() {
    let _lock = crate::storage::lock_test_env();
    let temp = tempfile::TempDir::new().unwrap();
    let _home = HomeGuard::set(temp.path());
    let writer_path = temp.path().join("writer/opencode.db");
    let f = fixture(&writer_path);
    f.0.execute_batch(
        "PRAGMA journal_mode=WAL; PRAGMA wal_autocheckpoint=0; PRAGMA wal_checkpoint(TRUNCATE);",
    )
    .unwrap();
    add(&f, 0, "committed only in WAL");
    let owner = temp.path().join("owner");
    std::fs::create_dir(&owner).unwrap();
    let path = owner.join("opencode.db");
    std::fs::copy(&writer_path, &path).unwrap();
    let wal = owner.join("opencode.db-wal");
    std::fs::copy(writer_path.with_file_name("opencode.db-wal"), &wal).unwrap();
    let before = (std::fs::read(&path).unwrap(), std::fs::read(&wal).unwrap());
    let error = import_opencode_db_session(&path, "budget").unwrap_err();
    assert_eq!(
        (
            error.to_string().contains("nonempty WAL"),
            std::fs::read(&path).unwrap(),
            std::fs::read(&wal).unwrap(),
            owner.join("opencode.db-shm").exists(),
            crate::session::session_exists(&imported_opencode_session_id("budget"))
        ),
        (true, before.0, before.1, false, false)
    );
}

#[test]
fn sqlite_budget_matches_existing_normalizer_for_multipart_unicode_tail() {
    let _lock = crate::storage::lock_test_env();
    let temp = tempfile::TempDir::new().unwrap();
    let _home = HomeGuard::set(temp.path());
    let path = temp.path().join("fixture/opencode.db");
    let f = fixture(&path);
    for i in 0..170 {
        let id = format!("m{i:05}");
        f.message("budget", &id, "user", i);
        let first = match i % 4 {
            0 => " \u{2003}".repeat(9000) + "start ",
            1 => "界".repeat(10923),
            2 => "🦀".repeat(8193),
            _ => " small\0text ".to_string(),
        };
        f.text("budget", &id, &format!("{id}_0"), &first)
            .text("budget", &id, &format!("{id}_1"), "\u{2003}\r\n\t")
            .text("budget", &id, &format!("{id}_2"), "\n end\u{2003}");
    }
    drop(f);
    // The pre-change public reader and normalizer are the reference only in
    // this disposable fixture. Production import must not take this path.
    let mut reference = Session::create_with_id("sqlite_budget_reference".into(), None, None);
    for msg in crate::opencode_db::load_messages(&path, "budget", None).unwrap() {
        append_text_message(&mut reference, Role::User, msg.text, msg.created_at);
    }
    normalize_imported_history(&mut reference, true);
    assert_eq!(
        texts(&import_opencode_db_session(&path, "budget").unwrap()),
        texts(&reference)
    );
}

#[test]
fn unreadable_opencode_db_does_not_block_other_store_resumes() {
    let _lock = crate::storage::lock_test_env();
    for wal in [false, true] {
        let temp = tempfile::TempDir::new().unwrap();
        let _home = HomeGuard::set(temp.path());
        let external = temp.path().join("external");
        let db = external.join(".local/share/opencode/opencode.db");
        std::fs::create_dir_all(db.parent().unwrap()).unwrap();
        if wal {
            let writer = temp.path().join("writer/opencode.db");
            let f = fixture(&writer);
            f.0.execute_batch("PRAGMA journal_mode=WAL; PRAGMA wal_autocheckpoint=0; PRAGMA wal_checkpoint(TRUNCATE);").unwrap();
            add(&f, 0, "committed only in WAL");
            std::fs::copy(&writer, &db).unwrap();
            std::fs::copy(
                writer.with_file_name("opencode.db-wal"),
                db.with_file_name("opencode.db-wal"),
            )
            .unwrap();
        } else {
            std::fs::write(&db, b"this is not a sqlite database").unwrap();
        }
        let session_id = "22222222-3333-4444-5555-666666666666";
        let transcript = external.join(format!(
            ".cursor/projects/tmp-fallback/agent-transcripts/{session_id}/{session_id}.jsonl"
        ));
        std::fs::create_dir_all(transcript.parent().unwrap()).unwrap();
        std::fs::write(&transcript, "{\"role\":\"user\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"cursor fallthrough\"}]}}\n").unwrap();
        assert_eq!(
            import_external_resume_id(session_id).unwrap(),
            Some(imported_cursor_session_id(session_id))
        );

        let legacy =
            external.join(".local/share/opencode/storage/session/global/ses_fallback.json");
        std::fs::create_dir_all(legacy.parent().unwrap()).unwrap();
        std::fs::write(
            &legacy,
            r#"{"id":"ses_fallback","directory":"/tmp/demo","title":"Legacy"}"#,
        )
        .unwrap();
        assert_eq!(
            import_external_resume_id("ses_fallback").unwrap(),
            Some(imported_opencode_session_id("ses_fallback"))
        );
        assert_eq!(
            import_opencode_session("ses_fallback").unwrap().id,
            imported_opencode_session_id("ses_fallback")
        );

        let pi = external.join(".pi/agent/sessions/project/fallback.jsonl");
        std::fs::create_dir_all(pi.parent().unwrap()).unwrap();
        std::fs::write(&pi, concat!(
            "{\"type\":\"session\",\"id\":\"pi-fallback\",\"timestamp\":\"2026-04-05T19:00:00Z\",\"cwd\":\"/tmp/pi\"}\n",
            "{\"type\":\"message\",\"timestamp\":\"2026-04-05T19:00:02Z\",\"message\":{\"role\":\"user\",\"content\":[{\"type\":\"text\",\"text\":\"hello pi\"}]}}\n"
        )).unwrap();
        let pi_path = pi.to_str().unwrap();
        assert_eq!(
            import_external_resume_id(pi_path).unwrap(),
            Some(imported_pi_session_id(pi_path))
        );
        for error in [
            import_external_resume_id("ses_missing").unwrap_err(),
            import_opencode_session("ses_missing").unwrap_err(),
        ] {
            let detail = format!("{error:#}");
            assert!(
                detail.contains(if wal { "nonempty WAL" } else { "database" }),
                "{detail}"
            );
        }
        assert!(!db.with_file_name("opencode.db-shm").exists());
    }
}

#[test]
fn sqlite_budget_invalid_limits_fail_before_opening_database() {
    let path = Path::new("/nonexistent/disposable-fixture/opencode.db");
    for (count, bytes, block) in [(0, 256, 128), (1, 0, 128), (1, 256, 0), (1, 128, 256)] {
        let error = crate::opencode_db::load_import_messages(path, "budget", count, bytes, block)
            .err()
            .expect("invalid budget must fail");
        assert_eq!(
            error.to_string(),
            "Invalid OpenCode import retention budgets"
        );
    }
}

fn opencode_text_messages(session: &Session) -> Vec<(Role, String)> {
    session
        .messages
        .iter()
        .map(|m| {
            let text = m
                .content
                .iter()
                .filter_map(|block| match block {
                    ContentBlock::Text { text, .. } => Some(text.as_str()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join("\n");
            (m.role.clone(), text)
        })
        .collect()
}

#[test]
fn test_import_opencode_sqlite_session() {
    let _lock = crate::storage::lock_test_env();
    let temp = tempfile::TempDir::new().unwrap();
    let _home = HomeGuard::set(temp.path());
    let db = temp
        .path()
        .join("external/.local/share/opencode/opencode.db");
    drop(crate::opencode_db::fixture::standard(&db));

    let imported = import_opencode_session("ses_main").unwrap();
    assert_eq!(imported.id, imported_opencode_session_id("ses_main"));
    assert_eq!(imported.provider_key.as_deref(), Some("anthropic"));
    assert_eq!(imported.model.as_deref(), Some("claude-x"));
    assert_eq!(imported.working_dir.as_deref(), Some("/tmp/oc-main"));
    assert_eq!(imported.title.as_deref(), Some("Main task"));
    assert_eq!(
        opencode_text_messages(&imported),
        vec![
            (Role::User, "hello opencode".to_string()),
            (Role::Assistant, "hi from assistant".to_string()),
        ]
    );

    // Resume targets pointing at the database path route to the DB loader.
    let target = jcode_session_types::ResumeTarget::OpenCodeSession {
        session_id: "ses_old".to_string(),
        session_path: db.to_string_lossy().to_string(),
    };
    let resolved = resolve_resume_target_to_jcode(&target).unwrap();
    assert_eq!(
        resolved,
        jcode_session_types::ResumeTarget::JcodeSession {
            session_id: imported_opencode_session_id("ses_old")
        }
    );
    assert_eq!(
        import_external_resume_id("ses_main").unwrap(),
        Some(imported_opencode_session_id("ses_main"))
    );
}

#[test]
fn unreadable_opencode_db_does_not_block_other_stores() {
    let _lock = crate::storage::lock_test_env();
    let temp = tempfile::TempDir::new().unwrap();
    let _home = HomeGuard::set(temp.path());
    let external = temp.path().join("external");
    let db = external.join(".local/share/opencode/opencode.db");
    std::fs::create_dir_all(db.parent().unwrap()).unwrap();
    std::fs::write(&db, b"not a sqlite database").unwrap();
    let session_id = "22222222-3333-4444-5555-666666666666";
    let transcript = external.join(format!(
        ".cursor/projects/tmp-demo/agent-transcripts/{session_id}/{session_id}.jsonl"
    ));
    std::fs::create_dir_all(transcript.parent().unwrap()).unwrap();
    std::fs::write(
        &transcript,
        "{\"role\":\"user\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"hi\"}]}}\n",
    )
    .unwrap();

    assert_eq!(
        import_external_resume_id(session_id).unwrap(),
        Some(imported_cursor_session_id(session_id))
    );
    let legacy = external.join(".local/share/opencode/storage/session/global");
    std::fs::create_dir_all(&legacy).unwrap();
    std::fs::write(
        legacy.join("ses_legacy.json"),
        r#"{"id":"ses_legacy","directory":"/tmp/demo","title":"Legacy"}"#,
    )
    .unwrap();
    assert_eq!(
        import_opencode_session("ses_legacy").unwrap().id,
        imported_opencode_session_id("ses_legacy")
    );
    // With no other match, the database error still surfaces.
    assert!(import_external_resume_id("ses_missing").is_err());
    assert!(import_opencode_session("ses_missing").is_err());
}
