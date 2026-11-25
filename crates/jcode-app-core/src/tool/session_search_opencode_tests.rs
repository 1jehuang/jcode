use super::session_search_tests::{run_report, run_search, with_temp_home};
use super::*;

#[test]
fn external_opencode_sqlite_sessions_are_searchable() {
    with_temp_home(|home| {
        let db = home.join("external/.local/share/opencode/opencode.db");
        drop(crate::opencode_db::fixture::standard(&db));

        let mut options = SearchOptions::for_test("current-session");
        options.source_filter = Some("opencode".to_string());
        let report = run_report(home, "hi from assistant", &options);
        assert_eq!(report.external_sources, vec!["opencode"]);
        let result = report
            .results
            .iter()
            .find(|r| r.message_id.as_deref() == Some("msg_b"))
            .expect("assistant text hit");
        assert_eq!(result.session_id, "opencode:ses_main");
        assert_eq!(result.working_dir.as_deref(), Some("/tmp/oc-main"));
        assert_eq!(result.provider_key.as_deref(), Some("anthropic"));

        // Subagent children, archived sessions and non-text parts stay out.
        let report = run_report(home, "THINKING", &options);
        assert!(report.results.is_empty(), "reasoning parts must not match");
        let report = run_report(home, "Subagent", &options);
        assert!(report.results.is_empty(), "child sessions must be excluded");
    });
}

fn opencode_db_fixture(home: &Path) -> crate::opencode_db::fixture::Fixture {
    crate::opencode_db::fixture::Fixture::create(
        &home.join("external/.local/share/opencode/opencode.db"),
    )
}

// These fixtures go through the public Tool::execute boundary, including input
// parsing, blocking dispatch, source discovery, loading, grouping and rendering.
// JCODE_HOME redirects both Jcode and external stores into disposable files.
#[derive(Clone, Copy)]
enum OpenCodePublicFixture {
    Restored,
    Duplicates,
    EqualTimeCopies,
    DatabasePrecedence,
}

fn write_opencode_public_metadata(home: &Path, file: &str, id: &str, title: &str, updated: i64) {
    let path = home
        .join("external/.local/share/opencode/storage/session/proj")
        .join(file);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(
        &path,
        json!({"id": id, "title": title, "time": {"created": 1, "updated": updated}}).to_string(),
    )
    .unwrap();
    // All copied old records appear newer on disk than the restored newest one.
    let modified = if id == "ses_restored" { 1 } else { 100 };
    std::fs::File::open(path)
        .unwrap()
        .set_times(
            std::fs::FileTimes::new()
                .set_modified(std::time::UNIX_EPOCH + std::time::Duration::from_secs(modified)),
        )
        .unwrap();
}

fn run_opencode_public_search(fixture: OpenCodePublicFixture) -> String {
    with_temp_home(|home| {
        match fixture {
            OpenCodePublicFixture::Restored => {
                // cap=2 used to produce an mtime window of 18. This fixture is
                // deliberately larger, so the stored-newest file lies outside it.
                for i in 0..19 {
                    write_opencode_public_metadata(
                        home,
                        &format!("old-{i:02}.json"),
                        &format!("ses_old_{i:02}"),
                        "recallneedle copied-old",
                        1_000 + i,
                    );
                }
                write_opencode_public_metadata(
                    home,
                    "restored.json",
                    "ses_restored",
                    "recallneedle restored-newest",
                    9_000,
                );
            }
            OpenCodePublicFixture::Duplicates | OpenCodePublicFixture::EqualTimeCopies => {
                let equal_time = matches!(fixture, OpenCodePublicFixture::EqualTimeCopies);
                // Insert the rejected copy first so normal directory traversal
                // also exercises replacement, not only first-entry selection.
                write_opencode_public_metadata(
                    home,
                    if equal_time {
                        "z-copy.json"
                    } else {
                        "a-copy.json"
                    },
                    "ses_copied",
                    "recallneedle rejected-copy",
                    if equal_time { 4_000 } else { 3_000 },
                );
                write_opencode_public_metadata(
                    home,
                    if equal_time {
                        "a-copy.json"
                    } else {
                        "z-copy.json"
                    },
                    "ses_copied",
                    "recallneedle selected-copy",
                    4_000,
                );
                write_opencode_public_metadata(
                    home,
                    "distinct.json",
                    "ses_distinct",
                    "recallneedle distinct",
                    2_000,
                );
            }
            OpenCodePublicFixture::DatabasePrecedence => {
                let f = opencode_db_fixture(home);
                f.session(
                    "ses_shared",
                    None,
                    "recallneedle database-authoritative",
                    "/tmp/c",
                    "p",
                    "m",
                    1_000,
                    None,
                );
                drop(f);
                write_opencode_public_metadata(
                    home,
                    "shared.json",
                    "ses_shared",
                    "recallneedle rejected-legacy",
                    9_000,
                );
                write_opencode_public_metadata(
                    home,
                    "distinct.json",
                    "ses_distinct",
                    "recallneedle distinct",
                    2_000,
                );
            }
        }

        let ctx = ToolContext {
            session_id: "public-search-current".to_string(),
            message_id: "public-search-message".to_string(),
            tool_call_id: "public-search-call".to_string(),
            working_dir: Some(home.to_path_buf()),
            stdin_request_tx: None,
            graceful_shutdown_signal: None,
            execution_mode: crate::tool::ToolExecutionMode::Direct,
        };
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(SessionSearchTool::new().execute(
                json!({
                    "query": "recallneedle",
                    "source": "opencode",
                    "role": "metadata",
                    "max_scan_sessions": 2,
                    "limit": 10
                }),
                ctx,
            ))
            .expect("public session_search should succeed")
            .output
    })
}

#[test]
fn public_opencode_search_keeps_restored_stored_newest_beyond_mtime_window() {
    let output = run_opencode_public_search(OpenCodePublicFixture::Restored);
    assert!(
        output.contains("- Session ID: `opencode:ses_restored`"),
        "{output}"
    );
}

#[test]
fn public_opencode_search_reports_two_selected_sessions_from_twenty_metadata_files() {
    let output = run_opencode_public_search(OpenCodePublicFixture::Restored);
    assert!(
        output.contains("2 external sessions from opencode; scan truncated"),
        "{output}"
    );
}

#[test]
fn public_opencode_search_duplicate_copies_do_not_displace_distinct_session() {
    let output = run_opencode_public_search(OpenCodePublicFixture::Duplicates);
    assert!(
        output.contains("- Session ID: `opencode:ses_distinct`"),
        "{output}"
    );
}

#[test]
fn public_opencode_search_duplicate_copies_consume_one_result_slot() {
    let output = run_opencode_public_search(OpenCodePublicFixture::Duplicates);
    assert_eq!(
        output
            .matches("- Session ID: `opencode:ses_copied`")
            .count(),
        1,
        "{output}"
    );
}

#[test]
fn public_opencode_search_duplicate_copies_do_not_report_false_truncation() {
    let output = run_opencode_public_search(OpenCodePublicFixture::Duplicates);
    assert!(!output.contains("scan truncated"), "{output}");
}

#[test]
fn public_opencode_search_duplicate_copies_report_two_unique_scanned_sessions() {
    let output = run_opencode_public_search(OpenCodePublicFixture::Duplicates);
    assert!(
        output.contains("2 external sessions from opencode._"),
        "{output}"
    );
}

#[test]
fn public_opencode_search_chooses_newest_stored_update_for_duplicate_id() {
    let output = run_opencode_public_search(OpenCodePublicFixture::Duplicates);
    assert!(!output.contains("rejected-copy"), "{output}");
}

#[test]
fn public_opencode_search_equal_time_copies_choose_lexically_first_path() {
    let output = run_opencode_public_search(OpenCodePublicFixture::EqualTimeCopies);
    assert!(
        output.contains("- Title: recallneedle selected-copy"),
        "{output}"
    );
}

#[test]
fn public_opencode_search_database_wins_over_newer_legacy_copy() {
    let output = run_opencode_public_search(OpenCodePublicFixture::DatabasePrecedence);
    assert!(
        output.contains("- Title: recallneedle database-authoritative"),
        "{output}"
    );
}

#[test]
fn public_opencode_search_database_duplicate_does_not_consume_extra_scan_slot() {
    let output = run_opencode_public_search(OpenCodePublicFixture::DatabasePrecedence);
    assert!(
        output.contains("2 external sessions from opencode._"),
        "{output}"
    );
}

#[test]
fn opencode_sqlite_search_preserves_unicode_case_matches() {
    with_temp_home(|home| {
        let f = opencode_db_fixture(home);
        f.session(
            "ses_unicode",
            None,
            "Chat",
            "/tmp/c",
            "p",
            "m",
            3_000_000,
            None,
        )
        .message("ses_unicode", "msg_unicode", "user", 100)
        .text(
            "ses_unicode",
            "msg_unicode",
            "prt_unicode",
            "Kubernetes deployment",
        );
        drop(f);
        let mut options = SearchOptions::for_test("current-session");
        options.source_filter = Some("opencode".to_string());
        for include_tools in [false, true] {
            options.include_tools = include_tools;
            let results = run_search(home, "kubernetes deployment", &options);
            assert!(
                results
                    .iter()
                    .any(|r| r.message_id.as_deref() == Some("msg_unicode")),
                "Unicode case match must reach integrated search results"
            );
        }
    });
}

#[test]
fn opencode_sources_rank_stored_updates_before_applying_scan_cap() {
    with_temp_home(|home| {
        let f = opencode_db_fixture(home);
        f.session("ses_db", None, "Chat", "/tmp/c", "p", "m", 3_000_000, None)
            .message("ses_db", "msg_db", "user", 100)
            .text("ses_db", "msg_db", "prt_db", "recency-needle");
        drop(f);
        let legacy = home.join("external/.local/share/opencode/storage/session/proj");
        std::fs::create_dir_all(&legacy).unwrap();
        std::fs::write(legacy.join("ses_old.json"), json!({"id": "ses_old", "title": "Old copied file", "time": {"created": 1, "updated": 2_000_000}}).to_string()).unwrap();
        let mut options = SearchOptions::for_test("current-session");
        options.source_filter = Some("opencode".to_string());
        options.max_scan_sessions = 1;
        let report = run_report(home, "recency-needle", &options);
        assert!(report.truncated);
        assert_eq!(report.scanned_external_sessions, 1);
        assert!(
            report
                .results
                .iter()
                .any(|r| r.session_id == "opencode:ses_db"),
            "A copied old JSON file must not displace the newer database session"
        );

        // The legacy-only preselection must also use stored update time, not mtime.
        let newest = legacy.join("ses_newest.json");
        std::fs::write(&newest, json!({"id": "ses_newest", "title": "recency-needle", "time": {"created": 1, "updated": 4_000_000}}).to_string()).unwrap();
        std::fs::File::open(&newest)
            .unwrap()
            .set_times(
                std::fs::FileTimes::new()
                    .set_modified(std::time::UNIX_EPOCH + std::time::Duration::from_secs(1)),
            )
            .unwrap();
        let results = run_search(home, "recency-needle", &options);
        assert!(
            results
                .iter()
                .any(|r| r.session_id == "opencode:ses_newest"),
            "Stored timestamps must determine selection even within the legacy store"
        );
        assert!(!results.iter().any(|r| r.session_id == "opencode:ses_db"));
    });
}

#[test]
fn opencode_sqlite_tool_output_is_searchable_with_include_tools() {
    with_temp_home(|home| {
        let f = opencode_db_fixture(home);
        f.session(
            "ses_tool", None, "Tool run", "/tmp/t", "p", "m", 3_000_000, None,
        )
        .message("ses_tool", "msg_t", "assistant", 100)
        .text("ses_tool", "msg_t", "prt_t0", "running a command")
        .part(
            "ses_tool",
            "msg_t",
            "prt_t1",
            json!({"type": "tool", "tool": "bash", "state": {
                    "input": {"command": "ls toolinputneedle"},
                    "output": "zebraquartz-output-needle"}}),
        )
        .part(
            "ses_tool",
            "msg_t",
            "prt_t2",
            json!({"type": "reasoning", "text": "reasonneedle pondering"}),
        );
        drop(f);
        let mut options = SearchOptions::for_test("current-session");
        options.source_filter = Some("opencode".to_string());
        for query in [
            "zebraquartz-output-needle",
            "toolinputneedle",
            "reasonneedle",
        ] {
            options.include_tools = false;
            assert!(
                run_search(home, query, &options).is_empty(),
                "{query} must not match without include_tools"
            );
            options.include_tools = true;
            let results = run_search(home, query, &options);
            assert!(
                results
                    .iter()
                    .any(|r| r.message_id.as_deref() == Some("msg_t")),
                "{query} must match inside tool/reasoning parts with include_tools"
            );
        }
    });
}

#[test]
fn opencode_sqlite_search_loads_only_matching_histories() {
    with_temp_home(|home| {
        let f = opencode_db_fixture(home);
        for i in 0..12 {
            let id = format!("ses_{i:02}");
            let msg = format!("msg_{i:02}");
            let text = if i == 7 {
                "the Unique-Needle lives here".to_string()
            } else {
                format!("ordinary chatter number {i}")
            };
            f.session(&id, None, "Chat", "/tmp/c", "p", "m", 3_000_000 + i, None)
                .message(&id, &msg, "user", 100)
                .text(&id, &msg, &format!("prt_{i:02}"), &text);
        }
        // Only a tool part matches: must not load without include_tools.
        f.part(
            "ses_03",
            "msg_03",
            "prt_03t",
            json!({"type": "tool", "state": {"output": "unique-needle in tool"}}),
        );
        drop(f);
        let mut options = SearchOptions::for_test("current-session");
        options.source_filter = Some("opencode".to_string());
        OPENCODE_DB_LOADS.with(|loads| loads.set(0));
        let results = run_search(home, "unique-needle", &options);
        assert!(results.iter().any(|r| r.session_id == "opencode:ses_07"));
        assert_eq!(OPENCODE_DB_LOADS.with(|loads| loads.get()), 1);

        OPENCODE_DB_LOADS.with(|loads| loads.set(0));
        assert!(run_search(home, "nothing-matches-this", &options).is_empty());
        assert_eq!(OPENCODE_DB_LOADS.with(|loads| loads.get()), 0);

        options.include_tools = true;
        OPENCODE_DB_LOADS.with(|loads| loads.set(0));
        run_search(home, "unique-needle", &options);
        assert_eq!(OPENCODE_DB_LOADS.with(|loads| loads.get()), 2);
    });
}

#[test]
fn opencode_sources_share_one_scan_allowance_and_keep_cursor_matches() {
    with_temp_home(|home| {
        let f = opencode_db_fixture(home);
        f.session("ses_db", None, "Db", "/tmp/d", "p", "m", 3_000_000, None)
            .message("ses_db", "msg_d", "user", 100)
            .text("ses_db", "msg_d", "prt_d", "database chatter");
        drop(f);
        let legacy = home.join("external/.local/share/opencode/storage/session/proj");
        std::fs::create_dir_all(&legacy).expect("legacy dir");
        std::fs::write(
            legacy.join("ses_legacy.json"),
            json!({"id": "ses_legacy", "title": "Legacy", "directory": "/tmp/l",
                   "time": {"created": 1, "updated": 2}})
            .to_string(),
        )
        .expect("legacy session");
        let session_id = "99999999-2222-3333-4444-555555555555";
        let cursor_dir = home.join(format!(
            "external/.cursor/projects/tmp-proj/agent-transcripts/{session_id}"
        ));
        std::fs::create_dir_all(&cursor_dir).expect("cursor dir");
        std::fs::write(
            cursor_dir.join(format!("{session_id}.jsonl")),
            json!({"role": "user",
                   "message": {"content": [{"type": "text", "text": "cursor-cap-needle here"}]}})
            .to_string(),
        )
        .expect("cursor jsonl");
        // Earlier sources also match, so they fill any combined pre-match cap.
        let claude = home.join("external/.claude/projects/demo");
        std::fs::create_dir_all(&claude).expect("claude dir");
        std::fs::write(
            claude.join("claude-cap.jsonl"),
            json!({"type": "user", "uuid": "u1", "sessionId": "claude-cap", "cwd": "/tmp/c",
                   "message": {"role": "user", "content": "cursor-cap-needle claude"},
                   "timestamp": "2026-04-04T12:00:00Z"})
            .to_string(),
        )
        .expect("claude jsonl");
        let codex = home.join("external/.codex/sessions/2026/05/01");
        std::fs::create_dir_all(&codex).expect("codex dir");
        std::fs::write(
            codex.join("codex-cap.jsonl"),
            [
                json!({"type": "session_meta", "payload": {"id": "codex-cap",
                       "timestamp": "2026-05-01T00:00:00Z", "cwd": "/tmp/x"}}),
                json!({"type": "message", "id": "m1", "role": "user",
                       "timestamp": "2026-05-01T00:01:00Z",
                       "content": [{"type": "input_text", "text": "cursor-cap-needle codex"}]}),
            ]
            .map(|v| v.to_string())
            .join("\n"),
        )
        .expect("codex jsonl");
        let pi = home.join("external/.pi/agent/sessions/proj");
        std::fs::create_dir_all(&pi).expect("pi dir");
        std::fs::write(
            pi.join("pi-cap.jsonl"),
            [
                json!({"type": "session", "id": "pi-cap", "timestamp": "2026-05-01T00:00:00Z",
                       "cwd": "/tmp/p"}),
                json!({"type": "message", "timestamp": "2026-05-01T00:01:00Z",
                       "message": {"role": "user", "content": "cursor-cap-needle pi"}}),
            ]
            .map(|v| v.to_string())
            .join("\n"),
        )
        .expect("pi jsonl");
        std::fs::remove_dir_all(home.join("sessions")).expect("remove jcode sessions dir");

        let mut options = SearchOptions::for_test("current-session");
        options.max_scan_sessions = 1;
        let report = run_report(home, "cursor-cap-needle", &options);
        assert!(
            report
                .results
                .iter()
                .any(|r| r.session_id == format!("cursor:{session_id}")),
            "cursor match dropped: {:?}",
            report.results
        );
        for source in ["claude", "codex", "pi"] {
            assert!(
                report.results.iter().any(|r| r.source == source),
                "{source} fixture must match too: {:?}",
                report.results
            );
        }
    });
}

#[test]
fn sqlite_precedence_excludes_older_archived_and_child_legacy_copies_before_capping() {
    with_temp_home(|home| {
        let db = opencode_db_fixture(home);
        db.session(
            "newest",
            None,
            "scope-needle newest",
            "/fixture",
            "p",
            "m",
            9000,
            None,
        )
        .session(
            "older",
            None,
            "scope-needle older",
            "/fixture",
            "p",
            "m",
            1,
            None,
        )
        .session(
            "archived",
            None,
            "scope-needle archived",
            "/fixture",
            "p",
            "m",
            2,
            Some(3),
        )
        .session(
            "child",
            Some("newest"),
            "scope-needle child",
            "/fixture",
            "p",
            "m",
            3,
            None,
        );
        drop(db);
        for id in ["older", "archived", "child"] {
            write_opencode_public_metadata(
                home,
                &format!("{id}.json"),
                id,
                "scope-needle legacy copy",
                999_999,
            );
        }
        let mut options = SearchOptions::for_test("current-session");
        options.source_filter = Some("opencode".into());
        options.max_scan_sessions = 1;
        let report = run_report(home, "scope-needle", &options);
        assert!(
            report
                .results
                .iter()
                .any(|row| row.session_id == "opencode:newest")
        );
        assert!(
            report
                .results
                .iter()
                .all(|row| row.session_id == "opencode:newest")
        );
        assert_eq!(report.scanned_external_sessions, 1);
    });
}

#[test]
fn sqlite_only_search_reports_when_more_sessions_exist_than_the_scan_cap() {
    with_temp_home(|home| {
        let db = opencode_db_fixture(home);
        for i in 0..3 {
            db.session(
                &format!("session-{i}"),
                None,
                "cap-needle",
                "/fixture",
                "p",
                "m",
                i,
                None,
            );
        }
        drop(db);
        let mut options = SearchOptions::for_test("current-session");
        options.source_filter = Some("opencode".into());
        options.max_scan_sessions = 1;
        let report = run_report(home, "cap-needle", &options);
        assert!(report.truncated);
        assert_eq!(report.scanned_external_sessions, 1);
    });
}

#[test]
fn unreadable_sqlite_metadata_does_not_hide_a_valid_legacy_copy() {
    with_temp_home(|home| {
        let db = opencode_db_fixture(home);
        db.session(
            "fallback",
            None,
            "broken metadata",
            "/fixture",
            "p",
            "m",
            9000,
            None,
        );
        db.0.execute("UPDATE session SET model = '{'", []).unwrap();
        drop(db);
        write_opencode_public_metadata(
            home,
            "fallback.json",
            "fallback",
            "fallback-needle legacy",
            100,
        );
        let mut options = SearchOptions::for_test("current-session");
        options.source_filter = Some("opencode".into());
        let report = run_report(home, "fallback-needle", &options);
        assert!(report.parse_errors > 0);
        assert!(
            report
                .results
                .iter()
                .any(|row| row.session_id == "opencode:fallback")
        );
    });
}
