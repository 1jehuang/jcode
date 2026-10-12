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
            images: Vec::new(),
            urgent: true,
            source: SoftInterruptSource::System,
            client_id: None,
        },
    )
    .expect("append first interrupt");
    append(
        session_id,
        SoftInterruptMessage {
            content: "world".to_string(),
            images: Vec::new(),
            urgent: false,
            source: SoftInterruptSource::BackgroundTask,
            client_id: None,
        },
    )
    .expect("append second interrupt");

    let loaded = load(session_id).expect("load interrupts");
    assert_eq!(loaded.len(), 2);
    assert_eq!(loaded[0].content, "hello");
    assert!(loaded[0].urgent);
    assert_eq!(loaded[1].content, "world");

    let taken = take(session_id).expect("take interrupts");
    assert_eq!(taken.len(), 2);
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
