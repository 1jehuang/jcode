use super::*;

struct HomeGuard(Option<std::ffi::OsString>);

impl HomeGuard {
    fn set(path: &std::path::Path) -> Self {
        let previous = std::env::var_os("JCODE_HOME");
        crate::env::set_var("JCODE_HOME", path);
        Self(previous)
    }
}

impl Drop for HomeGuard {
    fn drop(&mut self) {
        match self.0.take() {
            Some(previous) => crate::env::set_var("JCODE_HOME", previous),
            None => crate::env::remove_var("JCODE_HOME"),
        }
    }
}

fn session(id: &str, title: &str) -> Session {
    let mut session = Session::create_with_id(id.to_owned(), None, Some(title.to_owned()));
    session.short_name = Some(format!("short-{id}"));
    session
}

#[test]
fn migrates_old_schema_without_treating_metadata_as_resume_entries() -> Result<()> {
    let _lock = crate::storage::lock_test_env();
    let home = tempfile::tempdir()?;
    let _home = HomeGuard::set(home.path());
    let path = home.path().join("session-metadata-v1.sqlite3");
    {
        let connection = Connection::open(&path)?;
        connection.execute_batch(
            "CREATE TABLE recent_sessions (
                session_id TEXT PRIMARY KEY NOT NULL,
                working_dir TEXT,
                generated_title TEXT,
                custom_title TEXT,
                todo_title TEXT,
                updated_at_ms INTEGER NOT NULL,
                last_active_at_ms INTEGER
             );
             INSERT INTO recent_sessions (session_id, generated_title, updated_at_ms)
             VALUES ('legacy', 'Legacy metadata', 123);",
        )?;
    }
    let connection = open_at(&path)?;
    // Every new column must exist even for the oldest supported schema.
    let migrated: (Option<String>, Option<String>, Option<String>) = connection.query_row(
        "SELECT resume_short_name, resume_fingerprint, resume_updated_at
         FROM recent_sessions WHERE session_id = 'legacy'",
        [],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
    )?;
    assert_eq!(migrated, (None, None, None));
    assert_eq!(
        recent_with(&connection, 10)?[0].generated_title.as_deref(),
        Some("Legacy metadata")
    );
    assert!(resume_entries()?.is_empty());
    // Reopening an already migrated database must be safe too.
    drop(open_at(&path)?);
    Ok(())
}

#[test]
fn save_populates_resume_index_before_any_resume_lookup() -> Result<()> {
    let _lock = crate::storage::lock_test_env();
    let home = tempfile::tempdir()?;
    let _home = HomeGuard::set(home.path());
    let mut session = session("session_sqlite_save", "Persisted title");
    session.save()?;
    // No title lookup or file scan has run before this query.
    let entries = resume_entries()?;
    let entry = entries
        .get(&session.id)
        .expect("save must eagerly index resume metadata");
    assert_eq!(entry.short_name, session.short_name);
    assert_eq!(entry.title.as_deref(), Some("Persisted title"));
    assert_eq!(entry.updated_at, session.updated_at);
    Ok(())
}

#[test]
fn generic_metadata_upsert_invalidates_all_resume_fields() -> Result<()> {
    let _lock = crate::storage::lock_test_env();
    let home = tempfile::tempdir()?;
    let _home = HomeGuard::set(home.path());
    let mut session = session("session_sqlite_invalidate", "Original title");
    session.save()?;
    assert!(resume_entries()?.contains_key(&session.id));
    let mut metadata = recent(10)?
        .into_iter()
        .find(|entry| entry.session_id == session.id)
        .unwrap();
    metadata.generated_title = Some("Metadata-only change".into());
    upsert(&metadata)?;
    assert!(!resume_entries()?.contains_key(&session.id));
    with_connection(|connection| {
        let fields: (Option<String>, Option<String>, Option<String>) = connection.query_row(
            "SELECT resume_short_name, resume_fingerprint, resume_updated_at
             FROM recent_sessions WHERE session_id = ?1",
            [&session.id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )?;
        assert_eq!(fields, (None, None, None));
        Ok(())
    })?;
    // A subsequent real session save makes the row usable again.
    session.save()?;
    assert!(resume_entries()?.contains_key(&session.id));
    Ok(())
}

#[test]
fn malformed_fingerprint_does_not_discard_other_resume_entries() -> Result<()> {
    let _lock = crate::storage::lock_test_env();
    let home = tempfile::tempdir()?;
    let _home = HomeGuard::set(home.path());
    let mut healthy = session("session_sqlite_healthy", "Healthy title");
    let mut corrupt = session("session_sqlite_corrupt", "Corrupt title");
    healthy.save()?;
    corrupt.save()?;
    for malformed in ["not json", "{\"snapshot\":123}", "null"] {
        with_connection(|connection| {
            connection.execute(
                "UPDATE recent_sessions SET resume_fingerprint = ?2 WHERE session_id = ?1",
                params![corrupt.id, malformed],
            )?;
            Ok(())
        })?;
        let entries = resume_entries()?;
        assert!(
            entries.contains_key(&healthy.id),
            "bad row must not poison valid rows"
        );
        assert!(
            !entries.contains_key(&corrupt.id),
            "invalid fingerprint: {malformed}"
        );
    }
    Ok(())
}

#[test]
fn malformed_timestamp_does_not_discard_other_resume_entries() -> Result<()> {
    let _lock = crate::storage::lock_test_env();
    let home = tempfile::tempdir()?;
    let _home = HomeGuard::set(home.path());
    let mut healthy = session("session_timestamp_healthy", "Healthy title");
    let mut corrupt = session("session_timestamp_corrupt", "Corrupt title");
    healthy.save()?;
    corrupt.save()?;
    for malformed in [Some("not a timestamp"), Some(""), None] {
        with_connection(|connection| {
            connection.execute(
                "UPDATE recent_sessions SET resume_updated_at = ?2 WHERE session_id = ?1",
                params![corrupt.id, malformed],
            )?;
            Ok(())
        })?;
        let entries = resume_entries()?;
        assert!(entries.contains_key(&healthy.id));
        assert!(!entries.contains_key(&corrupt.id));
    }
    // Skipped rows remain recoverable from the authoritative snapshot.
    assert_eq!(
        crate::session::find_session_by_name_or_id("Corrupt title")?,
        corrupt.id
    );
    assert!(resume_entries()?.contains_key(&corrupt.id));
    Ok(())
}

#[test]
fn concurrent_schema_migrations_are_idempotent() -> Result<()> {
    let home = tempfile::tempdir()?;
    let path = home.path().join("index.sqlite3");
    // Initialize WAL before racing the additive migrations themselves.
    let connection = Connection::open(&path)?;
    connection.execute_batch(
        "PRAGMA journal_mode=WAL;
         CREATE TABLE recent_sessions (
             session_id TEXT PRIMARY KEY NOT NULL,
             working_dir TEXT, generated_title TEXT, custom_title TEXT,
             todo_title TEXT, updated_at_ms INTEGER NOT NULL,
             last_active_at_ms INTEGER
         );",
    )?;
    let barrier = std::sync::Barrier::new(4);
    std::thread::scope(|scope| {
        let handles: Vec<_> = (0..4)
            .map(|_| {
                scope.spawn(|| {
                    barrier.wait();
                    let connection = open_at(&path)?;
                    assert!(recent_with(&connection, 1)?.is_empty());
                    Ok::<_, anyhow::Error>(())
                })
            })
            .collect();
        for handle in handles {
            handle.join().expect("migration worker panicked")?;
        }
        Ok::<_, anyhow::Error>(())
    })?;
    Ok(())
}

#[test]
fn resume_title_precedence_excludes_todo_title() -> Result<()> {
    let _lock = crate::storage::lock_test_env();
    let home = tempfile::tempdir()?;
    let _home = HomeGuard::set(home.path());
    let mut session = session("session_sqlite_titles", "Generated resume title");
    session.saved = true;
    session.save_label = Some("Saved resume label".into());
    session.custom_title = Some("Custom resume title".into());
    session.save()?;
    for (custom, saved, label, expected) in [
        (
            Some("Custom resume title"),
            true,
            Some("Saved resume label"),
            Some("Custom resume title"),
        ),
        (
            None,
            true,
            Some("Saved resume label"),
            Some("Saved resume label"),
        ),
        (
            None,
            false,
            Some("Ignored unsaved label"),
            Some("Generated resume title"),
        ),
        (Some("  "), true, Some("  "), Some("Generated resume title")),
    ] {
        session.custom_title = custom.map(str::to_owned);
        session.saved = saved;
        session.save_label = label.map(str::to_owned);
        session.save()?;
        with_connection(|connection| {
            connection.execute(
                "UPDATE recent_sessions SET todo_title = 'Todo-only forbidden resume title'
                 WHERE session_id = ?1",
                [&session.id],
            )?;
            Ok(())
        })?;
        assert_eq!(resume_entries()?[&session.id].title.as_deref(), expected);
    }
    assert!(
        crate::session::find_session_by_name_or_id("Todo-only forbidden resume title").is_err()
    );
    assert_eq!(
        crate::session::find_session_by_name_or_id("Generated resume title")?,
        session.id
    );
    Ok(())
}

#[test]
fn metadata_only_row_is_backfilled_by_resume_file_scan() -> Result<()> {
    let _lock = crate::storage::lock_test_env();
    let home = tempfile::tempdir()?;
    let _home = HomeGuard::set(home.path());
    let mut session = session("session_sqlite_backfill", "Backfill unique title");
    session.save()?;
    let metadata = recent(10)?
        .into_iter()
        .find(|entry| entry.session_id == session.id)
        .unwrap();
    upsert(&metadata)?;
    assert!(resume_entries()?.is_empty());
    assert_eq!(
        crate::session::find_session_by_name_or_id("Backfill unique title")?,
        session.id
    );
    assert_eq!(
        resume_entries()?[&session.id].title.as_deref(),
        Some("Backfill unique title")
    );
    Ok(())
}

#[test]
fn resume_timestamp_roundtrip_preserves_nanoseconds() -> Result<()> {
    let _lock = crate::storage::lock_test_env();
    let home = tempfile::tempdir()?;
    let _home = HomeGuard::set(home.path());
    let mut session = session("session_sqlite_precision", "Precise title");
    session.save()?;
    session.updated_at = chrono::DateTime::parse_from_rfc3339("2026-10-29T12:34:56.123456789Z")?
        .with_timezone(&chrono::Utc);
    crate::storage::write_json_fast(&crate::session::session_path(&session.id)?, &session)?;
    upsert_session(&session)?;
    assert_eq!(
        resume_entries()?[&session.id].updated_at,
        session.updated_at
    );
    Ok(())
}

#[test]
fn stale_backfill_cannot_overwrite_a_newer_save() -> Result<()> {
    let _lock = crate::storage::lock_test_env();
    let home = tempfile::tempdir()?;
    let _home = HomeGuard::set(home.path());
    let mut current = session("session_sqlite_race", "Old backfill title");
    current.save()?;
    let older = Session::load_startup_stub(&current.id)?;
    let fingerprint = SessionFingerprint::read(&crate::session::session_path(&current.id)?)
        .expect("saved snapshot fingerprint");
    current.rename_title(Some("New title saved during backfill".into()));
    current.save()?;
    backfill_resume_session(&older, &fingerprint)?;
    assert_eq!(
        resume_entries()?[&current.id].title.as_deref(),
        current.display_title()
    );
    assert_eq!(recent(1)?[0].custom_title, current.custom_title);
    Ok(())
}
