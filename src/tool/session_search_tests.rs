use super::*;
use crate::message::{ContentBlock, Role};
use crate::session::{Session, StoredDisplayRole};
use serde_json::json;
use std::path::Path;

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

fn run_search(home: &Path, query: &str, options: &SearchOptions) -> Vec<SearchResult> {
    search_sessions_blocking(
        &home.join("sessions"),
        &QueryProfile::new(query),
        options,
        "test-log-session",
    )
    .expect("search succeeds")
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
        assert!(results.is_empty());
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
        let results = run_search(home, "format-needle", &options);
        let output = format_results("format-needle", &results, &options);
        assert!(output.contains("Session ID: `format-session`"));
        assert!(output.contains("Match: message #1"));
        assert!(
            output.contains("````text"),
            "fence should grow when snippet contains ```"
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
