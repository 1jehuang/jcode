use super::*;
use crate::message::{ContentBlock, Role};
use crate::session::{EnvSnapshot, StoredCompactionState, StoredTokenUsage};

fn text(value: &str) -> Vec<ContentBlock> {
    vec![ContentBlock::Text {
        text: value.into(),
        cache_control: None,
    }]
}

fn environment(session: &Session) -> EnvSnapshot {
    EnvSnapshot {
        captured_at: session.created_at,
        reason: "storage boundary".into(),
        session_id: session.id.clone(),
        working_dir: None,
        provider: "offline".into(),
        model: "storage-contract".into(),
        jcode_version: "test".into(),
        jcode_git_hash: None,
        jcode_git_dirty: None,
        os: std::env::consts::OS.into(),
        arch: std::env::consts::ARCH.into(),
        pid: std::process::id(),
        is_selfdev: false,
        is_debug: false,
        is_canary: false,
        testing_build: None,
        working_git: None,
    }
}

#[test]
fn public_stores_preserve_append_edit_metadata_and_export() -> Result<()> {
    for backend in [SessionBackend::Files, SessionBackend::Sqlite] {
        let root = tempfile::tempdir()?;
        let store = SessionStore::open(root.path(), backend)?;
        let mut session = Session::create_with_id("boundary".into(), None, None);
        session.is_debug = false;
        assert!(!store.exists(&session.id)?);
        store.save(&mut session)?;
        assert!(
            !store.exists(&session.id)?,
            "empty panels must not create sessions"
        );
        session.add_message(Role::User, text("first prompt"));
        session.extra.insert(
            "future_metadata".into(),
            serde_json::json!({"nested": [1, "preserved"], "access_token": "opaque"}),
        );
        session.route_api_method = Some("test-route".into());
        session.migration_epoch = 7;
        session.compaction = Some(StoredCompactionState {
            summary_text: "summary".into(),
            openai_encrypted_content: Some("opaque".into()),
            covers_up_to_turn: 1,
            original_turn_count: 1,
            compacted_count: 1,
        });
        store.save(&mut session)?;
        session.add_message(Role::Assistant, text("answer"));
        session.messages.last_mut().unwrap().token_usage = Some(StoredTokenUsage {
            prompt_tokens: None,
            input_tokens: 10,
            output_tokens: 5,
            cache_read_input_tokens: Some(3),
            cache_creation_input_tokens: Some(2),
        });
        session.record_replay_display_message("system", None, "replay notice");
        session.record_memory_injection(
            "recalled".into(),
            "memory body".into(),
            1,
            0,
            vec!["memory-id".into()],
        );
        session.record_env_snapshot(environment(&session));
        session.custom_title = Some("custom".into());
        store.save(&mut session)?;
        let summary = store.list()?.pop().unwrap();
        assert_eq!(
            (
                summary.visible_message_count,
                summary.user_message_count,
                summary.assistant_message_count
            ),
            (2, 1, 1)
        );
        assert_eq!(summary.estimated_tokens, 20);
        assert_eq!(summary.first_user_prompt.as_deref(), Some("first prompt"));
        assert_eq!(summary.custom_title.as_deref(), Some("custom"));
        assert!(summary.search_text.contains("answer"));
        let haystack = store.search_text("boundary")?;
        for persisted_text in [
            "first prompt",
            "replay notice",
            "memory body",
            "storage boundary",
            "opaque",
        ] {
            assert!(
                haystack.contains(persisted_text),
                "search prefilter omitted {persisted_text:?}"
            );
        }
        assert!(
            haystack.contains("openai_encrypted_content"),
            "search must retain compaction JSON"
        );
        let expected = serde_json::to_value(&session)?;
        drop(store);
        let reopened = SessionStore::open(root.path(), backend)?;
        let mut loaded = reopened.load("boundary")?;
        assert_eq!(serde_json::to_value(&loaded)?, expected);
        assert_eq!(
            loaded.redacted_for_export().extra["future_metadata"]["access_token"],
            "[REDACTED_SECRET]"
        );
        loaded.replace_messages(vec![loaded.messages[0].clone()]);
        loaded.compaction = None;
        loaded.extra.insert(
            "future_metadata".into(),
            serde_json::json!({"changed": true}),
        );
        reopened.save(&mut loaded)?;
        assert_eq!(reopened.list()?[0].visible_message_count, 1);
        let exported = root.path().join("export.json");
        reopened.export("boundary", &exported)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&exported)?.permissions().mode() & 0o777,
                0o600
            );
            if backend == SessionBackend::Sqlite {
                assert_eq!(
                    std::fs::metadata(root.path().join("sessions.sqlite3"))?
                        .permissions()
                        .mode()
                        & 0o777,
                    0o600
                );
            }
        }
        let value: serde_json::Value = serde_json::from_slice(&std::fs::read(exported)?)?;
        assert_eq!(value, serde_json::to_value(&loaded)?);
        assert!(reopened.search_text("boundary")?.contains("first prompt"));
        assert!(reopened.load("../outside").is_err());
        if backend == SessionBackend::Sqlite {
            assert!(!root.path().join("sessions/boundary.json").exists());
        }
    }
    Ok(())
}

#[test]
fn sqlite_import_replays_legacy_journal_once_without_changing_source() -> Result<()> {
    use std::io::Write;
    let root = tempfile::tempdir()?;
    let files = SessionStore::open(root.path(), SessionBackend::Files)?;
    let mut session = Session::create_with_id("legacy".into(), None, None);
    session.add_message(
        Role::User,
        text(&"large original transcript ".repeat(50_000)),
    );
    session.extra.insert(
        "unknown_version_field".into(),
        serde_json::json!({"keep": true}),
    );
    files.save(&mut session)?;
    session.add_message(Role::Assistant, text("journal tail"));
    files.save(&mut session)?;
    let snapshot = root.path().join("sessions/legacy.json");
    let journal = root.path().join("sessions/legacy.journal.jsonl");
    std::fs::OpenOptions::new()
        .append(true)
        .open(&journal)?
        .write_all(b"{corrupt tail\n")?;
    let snapshot_modified = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_000);
    let journal_modified = std::time::UNIX_EPOCH + std::time::Duration::from_secs(2_000);
    std::fs::File::open(&snapshot)?.set_modified(snapshot_modified)?;
    std::fs::File::open(&journal)?.set_modified(journal_modified)?;
    let discovery_modified = DateTime::<Utc>::from(journal_modified);
    assert_ne!(session.updated_at, discovery_modified);
    assert_eq!(
        files.list()?.pop().unwrap().discovery_modified_at,
        Some(discovery_modified)
    );
    let snapshot_before = std::fs::read(&snapshot)?;
    let journal_before = std::fs::read(&journal)?;
    let sqlite = SessionStore::open(root.path(), SessionBackend::Sqlite)?;
    assert!(!sqlite.contains_native("legacy")?);
    assert!(sqlite.exists("legacy")?);
    let imported = sqlite.load("legacy")?;
    assert_eq!(
        serde_json::to_value(&imported)?,
        serde_json::to_value(&session)?
    );
    assert!(sqlite.contains_native("legacy")?);
    assert_eq!(
        sqlite.list()?.pop().unwrap().discovery_modified_at,
        Some(discovery_modified)
    );
    assert_eq!(std::fs::read(&snapshot)?, snapshot_before);
    assert_eq!(std::fs::read(&journal)?, journal_before);
    for backend in [SessionBackend::Files, SessionBackend::Sqlite] {
        let destination = tempfile::tempdir()?;
        let imported_store = SessionStore::open(destination.path(), backend)?;
        imported_store.import_from_path(&snapshot)?;
        let first_import = imported_store.load("legacy")?;
        assert_eq!(
            serde_json::to_value(&first_import)?,
            serde_json::to_value(&session)?
        );
        imported_store.import_from_path(&snapshot)?;
        assert_eq!(imported_store.list_search_candidates(1)?.len(), 1);
        if backend == SessionBackend::Sqlite {
            assert_eq!(
                imported_store.list()?.pop().unwrap().discovery_modified_at,
                Some(discovery_modified)
            );
        }
    }
    assert_eq!(std::fs::read(&snapshot)?, snapshot_before);
    assert_eq!(std::fs::read(&journal)?, journal_before);
    assert_eq!(std::fs::read_dir(root.path().join("sessions"))?.count(), 2);
    session.add_message(Role::Assistant, text("later file-only update"));
    files.save(&mut session)?;
    assert_eq!(
        sqlite.load("legacy")?.messages.len(),
        2,
        "an imported session is authoritative in SQLite"
    );
    let mut imported = sqlite.load("legacy")?;
    imported.custom_title = Some("metadata update after import".into());
    let before_save = Utc::now();
    sqlite.save(&mut imported)?;
    assert!(sqlite.list()?.pop().unwrap().discovery_modified_at.unwrap() >= before_save);
    Ok(())
}

#[test]
fn sqlite_commits_are_atomic_and_reject_stale_or_stripped_writers() -> Result<()> {
    let root = tempfile::tempdir()?;
    let store = SessionStore::open(root.path(), SessionBackend::Sqlite)?;
    let other = SessionStore::open(root.path(), SessionBackend::Sqlite)?;
    let mut session = Session::create_with_id("atomic".into(), None, None);
    session.add_message(Role::User, text("durable original"));
    store.save(&mut session)?;
    let mut stale = other.load("atomic")?;
    let original = serde_json::to_value(&stale)?;
    let connection = rusqlite::Connection::open(root.path().join("sessions.sqlite3"))?;
    connection.execute_batch("CREATE TRIGGER reject_replay BEFORE INSERT ON session_replay BEGIN SELECT RAISE(ABORT,'injected storage failure'); END;")?;
    session.add_message(Role::Assistant, text("pending answer"));
    session.record_replay_display_message("system", None, "pending event");
    session.custom_title = Some("pending title".into());
    assert!(store.save(&mut session).is_err());
    assert_eq!(serde_json::to_value(store.load("atomic")?)?, original);
    connection.execute_batch("DROP TRIGGER reject_replay;")?;
    store.save(&mut session)?;
    stale.custom_title = Some("stale title".into());
    assert!(other.save(&mut stale).is_err());
    let mut stub = store.load_startup_stub("atomic")?;
    assert!(store.save_prepared(&mut stub).is_err());
    assert_eq!(store.load("atomic")?.messages.len(), 2);
    let first_version = store.list_search_candidates(1)?[0].version;
    session.saved = true;
    store.save(&mut session)?;
    assert!(store.list_search_candidates(1)?[0].version > first_version);
    let saved = store.list_summaries(&SessionListOptions {
        updated_since: Some(Utc::now() + chrono::Duration::days(1)),
        include_saved_beyond_window: true,
        limit: 1,
        ..Default::default()
    })?;
    assert_eq!(saved.len(), 1);
    assert!(saved[0].saved);
    Ok(())
}

#[test]
fn sqlite_orders_and_filters_nanoseconds_before_and_after_v1_migration() -> Result<()> {
    let root = tempfile::tempdir()?;
    let store = SessionStore::open(root.path(), SessionBackend::Sqlite)?;
    for (id, timestamp) in [
        ("a-older", "2026-10-27T08:00:00.000000100Z"),
        ("z-newer", "2026-10-27T08:00:00.000000900Z"),
    ] {
        let mut session = Session::create_with_id(id.into(), None, Some(id.into()));
        session.updated_at = chrono::DateTime::parse_from_rfc3339(timestamp)?.with_timezone(&Utc);
        let snapshot = root.path().join(format!("{id}.json"));
        crate::storage::write_json_secret(&snapshot, &session)?;
        store.import_from_path(&snapshot)?;
    }
    let assert_order = |store: &SessionStore| -> Result<()> {
        let summaries = store.list()?;
        assert_eq!(
            summaries
                .iter()
                .map(|row| row.id.as_str())
                .collect::<Vec<_>>(),
            ["z-newer", "a-older"]
        );
        assert_eq!(store.list_search_candidates(1)?[0].id, "z-newer");
        let rows = store.list_summaries(&SessionListOptions {
            updated_since: Some(
                chrono::DateTime::parse_from_rfc3339("2026-10-27T08:00:00.000000500Z")?
                    .with_timezone(&Utc),
            ),
            ..Default::default()
        })?;
        assert_eq!(
            rows.iter().map(|row| row.id.as_str()).collect::<Vec<_>>(),
            ["z-newer"]
        );
        Ok(())
    };
    assert_order(&store)?;
    drop(store);
    // Recreate the previous persisted schema using real rows written above.
    let connection = rusqlite::Connection::open(root.path().join("sessions.sqlite3"))?;
    for table in [
        "sessions",
        "session_messages",
        "session_replay",
        "session_environment",
        "session_memory",
        "session_compaction",
    ] {
        connection.execute_batch(&format!(
            "DROP TRIGGER IF EXISTS {table}_search_fts_insert;
             DROP TRIGGER IF EXISTS {table}_search_fts_delete;
             DROP TRIGGER IF EXISTS {table}_search_fts_update;
             DROP TABLE IF EXISTS {table}_search_fts;
             DROP VIEW IF EXISTS {table}_search_content;"
        ))?;
    }
    connection.execute_batch(
        "DROP INDEX sessions_discovery;
         DROP INDEX sessions_saved_discovery;
         ALTER TABLE sessions DROP COLUMN discovery_seconds;
         ALTER TABLE sessions DROP COLUMN discovery_nanos;
         ALTER TABLE sessions DROP COLUMN sort_key;
         ALTER TABLE sessions DROP COLUMN picker_debug;
         ALTER TABLE sessions DROP COLUMN picker_has_history;
         DROP INDEX sessions_updated_precise;
         DROP INDEX sessions_saved_precise;
         ALTER TABLE sessions DROP COLUMN updated_seconds;
         ALTER TABLE sessions DROP COLUMN updated_nanos;
         ALTER TABLE sessions DROP COLUMN transcript_revision;
         CREATE INDEX sessions_updated ON sessions(updated_ms DESC);
         CREATE INDEX sessions_saved ON sessions(saved,updated_ms DESC);
         PRAGMA user_version=1;",
    )?;
    drop(connection);
    let migrated = SessionStore::open(root.path(), SessionBackend::Sqlite)?;
    assert_order(&migrated)?;
    assert_eq!(
        migrated
            .load("z-newer")?
            .updated_at
            .timestamp_subsec_nanos(),
        900
    );
    Ok(())
}

#[test]
fn sqlite_rebases_only_metadata_sidewriters_without_losing_either_metadata_edit() -> Result<()> {
    for kind in [
        "metadata",
        "messages",
        "replay",
        "environment",
        "memory",
        "compaction",
    ] {
        let root = tempfile::tempdir()?;
        let store = SessionStore::open(root.path(), SessionBackend::Sqlite)?;
        let other = SessionStore::open(root.path(), SessionBackend::Sqlite)?;
        let mut original = Session::create_with_id("race".into(), None, None);
        original.add_message(Role::User, text("initial request"));
        store.save(&mut original)?;
        let mut owner = store.load("race")?;
        let mut sidewriter = other.load("race")?;
        sidewriter.custom_title = Some("sidewriter title".into());
        sidewriter.saved = true;
        sidewriter.model = Some("sidewriter model".into());
        sidewriter.extra.insert(
            "future_sidewriter".into(),
            serde_json::json!({"kept": true}),
        );
        match kind {
            "metadata" => {}
            "messages" => {
                sidewriter.add_message(Role::Assistant, text("other writer's reply"));
            }
            "replay" => sidewriter.record_replay_display_message("system", None, "other replay"),
            "environment" => sidewriter.record_env_snapshot(environment(&sidewriter)),
            "memory" => sidewriter.record_memory_injection(
                "other memory".into(),
                "body".into(),
                1,
                0,
                vec![],
            ),
            "compaction" => {
                sidewriter.compaction = Some(StoredCompactionState {
                    summary_text: "other summary".into(),
                    openai_encrypted_content: None,
                    covers_up_to_turn: 1,
                    original_turn_count: 1,
                    compacted_count: 1,
                })
            }
            _ => unreachable!(),
        }
        other.save(&mut sidewriter)?;
        let committed_sidewriter = serde_json::to_value(other.load("race")?)?;
        owner.add_message(Role::Assistant, text("live owner's reply"));
        owner.model = Some("owner model".into());
        if kind == "metadata" {
            store.save(&mut owner)?;
            assert_eq!(owner.custom_title.as_deref(), Some("sidewriter title"));
            assert_eq!(owner.model.as_deref(), Some("owner model"));
            assert!(owner.saved);
            assert_eq!(owner.extra["future_sidewriter"]["kept"], true);
            assert_eq!(
                serde_json::to_value(store.load("race")?)?,
                serde_json::to_value(&owner)?
            );
            owner.add_message(Role::User, text("following request"));
            store.save(&mut owner)?;
            let rows = store.list_summaries(&SessionListOptions {
                updated_since: Some(Utc::now() + chrono::Duration::days(1)),
                include_saved_beyond_window: true,
                ..Default::default()
            })?;
            assert_eq!(rows.len(), 1);
            assert_eq!(rows[0].visible_message_count, 3);
        } else {
            assert!(
                store.save(&mut owner).is_err(),
                "concurrent {kind} changes must reject a stale owner"
            );
            assert_eq!(
                serde_json::to_value(other.load("race")?)?,
                committed_sidewriter
            );
        }
    }
    Ok(())
}

#[test]
fn file_remote_startup_save_preserves_unloaded_side_vectors() -> Result<()> {
    struct Restore(Vec<(&'static str, Option<std::ffi::OsString>)>);
    impl Drop for Restore {
        fn drop(&mut self) {
            for (key, value) in self.0.drain(..) {
                if let Some(value) = value {
                    crate::env::set_var(key, value);
                } else {
                    crate::env::remove_var(key);
                }
            }
        }
    }
    let _lock = crate::storage::lock_test_env();
    let _restore = Restore(
        ["JCODE_HOME", "JCODE_SESSION_BACKEND"]
            .into_iter()
            .map(|key| (key, std::env::var_os(key)))
            .collect(),
    );
    let root = tempfile::tempdir()?;
    crate::env::set_var("JCODE_HOME", root.path());
    crate::env::set_var("JCODE_SESSION_BACKEND", "files");
    let store = SessionStore::open(root.path(), SessionBackend::Files)?;
    let mut session = Session::create_with_id("remote".into(), None, None);
    session.add_message(Role::User, text("remote request"));
    session.record_env_snapshot(environment(&session));
    session.record_memory_injection("memory".into(), "body".into(), 1, 0, vec![]);
    session.record_replay_display_message("system", None, "replay");
    store.save(&mut session)?;
    let mut remote = Session::load_for_remote_startup("remote")?;
    assert!(remote.env_snapshots.is_empty());
    assert!(remote.memory_injections.is_empty());
    assert!(remote.replay_events.is_empty());
    remote.save_prepared()?;
    let authoritative = store.load("remote")?;
    assert_eq!(authoritative.env_snapshots.len(), 1);
    assert_eq!(authoritative.memory_injections.len(), 1);
    assert_eq!(authoritative.replay_events.len(), 1);
    Ok(())
}

#[test]
fn imports_refuse_corrupt_primary_even_when_an_older_backup_is_valid() -> Result<()> {
    let source = tempfile::tempdir()?;
    std::fs::create_dir(source.path().join("sessions"))?;
    let snapshot = source.path().join("sessions/broken.json");
    let backup = source.path().join("sessions/broken.bak");
    let mut session = Session::create_with_id("broken".into(), None, None);
    session.add_message(Role::User, text("older backup only"));
    crate::storage::write_json_secret(&backup, &session)?;
    std::fs::write(&snapshot, b"{corrupt primary")?;
    let backup_bytes = std::fs::read(&backup)?;
    for backend in [SessionBackend::Files, SessionBackend::Sqlite] {
        let destination = tempfile::tempdir()?;
        let store = SessionStore::open(destination.path(), backend)?;
        assert!(store.import_from_path(&snapshot).is_err());
        assert!(!store.contains_native("broken")?);
    }
    let sqlite = SessionStore::open(source.path(), SessionBackend::Sqlite)?;
    assert!(sqlite.load("broken").is_err());
    assert!(!sqlite.contains_native("broken")?);
    assert_eq!(std::fs::read(&snapshot)?.as_slice(), b"{corrupt primary");
    assert_eq!(std::fs::read(&backup)?, backup_bytes);
    let files = SessionStore::open(source.path(), SessionBackend::Files)?;
    assert_eq!(
        files.load("broken")?.messages.len(),
        1,
        "normal legacy recovery remains enabled"
    );
    Ok(())
}

#[cfg(unix)]
#[test]
fn sqlite_live_files_are_private_with_shared_umask() -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    const CHILD: &str = "JCODE_STORAGE_PRIVACY_TEST_CHILD";
    if std::env::var_os(CHILD).is_none() {
        let output = std::process::Command::new(std::env::current_exe()?)
            .args([
                "--exact",
                "session::store::tests::sqlite_live_files_are_private_with_shared_umask",
                "--nocapture",
            ])
            .env(CHILD, "1")
            .output()?;
        assert!(
            output.status.success(),
            "isolated privacy test failed: {}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        return Ok(());
    }
    // The exact-test child keeps process-global umask out of parallel tests.
    unsafe {
        libc::umask(0o002);
    }
    let root = tempfile::tempdir()?;
    let store = SessionStore::open(root.path(), SessionBackend::Sqlite)?;
    let mut session = Session::create_with_id("private".into(), None, Some("private".into()));
    session.add_message(Role::User, text("private transcript"));
    store.save(&mut session)?;
    let paths = [
        "sessions.sqlite3",
        "sessions.sqlite3-wal",
        "sessions.sqlite3-shm",
    ];
    for filename in paths {
        let path = root.path().join(filename);
        assert_eq!(
            std::fs::metadata(&path)?.permissions().mode() & 0o777,
            0o600,
            "{filename}"
        );
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o664))?;
    }
    assert_eq!(store.load("private")?.messages.len(), 1);
    for filename in paths {
        assert_eq!(
            std::fs::metadata(root.path().join(filename))?
                .permissions()
                .mode()
                & 0o777,
            0o600,
            "repair {filename}"
        );
    }
    Ok(())
}

fn expected_discovered(
    store: &SessionStore,
    limit: usize,
    include_saved: bool,
) -> Result<Vec<String>> {
    let mut rows = store.list()?;
    rows.retain(|row| !row.id.starts_with("imported_"));
    rows.sort_by(|a, b| {
        (discovery_key(b), session_sort_key(&b.id), &b.id).cmp(&(
            discovery_key(a),
            session_sort_key(&a.id),
            &a.id,
        ))
    });
    Ok(rows
        .into_iter()
        .enumerate()
        .filter(|(index, row)| *index < limit || (include_saved && row.saved))
        .map(|(_, row)| row.id)
        .collect())
}

#[test]
fn sqlite_discovered_summaries_match_picker_order_across_v3_migration() -> Result<()> {
    let root = tempfile::tempdir()?;
    let store = SessionStore::open(root.path(), SessionBackend::Sqlite)?;
    let base = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000);
    // (id, discovery offset ms, saved). Equal offsets exercise the id tie-breakers.
    let cases = [
        ("session_1700000000002_a", 5, false),
        ("session_1700000000009_b", 5, true),
        ("session_1700000000009_a", 5, false),
        ("plain", 7, false),
        ("plain_12", 7, false),
        ("old_saved", 1, true),
        ("old_unsaved", 1, false),
        ("oldest_saved", 0, true),
        ("imported_session_1700000000099_x", 99, true),
    ];
    for (id, offset_ms, saved) in cases {
        let mut session = Session::create_with_id(id.into(), None, None);
        session.saved = saved;
        session.is_debug = id.ends_with("_a") || id == "plain";
        if id != "old_unsaved" {
            session.add_message(Role::User, text("picker history"));
        }
        let snapshot = root.path().join(format!("{id}.json"));
        crate::storage::write_json_secret(&snapshot, &session)?;
        std::fs::OpenOptions::new()
            .write(true)
            .open(&snapshot)?
            .set_modified(base + std::time::Duration::from_millis(offset_ms))?;
        store.import_from_path(&snapshot)?;
    }
    let assert_all = |store: &SessionStore| -> Result<()> {
        for limit in [0, 1, 2, 3, 5, 8, 100] {
            for include_saved in [false, true] {
                let actual: Vec<String> = store
                    .list_discovered_summaries(limit, include_saved)?
                    .into_iter()
                    .map(|row| row.id)
                    .collect();
                for visible_limit in [0, 1, 2, 4, 100] {
                    let ordered = store.list_discovered_summaries(limit, include_saved)?;
                    let (mut visible, mut debug) = (0, 0);
                    let mut expected = Vec::new();
                    for row in ordered {
                        if row.visible_message_count == 0 {
                            continue;
                        }
                        let keep = if visible >= visible_limit {
                            row.saved
                        } else if row.is_debug {
                            debug += 1;
                            debug <= visible_limit
                        } else {
                            visible += 1;
                            true
                        };
                        if keep {
                            expected.push(row.id);
                        }
                    }
                    let actual = store
                        .list_picker_summaries(visible_limit, limit, include_saved)?
                        .into_iter()
                        .map(|row| row.id)
                        .collect::<Vec<_>>();
                    assert_eq!(
                        actual, expected,
                        "visible={visible_limit} candidates={limit} saved={include_saved}"
                    );
                }
                assert_eq!(
                    actual,
                    expected_discovered(store, limit, include_saved)?,
                    "limit {limit} include_saved {include_saved}"
                );
            }
        }
        assert!(
            store
                .list_discovered_summaries(100, true)?
                .iter()
                .all(|row| !row.id.starts_with("imported_"))
        );
        Ok(())
    };
    assert_all(&store)?;
    assert_eq!(
        store.list_discovered_summaries(3, false)?[0].id,
        "plain_12",
        "plain_12 has a larger numeric key than plain"
    );
    drop(store);
    // Recreate the v3 schema from the rows written above.
    let connection = rusqlite::Connection::open(root.path().join("sessions.sqlite3"))?;
    for table in [
        "sessions",
        "session_messages",
        "session_replay",
        "session_environment",
        "session_memory",
        "session_compaction",
    ] {
        connection.execute_batch(&format!(
            "DROP TRIGGER IF EXISTS {table}_search_fts_insert;
             DROP TRIGGER IF EXISTS {table}_search_fts_delete;
             DROP TRIGGER IF EXISTS {table}_search_fts_update;
             DROP TABLE IF EXISTS {table}_search_fts;
             DROP VIEW IF EXISTS {table}_search_content;"
        ))?;
    }
    connection.execute_batch(
        "DROP INDEX sessions_discovery;
         DROP INDEX sessions_saved_discovery;
         ALTER TABLE sessions DROP COLUMN discovery_seconds;
         ALTER TABLE sessions DROP COLUMN discovery_nanos;
         ALTER TABLE sessions DROP COLUMN sort_key;
         ALTER TABLE sessions DROP COLUMN picker_debug;
         ALTER TABLE sessions DROP COLUMN picker_has_history;
         PRAGMA user_version=3;",
    )?;
    drop(connection);
    let migrated = SessionStore::open(root.path(), SessionBackend::Sqlite)?;
    assert_all(&migrated)?;
    let connection = rusqlite::Connection::open(root.path().join("sessions.sqlite3"))?;
    assert_eq!(
        connection.query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))?,
        7
    );
    // A later save moves the session to the front through the indexed columns.
    let mut loaded = migrated.load("old_unsaved")?;
    loaded.add_message(Role::User, text("updated session"));
    loaded.custom_title = Some("touched".into());
    migrated.save(&mut loaded)?;
    assert_eq!(
        migrated.list_discovered_summaries(1, false)?[0].id,
        "old_unsaved"
    );
    assert_all(&migrated)?;
    Ok(())
}
