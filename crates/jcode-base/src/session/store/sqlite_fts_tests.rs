use super::*;
use crate::message::{ContentBlock, Role};
use crate::session::store::{SessionBackend, SessionStore};
use crate::session::{EnvSnapshot, Session, StoredCompactionState};

fn text(value: &str) -> Vec<ContentBlock> {
    vec![ContentBlock::Text {
        text: value.into(),
        cache_control: None,
    }]
}

fn hits(store: &SessionStore, query: &str) -> Result<Vec<String>> {
    Ok(store
        .indexed_search_candidates(query)?
        .expect("SQLite index"))
}

#[test]
fn sqlite_fts_tracks_public_commits_and_transaction_rollback() -> Result<()> {
    let root = tempfile::tempdir()?;
    let store = SessionStore::open(root.path(), SessionBackend::Sqlite)?;
    let mut session = Session::create_with_id("indexed-session".into(), None, None);
    session.add_message(
        Role::User,
        text("firstneedle Observability Kelvin İmage 工具 42"),
    );
    store.save(&mut session)?;
    for query in [
        "firstneedle",
        "servabil",
        "kelvin",
        "mage",
        "工具",
        "42",
        "indexed-session",
    ] {
        assert_eq!(hits(&store, query)?, vec![session.id.clone()], "{query}");
    }
    assert!(hits(&store, "absentneedle")?.is_empty());
    session.add_message(Role::Assistant, text("appendneedle"));
    session.record_replay_display_message("system", None, "replayneedle");
    session.record_memory_injection("test".into(), "memoryneedle".into(), 1, 0, vec![]);
    session.record_env_snapshot(EnvSnapshot {
        captured_at: session.created_at,
        reason: "environmentneedle".into(),
        session_id: session.id.clone(),
        working_dir: None,
        provider: "offline".into(),
        model: "test".into(),
        jcode_version: "test".into(),
        jcode_git_hash: None,
        jcode_git_dirty: None,
        os: "test".into(),
        arch: "test".into(),
        pid: std::process::id(),
        is_selfdev: false,
        is_debug: false,
        is_canary: false,
        testing_build: None,
        working_git: None,
    });
    session.compaction = Some(StoredCompactionState {
        summary_text: "compactionneedle".into(),
        openai_encrypted_content: None,
        covers_up_to_turn: 1,
        original_turn_count: 1,
        compacted_count: 1,
    });
    session.custom_title = Some("titleneedle".into());
    store.save(&mut session)?;
    for query in [
        "appendneedle",
        "firstneedle appendneedle",
        "replayneedle",
        "memoryneedle",
        "environmentneedle",
        "compactionneedle",
        "titleneedle",
    ] {
        assert_eq!(hits(&store, query)?, vec![session.id.clone()], "{query}");
    }
    let mut connection = Connection::open(root.path().join("sessions.sqlite3"))?;
    {
        let transaction = connection.transaction()?;
        transaction.execute(
            "UPDATE sessions SET metadata=?1 WHERE id=?2",
            ["rollbackneedle", &session.id],
        )?;
        assert_eq!(
            candidates(&transaction, "rollbackneedle")?,
            vec![session.id.clone()]
        );
    }
    assert!(hits(&store, "rollbackneedle")?.is_empty());
    session.replace_messages(vec![session.messages[0].clone()]);
    session.compaction = None;
    session.custom_title = Some("replacementneedle".into());
    store.save(&mut session)?;
    assert!(hits(&store, "appendneedle")?.is_empty());
    assert!(hits(&store, "compactionneedle")?.is_empty());
    assert!(hits(&store, "titleneedle")?.is_empty());
    assert_eq!(hits(&store, "replacementneedle")?, vec![session.id.clone()]);
    for (table, _, _) in SOURCES {
        let index = format!("{table}_search_fts");
        connection.execute(
            &format!("INSERT INTO {index}({index},rank) VALUES('integrity-check',1)"),
            [],
        )?;
    }
    assert!(!root.path().join("sessions/indexed-session.json").exists());
    Ok(())
}

#[test]
fn sqlite_fts_migration_rebuilds_existing_content() -> Result<()> {
    let root = tempfile::tempdir()?;
    let store = SessionStore::open(root.path(), SessionBackend::Sqlite)?;
    let mut session = Session::create_with_id("migration-session".into(), None, None);
    session.add_message(Role::User, text("migrationneedle"));
    store.save(&mut session)?;
    drop(store);
    {
        let mut connection = Connection::open(root.path().join("sessions.sqlite3"))?;
        for (table, _, _) in SOURCES {
            connection.execute_batch(&format!(
                "DROP TRIGGER {table}_search_fts_insert;
                 DROP TRIGGER {table}_search_fts_delete;
                 DROP TRIGGER {table}_search_fts_update;
                 DROP TABLE {table}_search_fts;
                 DROP VIEW {table}_search_content;"
            ))?;
        }
        connection.execute_batch(
            "PRAGMA user_version=4; ALTER TABLE session_memory RENAME TO memory_hold;",
        )?;
        assert!(migrate(&mut connection).is_err());
        assert_eq!(
            connection.query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))?,
            4
        );
        assert_eq!(
            connection.query_row(
                "SELECT count(*) FROM sqlite_master WHERE name='sessions_search_fts'",
                [],
                |row| row.get::<_, i64>(0)
            )?,
            0
        );
        connection.execute_batch("ALTER TABLE memory_hold RENAME TO session_memory;")?;
    }
    let reopened = SessionStore::open(root.path(), SessionBackend::Sqlite)?;
    assert_eq!(hits(&reopened, "migrationneedle")?, vec![session.id]);
    Ok(())
}
