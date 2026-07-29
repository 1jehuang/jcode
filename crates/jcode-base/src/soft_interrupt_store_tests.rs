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
            urgent: true,
            source: SoftInterruptSource::System,
        },
    )
    .expect("append first interrupt");
    append(
        session_id,
        SoftInterruptMessage {
            content: "world".to_string(),
            urgent: false,
            source: SoftInterruptSource::BackgroundTask,
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
            urgent: false,
            source: SoftInterruptSource::User,
        },
    )
    .expect("append later interrupt");
    clear(session_id).expect("clear interrupts");
    assert!(load(session_id).expect("load after clear").is_empty());

    if let Some(prev_home) = prev_home {
        crate::env::set_var("JCODE_HOME", prev_home);
    } else {
        crate::env::remove_var("JCODE_HOME");
    }
}
