use super::super::{SessionPicker, SessionSource, SessionStatus};
use super::*;
use crossterm::event::{KeyCode, KeyModifiers};

#[test]
fn warm_queries_complete_immediately_across_edits_and_backspace() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("warm.jsonl");
    std::fs::write(&path, "{\"text\":\"deploy configuration cloud\"}\n").unwrap();
    let cancel = AtomicBool::new(false);
    text_cache::segments(&path, &cancel).unwrap();
    let session = external_session("warm", &path);
    let mut search = TranscriptSearch::default();
    for query in [
        "dep",
        "deploy",
        "deploy conf",
        "deploy configuration",
        "deploy",
        "clodu",
    ] {
        search.reset(query, std::iter::once(&session));
        // A future timestamp verifies scheduling does not consult a timer.
        search.started_at = Some(Instant::now() + Duration::from_secs(60));
        let deadline = Instant::now() + Duration::from_secs(5);
        while search.pending() {
            assert!(
                Instant::now() < deadline,
                "warm query {query} waited for cold work"
            );
            search.poll();
            std::thread::sleep(Duration::from_millis(1));
        }
        assert!(search.contains("warm"), "query {query} lost its match");
        if query == "clodu" {
            assert!(!search.contains_literal("warm"));
        }
    }
}

#[test]
fn typing_prepares_cold_text_for_the_latest_query_without_waiting() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("during-typing.jsonl");
    std::fs::write(&path, "{\"text\":\"deploy configuration\"}\n".repeat(200)).unwrap();
    let session = external_session("typing", &path);
    assert!(!text_cache::is_cached_hint(&path));
    let mut search = TranscriptSearch::default();
    search.reset("dep", std::iter::once(&session));
    let old_cancel = search.cancel.clone();
    search.reset("deploy conf", std::iter::once(&session));
    assert!(old_cancel.load(Ordering::Relaxed));
    search.started_at = Some(Instant::now() + Duration::from_secs(60));
    let deadline = Instant::now() + Duration::from_secs(5);
    while !search.contains("typing") {
        assert!(
            Instant::now() < deadline,
            "preparation failed to serve latest query"
        );
        search.poll();
        std::thread::sleep(Duration::from_millis(1));
    }
    assert!(search.contains_literal("typing"));
}

#[test]
fn uncached_streaming_search_does_not_wait_for_a_typing_timer() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("oversized.jsonl");
    // Too large for warmup or decoded-text caching, so this must use cold IO.
    let mut text = format!("{}\n", r#"{"text":"needle"}"#);
    text.push_str(&" ".repeat(17 * 1024 * 1024));
    std::fs::write(&path, text).unwrap();
    let session = external_session("cold", &path);
    let mut search = TranscriptSearch::default();
    search.reset("needle", std::iter::once(&session));
    search.started_at = Some(Instant::now() + Duration::from_secs(60));
    search.poll();
    assert!(search.started_at.is_none());
    assert!(search.candidates.is_empty());
    let deadline = Instant::now() + Duration::from_secs(5);
    while search.pending() {
        assert!(
            Instant::now() < deadline,
            "cold search waited for typing timer"
        );
        search.poll();
        std::thread::sleep(Duration::from_millis(1));
    }
    assert!(search.contains_literal("cold"));
    assert!(!text_cache::is_cached_hint(&path));
}

#[test]
fn cached_literal_exclusions_do_not_hide_future_fuzzy_matches() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("fuzzy.json");
    std::fs::write(&path, r#"{"text":"cloud"}"#).unwrap();
    let cancel = AtomicBool::new(false);
    assert!(!paths_match_mode(
        std::slice::from_ref(&path),
        &["clod".into()],
        &cancel,
        false
    ));
    assert!(!paths_match_mode(
        std::slice::from_ref(&path),
        &["clodu".into()],
        &cancel,
        false
    ));
    assert!(paths_match_scored(&[path], &["clodu".into()], &cancel).is_some());
}

#[test]
fn cached_path_scores_preserve_first_path_and_cross_file_tokens() {
    let dir = tempfile::tempdir().unwrap();
    let first = dir.path().join("first.json");
    let second = dir.path().join("second.json");
    std::fs::write(&first, r#"{"text":"configuration deploy"}"#).unwrap();
    std::fs::write(&second, r#"{"text":"config cloud"}"#).unwrap();
    let paths = vec![first, second];
    let cancel = AtomicBool::new(false);
    for path in &paths {
        text_cache::segments(path, &cancel).unwrap();
    }
    for (query, fuzzy) in [
        ("deploy cloud", false),
        ("conifg clodu", true),
        ("absent", false),
    ] {
        let tokens = search_query_tokens(query);
        let mut scores = WordScores::default();
        let mut reference = Scan::new(&tokens, &[], &cancel, mode(fuzzy), &mut scores);
        for path in &paths {
            scan_file_streaming(path, &mut reference);
            if reference.complete() {
                break;
            }
        }
        assert_eq!(
            cached_paths_scan(&paths, &tokens, &cancel, fuzzy),
            Some(reference.all_found().then_some(reference.score))
        );
    }
}

#[test]
fn missing_native_alternative_does_not_delay_resident_fuzzy_evidence() {
    let dir = tempfile::tempdir().unwrap();
    let snapshot = dir.path().join("session.json");
    let missing_journal = dir.path().join("session.journal.jsonl");
    std::fs::write(&snapshot, r#"{"text":"cloud deployment"}"#).unwrap();
    let cancel = AtomicBool::new(false);
    text_cache::segments(&snapshot, &cancel).unwrap();
    let tokens = search_query_tokens("clodu");
    let paths = vec![missing_journal, snapshot];
    let expected = scan_paths(&paths, &tokens, &cancel, true);
    assert!(expected.is_some());
    assert_eq!(
        cached_paths_scan(&paths, &tokens, &cancel, true),
        Some(expected)
    );
}

#[test]
fn incremental_token_sequences_match_fresh_streaming_results_and_scores() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("equivalence.jsonl");
    std::fs::write(&path, "{\"text\":\"deployment café clod configuration\"}\n{broken\n{\"text\":\"deploy cloud config βeta\"}\n").unwrap();
    let cancel = AtomicBool::new(false);
    text_cache::segments(&path, &cancel).unwrap();
    let words = [
        "dep", "deploy", "deply", "cloud", "clodu", "café", "cafe", "βeta", "absent",
    ];
    for fuzzy in [false, true] {
        for first in words {
            for second in words {
                let tokens = vec![first.to_string(), second.to_string()];
                let mut scores = WordScores::default();
                let mut reference = Scan::new(&tokens, &[], &cancel, mode(fuzzy), &mut scores);
                scan_file_streaming(&path, &mut reference);
                let expected = reference.all_found().then_some(reference.score);
                assert_eq!(
                    cached_paths_scan(std::slice::from_ref(&path), &tokens, &cancel, fuzzy),
                    Some(expected),
                    "tokens={tokens:?} fuzzy={fuzzy}"
                );
                assert_eq!(
                    scan_paths(std::slice::from_ref(&path), &tokens, &cancel, fuzzy),
                    expected
                );
            }
        }
    }
}

#[test]
fn warm_hint_cannot_publish_stale_content_after_file_change() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("changed.json");
    let cancel = AtomicBool::new(false);
    std::fs::write(&path, r#"{"text":"oldneedle"}"#).unwrap();
    text_cache::segments(&path, &cancel).unwrap();
    std::fs::write(&path, r#"{"text":"replacement words"}"#).unwrap();
    assert!(text_cache::is_cached_hint(&path));
    assert_eq!(
        cached_paths_scan(
            std::slice::from_ref(&path),
            &["oldneedle".into()],
            &cancel,
            false
        ),
        None
    );
    assert!(!paths_match_mode(
        &[path],
        &["oldneedle".into()],
        &cancel,
        false
    ));
}

#[test]
#[ignore = "manual incremental typing readiness measurement"]
fn benchmark_incremental_query_readiness() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("incremental.jsonl");
    let record = format!(
        "{{\"text\":\"{} searchalpha betabravo\"}}\n",
        "ordinary text ".repeat(40)
    );
    std::fs::write(&path, record.repeat(300)).unwrap();
    let session = external_session("incremental", &path);
    // Prepare the query-independent text exactly as a preceding search would.
    assert!(paths_match_mode(
        &[path],
        &["searchalpha".into()],
        &AtomicBool::new(false),
        false
    ));
    let mut search = TranscriptSearch::default();
    let mut samples = Vec::new();
    for query in [
        "searchal",
        "searchalpha",
        "searchalpha bet",
        "searchalpha beta",
        "searchalpha",
    ] {
        let start = Instant::now();
        search.reset(query, std::iter::once(&session));
        let deadline = start + Duration::from_secs(5);
        while !search.contains("incremental") {
            assert!(Instant::now() < deadline, "query {query} timed out");
            search.poll();
            std::thread::sleep(Duration::from_millis(1));
        }
        samples.push(start.elapsed().as_secs_f64() * 1000.0);
    }
    eprintln!(
        "incremental query first-result readiness_ms={samples:?} (warm transcript, immediate scheduling, modeled 1ms polling, includes query edits and backspace)"
    );
}

#[test]
fn repeated_search_and_reopened_picker_observe_transcript_changes() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("session.jsonl");
    std::fs::write(&path, "{\"text\":\"cachealpha cachebeta\"}\n").unwrap();
    let session = external_session("cache-flow", &path);
    let run = |query: &str| {
        let mut search = TranscriptSearch::default();
        search.reset(query, std::iter::once(&session));
        search.started_at = Some(Instant::now());
        let deadline = Instant::now() + Duration::from_secs(5);
        while search.pending() {
            assert!(Instant::now() < deadline);
            search.poll();
            std::thread::sleep(Duration::from_millis(1));
        }
        search.contains("cache-flow")
    };
    assert!(run("cachealpha"));
    assert!(run("cachebeta"));
    assert!(run("cachealpha"));
    std::fs::write(
        &path,
        "{\"text\":\"replacement content with a different length\"}\n",
    )
    .unwrap();
    assert!(!run("cachealpha"));
    assert!(run("replacement"));
    std::fs::remove_file(&path).unwrap();
    assert!(!run("replacement"));
}

#[test]
#[ignore = "manual cold versus warm transcript search measurement"]
fn benchmark_reusable_transcript_search() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("search.jsonl");
    let record = format!(
        "{{\"text\":\"{}\"}}\n",
        "ordinary transcript words ".repeat(30)
    );
    std::fs::write(&path, record.repeat(1000)).unwrap();
    let cancel = AtomicBool::new(false);
    let paths = vec![path];
    // Absent terms force a complete scan rather than a lucky early match.
    let cold = Instant::now();
    assert!(!paths_match_mode(
        &paths,
        &["missingcoldneedle".into()],
        &cancel,
        false
    ));
    let cold = cold.elapsed();
    let mut samples = Vec::new();
    for index in 0..30 {
        let start = Instant::now();
        assert!(!paths_match_mode(
            &paths,
            &[format!("missingwarmneedle{index}")],
            &cancel,
            false
        ));
        samples.push(start.elapsed().as_secs_f64() * 1000.0);
    }
    samples.sort_by(f64::total_cmp);
    eprintln!(
        "reusable transcript search: cold_ms={:.3} warm_p50_ms={:.3} warm_p95_ms={:.3} samples=30 (scan only, debounce excluded)",
        cold.as_secs_f64() * 1000.0,
        samples[15],
        samples[28]
    );
}

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
        todo_statuses: Vec::new(),
    }
}

fn wait_for_search(picker: &mut SessionPicker) {
    if picker.transcript_search.started_at.is_some() {
        picker.transcript_search.started_at = Some(Instant::now());
    }
    let deadline = Instant::now() + Duration::from_secs(5);
    while picker.transcript_search.pending() {
        assert!(Instant::now() < deadline, "transcript search timed out");
        picker.poll_transcript_search();
        std::thread::sleep(Duration::from_millis(1));
    }
}

#[test]
fn candidate_priority_prefers_opening_then_intent_and_preserves_ties() {
    let query = "repair the broken search";
    let candidate = |id: &str, intent: &str, opening: &str| Candidate {
        entry: Arc::new(Entry {
            id: id.into(),
            paths: Vec::new(),
            search_index: query.into(),
            title: String::new(),
            short_name: String::new(),
            save_label: None,
            has_prompt: !opening.is_empty(),
            opening_prompt: opening.into(),
            intent_text: intent.into(),
            working_dir: None,
            server_name: None,
            cloud: false,
        }),
        missing_tokens: Vec::new(),
        exact_field: false,
        prior: Vec::new(),
    };
    let candidates = vec![
        candidate("weak-first", "unrelated", ""),
        candidate("weak-second", "unrelated", ""),
        candidate("intent", "Repair Search", ""),
        candidate("opening", query, query),
    ];
    let ranked = prioritize_candidates(
        candidates,
        &PhraseQuery::new(query),
        &search_query_tokens(query),
        &[],
        &AtomicBool::new(false),
    );
    assert_eq!(
        ranked
            .iter()
            .map(|(candidate, _)| candidate.id.as_str())
            .collect::<Vec<_>>(),
        ["opening", "intent", "weak-first", "weak-second"]
    );
}

#[test]
fn progressive_search_publishes_best_opening_in_first_batch_and_keeps_weak_matches() {
    let query = "repair the broken search";
    let mut sessions: Vec<_> = (0..130)
        .map(|index| {
            let mut session = external_session(&format!("weak-{index}"), Path::new("unused"));
            session.search_index = query.into();
            session.first_user_prompt = Some("unrelated opening request".into());
            session
        })
        .collect();
    let mut best = external_session("best", Path::new("unused"));
    best.search_index = query.into();
    best.first_user_prompt = Some(query.into());
    sessions.push(best);
    let mut search = TranscriptSearch::default();
    search.reset(query, sessions.iter());
    search.started_at = Some(Instant::now());
    let deadline = Instant::now() + Duration::from_secs(5);
    while search.ranks.is_empty() {
        assert!(
            Instant::now() < deadline,
            "search did not publish a first batch"
        );
        search.poll();
        std::thread::sleep(Duration::from_millis(1));
    }
    assert!(
        search.contains("best"),
        "strongest opening must arrive in the first batch"
    );
    while search.pending() {
        assert!(Instant::now() < deadline, "search did not finish");
        search.poll();
        std::thread::sleep(Duration::from_millis(1));
    }
    assert_eq!(search.ranks.len(), sessions.len());
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
        picker.search_selection.holding() && !picker.transcript_search.has_results(),
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
    picker.transcript_search.started_at = Some(Instant::now());
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
fn typing_starts_cold_work_on_first_poll_and_metadata_is_async() {
    let temp = tempfile::tempdir().unwrap();
    let session = external_session("one", &temp.path().join("absent.jsonl"));
    let mut picker = SessionPicker::new(vec![session]);
    picker.search_query = "needle".into();
    picker.rebuild_items();
    picker.poll_transcript_search();
    assert!(picker.transcript_search.started_at.is_none());
    assert!(picker.transcript_search.candidates.is_empty());
    picker.search_query = "TITLE metadata".into();
    picker.rebuild_items();
    assert!(picker.transcript_search.pending());
    wait_for_search(&mut picker);
    assert_eq!(picker.visible_sessions.len(), 1);
    assert!(!picker.transcript_search.pending());
}

#[test]
fn fuzzy_metadata_matching_runs_on_worker_not_on_keystroke() {
    let temp = tempfile::tempdir().unwrap();
    let mut session = external_session("one", &temp.path().join("absent.jsonl"));
    session.search_index = "config deploy".into();
    let mut picker = SessionPicker::new(vec![session]);
    picker.search_query = "conifg".into();
    picker.rebuild_items();
    // The row on screen is the held pre-query list, not a match.
    assert!(picker.search_selection.holding());
    assert!(!picker.transcript_search.has_results());
    assert!(picker.transcript_search.receiver.is_none());
    assert!(picker.transcript_search.candidates.is_empty());
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
    assert!(picker.search_selection.holding());
    // Candidate selection reads every index, so it is left to the worker.
    assert!(picker.transcript_search.candidates.is_empty());
    assert!(picker.transcript_search.pending());
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
    assert!(picker.search_selection.holding());
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
    picker
        .transcript_search
        .ranks
        .insert("direct".into(), SearchRank::default());
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

fn picker_with_pending_better_match() -> SessionPicker {
    let mut direct = external_session("direct", Path::new("unused"));
    direct.search_index = "needle".into();
    let discovered = external_session("discovered", Path::new("unused"));
    let mut picker = SessionPicker::new(vec![direct, discovered]);
    picker.search_query = "needle".into();
    picker.rebuild_items();
    picker
        .transcript_search
        .ranks
        .insert("direct".into(), SearchRank::default());
    picker.rebuild_items();
    assert_eq!(picker.selected_session().unwrap().id, "direct");
    let (sender, receiver) = mpsc::channel();
    sender
        .send((
            "discovered".into(),
            SearchRank {
                opening_prompt: 10,
                literal: true,
                ..SearchRank::default()
            },
        ))
        .unwrap();
    picker.transcript_search.started_at = None;
    picker.transcript_search.receiver = Some(receiver);
    picker
}

#[test]
fn early_progressive_results_follow_best_match_and_reset_preview() {
    let mut picker = picker_with_pending_better_match();
    picker.scroll_offset = 17;
    picker.auto_scroll_preview = false;
    picker.poll_transcript_search();
    assert_eq!(picker.selected_session().unwrap().id, "discovered");
    assert_eq!(picker.scroll_offset, 0);
    assert!(picker.auto_scroll_preview);
}

#[test]
fn late_progressive_results_reorder_without_switching_or_resetting_preview() {
    let mut picker = picker_with_pending_better_match();
    picker.search_selection.expire_for_test();
    picker.scroll_offset = 17;
    picker.auto_scroll_preview = false;
    picker.poll_transcript_search();
    assert_eq!(picker.selected_session().unwrap().id, "direct");
    assert_eq!(picker.scroll_offset, 17);
    assert!(!picker.auto_scroll_preview);
    assert_eq!(
        picker
            .session_by_ref(picker.visible_sessions[0])
            .unwrap()
            .id,
        "discovered"
    );
}

#[test]
fn manual_navigation_or_preview_scroll_stops_early_best_match_following() {
    for scroll in [false, true] {
        let mut picker = picker_with_pending_better_match();
        if scroll {
            picker.scroll_preview_up(1);
        } else {
            // Even pressing Up at the top means the user has taken control.
            picker.previous();
        }
        picker.poll_transcript_search();
        assert_eq!(picker.selected_session().unwrap().id, "direct");
        assert_eq!(
            picker
                .session_by_ref(picker.visible_sessions[0])
                .unwrap()
                .id,
            "discovered"
        );
    }
}

#[test]
fn pending_search_visually_distinguishes_auto_focus_from_user_control() {
    let mut picker = picker_with_pending_better_match();
    let (sender, receiver) = mpsc::channel();
    picker.transcript_search.receiver = Some(receiver);
    let backend = ratatui::backend::TestBackend::new(100, 12);
    let mut terminal = ratatui::Terminal::new(backend).unwrap();
    picker.search_selection.show_busy_for_test();
    let mut render = |picker: &mut SessionPicker| {
        terminal
            .draw(|frame| picker.render_session_list_impl(frame, frame.area(), true))
            .unwrap();
        terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>()
    };
    assert!(render(&mut picker).contains("finding best match"));
    picker.pending_search_submission = Some(crossterm::event::KeyModifiers::empty());
    assert!(render(&mut picker).contains("confirming best match"));
    picker.previous();
    assert!(render(&mut picker).contains("selection held"));
    drop(sender);
    let completed = render(&mut picker);
    assert!(!completed.contains("finding best match"));
    assert!(!completed.contains("searching in background"));
}

#[test]
fn typing_keeps_previous_rows_until_results_arrive_then_releases_them() {
    let temp = tempfile::tempdir().unwrap();
    let mut keep = external_session("keep", &temp.path().join("absent-a.jsonl"));
    keep.search_index = "needle".into();
    let mut drop_me = external_session("drop", &temp.path().join("absent-b.jsonl"));
    drop_me.search_index = "other".into();
    let mut picker = SessionPicker::new(vec![keep, drop_me]);
    let before = picker.visible_sessions.len();
    // A typo has no literal match yet: the list must not blank meanwhile.
    picker.search_query = "nedle".into();
    picker.rebuild_items();
    assert_eq!(picker.visible_sessions.len(), before);
    assert!(picker.search_selection.holding());
    // Enter must not resume a row that belongs to the previous query.
    assert!(picker.queue_search_submission(KeyModifiers::NONE));
    picker.pending_search_submission = None;
    wait_for_search(&mut picker);
    assert!(!picker.search_selection.holding());
    assert_eq!(picker.visible_sessions.len(), 1);
    assert_eq!(picker.selected_session().unwrap().id, "keep");
    // A query nothing matches ends with an empty list, not stale rows.
    picker.search_query = "zzzzqqqq".into();
    picker.rebuild_items();
    assert_eq!(picker.visible_sessions.len(), 1);
    wait_for_search(&mut picker);
    assert!(picker.visible_sessions.is_empty());
    assert!(!picker.search_selection.holding());
}

#[test]
fn unchanged_sessions_share_their_index_across_keystrokes() {
    let temp = tempfile::tempdir().unwrap();
    let mut session = external_session("big", &temp.path().join("absent.jsonl"));
    session.search_index = format!("{} modal", "x".repeat(IMMEDIATE_METADATA_BYTES + 1));
    let mut search = TranscriptSearch::default();
    search.reset("m", std::iter::once(&session));
    let first = search.corpus[0].clone();
    search.reset("mo", std::iter::once(&session));
    assert!(Arc::ptr_eq(&first, &search.corpus[0]));
    assert!(search.candidates.is_empty(), "candidates are worker work");
    session.search_index.push_str(" refreshed");
    search.reset("mod", std::iter::once(&session));
    assert!(!Arc::ptr_eq(&first, &search.corpus[0]));
}
