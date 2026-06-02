use super::*;
use crate::message::{ContentBlock, Role};
use crate::session::{Session, StoredDisplayRole};
use chrono::Duration;
use serde_json::json;
use std::path::Path;
use std::time::Instant;

fn with_temp_home<T>(f: impl FnOnce(&Path) -> T) -> T {
    let _guard = crate::storage::lock_test_env();
    let temp = tempfile::TempDir::new().expect("create temp dir");
    let previous_home = std::env::var("JCODE_HOME").ok();
    crate::env::set_var("JCODE_HOME", temp.path());
    std::fs::create_dir_all(temp.path().join("sessions")).expect("create sessions dir");

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f(temp.path())));

    if let Some(previous_home) = previous_home {
        crate::env::set_var("JCODE_HOME", previous_home);
    } else {
        crate::env::remove_var("JCODE_HOME");
    }

    result.unwrap_or_else(|payload| std::panic::resume_unwind(payload))
}

fn text(text: &str) -> ContentBlock {
    ContentBlock::Text {
        text: text.to_string(),
        cache_control: None,
    }
}

fn save_test_session(id: &str, messages: Vec<(Role, Vec<ContentBlock>)>) -> Session {
    let mut session = Session::create_with_id(id.to_string(), None, None);
    session.short_name = Some(format!("short-{id}"));
    session.working_dir = Some("/tmp/project".to_string());
    for (role, content) in messages {
        session.add_message(role, content);
    }
    session.save().expect("save test session");
    session
}

fn run_report(home: &Path, query: &str, options: &SearchOptions) -> SearchReport {
    search_sessions_blocking(
        &home.join("sessions"),
        &QueryProfile::new(query),
        options,
        "test-log-session",
    )
    .expect("search succeeds")
}

fn run_search(home: &Path, query: &str, options: &SearchOptions) -> Vec<SearchResult> {
    run_report(home, query, options).results
}

#[test]
fn token_overlap_matches_when_exact_phrase_is_absent() {
    with_temp_home(|home| {
        save_test_session(
            "airpods-session",
            vec![(
                Role::Assistant,
                vec![text(
                    "Try reconnecting your AirPods after the Bluetooth audio drops.",
                )],
            )],
        );

        let options = SearchOptions::for_test("current-session");
        let results = run_search(home, "airpods reconnect bluetooth", &options);

        assert!(!results.is_empty(), "expected token-overlap match");
        assert!(results[0].snippet.to_lowercase().contains("airpods"));
        assert_eq!(results[0].kind, SearchResultKind::Message);
        assert_eq!(results[0].message_index, Some(0));
    });
}

#[test]
fn tool_use_input_is_hidden_by_default_and_searchable_when_requested() {
    with_temp_home(|home| {
        save_test_session(
            "tool-session",
            vec![(
                Role::Assistant,
                vec![ContentBlock::ToolUse {
                    id: "tool-1".to_string(),
                    name: "websearch".to_string(),
                    input: json!({
                        "query": "best time post hackernews visibility upvotes"
                    }),
                }],
            )],
        );

        let options = SearchOptions::for_test("current-session");
        let hidden_results = run_search(home, "hackernews visibility upvotes", &options);
        assert!(
            hidden_results.is_empty(),
            "tool-only messages should be hidden by default"
        );

        let mut options = SearchOptions::for_test("current-session");
        options.include_tools = true;
        let results = run_search(home, "hackernews visibility upvotes", &options);
        assert!(!results.is_empty(), "expected tool input match");
        assert!(results[0].snippet.to_lowercase().contains("hackernews"));
    });
}

#[test]
fn journal_entries_are_searchable() {
    with_temp_home(|home| {
        let mut session = Session::create_with_id("journal-session".to_string(), None, None);
        session.short_name = Some("journal-test".to_string());
        session.working_dir = Some("/tmp/project".to_string());
        session.add_message(Role::User, vec![text("snapshot-only baseline message")]);
        session.save().expect("save snapshot");
        session.add_message(
            Role::Assistant,
            vec![text(
                "journal-only-needle appears after the snapshot checkpoint",
            )],
        );
        session.save().expect("append journal entry");

        let snapshot = std::fs::read_to_string(home.join("sessions/journal-session.json"))
            .expect("read snapshot");
        assert!(
            !snapshot.contains("journal-only-needle"),
            "test should prove the hit lives only in the journal"
        );

        let options = SearchOptions::for_test("current-session");
        let results = run_search(home, "journal-only-needle", &options);
        assert!(!results.is_empty(), "expected journal-backed match");
        assert_eq!(results[0].message_index, Some(1));
    });
}

#[test]
fn empty_sessions_dir_returns_no_results_instead_of_panicking() {
    with_temp_home(|home| {
        let options = SearchOptions::for_test("current-session");
        let results = run_search(home, "anything distinctive", &options);
        assert!(results.is_empty());
    });
}

#[test]
fn timestamped_session_collection_respects_recent_limit_without_mtime_stat() {
    with_temp_home(|home| {
        save_test_session(
            "session_1760000000000_old",
            vec![(Role::User, vec![text("old-needle")])],
        );
        save_test_session(
            "session_1760000001000_mid",
            vec![(Role::User, vec![text("mid-needle")])],
        );
        save_test_session(
            "session_1760000002000_new",
            vec![(Role::User, vec![text("new-needle")])],
        );

        let collection = collect_session_files(&home.join("sessions"), 2).expect("collect files");
        assert!(collection.truncated);
        let ids = collection
            .files
            .iter()
            .map(|candidate| candidate.session_id_hint.as_str())
            .collect::<Vec<_>>();

        assert_eq!(ids.len(), 2);
        assert!(ids.contains(&"session_1760000002000_new"));
        assert!(ids.contains(&"session_1760000001000_mid"));
        assert!(!ids.contains(&"session_1760000000000_old"));
    });
}

#[test]
#[ignore = "local performance benchmark over the real ~/.jcode session corpus"]
fn bench_real_session_search_corpus() {
    if std::env::var("JCODE_SESSION_SEARCH_BENCH_REAL")
        .ok()
        .as_deref()
        != Some("1")
    {
        eprintln!("set JCODE_SESSION_SEARCH_BENCH_REAL=1 to run against the real session corpus");
        return;
    }

    let sessions_dir = crate::storage::jcode_dir()
        .expect("jcode dir")
        .join("sessions");
    let mut options = SearchOptions::for_test("benchmark-current-session");
    options.include_external = false;
    options.max_scan_sessions = 1000;

    for query in ["session_search", "optimization", "nonexistentneedle123"] {
        let start = Instant::now();
        let report = search_sessions_blocking(
            &sessions_dir,
            &QueryProfile::new(query),
            &options,
            "benchmark-log-session",
        )
        .expect("search succeeds");
        eprintln!(
            "BENCH query={query} elapsed_ms={} scanned={} candidates={} results={} truncated={}",
            start.elapsed().as_millis(),
            report.scanned_jcode_sessions,
            report.candidate_jcode_sessions,
            report.results.len(),
            report.truncated
        );
    }
}

#[test]
fn stop_word_only_query_is_not_actionable() {
    with_temp_home(|home| {
        save_test_session(
            "generic-session",
            vec![(
                Role::User,
                vec![text("This message should never be returned.")],
            )],
        );

        let query = QueryProfile::new("the and of");
        assert!(!query.is_actionable());

        let options = SearchOptions::for_test("current-session");
        let results =
            search_sessions_blocking(&home.join("sessions"), &query, &options, "test-log-session")
                .expect("search succeeds");
        assert!(results.results.is_empty());
    });
}

#[test]
fn current_session_is_excluded_by_default_but_can_be_included() {
    with_temp_home(|home| {
        save_test_session(
            "current-session",
            vec![(Role::User, vec![text("current-only-needle")])],
        );

        let options = SearchOptions::for_test("current-session");
        assert!(run_search(home, "current-only-needle", &options).is_empty());

        let mut options = SearchOptions::for_test("current-session");
        options.include_current = true;
        let results = run_search(home, "current-only-needle", &options);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].session_id, "current-session");
    });
}

#[test]
fn metadata_is_searchable_and_returned_with_locator() {
    with_temp_home(|home| {
        let mut session = save_test_session(
            "metadata-session",
            vec![(Role::User, vec![text("ordinary content without the label")])],
        );
        session.short_name = Some("pegasus".to_string());
        session.title = Some("Saved architecture discussion".to_string());
        session.save_label = Some("project-pegasus".to_string());
        session.save().expect("save metadata update");

        let options = SearchOptions::for_test("current-session");
        let results = run_search(home, "project-pegasus", &options);
        assert!(!results.is_empty(), "metadata should be searchable");
        assert_eq!(results[0].kind, SearchResultKind::Metadata);
        assert_eq!(results[0].message_index, None);
        assert!(results[0].snippet.contains("Save label: project-pegasus"));
    });
}

#[test]
fn system_reminders_are_hidden_by_default_and_opt_in_searchable() {
    with_temp_home(|home| {
        let mut session = Session::create_with_id("system-session".to_string(), None, None);
        session.working_dir = Some("/tmp/project".to_string());
        session.add_message(
            Role::User,
            vec![text(
                "<system-reminder>\nsecret-system-needle\n</system-reminder>",
            )],
        );
        session.add_message_with_display_role(
            Role::Assistant,
            vec![text("display-role-needle")],
            Some(StoredDisplayRole::System),
        );
        session.save().expect("save system session");

        let options = SearchOptions::for_test("current-session");
        assert!(run_search(home, "secret-system-needle", &options).is_empty());
        assert!(run_search(home, "display-role-needle", &options).is_empty());

        let mut options = SearchOptions::for_test("current-session");
        options.include_system = true;
        assert!(!run_search(home, "secret-system-needle", &options).is_empty());
        assert!(!run_search(home, "display-role-needle", &options).is_empty());
    });
}

#[test]
fn working_dir_filter_is_case_insensitive_and_prefix_based() {
    with_temp_home(|home| {
        let mut session = save_test_session(
            "dir-session",
            vec![(Role::Assistant, vec![text("directory-filter-needle")])],
        );
        session.working_dir = Some("/tmp/Project/Subdir".to_string());
        session.save().expect("save working dir update");

        let mut options = SearchOptions::for_test("current-session");
        options.working_dir_filter = Some("/TMP/project".to_string());
        let results = run_search(home, "directory-filter-needle", &options);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].session_id, "dir-session");
    });
}

#[test]
fn results_are_grouped_by_session_by_default() {
    with_temp_home(|home| {
        save_test_session(
            "many-hit-session",
            vec![
                (Role::User, vec![text("duplicate-needle alpha")]),
                (Role::Assistant, vec![text("duplicate-needle beta")]),
            ],
        );
        save_test_session(
            "single-hit-session",
            vec![(Role::User, vec![text("duplicate-needle gamma")])],
        );

        let mut options = SearchOptions::for_test("current-session");
        options.limit = 10;
        let results = run_search(home, "duplicate-needle", &options);
        let many_count = results
            .iter()
            .filter(|result| result.session_id == "many-hit-session")
            .count();
        assert_eq!(many_count, 1, "default max_per_session should be 1");
        assert_eq!(results.len(), 2);
    });
}

#[test]
fn formatter_emits_stable_locators_and_safe_code_fences() {
    with_temp_home(|home| {
        save_test_session(
            "format-session",
            vec![(
                Role::Assistant,
                vec![text("format-needle with a markdown fence ``` inside")],
            )],
        );

        let options = SearchOptions::for_test("current-session");
        let report = run_report(home, "format-needle", &options);
        let output = format_results("format-needle", &report, &options);
        assert!(output.contains("Session ID: `format-session`"));
        assert!(output.contains("Match: message #1"));
        assert!(
            output.contains("````text"),
            "fence should grow when snippet contains ```"
        );
    });
}

#[test]
fn filters_cover_role_provider_model_flags_and_dates() {
    with_temp_home(|home| {
        let mut session = save_test_session(
            "filter-session",
            vec![
                (Role::User, vec![text("filterable-needle from the user")]),
                (
                    Role::Assistant,
                    vec![text("filterable-needle from the assistant")],
                ),
            ],
        );
        session.provider_key = Some("anthropic".to_string());
        session.model = Some("claude-sonnet-4".to_string());
        session.saved = true;
        session.is_debug = true;
        session.is_canary = true;
        session.save().expect("save filter metadata");

        let mut options = SearchOptions::for_test("current-session");
        options.limit = 10;
        options.max_per_session = 10;
        options.role_filter = Some(RoleFilter::User);
        let results = run_search(home, "filterable-needle", &options);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].role, "user");

        options.role_filter = Some(RoleFilter::Assistant);
        let results = run_search(home, "filterable-needle", &options);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].role, "assistant");

        options.role_filter = None;
        options.provider_filter = Some("anthropic".to_string());
        options.model_filter = Some("sonnet".to_string());
        options.saved_filter = Some(true);
        options.debug_filter = Some(true);
        options.canary_filter = Some(true);
        options.before = Some(Utc::now() + Duration::days(1));
        assert!(!run_search(home, "filterable-needle", &options).is_empty());

        options.model_filter = Some("nonexistent-model".to_string());
        assert!(run_search(home, "filterable-needle", &options).is_empty());

        options.model_filter = Some("sonnet".to_string());
        options.saved_filter = Some(false);
        assert!(run_search(home, "filterable-needle", &options).is_empty());

        options.saved_filter = Some(true);
        options.after = Some(Utc::now() + Duration::days(1));
        assert!(run_search(home, "filterable-needle", &options).is_empty());
    });
}

#[test]
fn role_all_searches_user_assistant_and_metadata() {
    with_temp_home(|home| {
        let mut session = save_test_session(
            "role-all-session",
            vec![
                (Role::User, vec![text("shared-needle from the user")]),
                (
                    Role::Assistant,
                    vec![text("shared-needle from the assistant")],
                ),
            ],
        );
        session.save_label = Some("shared-needle metadata".to_string());
        session.save().expect("save metadata update");

        let mut options = SearchOptions::for_test("current-session");
        options.limit = 10;
        options.max_per_session = 10;
        options.role_filter = parse_role_filter(Some("all")).expect("parse all role");

        let results = run_search(home, "shared-needle", &options);
        let roles = results
            .iter()
            .map(|result| result.role.as_str())
            .collect::<Vec<_>>();
        assert!(roles.contains(&"user"), "all should include user messages");
        assert!(
            roles.contains(&"assistant"),
            "all should include assistant messages"
        );
        assert!(roles.contains(&"metadata"), "all should include metadata");

        options.role_filter = Some(RoleFilter::User);
        let user_results = run_search(home, "shared-needle", &options);
        assert_eq!(user_results.len(), 1);
        assert_eq!(user_results[0].role, "user");
    });
}

#[test]
fn role_parser_accepts_all_as_default_all_roles_filter() {
    assert_eq!(parse_role_filter(None).unwrap(), None);
    assert_eq!(parse_role_filter(Some("all")).unwrap(), None);
    assert_eq!(parse_role_filter(Some(" ALL ")).unwrap(), None);
    assert_eq!(
        parse_role_filter(Some("assistant")).unwrap(),
        Some(RoleFilter::Assistant)
    );
    let err = parse_role_filter(Some("browser")).expect_err("invalid role should fail");
    assert!(err.contains("all, user, assistant, or metadata"));
}

#[test]
fn context_expansion_returns_neighboring_messages_without_matching_hit() {
    with_temp_home(|home| {
        save_test_session(
            "context-session",
            vec![
                (Role::User, vec![text("context-before-line")]),
                (Role::Assistant, vec![text("context-hit-needle")]),
                (Role::User, vec![text("context-after-line")]),
            ],
        );

        let mut options = SearchOptions::for_test("current-session");
        options.context_before = 1;
        options.context_after = 1;
        let results = run_search(home, "context-hit-needle", &options);

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].message_index, Some(1));
        assert_eq!(results[0].context.len(), 2);
        assert!(results[0].context[0].text.contains("context-before-line"));
        assert!(results[0].context[1].text.contains("context-after-line"));
    });
}

#[test]
fn external_codex_sessions_are_searchable_without_jcode_session_dir() {
    with_temp_home(|home| {
        let codex_dir = home.join("external/.codex/sessions/2026/05/01");
        std::fs::create_dir_all(&codex_dir).expect("create codex dir");
        let lines = [
            json!({
                "type": "session_meta",
                "payload": {
                    "id": "codex-test",
                    "timestamp": "2026-05-01T00:00:00Z",
                    "cwd": "/tmp/external-project"
                }
            }),
            json!({
                "type": "message",
                "id": "m1",
                "role": "user",
                "timestamp": "2026-05-01T00:01:00Z",
                "content": [{"type": "input_text", "text": "external before context"}]
            }),
            json!({
                "type": "message",
                "id": "m2",
                "role": "assistant",
                "timestamp": "2026-05-01T00:02:00Z",
                "content": [{"type": "output_text", "text": "external-codex-needle answer"}]
            }),
            json!({
                "type": "message",
                "id": "m3",
                "role": "user",
                "timestamp": "2026-05-01T00:03:00Z",
                "content": [{"type": "input_text", "text": "external after context"}]
            }),
        ];
        let body = lines
            .iter()
            .map(|line| serde_json::to_string(line).expect("serialize codex line"))
            .collect::<Vec<_>>()
            .join("\n");
        std::fs::write(codex_dir.join("codex-test.jsonl"), body).expect("write codex jsonl");
        std::fs::remove_dir_all(home.join("sessions")).expect("remove jcode sessions dir");

        let mut options = SearchOptions::for_test("current-session");
        options.source_filter = Some("codex".to_string());
        options.context_before = 1;
        options.context_after = 1;
        let report = run_report(home, "external-codex-needle", &options);

        assert_eq!(report.scanned_jcode_sessions, 0);
        assert!(report.scanned_external_sessions >= 1);
        assert_eq!(report.external_sources, vec!["codex"]);
        assert_eq!(report.results.len(), 1);
        let result = &report.results[0];
        assert_eq!(result.source, "codex");
        assert_eq!(result.session_id, "codex:codex-test");
        assert_eq!(result.working_dir.as_deref(), Some("/tmp/external-project"));
        assert_eq!(result.message_id.as_deref(), Some("m2"));
        assert!(
            result
                .context
                .iter()
                .any(|line| line.text.contains("external before context"))
        );
        assert!(
            result
                .context
                .iter()
                .any(|line| line.text.contains("external after context"))
        );
    });
}

#[test]
fn limit_validation_reports_friendly_errors() {
    assert_eq!(
        validate_bounded_usize(Some(3), DEFAULT_LIMIT, 1, MAX_LIMIT, "limit").unwrap(),
        3
    );
    let err = validate_bounded_usize(Some(0), DEFAULT_LIMIT, 1, MAX_LIMIT, "limit")
        .expect_err("zero limit should be rejected");
    assert!(err.contains("limit must be between 1"));
    let err = validate_bounded_usize(Some(-1), DEFAULT_LIMIT, 1, MAX_LIMIT, "limit")
        .expect_err("negative limit should be rejected");
    assert!(err.contains("received -1"));
}
