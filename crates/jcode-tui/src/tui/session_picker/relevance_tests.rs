use super::super::*;
use super::*;
use std::path::Path;
use std::time::{Duration, Instant};

fn session(id: &str, title: &str, age: i64, path: &Path) -> SessionInfo {
    let now = chrono::Utc::now() - chrono::Duration::seconds(age);
    SessionInfo {
        id: id.into(),
        parent_id: None,
        short_name: id.into(),
        icon: "x".into(),
        title: title.into(),
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
        search_index: title.to_lowercase(),
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

fn ids(picker: &SessionPicker) -> Vec<String> {
    picker
        .visible_sessions
        .iter()
        .map(|r| picker.session_by_ref(*r).unwrap().id.clone())
        .collect()
}

fn wait(picker: &mut SessionPicker) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while picker.transcript_search.pending() {
        assert!(Instant::now() < deadline, "search timed out");
        picker.poll_transcript_search();
        std::thread::sleep(Duration::from_millis(2));
    }
}

#[test]
fn relevance_overrides_recency_and_clearing_restores_default_order_and_selection() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("absent.json");
    let recent = session("recent", "needlessly needlework", 0, &path);
    let exact = session("exact", "needle", 200, &path);
    let whole = session("whole", "repair needle behavior", 100, &path);
    let mut picker = SessionPicker::new(vec![recent, exact, whole]);
    assert_eq!(ids(&picker), ["recent", "whole", "exact"]);
    picker.search_query = "needle".into();
    picker.rebuild_items();
    wait(&mut picker);
    assert_eq!(ids(&picker), ["exact", "whole", "recent"]);
    assert_eq!(picker.selected_session().unwrap().id, "exact");
    wait(&mut picker);
    assert_eq!(ids(&picker), ["exact", "whole", "recent"]);
    picker.search_query = "  ".into();
    picker.rebuild_items();
    assert_eq!(ids(&picker), ["recent", "whole", "exact"]);
    assert_eq!(picker.selected_session().unwrap().id, "exact");
}

#[test]
fn progressive_transcript_hits_keep_metadata_rank_and_selected_session() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("transcript.json");
    std::fs::write(&path, r#"{"text":"needle thread"}"#).unwrap();
    let direct = session("direct", "needle thread", 100, &path);
    let partial = session("partial", "needle", 50, &path);
    let transcript = session("transcript", "unrelated", 0, &path);
    let mut picker = SessionPicker::new(vec![transcript, partial, direct]);
    picker.search_query = "needle thread".into();
    picker.rebuild_items();
    wait(&mut picker);
    let selected = picker.selected_session().unwrap().id.clone();
    assert_eq!(selected, "direct");
    assert_eq!(ids(&picker), ["direct", "partial", "transcript"]);
    assert_eq!(picker.selected_session().unwrap().id, selected);
    picker.rebuild_items();
    assert_eq!(ids(&picker), ["direct", "partial", "transcript"]);
}

#[test]
fn relevance_ties_keep_recency_and_multiword_matches_score_every_term() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("absent.json");
    let older = session("older", "config worker", 100, &path);
    let newer = session("newer", "config worker", 0, &path);
    let weaker = session("weaker", "configuration workbench", 0, &path);
    let mut picker = SessionPicker::new(vec![weaker, newer, older]);
    picker.search_query = "conifg worker".into();
    picker.rebuild_items();
    wait(&mut picker);
    assert_eq!(&ids(&picker)[..2], ["newer", "older"]);
}

#[test]
fn async_rank_updates_never_replace_stronger_evidence() {
    let (sender, receiver) = mpsc::channel();
    let mut search = TranscriptSearch::default();
    search.receiver = Some(receiver);
    let strong = SearchRank {
        literal: true,
        metadata_terms: 2,
        whole_terms: 2,
        score: 123,
        ..SearchRank::default()
    };
    sender.send(("session".into(), strong)).unwrap();
    assert!(search.poll());
    sender
        .send(("session".into(), SearchRank::default()))
        .unwrap();
    assert!(!search.poll());
    assert_eq!(search.rank("session"), strong);
    assert!(search.contains_literal("session"));
}

#[test]
fn fuzzy_rank_sums_literal_and_fuzzy_terms() {
    let index = "config worker";
    let tokens = search_query_tokens("conifg worker");
    let rank = fuzzy_metadata_rank(index, &tokens);
    let expected: i64 = tokens
        .iter()
        .map(|token| {
            i64::from(
                jcode_fuzzy::PreparedTokenQuery::new(token)
                    .score(index)
                    .unwrap(),
            )
        })
        .sum();
    assert_eq!(rank.score, expected);
    assert_eq!(rank.metadata_terms, 1);
    assert!(!rank.literal);
}

#[test]
fn global_search_does_not_boost_saved_or_server_sections() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("absent.json");
    let mut saved = session("saved", "needlework", 0, &path);
    saved.saved = true;
    let server = session("server", "repair needle behavior", 50, &path);
    let orphan = session("orphan", "needle", 100, &path);
    let mut picker = SessionPicker::new_grouped(
        vec![ServerGroup {
            name: "server".into(),
            icon: "x".into(),
            version: "test".into(),
            git_hash: "test".into(),
            is_running: true,
            sessions: vec![saved, server],
        }],
        vec![orphan],
    );
    assert_eq!(ids(&picker), ["saved", "server", "orphan"]);
    assert!(
        picker
            .items
            .iter()
            .any(|item| matches!(item, PickerItem::SavedHeader { .. }))
    );
    picker.search_query = "needle".into();
    picker.rebuild_items();
    wait(&mut picker);
    assert_eq!(ids(&picker), ["orphan", "server", "saved"]);
    assert!(picker.item_to_session.iter().all(Option::is_some));
    picker.search_query.clear();
    picker.rebuild_items();
    assert_eq!(ids(&picker), ["saved", "server", "orphan"]);
    assert!(
        picker
            .items
            .iter()
            .any(|item| matches!(item, PickerItem::SavedHeader { .. }))
    );
}

#[test]
fn negative_fuzzy_scores_survive_async_merge() {
    let (sender, receiver) = mpsc::channel();
    let mut search = TranscriptSearch::default();
    search.receiver = Some(receiver);
    let rank = SearchRank {
        score: -7,
        ..SearchRank::default()
    };
    sender.send(("session".into(), rank)).unwrap();
    assert!(search.poll());
    assert_eq!(search.rank("session"), rank);
}

#[test]
fn newly_discovered_stronger_hit_reorders_without_moving_manually_scrolled_selection() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("absent.json");
    let mut picker = SessionPicker::new(vec![
        session("first", "unrelated", 0, &path),
        session("stronger", "different", 100, &path),
    ]);
    picker.search_query = "needle".into();
    picker.rebuild_items();
    let (sender, receiver) = mpsc::channel();
    picker.transcript_search.started_at = None;
    picker.transcript_search.receiver = Some(receiver);
    sender
        .send((
            "first".into(),
            SearchRank {
                score: 10,
                ..SearchRank::default()
            },
        ))
        .unwrap();
    picker.search_selection.publish_next_poll_for_test();
    picker.poll_transcript_search();
    assert_eq!(picker.selected_session().unwrap().id, "first");
    picker.scroll_preview_up(1);
    picker.scroll_offset = 7;
    picker.auto_scroll_preview = false;
    sender
        .send((
            "stronger".into(),
            SearchRank {
                literal: true,
                ..SearchRank::default()
            },
        ))
        .unwrap();
    picker.search_selection.publish_next_poll_for_test();
    picker.poll_transcript_search();
    assert_eq!(ids(&picker), ["stronger", "first"]);
    assert_eq!(picker.selected_session().unwrap().id, "first");
    assert_eq!(picker.scroll_offset, 7);
    assert!(!picker.auto_scroll_preview);
}

#[test]
fn optimized_literal_rank_preserves_unicode_and_punctuation_boundaries() {
    for index in [
        "needle needlework",
        "needleneedle needle",
        "café caféine",
        "éclair éclair",
        "config-test config",
        "東京 東京駅",
        "aa aaa aa",
    ] {
        for query in [
            "needle",
            "café",
            "éclair",
            "config-test",
            "config",
            "東京",
            "aa",
            "needle café",
        ] {
            let tokens = search_query_tokens(query);
            let rank = literal_metadata_rank(index, &tokens);
            let expected = tokens
                .iter()
                .filter(|token| {
                    index
                        .split(|c: char| !c.is_alphanumeric())
                        .any(|word| word == token.as_str())
                })
                .count();
            assert_eq!(rank.whole_terms, expected, "{index:?} / {query:?}");
            assert_eq!(
                rank.metadata_terms,
                tokens
                    .iter()
                    .filter(|token| index.contains(token.as_str()))
                    .count()
            );
        }
    }
    assert!(normalized_equals("  CAFÉ  ", "café"));
    assert!(normalized_equals(" NEEDLE ", "needle"));
    assert_eq!(lowercase_field("İSTANBUL"), "i\u{307}stanbul");
    assert_eq!(lowercase_field("CAFÉ"), "café");
    assert!(matches!(
        lowercase_field("already lowercase"),
        std::borrow::Cow::Borrowed(_)
    ));
}

#[test]
fn retyped_wake_prompt_beats_incidental_literal_transcript_hits() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("transcript.json");
    // A huge unrelated session can contain every query word, but not together.
    std::fs::write(&path, r#"{"text":"do unrelated we unrelated have unrelated logs unrelated of unrelated how unrelated long unrelated it unrelated takes unrelated to unrelated conenct unrelated internet unrelated after unrelated wake"}"#).unwrap();
    let mut target = session(
        "wake",
        "Network resume timing",
        100,
        &temp.path().join("absent.json"),
    );
    target.first_user_prompt = Some("do we have logs of how long it takes to connect to internet after wake from msleep? happened just now and seemed fast".into());
    let unrelated = session("cache", "Cache miss investigation", 0, &path);
    let mut picker = SessionPicker::new(vec![unrelated, target]);
    for query in [
        "do we have logs of how long",
        "do we have logs ofhow long",
        "do we have logs of how long it takes to conenct to internet after wake",
        "do we have logs of how long it takes to connect to internet after wake from msleep?",
    ] {
        picker.search_query = query.into();
        picker.rebuild_items();
        wait(&mut picker);
        assert_eq!(
            ids(&picker).first().map(String::as_str),
            Some("wake"),
            "{query}"
        );
    }
}

#[test]
fn opening_prompt_evidence_requires_a_local_multiword_match() {
    assert_eq!(
        opening_prompt_rank(
            "do something we something have something logs",
            "do we have logs"
        ),
        0
    );
    assert_eq!(opening_prompt_rank("logs about wake", "logs wake"), 0);
    assert_eq!(
        opening_prompt_rank(
            "unrelated discussion about caches",
            "do we have logs of how long"
        ),
        0
    );
    assert!(
        opening_prompt_rank(
            "DO we have logs of how long?",
            "do we have logs of how long"
        ) > 0
    );
}

#[test]
fn mxabox_literal_transcript_hit_beats_recent_fuzzy_metadata_and_transcripts() {
    let temp = tempfile::tempdir().unwrap();
    let literal = temp.path().join("literal.json");
    let fuzzy = temp.path().join("fuzzy.json");
    std::fs::write(&literal, r#"{"text":"repair mxabox integration"}"#).unwrap();
    std::fs::write(&fuzzy, r#"{"text":"repair maxbox integration"}"#).unwrap();
    let mut picker = SessionPicker::new(vec![
        session("metadata", "maxbox", 0, &fuzzy),
        session("transcript", "unrelated", 1, &fuzzy),
        session("literal", "old integration work", 1000, &literal),
    ]);
    picker.search_query = "mxabox".into();
    picker.rebuild_items();
    wait(&mut picker);
    assert_eq!(ids(&picker).first().map(String::as_str), Some("literal"));
}

#[test]
fn fuzzy_transcript_ranking_uses_matched_words_not_only_recency() {
    let temp = tempfile::tempdir().unwrap();
    let close = temp.path().join("close.json");
    let loose = temp.path().join("loose.json");
    std::fs::write(&close, r#"{"text":"maxbox"}"#).unwrap();
    std::fs::write(&loose, r#"{"text":"maxboxes"}"#).unwrap();
    let tokens = search_query_tokens("mxabox");
    let cancel = AtomicBool::new(false);
    let close_score = paths_match_scored(&[close.clone()], &tokens, &cancel).unwrap();
    let loose_score = paths_match_scored(&[loose.clone()], &tokens, &cancel).unwrap();
    assert!(close_score > loose_score, "{close_score} <= {loose_score}");
    let mut picker = SessionPicker::new(vec![
        session("loose", "unrelated", 0, &loose),
        session("close", "unrelated", 1000, &close),
    ]);
    picker.search_query = "mxabox".into();
    picker.rebuild_items();
    wait(&mut picker);
    assert_eq!(ids(&picker), ["close", "loose"]);
}

#[test]
fn user_prompt_transcript_hit_beats_recent_tool_output_hit() {
    let temp = tempfile::tempdir().unwrap();
    let user_path = temp.path().join("user.jsonl");
    let tool_path = temp.path().join("tool.jsonl");
    std::fs::write(
        &user_path,
        concat!(
            r#"{"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"please fix zanzibar parsing"}]}}"#,
            "\n"
        ),
    )
    .unwrap();
    std::fs::write(
        &tool_path,
        concat!(
            r#"{"type":"response_item","payload":{"type":"function_call_output","call_id":"c1","output":"grep found zanzibar parsing"}}"#,
            "\n"
        ),
    )
    .unwrap();
    let tool = session("tool", "unrelated", 0, &tool_path);
    let user = session("user", "unrelated", 500, &user_path);
    let mut picker = SessionPicker::new(vec![tool, user]);
    picker.search_query = "zanzibar parsing".into();
    picker.rebuild_items();
    wait(&mut picker);
    assert_eq!(ids(&picker), ["user", "tool"]);
}

#[test]
fn retyped_later_user_prompt_beats_scattered_literal_hits() {
    let temp = tempfile::tempdir().unwrap();
    let target_path = temp.path().join("target.jsonl");
    let noise_path = temp.path().join("noise.jsonl");
    let user = |text: &str| {
        format!(
            "{{\"type\":\"response_item\",\"payload\":{{\"type\":\"message\",\"role\":\"user\",\"content\":[{{\"type\":\"input_text\",\"text\":\"{text}\"}}]}}}}\n"
        )
    };
    std::fs::write(
        &target_path,
        user("can you tell me about resume session search") + &user("okay ltes do that"),
    )
    .unwrap();
    // Every query word appears literally, but never together.
    std::fs::write(
        &noise_path,
        user("okay great") + &user("oaky is a typo") + &user("ltes test") + &user("do that later"),
    )
    .unwrap();
    for query in ["oaky ltes do that", "okay ltes do that"] {
        let target = session("target", "resume search", 500, &target_path);
        let noise = session("noise", "unrelated", 0, &noise_path);
        let mut picker = SessionPicker::new(vec![noise, target]);
        picker.search_query = query.into();
        picker.rebuild_items();
        wait(&mut picker);
        assert_eq!(
            ids(&picker).first().map(String::as_str),
            Some("target"),
            "{query}"
        );
    }
}

#[test]
fn typo_in_user_prompt_beats_literal_words_scattered_in_tool_output() {
    let temp = tempfile::tempdir().unwrap();
    let user = |text: &str| {
        format!(
            "{{\"type\":\"response_item\",\"payload\":{{\"type\":\"message\",\"role\":\"user\",\"content\":[{{\"type\":\"input_text\",\"text\":\"{text}\"}}]}}}}\n"
        )
    };
    let tool = |text: &str| {
        format!(
            "{{\"type\":\"response_item\",\"payload\":{{\"type\":\"function_call_output\",\"call_id\":\"c\",\"output\":\"{text}\"}}}}\n"
        )
    };
    let mut sessions = Vec::new();
    // Every word appears literally, but "zanzibar" only in a tool dump.
    for index in 0..8 {
        let path = temp.path().join(format!("common{index}.jsonl"));
        std::fs::write(&path, user("build the app") + &tool("zanzibar")).unwrap();
        sessions.push(session(
            &format!("common{index}"),
            "unrelated",
            index,
            &path,
        ));
    }
    // The user typed the distinctive word, with a typo in a common one.
    let path = temp.path().join("rare.jsonl");
    std::fs::write(&path, user("biuld the zanzibar parser")).unwrap();
    sessions.push(session("rare", "unrelated", 500, &path));
    let mut picker = SessionPicker::new(sessions);
    picker.search_query = "build the zanzibar".into();
    picker.rebuild_items();
    wait(&mut picker);
    assert_eq!(ids(&picker).first().map(String::as_str), Some("rare"));
}
