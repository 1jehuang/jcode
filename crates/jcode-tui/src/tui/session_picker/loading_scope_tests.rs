//! Focused loading scope regression tests.
use super::*;

#[test]
fn invalidated_in_flight_load_does_not_repopulate_session_list_cache() {
    let _env_lock = crate::storage::lock_test_env();
    let temp = tempfile::tempdir().expect("temp dir");
    let _home = EnvVarGuard::set_path("JCODE_HOME", temp.path());
    let _limit = EnvVarGuard::set_str("JCODE_SESSION_PICKER_MAX_SESSIONS", "100");

    std::fs::create_dir_all(temp.path().join("sessions")).expect("create sessions dir");

    let save_session = |idx: usize| {
        let mut session = Session::create_with_id(
            format!("session_race_{idx:03}_1780000000{idx:03}"),
            Some(format!("/tmp/session-race-{idx}")),
            Some(format!("Race Session {idx}")),
        );
        session.append_stored_message(crate::session::StoredMessage {
            id: format!("msg-{idx}"),
            role: crate::message::Role::User,
            content: vec![crate::message::ContentBlock::Text {
                text: format!("race session {idx}"),
                cache_control: None,
            }],
            display_role: None,
            timestamp: None,
            tool_duration_ms: None,
            token_usage: None,
        });
        session.save().expect("save race session");
    };

    save_session(0);
    invalidate_session_list_cache();

    let (publish_reached_tx, publish_reached_rx) = mpsc::channel();
    let (release_publish_tx, release_publish_rx) = mpsc::channel();
    let hook_used = Arc::new(AtomicBool::new(false));
    let hook_used_for_hook = Arc::clone(&hook_used);
    let loader_thread_id = Arc::new(OnceLock::new());
    let loader_thread_id_for_hook = Arc::clone(&loader_thread_id);
    set_before_session_list_cache_publish_hook(Some(Box::new(move || {
        let is_target_loader = loader_thread_id_for_hook
            .get()
            .is_some_and(|thread_id| *thread_id == std::thread::current().id());
        if is_target_loader
            && hook_used_for_hook
                .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
                .is_ok()
        {
            publish_reached_tx.send(()).expect("signal publish reached");
            release_publish_rx.recv().expect("wait for release");
        }
    })));

    let loader_thread_id_for_loader = Arc::clone(&loader_thread_id);
    let loader = std::thread::spawn(move || {
        loader_thread_id_for_loader
            .set(std::thread::current().id())
            .expect("record loader thread");
        // Keep the target load in flight even if a detached grouped load has
        // already published an entry for this temporary home. The regression
        // under test is publication after invalidation, not cache-hit ordering.
        load_sessions_inner(true)
    });
    publish_reached_rx
        .recv_timeout(std::time::Duration::from_secs(5))
        .expect("in-flight load reached cache publication");

    for idx in 1..4 {
        save_session(idx);
    }
    invalidate_session_list_cache();
    release_publish_tx.send(()).expect("release in-flight load");

    let first_load = loader
        .join()
        .expect("loader thread joins")
        .expect("in-flight load succeeds");
    assert_eq!(first_load.len(), 1, "in-flight load saw initial snapshot");

    set_before_session_list_cache_publish_hook(None);
    let second_load = load_sessions().expect("load after invalidation");
    assert_eq!(
        second_load.len(),
        4,
        "stale in-flight result must not repopulate the cache after invalidation"
    );
}

#[test]
fn onboarding_scoped_loader_returns_only_codex_sessions() {
    use crate::tui::app::onboarding_flow::ExternalCli;
    let _env_lock = crate::storage::lock_test_env();
    let temp = tempfile::tempdir().expect("temp dir");
    let _home = EnvVarGuard::set_path("JCODE_HOME", temp.path());

    // A Codex transcript that the onboarding picker should surface.
    let codex_dir = temp.path().join("external/.codex/sessions/2026/05/01");
    std::fs::create_dir_all(&codex_dir).expect("create codex dir");
    std::fs::write(
        codex_dir.join("rollout-2026-05-01T10-00-00-test.jsonl"),
        "{\"timestamp\":\"2026-05-01T10:00:00Z\",\"type\":\"session_meta\",\"payload\":{\"id\":\"codex-onboarding-test\",\"timestamp\":\"2026-05-01T09:59:00Z\",\"cwd\":\"/tmp/codex-onboard\"}}\n",
    )
    .expect("write codex transcript");

    // A jcode session that must NOT appear in the scoped Codex view (the whole
    // point of the scoped loader is to skip parsing these on onboarding).
    let mut jcode_session = Session::create_with_id(
        "session_onboarding_jcode_1780000000000".to_string(),
        Some("/tmp/jcode-onboard".to_string()),
        Some("Jcode Onboarding".to_string()),
    );
    jcode_session.append_stored_message(crate::session::StoredMessage {
        id: "msg-1".to_string(),
        role: crate::message::Role::User,
        content: vec![crate::message::ContentBlock::Text {
            text: "should not show in codex onboarding view".to_string(),
            cache_control: None,
        }],
        display_role: None,
        timestamp: None,
        tool_duration_ms: None,
        token_usage: None,
    });
    jcode_session.save().expect("save jcode session");

    let (groups, orphans) = load_external_cli_sessions_grouped(ExternalCli::Codex);
    assert!(groups.is_empty(), "scoped loader produces only orphans");
    assert!(
        orphans
            .iter()
            .any(|s| s.id == "codex:codex-onboarding-test"),
        "expected codex transcript in scoped onboarding load: {:?}",
        orphans.iter().map(|s| &s.id).collect::<Vec<_>>()
    );
    assert!(
        orphans
            .iter()
            .all(|s| matches!(s.resume_target, ResumeTarget::CodexSession { .. })),
        "scoped Codex load must not include jcode/other-CLI sessions"
    );
}

#[test]
fn parallel_fill_skips_many_recent_empty_sessions_to_reach_scan_limit() {
    let _env_lock = crate::storage::lock_test_env();
    let temp = tempfile::tempdir().expect("temp dir");
    let _home = EnvVarGuard::set_path("JCODE_HOME", temp.path());
    let _scan_limit = EnvVarGuard::set_str("JCODE_SESSION_PICKER_MAX_SESSIONS", "50");

    let sessions_dir = temp.path().join("sessions");
    std::fs::create_dir_all(&sessions_dir).expect("create sessions dir");

    let push_message = |session: &mut Session, text: &str| {
        session.append_stored_message(crate::session::StoredMessage {
            id: format!("msg-{text}"),
            role: crate::message::Role::User,
            content: vec![crate::message::ContentBlock::Text {
                text: text.to_string(),
                cache_control: None,
            }],
            display_role: None,
            timestamp: None,
            tool_duration_ms: None,
            token_usage: None,
        });
    };

    // Many recent but empty sessions (no visible messages) that the parallel
    // two-phase fill must skip while still collecting `scan_limit` real ones.
    for idx in 0..200 {
        let mut session = Session::create_with_id(
            format!("session_empty_{}", 1_790_000_000_000u64 + idx as u64),
            Some(format!("/tmp/empty-{idx:03}")),
            Some(format!("Empty {idx:03}")),
        );
        session.save().expect("save empty session");
    }
    // Older but non-empty sessions that should fill the list despite being less
    // recent than the empty stubs above.
    for idx in 0..60 {
        let mut session = Session::create_with_id(
            format!("session_full_{}", 1_780_000_000_000u64 + idx as u64),
            Some(format!("/tmp/full-{idx:03}")),
            Some(format!("Full {idx:03}")),
        );
        push_message(&mut session, &format!("real content {idx:03}"));
        session.save().expect("save full session");
    }

    invalidate_session_list_cache();
    let sessions = load_sessions().expect("load sessions");
    let visible: Vec<&SessionInfo> = sessions
        .iter()
        .filter(|s| s.id.starts_with("session_full_"))
        .collect();
    assert_eq!(
        visible.len(),
        50,
        "expected exactly scan_limit non-empty sessions, got {}",
        visible.len()
    );
    assert!(
        !sessions.iter().any(|s| s.id.starts_with("session_empty_")),
        "empty sessions must be filtered out of the loaded list"
    );
}

#[test]
fn hidden_debug_sessions_do_not_consume_default_resume_budget() {
    let _env_lock = crate::storage::lock_test_env();
    let temp = tempfile::tempdir().expect("temp dir");
    let _home = EnvVarGuard::set_path("JCODE_HOME", temp.path());
    let _scan_limit = EnvVarGuard::set_str("JCODE_SESSION_PICKER_MAX_SESSIONS", "50");

    let push_message = |session: &mut Session, text: &str| {
        session.append_stored_message(crate::session::StoredMessage {
            id: format!("msg-{text}"),
            role: crate::message::Role::User,
            content: vec![crate::message::ContentBlock::Text {
                text: text.to_string(),
                cache_control: None,
            }],
            display_role: None,
            timestamp: None,
            tool_duration_ms: None,
            token_usage: None,
        });
    };

    // Write ordinary sessions first so their filesystem mtimes are older than
    // the hidden debug burst below, matching the reported real-world ordering.
    for idx in 0..60 {
        let mut session = Session::create_with_id(
            format!("session_regular_{}", 1_780_000_000_000u64 + idx as u64),
            Some(format!("/tmp/regular-{idx:03}")),
            Some(format!("Regular {idx:03}")),
        );
        session.is_debug = false;
        session.is_canary = false;
        push_message(&mut session, &format!("regular content {idx:03}"));
        session.save().expect("save regular session");
    }

    // These newer self-dev/worker sessions are hidden by default. Previously the
    // loader stopped after the first 50, leaving no ordinary Jcode sessions for
    // the picker even though older resumable sessions existed.
    for idx in 0..75 {
        let mut session = Session::create_with_id(
            format!("session_debug_{}", 1_790_000_000_000u64 + idx as u64),
            Some(format!("/tmp/debug-{idx:03}")),
            Some(format!("Debug {idx:03}")),
        );
        session.is_debug = true;
        push_message(&mut session, &format!("debug content {idx:03}"));
        session.save().expect("save debug session");
    }

    invalidate_session_list_cache();
    let sessions = load_sessions().expect("load sessions");
    let regular_count = sessions.iter().filter(|session| !session.is_debug).count();
    let debug_count = sessions.iter().filter(|session| session.is_debug).count();

    assert_eq!(
        regular_count, 50,
        "ordinary sessions should fill the visible budget"
    );
    assert_eq!(
        debug_count, 50,
        "debug sessions should retain their own bounded budget"
    );
}

#[test]
fn session_matches_picker_query_requires_all_tokens_order_independent() {
    let _env_lock = crate::storage::lock_test_env();
    let temp = tempfile::tempdir().expect("temp dir");
    let _home = EnvVarGuard::set_path("JCODE_HOME", temp.path());

    let mut session = Session::create_with_id(
        "session_token_match".to_string(),
        Some("/tmp/token-match".to_string()),
        Some("Token Match".to_string()),
    );
    session.append_stored_message(crate::session::StoredMessage {
        id: "msg1".to_string(),
        role: crate::message::Role::User,
        content: vec![crate::message::ContentBlock::Text {
            text: "please deploy the production api gateway now".to_string(),
            cache_control: None,
        }],
        display_role: None,
        timestamp: None,
        tool_duration_ms: None,
        token_usage: None,
    });
    session.save().expect("save session");

    let sessions = load_sessions().expect("load sessions");
    let loaded = sessions
        .iter()
        .find(|candidate| candidate.id == "session_token_match")
        .expect("session present");

    // All tokens present, any order -> match (the old contiguous-substring matcher
    // would have failed on reordered / non-adjacent words).
    assert!(session_matches_picker_query(loaded, "api deploy"));
    assert!(session_matches_picker_query(loaded, "deploy api"));
    assert!(session_matches_picker_query(loaded, "  DEPLOY   Gateway  "));
    // A token that doesn't appear anywhere -> no match, even if others do.
    assert!(!session_matches_picker_query(loaded, "deploy staging"));
    // Empty query matches everything.
    assert!(session_matches_picker_query(loaded, "   "));
}

/// Regression tests for issue #674: the picker must be able to list only
/// jcode's own sessions, and toggling that must not be masked by either cache.
mod external_session_opt_out {
    use super::super::super::{GroupedSessionListDiskCache, session_list_disk_cache_is_usable};
    use std::path::{Path, PathBuf};

    fn disk_cache(dir: &Path, external_sessions: bool) -> GroupedSessionListDiskCache {
        GroupedSessionListDiskCache {
            version: super::super::super::SESSION_LIST_DISK_CACHE_VERSION,
            generated_at: chrono::Utc::now(),
            sessions_dir: dir.to_path_buf(),
            scan_limit: 50,
            include_old_saved_sessions:
                super::super::super::include_old_saved_sessions_on_initial_load(),
            external_sessions,
            server_groups: Vec::new(),
            orphan_sessions: Vec::new(),
        }
    }

    #[test]
    fn disk_cache_written_with_externals_is_rejected_after_opting_out() {
        let dir = PathBuf::from("/tmp/jcode-test-sessions");
        let cache = disk_cache(&dir, true);

        assert!(
            session_list_disk_cache_is_usable(&cache, &dir, 50, true),
            "same setting -> reusable"
        );
        assert!(
            !session_list_disk_cache_is_usable(&cache, &dir, 50, false),
            "opting out must not be served a cache that still contains other CLIs' sessions"
        );
    }

    #[test]
    fn disk_cache_written_without_externals_is_rejected_after_opting_back_in() {
        let dir = PathBuf::from("/tmp/jcode-test-sessions");
        let cache = disk_cache(&dir, false);

        assert!(session_list_disk_cache_is_usable(&cache, &dir, 50, false));
        assert!(
            !session_list_disk_cache_is_usable(&cache, &dir, 50, true),
            "opting back in must trigger a rescan"
        );
    }

    /// An older cache file has no `external_sessions` field; it was written
    /// with externals included, so it must deserialize that way.
    #[test]
    fn legacy_disk_cache_without_the_field_defaults_to_externals_included() {
        let json = serde_json::json!({
            "version": super::super::super::SESSION_LIST_DISK_CACHE_VERSION,
            "generated_at": chrono::Utc::now(),
            "sessions_dir": "/tmp/jcode-test-sessions",
            "scan_limit": 50,
            "include_old_saved_sessions": false,
            "server_groups": [],
            "orphan_sessions": []
        });
        let cache: GroupedSessionListDiskCache =
            serde_json::from_value(json).expect("legacy cache must still parse");
        assert!(cache.external_sessions);
    }

    /// The config default must keep today's behavior (externals shown).
    #[test]
    fn external_sessions_default_is_on() {
        assert!(jcode_config_types::DisplayConfig::default().external_sessions);
    }
}
