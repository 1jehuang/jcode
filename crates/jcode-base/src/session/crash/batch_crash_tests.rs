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
        if let Some(previous) = &self.0 {
            crate::env::set_var("JCODE_HOME", previous);
        } else {
            crate::env::remove_var("JCODE_HOME");
        }
    }
}

#[test]
fn test_crashed_sessions_info_struct() {
    let info = CrashedSessionsInfo {
        session_ids: vec!["session_test_1".to_string(), "session_test_2".to_string()],
        display_names: vec!["fox".to_string(), "oak".to_string()],
        most_recent_crash: Utc::now(),
        omitted_crashed_count: 0,
    };
    assert_eq!(info.session_ids.len(), 2);
    assert_eq!(info.display_names.len(), 2);
    assert_eq!(info.display_names[0], "fox");
}

#[test]
fn idle_dead_pid_without_streaming_marker_is_not_a_crash() {
    // Upstream #988 extended to the PID detector: a dead owner PID with
    // no leftover streaming marker means the session was idle — ordinary
    // close, not crash. Daemon restarts must not stamp idle sessions.
    let _guard = crate::storage::lock_test_env();
    let temp = tempfile::tempdir().expect("tempdir");
    crate::env::set_var("JCODE_HOME", temp.path());

    let dir = storage::streaming_pids_dir().expect("streaming dir");
    std::fs::create_dir_all(&dir).expect("marker dir");
    assert!(
        !had_streaming_marker("session_idle_1"),
        "no marker file means idle"
    );

    crate::env::remove_var("JCODE_HOME");
}

#[test]
fn leftover_streaming_marker_means_interrupted() {
    // A marker file left behind (StreamingGuard clears it on every
    // orderly exit) proves the owner died mid-turn — genuine crash.
    let _guard = crate::storage::lock_test_env();
    let temp = tempfile::tempdir().expect("tempdir");
    crate::env::set_var("JCODE_HOME", temp.path());

    let dir = storage::streaming_pids_dir().expect("streaming dir");
    std::fs::create_dir_all(&dir).expect("marker dir");
    std::fs::write(dir.join("session_midturn_1"), "999999").expect("marker");
    assert!(
        had_streaming_marker("session_midturn_1"),
        "leftover marker means interrupted"
    );
    assert!(
        !had_streaming_marker("session_other_1"),
        "other sessions unaffected"
    );

    crate::env::remove_var("JCODE_HOME");
}

#[test]
fn exact_resume_resolution_defers_corruption_to_authoritative_load() -> anyhow::Result<()> {
    let _guard = crate::storage::lock_test_env();
    let temp = tempfile::tempdir()?;
    let _home = HomeGuard::set(temp.path());
    std::fs::create_dir_all(temp.path().join("sessions"))?;
    for id in ["session_corrupt_1770000000000", "imported_codex_corrupt"] {
        let path = session_path(id)?;
        std::fs::write(&path, b"not JSON")?;
        assert_eq!(find_session_by_name_or_id(id)?, id);
        assert!(
            Session::load(id).is_err(),
            "authoritative load must reject corruption"
        );
        assert_eq!(std::fs::read(&path)?, b"not JSON");
    }
    Ok(())
}

#[test]
fn exact_resume_resolution_rejects_paths_and_directories() -> anyhow::Result<()> {
    let _guard = crate::storage::lock_test_env();
    let temp = tempfile::tempdir()?;
    let _home = HomeGuard::set(temp.path());
    std::fs::create_dir_all(temp.path().join("sessions"))?;
    std::fs::write(temp.path().join("outside.json"), b"{}")?;
    std::fs::create_dir(temp.path().join("sessions/session_directory_1770000000000"))?;
    std::fs::create_dir(
        temp.path()
            .join("sessions/session_directory_1770000000000.json"),
    )?;
    for query in [
        "../outside".to_string(),
        temp.path().join("outside").to_string_lossy().into_owned(),
        "..\\outside".to_string(),
        "session_directory_1770000000000".to_string(),
    ] {
        assert!(
            find_session_by_name_or_id(&query).is_err(),
            "accepted {query:?}"
        );
    }
    Ok(())
}

#[test]
fn exact_resume_resolution_preserves_journal_freshness() -> anyhow::Result<()> {
    let _guard = crate::storage::lock_test_env();
    let temp = tempfile::tempdir()?;
    let _home = HomeGuard::set(temp.path());
    let id = "session_resume_journal_1770000000000";
    let mut session = Session::create_with_id(id.to_string(), None, None);
    session.add_message(
        Role::User,
        vec![ContentBlock::Text {
            text: "x".repeat(1024 * 1024),
            cache_control: None,
        }],
    );
    session.save()?;
    let snapshot = std::fs::read(session_path(id)?)?;
    session.custom_title = Some("Newest journal title".to_string());
    session.status = SessionStatus::Closed;
    session.save()?;
    assert_eq!(std::fs::read(session_path(id)?)?, snapshot);
    assert_eq!(find_session_by_name_or_id(id)?, id);
    let loaded = Session::load(id)?;
    assert_eq!(loaded.custom_title.as_deref(), Some("Newest journal title"));
    assert!(matches!(loaded.status, SessionStatus::Closed));
    let stub = Session::load_startup_stub(id)?;
    assert_eq!(stub.custom_title, loaded.custom_title);
    assert_eq!(find_session_by_name_or_id("Newest journal title")?, id);
    Ok(())
}

#[test]
#[ignore = "read-only real local corpus benchmark, requires JCODE_RESUME_BENCH_IDS"]
fn benchmark_exact_resume_resolution_real_corpus() -> anyhow::Result<()> {
    // Run alone with --local and --ignored --nocapture. Never save, restore,
    // clear caches, or call the full loader (which may recover a backup).
    // This measures resolution helpers only, NOT end-to-end TUI startup.
    let ids = std::env::var("JCODE_RESUME_BENCH_IDS")?;
    for id in ids.split(',') {
        let path = session_path(id)?;
        let before = std::fs::metadata(&path)?;
        let mut old_us = Vec::new();
        let mut new_us = Vec::new();
        for _ in 0..9 {
            let start = std::time::Instant::now();
            assert!(Session::load_startup_stub(id).is_ok());
            old_us.push(start.elapsed().as_micros());
            let start = std::time::Instant::now();
            assert_eq!(find_session_by_name_or_id(id)?, id);
            new_us.push(start.elapsed().as_micros());
        }
        let after = std::fs::metadata(&path)?;
        assert_eq!(before.len(), after.len());
        assert_eq!(before.modified()?, after.modified()?);
        let first_baseline_us = old_us[0];
        old_us.sort_unstable();
        new_us.sort_unstable();
        println!(
            "resolution_helper id={id} bytes={} samples=9 baseline_stub_us_p50={} pmax={} exact_stat_us_p50={} pmax={} first_baseline_us={} cache=uncontrolled_no_eviction",
            before.len(),
            old_us[4],
            old_us[8],
            new_us[4],
            new_us[8],
            first_baseline_us
        );
    }
    Ok(())
}

#[test]
fn short_resume_selects_latest_update_not_latest_filename() -> anyhow::Result<()> {
    let _guard = crate::storage::lock_test_env();
    let temp = tempfile::tempdir()?;
    let _home = HomeGuard::set(temp.path());
    let older_id = "session_resumeprobe_1770000000000_aaaaaaaaaaaaaaaa";
    let newer_id = "session_resumeprobe_1770000000001_bbbbbbbbbbbbbbbb";
    let mut older = Session::create_with_id(older_id.to_string(), None, None);
    let mut newer = Session::create_with_id(newer_id.to_string(), None, None);
    for session in [&mut older, &mut newer] {
        session.add_message(
            Role::User,
            vec![ContentBlock::Text {
                text: "Resume regression fixture".into(),
                cache_control: None,
            }],
        );
        session.save()?;
        assert!(session_path(&session.id)?.is_file());
    }
    // Saving the older-created session makes it the most recently updated.
    older.custom_title = Some("Most recently used".into());
    older.save()?;
    assert_eq!(find_session_by_name_or_id("resumeprobe")?, older_id);
    assert_eq!(find_session_by_metadata("resumeprobe")?, older_id);
    assert_eq!(find_session_by_name_or_id("most recently used")?, older_id);
    Ok(())
}

#[test]
fn short_resume_ignores_unrelated_snapshot_contents() -> anyhow::Result<()> {
    let _guard = crate::storage::lock_test_env();
    let temp = tempfile::tempdir()?;
    let _home = HomeGuard::set(temp.path());
    let id = "session_resumeprobe_1770000000000_aaaaaaaaaaaaaaaa";
    let mut session = Session::create_with_id(id.to_string(), None, None);
    session.add_message(
        Role::User,
        vec![ContentBlock::Text {
            text: "Resume regression fixture".into(),
            cache_control: None,
        }],
    );
    session.save()?;
    std::fs::write(
        session_path("session_unrelated_1770000000000")?,
        b"not json",
    )?;
    assert_eq!(find_session_by_name_or_id("resumeprobe")?, id);
    Ok(())
}

#[test]
#[ignore = "local corpus benchmark, writes disposable lookup cache, requires JCODE_RESUME_BENCH_NAME"]
fn benchmark_short_resume_resolution_real_corpus() -> anyhow::Result<()> {
    let name = std::env::var("JCODE_RESUME_BENCH_NAME")?;
    for _ in 0..5 {
        let start = std::time::Instant::now();
        let baseline = find_session_by_metadata_impl(&name, false)?;
        let baseline_ms = start.elapsed().as_secs_f64() * 1000.0;
        let start = std::time::Instant::now();
        let optimized = find_session_by_name_or_id(&name)?;
        let optimized_ms = start.elapsed().as_secs_f64() * 1000.0;
        assert_eq!(baseline, optimized);
        println!(
            "short_resume baseline_ms={baseline_ms:.3} optimized_ms={optimized_ms:.3} cache=uncontrolled_no_eviction"
        );
    }
    Ok(())
}

#[test]
fn title_resume_cache_tracks_changes_and_recovers_from_corruption() -> anyhow::Result<()> {
    let _guard = crate::storage::lock_test_env();
    let temp = tempfile::tempdir()?;
    let _home = HomeGuard::set(temp.path());
    let id = "session_cacheprobe_1770000000000_aaaaaaaaaaaaaaaa";
    let mut session = Session::create_with_id(id.into(), None, Some("Original title".into()));
    session.save()?;
    for _ in 0..2 {
        assert_eq!(find_session_by_name_or_id("original TITLE")?, id);
    }
    session.rename_title(Some("Updated title with different length".into()));
    session.save()?;
    assert!(find_session_by_name_or_id("Original title").is_err());
    assert_eq!(find_session_by_name_or_id("updated   TITLE")?, id);
    let index = rusqlite::Connection::open(temp.path().join("session-metadata-v1.sqlite3"))?;
    index.execute(
        "UPDATE recent_sessions SET resume_fingerprint = 'broken cache'",
        [],
    )?;
    assert_eq!(find_session_by_name_or_id("Updated title")?, id);
    std::fs::remove_file(session_path(id)?)?;
    assert!(find_session_by_name_or_id("Updated title").is_err());
    assert!(crate::recent_session_index::resume_entries()?.is_empty());
    assert!(!temp.path().join("resume-metadata-v1.json").exists());
    Ok(())
}

#[test]
fn title_resume_works_when_sqlite_is_unavailable() -> anyhow::Result<()> {
    let _guard = crate::storage::lock_test_env();
    let temp = tempfile::tempdir()?;
    let _home = HomeGuard::set(temp.path());
    std::fs::create_dir(temp.path().join("session-metadata-v1.sqlite3"))?;
    let id = "session_unavailable_1770000000000_aaaaaaaaaaaaaaaa";
    let mut session =
        Session::create_with_id(id.into(), None, Some("Unavailable index title".into()));
    session.save()?;
    assert_eq!(find_session_by_name_or_id("Unavailable index")?, id);
    assert!(!temp.path().join("resume-metadata-v1.json").exists());
    Ok(())
}

#[test]
fn title_resume_backfills_old_files_and_checks_partial_indexes() -> anyhow::Result<()> {
    let _guard = crate::storage::lock_test_env();
    let temp = tempfile::tempdir()?;
    let _home = HomeGuard::set(temp.path());
    let mut older = Session::create_with_id(
        "legacy_indexed".into(),
        None,
        Some("Shared legacy title".into()),
    );
    older.save()?;
    let mut newest = Session::create_with_id(
        "legacy_unindexed".into(),
        None,
        Some("Shared legacy title".into()),
    );
    newest.save()?;
    let index = rusqlite::Connection::open(temp.path().join("session-metadata-v1.sqlite3"))?;
    index.execute(
        "DELETE FROM recent_sessions WHERE session_id = ?1",
        [&newest.id],
    )?;
    // A matching indexed row cannot hide a newer unindexed match.
    assert_eq!(find_session_by_name_or_id("Shared legacy")?, newest.id);
    assert!(crate::recent_session_index::resume_entries()?.contains_key(&newest.id));
    index.execute("UPDATE recent_sessions SET resume_fingerprint = NULL", [])?;
    super::super::resume_cache::backfill()?;
    assert_eq!(crate::recent_session_index::resume_entries()?.len(), 2);
    assert_eq!(find_session_by_name_or_id("Shared legacy")?, newest.id);
    Ok(())
}

#[test]
fn title_resume_cache_invalidates_journal_only_updates() -> anyhow::Result<()> {
    let _guard = crate::storage::lock_test_env();
    let temp = tempfile::tempdir()?;
    let _home = HomeGuard::set(temp.path());
    let id = "session_journalprobe_1770000000000_aaaaaaaaaaaaaaaa";
    let mut session =
        Session::create_with_id(id.into(), None, Some("Before journal rename".into()));
    session.add_message(
        Role::User,
        vec![ContentBlock::Text {
            text: "x".repeat(2 * 1024 * 1024),
            cache_control: None,
        }],
    );
    session.save()?;
    assert_eq!(find_session_by_name_or_id("Before journal")?, id);
    let snapshot = std::fs::read(session_path(id)?)?;
    session.rename_title(Some("After journal rename".into()));
    session.save()?;
    assert_eq!(std::fs::read(session_path(id)?)?, snapshot);
    assert_eq!(find_session_by_name_or_id("After journal")?, id);
    assert!(find_session_by_name_or_id("Before journal").is_err());
    Ok(())
}

#[test]
fn title_resume_cache_preserves_newest_and_legacy_name_priority() -> anyhow::Result<()> {
    let _guard = crate::storage::lock_test_env();
    let temp = tempfile::tempdir()?;
    let _home = HomeGuard::set(temp.path());
    let mut older =
        Session::create_with_id("legacy_older".into(), None, Some("Shared title".into()));
    older.updated_at = Utc::now() - chrono::Duration::days(1);
    older.save()?;
    let mut newer =
        Session::create_with_id("legacy_newer".into(), None, Some("Shared title".into()));
    newer.save()?;
    for _ in 0..2 {
        assert_eq!(find_session_by_name_or_id("Shared title")?, newer.id);
        assert_eq!(
            find_session_by_metadata_impl("Shared title", false)?,
            newer.id
        );
    }
    older.short_name = Some("Shared title".into());
    older.save()?;
    assert_eq!(find_session_by_name_or_id("Shared title")?, older.id);
    Ok(())
}

#[test]
fn find_session_by_name_or_id_matches_custom_title() {
    let _guard = crate::storage::lock_test_env();
    let temp = tempfile::tempdir().expect("tempdir");
    crate::env::set_var("JCODE_HOME", temp.path());

    let session_id = "session_renamecli_1770000000000";
    let mut session = Session::create_with_id(
        session_id.to_string(),
        None,
        Some("Generated planning title".to_string()),
    );
    session.status = SessionStatus::Closed;
    session.rename_title(Some("RenameTest".to_string()));
    session.save().expect("save renamed session");

    assert_eq!(
        find_session_by_name_or_id("renametest").expect("resolve custom title"),
        session_id
    );
    assert_eq!(
        find_session_by_name_or_id("Rename").expect("resolve title fragment"),
        session_id
    );

    crate::env::remove_var("JCODE_HOME");
}

#[test]
fn find_session_by_name_or_id_accepts_imported_session_ids() -> anyhow::Result<()> {
    let _guard = crate::storage::lock_test_env();
    let temp = tempfile::tempdir()?;
    crate::env::set_var("JCODE_HOME", temp.path());

    let imported_id = "imported_codex_test_resume";
    let mut session =
        Session::create_with_id(imported_id.to_string(), None, Some("Imported".to_string()));
    session.status = SessionStatus::Closed;
    session.save()?;

    let resolved = find_session_by_name_or_id(imported_id)?;
    assert_eq!(resolved, imported_id);

    crate::env::remove_var("JCODE_HOME");
    Ok(())
}

/// Regression test for issue #336: resuming a *bare* external provider id
/// (e.g. an OpenCode `ses_...`) must resolve to its already-imported local
/// snapshot even when the external tool's storage has been removed, instead
/// of hard-failing with "No session found matching 'ses_...'".
#[test]
fn find_session_by_name_or_id_resolves_bare_opencode_id_to_imported_snapshot() -> anyhow::Result<()>
{
    let _guard = crate::storage::lock_test_env();
    let temp = tempfile::tempdir()?;
    crate::env::set_var("JCODE_HOME", temp.path());

    let provider_id = "ses_2c72f8f4cffee6Qh7GId7D81Se";
    let imported_id = crate::import::imported_opencode_session_id(provider_id);
    let mut session = Session::create_with_id(
        imported_id.clone(),
        None,
        Some("Analyzing /provider routing".to_string()),
    );
    session.status = SessionStatus::Closed;
    session.provider_session_id = Some(provider_id.to_string());
    session.provider_key = Some("opencode".to_string());
    session.save()?;

    // No external OpenCode store exists under JCODE_HOME/external, so the only
    // way to resolve the bare id is via the local imported snapshot.
    let resolved = find_session_by_name_or_id(provider_id)?;
    assert_eq!(resolved, imported_id);

    crate::env::remove_var("JCODE_HOME");
    Ok(())
}

/// A bare provider id that has *no* imported snapshot on disk must still fail
/// (so the caller can fall back to external re-import or surface an error).
#[test]
fn find_session_by_name_or_id_bare_id_without_snapshot_still_errors() -> anyhow::Result<()> {
    let _guard = crate::storage::lock_test_env();
    let temp = tempfile::tempdir()?;
    crate::env::set_var("JCODE_HOME", temp.path());
    std::fs::create_dir_all(temp.path().join("sessions"))?;

    let err = find_session_by_name_or_id("ses_does_not_exist_anywhere");
    assert!(err.is_err(), "expected unknown bare id to error");

    crate::env::remove_var("JCODE_HOME");
    Ok(())
}
