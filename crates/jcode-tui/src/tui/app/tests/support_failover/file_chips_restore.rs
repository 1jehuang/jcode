use super::*;

/// A failover retry must only restore the file chips belonging to the prompt
/// being restored: chips whose paths do not appear in the restored text
/// belong to a different draft and must never ride along onto the retry
/// (PR 1889 review, security).
#[test]
fn test_restore_failover_payload_ignores_foreign_file_chips() {
    let mut app = create_test_app();
    let mut payload = failover_test_payload(Some("look at docs/a.md please"));
    payload.file_chips = vec![
        std::path::PathBuf::from("docs/a.md"),
        std::path::PathBuf::from("secret/other_draft.txt"),
    ];
    app.restore_failover_payload(payload);
    assert_eq!(app.input, "look at docs/a.md please");
    assert_eq!(app.file_chips, vec![std::path::PathBuf::from("docs/a.md")]);
}

/// When the composer already holds a newer draft, the retry restore keeps
/// that draft's chips untouched: extending app.file_chips there is what let
/// the displaced draft's files attach to the retried prompt.
#[test]
fn test_restore_failover_payload_does_not_merge_chips_into_newer_draft() {
    let mut app = create_test_app();
    app.input = "newer draft".into();
    app.file_chips = vec![std::path::PathBuf::from("notes/newer.md")];
    let mut payload = failover_test_payload(Some("look at docs/a.md please"));
    payload.file_chips = vec![std::path::PathBuf::from("docs/a.md")];
    app.restore_failover_payload(payload);
    assert_eq!(app.input, "newer draft");
    assert_eq!(
        app.file_chips,
        vec![std::path::PathBuf::from("notes/newer.md")],
        "the newer draft's chips must stay exactly as they were"
    );
}

#[test]
fn test_restore_failover_payload_keeps_own_chips_after_marker_renumbering() {
    let mut app = create_test_app();
    // Attachment slot 1 already occupied; the restored prompt's image lands
    // at slot 2, so `[image 1]` in the payload remaps to `[image 2]`.
    app.pending_images = vec![("one".to_string(), "a".to_string())];
    let mut payload = failover_test_payload(Some("see [image 1] and docs/a.md"));
    payload.images = vec![("two".to_string(), "b".to_string())];
    payload.file_chips = vec![std::path::PathBuf::from("docs/a.md")];
    app.restore_failover_payload(payload);
    assert_eq!(app.input, "see [image 2] and docs/a.md");
    assert_eq!(
        app.file_chips,
        vec![std::path::PathBuf::from("docs/a.md")],
        "own chips must survive the marker renumbering"
    );
}
