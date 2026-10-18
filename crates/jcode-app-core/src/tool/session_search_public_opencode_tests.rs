//! Public Tool::execute regression checks against disposable OpenCode stores.
use super::*;

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
    // Copied old records appear newer on disk than the restored newest one.
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
                // cap=2 previously selected an mtime window of 18 metadata files.
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
                // Insert the rejected copy first to exercise replacement.
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
        tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap()
            .block_on(SessionSearchTool::new().execute(json!({"query": "recallneedle", "source": "opencode", "role": "metadata", "max_scan_sessions": 2, "limit": 10}), ctx))
            .expect("public session_search should succeed").output
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
