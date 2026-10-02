use super::*;

fn parse(message: &str) -> BackgroundProgress {
    parse_background_notification(message).expect("a background notification")
}

#[test]
fn progress_tick_carries_task_label_and_percent() {
    let progress = parse(
        "**Background task progress** `224715dw29` · `bash`\n\n[#####-----] 50% · Running tests (reported)",
    );
    assert_eq!(progress.task_id, "224715dw29");
    assert_eq!(progress.label, "bash");
    assert_eq!(progress.percent, Some(50.0));
    assert_eq!(progress.summary, "50% · Running tests");
    assert!(!progress.done);
}

#[test]
fn a_display_name_wins_over_the_tool_name() {
    let progress = parse(
        "**Background task progress** `refresh-model-list` · `Model list refresh` (`catalog`)\n\n40% · probing",
    );
    assert_eq!(progress.label, "Model list refresh");
    assert_eq!(progress.percent, Some(40.0));
}

#[test]
fn indeterminate_progress_has_no_percent_but_still_reports() {
    let progress = parse("**Background task progress** `t1` · `bash`\n\nworking (estimated)");
    assert_eq!(progress.percent, None);
    assert_eq!(progress.summary, "working");
}

#[test]
fn completion_is_reported_as_done() {
    let progress = parse(
        "**Background task** `t1` · `bash` · ✓ completed · 12.5s · exit 0\n\n_No output captured._",
    );
    assert!(progress.done);
    assert_eq!(progress.task_id, "t1");
    assert_eq!(progress.label, "bash");
    assert!(progress.summary.starts_with("✓ completed"));
}

#[test]
fn other_notifications_are_not_progress() {
    assert!(parse_background_notification("**DM from fox** hello").is_none());
    assert!(parse_background_notification("").is_none());
    // A malformed header (no task id) must not invent one.
    assert!(parse_background_notification("**Background task progress** no id here").is_none());
}

#[test]
fn a_percent_over_a_hundred_is_clamped() {
    let progress = parse("**Background task progress** `t1` · `bash`\n\n420% · overshoot");
    assert_eq!(progress.percent, Some(100.0));
}

/// The contract test: this parser reads prose the daemon writes, so it is
/// checked against the daemon's own formatter rather than against a copy of its
/// output. Without this, a wording change upstream would silently turn every
/// API client's progress bar off, and nothing would fail.
#[test]
fn the_daemons_own_formatter_round_trips() {
    use jcode_background_types::{
        BackgroundTaskProgress, BackgroundTaskProgressKind, BackgroundTaskProgressSource,
    };
    use jcode_base::bus::BackgroundTaskProgressEvent;

    let event = BackgroundTaskProgressEvent {
        session_id: "s1".into(),
        task_id: "t42".into(),
        tool_name: "bash".into(),
        display_name: Some("workspace tests".into()),
        progress: BackgroundTaskProgress {
            percent: Some(37.0),
            current: Some(37),
            total: Some(100),
            unit: Some("crates".into()),
            message: Some("Running jcode-tui tests".into()),
            eta_seconds: Some(90),
            kind: BackgroundTaskProgressKind::Determinate,
            source: BackgroundTaskProgressSource::Reported,
            updated_at: "2026-07-31T21:00:00Z".into(),
        },
    };
    let rendered = jcode_base::message::format_background_task_progress_markdown(&event);
    let parsed = parse(&rendered);
    assert_eq!(parsed.task_id, "t42");
    assert_eq!(parsed.label, "workspace tests");
    assert_eq!(parsed.percent, Some(37.0));
    assert!(
        parsed.summary.contains("Running jcode-tui tests"),
        "the status line was lost: {rendered}"
    );
    assert!(!parsed.done);

    // And the completion shape, from the same source of truth.
    let done = jcode_base::message::format_background_task_notification_markdown(
        &jcode_base::bus::BackgroundTaskCompleted {
            session_id: "s1".into(),
            task_id: "t42".into(),
            tool_name: "bash".into(),
            display_name: Some("workspace tests".into()),
            status: jcode_background_types::BackgroundTaskStatus::Completed,
            exit_code: Some(0),
            duration_secs: 12.5,
            output_preview: String::new(),
            output_file: std::path::PathBuf::from("/tmp/t42.output"),
            wake: false,
            notify: false,
        },
    );
    let parsed = parse(&done);
    assert_eq!(parsed.task_id, "t42");
    assert!(parsed.done, "a completion was read as a progress tick");
}
