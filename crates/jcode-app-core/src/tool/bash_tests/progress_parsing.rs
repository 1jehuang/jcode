use super::*;

#[test]
fn test_parse_progress_marker_handles_percent_payloads() {
    let progress = parse_progress_marker(
        r#"JCODE_PROGRESS {"percent":25,"message":"Downloading dependencies"}"#,
    )
    .expect("marker should parse");

    assert_eq!(progress.percent, Some(25.0));
    assert_eq!(
        progress.message.as_deref(),
        Some("Downloading dependencies")
    );
    assert_eq!(progress.kind, BackgroundTaskProgressKind::Determinate);
    assert_eq!(progress.source, BackgroundTaskProgressSource::Reported);
}

#[test]
fn test_parse_heuristic_progress_handles_ratio_output() {
    let progress = parse_heuristic_progress("Running test 3/10 tests")
        .expect("heuristic parser should not fail")
        .expect("heuristic ratio progress should parse");

    assert_eq!(progress.current, Some(3));
    assert_eq!(progress.total, Some(10));
    assert_eq!(progress.percent, Some(30.0));
    assert_eq!(progress.unit.as_deref(), Some("tests"));
    assert_eq!(progress.source, BackgroundTaskProgressSource::ParsedOutput);
}

#[test]
fn test_parse_heuristic_progress_handles_percent_output() {
    let progress = parse_heuristic_progress("download progress 42% complete")
        .expect("heuristic parser should not fail")
        .expect("heuristic percent progress should parse");

    assert_eq!(progress.percent, Some(42.0));
    assert_eq!(progress.source, BackgroundTaskProgressSource::ParsedOutput);
    assert_eq!(
        progress.message.as_deref(),
        Some("download progress 42% complete")
    );
}

#[test]
fn test_parse_heuristic_progress_handles_phase_output() {
    let progress = parse_heuristic_progress("Compiling jcode v0.10.2")
        .expect("heuristic parser should not fail")
        .expect("phase progress should parse");

    assert_eq!(progress.kind, BackgroundTaskProgressKind::Indeterminate);
    assert_eq!(progress.percent, None);
    assert_eq!(progress.message.as_deref(), Some("Compiling jcode v0.10.2"));
    assert_eq!(progress.source, BackgroundTaskProgressSource::ParsedOutput);
}

#[test]
fn test_parse_heuristic_progress_handles_of_output() {
    let progress = parse_heuristic_progress("Downloaded 3 of 12 crates")
        .expect("heuristic parser should not fail")
        .expect("heuristic of progress should parse");

    assert_eq!(progress.current, Some(3));
    assert_eq!(progress.total, Some(12));
    assert_eq!(progress.percent, Some(25.0));
    assert_eq!(progress.unit.as_deref(), Some("crates"));
}

#[test]
fn test_parse_heuristic_progress_handles_byte_ratio_output() {
    let progress = parse_heuristic_progress("Downloaded 1.5/3.0 GiB")
        .expect("heuristic parser should not fail")
        .expect("heuristic byte ratio progress should parse");

    assert_eq!(progress.percent, Some(50.0));
    assert_eq!(progress.unit.as_deref(), Some("gib"));
    assert_eq!(progress.source, BackgroundTaskProgressSource::ParsedOutput);
}
