use super::*;

#[test]
fn append_take_and_clear_round_trip() {
    let _guard = crate::storage::lock_test_env();
    let temp = tempfile::TempDir::new().expect("temp dir");
    let prev_home = std::env::var_os("JCODE_HOME");
    crate::env::set_var("JCODE_HOME", temp.path());

    let session_id = "ses_soft_interrupt_store";
    append(
        session_id,
        SoftInterruptMessage {
            content: "hello".to_string(),
            images: vec![("image/png".into(), "ZmFrZQ==".into())],
            urgent: true,
            source: SoftInterruptSource::System,
            client_id: Some("image-client".into()),
        },
    )
    .expect("append first interrupt");
    append(
        session_id,
        SoftInterruptMessage {
            content: "world".to_string(),
            images: Vec::new(),
            urgent: false,
            source: SoftInterruptSource::FileActivity,
            client_id: None,
        },
    )
    .expect("append second interrupt");

    let loaded = load(session_id).expect("load interrupts");
    assert_eq!(loaded.len(), 2);
    assert_eq!(loaded[0].content, "hello");
    assert!(loaded[0].urgent);
    assert_eq!(loaded[0].images, [("image/png".into(), "ZmFrZQ==".into())]);
    assert_eq!(loaded[0].client_id.as_deref(), Some("image-client"));
    assert_eq!(loaded[1].content, "world");
    assert_eq!(loaded[1].source, SoftInterruptSource::FileActivity);

    let taken = take(session_id).expect("take interrupts");
    assert_eq!(taken.len(), 2);
    assert_eq!(taken[0].images, loaded[0].images);
    assert_eq!(taken[0].client_id, loaded[0].client_id);
    assert!(load(session_id).expect("reload after take").is_empty());

    append(
        session_id,
        SoftInterruptMessage {
            content: "later".to_string(),
            images: Vec::new(),
            urgent: false,
            source: SoftInterruptSource::User,
            client_id: None,
        },
    )
    .expect("append later interrupt");
    clear(session_id).expect("clear interrupts");
    assert!(load(session_id).expect("load after clear").is_empty());

    for (content, client_id) in [("x", Some("f1")), ("y", None), ("z", Some("f2"))] {
        append(
            session_id,
            SoftInterruptMessage {
                content: content.to_string(),
                images: Vec::new(),
                urgent: false,
                source: SoftInterruptSource::User,
                client_id: client_id.map(str::to_string),
            },
        )
        .expect("append id-bearing interrupt");
    }
    let removed = remove_by_client_ids(session_id, &["f2".to_string(), "nope".to_string()])
        .expect("remove by id");
    assert_eq!(removed, vec!["f2".to_string()]);
    let left = load(session_id).expect("load after remove");
    assert_eq!(left.len(), 2);
    assert_eq!(left[0].client_id.as_deref(), Some("f1"));
    assert_eq!(left[1].client_id, None);
    clear(session_id).expect("clear interrupts");

    if let Some(prev_home) = prev_home {
        crate::env::set_var("JCODE_HOME", prev_home);
    } else {
        crate::env::remove_var("JCODE_HOME");
    }
}

#[cfg(unix)]
#[test]
fn unreadable_parent_is_an_error_and_preserves_stored_prompts() {
    use std::os::unix::fs::PermissionsExt;
    if unsafe { libc::geteuid() } == 0 {
        eprintln!("permission-denial fixture skipped: root bypasses directory permissions");
        return;
    }
    let _env = crate::auth::test_sandbox::AuthTestSandbox::new().unwrap();
    let session = "unreadable-parent";
    append(
        session,
        SoftInterruptMessage {
            content: "pending".into(),
            images: vec![("image/png".into(), "ZmFrZQ==".into())],
            urgent: false,
            source: SoftInterruptSource::User,
            client_id: Some("held-image".into()),
        },
    )
    .unwrap();
    let path = path_for_session(session).unwrap();
    let parent = path.parent().unwrap();
    let permissions = std::fs::metadata(parent).unwrap().permissions();
    std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0)).unwrap();
    let loaded = load(session);
    let removed = remove_by_client_ids(session, &["held-image".into()]);
    std::fs::set_permissions(parent, permissions).unwrap();
    assert!(loaded.is_err(), "an unsearchable store must not look empty");
    assert!(
        removed.is_err(),
        "unreadable input must not appear not_queued"
    );
    let left = load(session).unwrap();
    assert_eq!(left.len(), 1);
    assert_eq!(left[0].client_id.as_deref(), Some("held-image"));
    assert_eq!(left[0].images, [("image/png".into(), "ZmFrZQ==".into())]);
}

#[test]
fn corrupt_primary_keeps_existing_backup_recovery() {
    let _env = crate::auth::test_sandbox::AuthTestSandbox::new().unwrap();
    let session = "backup-recovery";
    append(
        session,
        SoftInterruptMessage {
            content: "recoverable".into(),
            images: Vec::new(),
            urgent: false,
            source: SoftInterruptSource::User,
            client_id: Some("recoverable-id".into()),
        },
    )
    .unwrap();
    let path = path_for_session(session).unwrap();
    std::fs::copy(&path, path.with_extension("bak")).unwrap();
    std::fs::write(&path, "broken JSON").unwrap();
    let recovered = load(session).unwrap();
    assert_eq!(recovered.len(), 1);
    assert_eq!(recovered[0].client_id.as_deref(), Some("recoverable-id"));
}
