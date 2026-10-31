use super::super::{SessionPicker, SessionSource, SessionStatus};
use super::*;
use crossterm::event::{KeyCode, KeyModifiers};

fn external_session(id: &str, path: &Path) -> SessionInfo {
    let now = chrono::Utc::now();
    SessionInfo {
        id: id.into(),
        parent_id: None,
        short_name: id.into(),
        icon: "x".into(),
        title: "Metadata title".into(),
        message_count: 1,
        user_message_count: 1,
        assistant_message_count: 0,
        created_at: now,
        last_message_time: now,
        last_active_at: None,
        working_dir: None,
        model: None,
        provider_key: None,
        is_canary: false,
        is_debug: false,
        saved: false,
        save_label: None,
        status: SessionStatus::Closed,
        needs_catchup: false,
        estimated_tokens: 1,
        first_user_prompt: None,
        messages_preview: Vec::new(),
        search_index: "metadata title".into(),
        server_name: None,
        server_icon: None,
        source: SessionSource::Codex,
        resume_target: ResumeTarget::CodexSession {
            session_id: id.into(),
            session_path: path.to_string_lossy().into_owned(),
        },
        external_path: Some(path.to_string_lossy().into_owned()),
    }
}

fn wait_for_search(picker: &mut SessionPicker) {
    if picker.transcript_search.started_at.is_some() {
        picker.transcript_search.started_at = Some(Instant::now() - DEBOUNCE);
    }
    let deadline = Instant::now() + Duration::from_secs(5);
    while picker.transcript_search.pending() {
        assert!(Instant::now() < deadline, "transcript search timed out");
        picker.poll_transcript_search();
        std::thread::sleep(Duration::from_millis(1));
    }
}

#[test]
fn transcript_search_tolerates_transposed_query_words() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("session.json");
    std::fs::write(
        &path,
        r#"{"messages":[{"text":"cloud acceptance"},{"text":"todo list"}]}"#,
    )
    .unwrap();
    assert!(paths_match(
        &[path.clone()],
        &search_query_tokens("clodu tood"),
        &AtomicBool::new(false),
    ));
    assert!(!paths_match(
        &[path],
        &search_query_tokens("clodu absent"),
        &AtomicBool::new(false),
    ));
}

#[test]
fn full_text_matches_tokens_in_any_order_across_snapshot_and_journal() {
    let temp = tempfile::tempdir().unwrap();
    let snapshot = temp.path().join("snapshot.json");
    let journal = temp.path().join("journal.jsonl");
    std::fs::write(&snapshot, r#"{"messages":[{"text":"DEPLOY the API"}]}"#).unwrap();
    std::fs::write(
        &journal,
        "{\"text\":\"other turn\"}\n{\"text\":\"caf\\u00e9\\nworks\"}\n",
    )
    .unwrap();
    let tokens = search_query_tokens("café api deploy");
    assert!(paths_match(
        &[snapshot.clone(), journal.clone()],
        &tokens,
        &AtomicBool::new(false)
    ));
    assert!(!paths_match(
        &[snapshot, journal],
        &search_query_tokens("missing api"),
        &AtomicBool::new(false)
    ));
}

#[test]
fn full_text_ignores_json_keys_and_cancellation_stops_scan() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("snapshot.json");
    std::fs::write(
        &path,
        r#"{"notprose":"real content","nested":[true,42,null,1.5]}"#,
    )
    .unwrap();
    assert!(!paths_match(
        &[path.clone()],
        &search_query_tokens("notprose"),
        &AtomicBool::new(false)
    ));
    assert!(!paths_match(
        &[path.clone()],
        &search_query_tokens("real"),
        &AtomicBool::new(true)
    ));
    assert!(paths_match(
        &[path],
        &search_query_tokens("real"),
        &AtomicBool::new(false)
    ));
}

#[test]
fn picker_finds_unsampled_middle_without_loading_preview() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("transcript.jsonl");
    let text = format!(
        "{} middleonlyplatypus {}",
        "x".repeat(80_000),
        "y".repeat(80_000)
    );
    std::fs::write(&path, serde_json::json!({"content":text}).to_string()).unwrap();
    let session = external_session("middle", &path);
    let mut picker = SessionPicker::new(vec![session]);
    picker.search_query = "MIDDLEONLYPLATYPUS title".into();
    picker.rebuild_items();
    assert!(
        picker.visible_sessions.is_empty(),
        "sample must not already contain the needle"
    );
    assert!(picker.pending_preview_load.is_none());
    wait_for_search(&mut picker);
    assert_eq!(picker.visible_sessions.len(), 1);
    assert_eq!(picker.selected_session().unwrap().id, "middle");
    assert!(picker.pending_preview_load.is_none());
}

#[test]
fn changed_query_cancels_old_worker_and_ignores_its_results() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("transcript.jsonl");
    std::fs::write(&path, r#"{"content":"oldneedle newneedle"}"#).unwrap();
    let mut picker = SessionPicker::new(vec![external_session("one", &path)]);
    picker.search_query = "oldneedle".into();
    picker.rebuild_items();
    picker.transcript_search.started_at = Some(Instant::now() - DEBOUNCE);
    picker.poll_transcript_search();
    let cancelled = picker.transcript_search.cancel.clone();
    picker.search_query = "absentneedle".into();
    picker.rebuild_items();
    assert!(cancelled.load(Ordering::Relaxed));
    wait_for_search(&mut picker);
    assert!(picker.visible_sessions.is_empty());
    picker.search_query.clear();
    picker.rebuild_items();
    assert!(!picker.transcript_search.pending());
    assert_eq!(picker.visible_sessions.len(), 1);
}

#[test]
fn typing_debounces_disk_work_and_metadata_search_stays_immediate() {
    let temp = tempfile::tempdir().unwrap();
    let session = external_session("one", &temp.path().join("absent.jsonl"));
    let mut picker = SessionPicker::new(vec![session]);
    picker.search_query = "needle".into();
    picker.rebuild_items();
    picker.poll_transcript_search();
    assert!(picker.transcript_search.started_at.is_some());
    assert!(picker.transcript_search.receiver.is_none());
    picker.search_query = "TITLE metadata".into();
    picker.rebuild_items();
    assert_eq!(picker.visible_sessions.len(), 1);
    assert!(!picker.transcript_search.pending());
}

#[test]
fn fuzzy_metadata_matching_is_debounced_and_never_runs_on_keystroke() {
    let temp = tempfile::tempdir().unwrap();
    let mut session = external_session("one", &temp.path().join("absent.jsonl"));
    session.search_index = "config deploy".into();
    let mut picker = SessionPicker::new(vec![session]);
    picker.search_query = "conifg".into();
    picker.rebuild_items();
    assert!(picker.visible_sessions.is_empty());
    assert!(picker.transcript_search.receiver.is_none());
    assert_eq!(picker.transcript_search.candidates.len(), 1);
    assert_eq!(
        picker.transcript_search.candidates[0].missing_tokens,
        ["conifg"]
    );
    wait_for_search(&mut picker);
    assert_eq!(picker.selected_session().unwrap().id, "one");
    assert!(!picker.transcript_search.contains_literal("one"));
}

#[test]
#[ignore = "manual typing latency benchmark, prints old synchronous matching versus key handling"]
fn benchmark_resume_search_keystroke_latency() {
    let temp = tempfile::tempdir().unwrap();
    let sessions: Vec<_> = (0..100)
        .map(|i| {
            let mut session =
                external_session(&format!("bench-{i}"), &temp.path().join("absent.jsonl"));
            session.search_index =
                "deployment performance synchronization throughput ".repeat(1400);
            session
        })
        .collect();
    let baseline = Instant::now();
    let tokens = search_query_tokens("modal cli");
    let fuzzy = jcode_fuzzy::PreparedTokenQuery::new("modal cli");
    for session in &sessions {
        for token in &tokens {
            std::hint::black_box(
                jcode_fuzzy::PreparedTokenQuery::new(token)
                    .matches_search_text(&session.search_index),
            );
        }
        std::hint::black_box(fuzzy.matches_search_text(&session.search_index));
    }
    let baseline = baseline.elapsed();
    let mut picker = SessionPicker::new(sessions);
    picker.search_active = true;
    picker.search_query = "modal cl".into();
    let start = Instant::now();
    picker
        .handle_search_key(KeyCode::Char('i'), KeyModifiers::NONE)
        .unwrap();
    let elapsed = start.elapsed();
    assert_eq!(picker.search_query, "modal cli");
    assert!(picker.transcript_search.receiver.is_none());
    eprintln!(
        "100 sessions x 64 KiB: old synchronous matching {baseline:?}, new full key handler {elapsed:?}"
    );
}

#[test]
fn large_history_defers_even_literal_metadata_matching() {
    let temp = tempfile::tempdir().unwrap();
    let mut session = external_session("large", &temp.path().join("absent.jsonl"));
    session.search_index = format!("{} modal cli", "x".repeat(IMMEDIATE_METADATA_BYTES + 1));
    let mut picker = SessionPicker::new(vec![session]);
    picker.search_query = "modal cli".into();
    picker.rebuild_items();
    assert!(picker.transcript_search.defers_metadata());
    assert!(picker.visible_sessions.is_empty());
    assert_eq!(
        picker.transcript_search.candidates[0].missing_tokens,
        ["modal", "cli"]
    );
    wait_for_search(&mut picker);
    assert_eq!(picker.selected_session().unwrap().id, "large");
    assert!(picker.transcript_search.contains_literal("large"));
}

#[test]
fn literal_transcript_hit_outranks_newer_fuzzy_metadata_hit() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("transcript.jsonl");
    std::fs::write(
        &path,
        r#"{"text":"optimizing muse kernels with modal cli"}"#,
    )
    .unwrap();
    let exact = external_session("kernels", &path);
    let mut fuzzy = external_session("unrelated", &temp.path().join("absent.jsonl"));
    fuzzy.search_index = "model cli".into();
    fuzzy.last_message_time = exact.last_message_time + chrono::Duration::seconds(10);
    let mut picker = SessionPicker::new(vec![fuzzy, exact]);
    picker.search_query = "modal cli".into();
    picker.rebuild_items();
    wait_for_search(&mut picker);
    assert_eq!(picker.visible_sessions.len(), 2);
    let first = picker.session_by_ref(picker.visible_sessions[0]).unwrap();
    assert_eq!(first.id, "kernels");
    assert!(picker.transcript_search.contains_literal("kernels"));
    assert!(!picker.transcript_search.contains_literal("unrelated"));
}

#[test]
fn session_refresh_restarts_search_for_new_transcript_paths() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("transcript.jsonl");
    std::fs::write(&path, r#"{"content":"freshneedle"}"#).unwrap();
    let mut picker = SessionPicker::new(vec![]);
    picker.search_query = "freshneedle".into();
    picker.rebuild_items();
    picker.reseed_grouped(vec![], vec![external_session("fresh", &path)]);
    wait_for_search(&mut picker);
    assert_eq!(picker.selected_session().unwrap().id, "fresh");
    assert_eq!(picker.search_query, "freshneedle");
}

#[test]
fn journal_search_skips_corrupt_records_and_missing_files() {
    let temp = tempfile::tempdir().unwrap();
    let journal = temp.path().join("session.journal.jsonl");
    std::fs::write(&journal, "not JSON\n{\"text\":\"laterneedle\"}\n").unwrap();
    assert!(paths_match(
        &[temp.path().join("missing.json"), journal],
        &search_query_tokens("laterneedle"),
        &AtomicBool::new(false),
    ));
}

#[test]
fn failed_snapshot_scan_preserves_matches_and_continues_to_journal() {
    let temp = tempfile::tempdir().unwrap();
    let snapshot = temp.path().join("snapshot.json");
    let journal = temp.path().join("journal.jsonl");
    std::fs::write(&snapshot, r#"{"text":"snapshotneedle", broken}"#).unwrap();
    std::fs::write(&journal, "{\"text\":\"journalneedle\"}\n").unwrap();
    assert!(paths_match(
        &[snapshot, journal.clone()],
        &search_query_tokens("snapshotneedle journalneedle"),
        &AtomicBool::new(false),
    ));
    // Opening a directory either fails immediately or fails on read, depending on OS.
    assert!(paths_match(
        &[temp.path().to_path_buf(), journal],
        &search_query_tokens("journalneedle"),
        &AtomicBool::new(false),
    ));
}

#[test]
fn native_picker_finds_middle_omitted_from_real_list_index() {
    let _lock = crate::storage::lock_test_env();
    let temp = tempfile::tempdir().unwrap();
    struct HomeGuard(Option<std::ffi::OsString>);
    impl Drop for HomeGuard {
        fn drop(&mut self) {
            if let Some(home) = &self.0 {
                crate::env::set_var("JCODE_HOME", home);
            } else {
                crate::env::remove_var("JCODE_HOME");
            }
            super::super::loading::invalidate_session_list_cache();
        }
    }
    let _home = HomeGuard(std::env::var_os("JCODE_HOME"));
    crate::env::set_var("JCODE_HOME", temp.path());
    super::super::loading::invalidate_session_list_cache();
    let id = "session_middleprobe_1770000000000_aaaaaaaaaaaaaaaa";
    let mut session = crate::session::Session::create_with_id(id.into(), None, None);
    // Place the needle outside both the per-message and per-session samples.
    let text = format!(
        "{} nativesearchmiddleplatypus {}",
        "head ".repeat(30_000),
        "tail ".repeat(30_000)
    );
    session.add_message(
        crate::message::Role::User,
        vec![crate::message::ContentBlock::Text {
            text,
            cache_control: None,
        }],
    );
    session.save().unwrap();
    let sessions = super::super::loading::load_sessions().unwrap();
    let info = sessions.into_iter().find(|info| info.id == id).unwrap();
    assert!(!info.search_index.contains("nativesearchmiddleplatypus"));
    let mut picker = SessionPicker::new(vec![info]);
    picker.search_query = "NATIVESEARCHMIDDLEPLATYPUS".into();
    picker.rebuild_items();
    assert!(picker.visible_sessions.is_empty());
    wait_for_search(&mut picker);
    assert_eq!(picker.selected_session().unwrap().id, id);
}

#[test]
fn progressive_results_preserve_selected_preview_scroll() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("transcript.jsonl");
    std::fs::write(&path, r#"{"text":"needle"}"#).unwrap();
    let mut direct = external_session("direct", &path);
    direct.search_index = "needle".into();
    let discovered = external_session("discovered", &path);
    let mut picker = SessionPicker::new(vec![direct, discovered]);
    picker.search_query = "needle".into();
    picker.rebuild_items();
    assert_eq!(picker.selected_session().unwrap().id, "direct");
    picker.scroll_offset = 17;
    picker.auto_scroll_preview = false;
    wait_for_search(&mut picker);
    assert_eq!(picker.visible_sessions.len(), 2);
    assert_eq!(picker.selected_session().unwrap().id, "direct");
    assert_eq!(picker.scroll_offset, 17);
    assert!(!picker.auto_scroll_preview);
}
