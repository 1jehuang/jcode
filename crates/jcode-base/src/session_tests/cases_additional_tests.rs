use super::*;

#[test]
fn test_session_exists_roundtrip() -> Result<()> {
    let tmp_dir = std::env::temp_dir().join(format!(
        "jcode-session-test-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|e| anyhow!(e))?
            .as_nanos()
    ));
    std::fs::create_dir_all(tmp_dir.join("sessions"))?;

    assert!(!session_path_in_dir(&tmp_dir, "missing-session").exists());

    let session_path = session_path_in_dir(&tmp_dir, "exists-session");
    std::fs::write(&session_path, "{}")?;
    assert!(session_path.exists());

    let random_id = format!(
        "missing-session-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|e| anyhow!(e))?
            .as_nanos()
    );
    assert!(!session_exists(&random_id));
    Ok(())
}

#[test]
fn released_provider_message_cache_rebuilds_from_canonical_history() {
    let mut session = Session::create_with_id(
        "session_provider_cache_release_test".to_string(),
        None,
        Some("Provider cache release".to_string()),
    );
    session.add_message(
        Role::User,
        vec![ContentBlock::Text {
            text: "large derived payload".repeat(1024),
            cache_control: None,
        }],
    );

    let before = session.provider_messages().to_vec();
    assert_eq!(before.len(), 1);
    assert_eq!(
        session.debug_memory_profile()["provider_messages_cache"]["count"],
        1
    );

    session.release_provider_messages_cache();
    let released = session.debug_memory_profile();
    assert_eq!(released["provider_messages_cache"]["count"], 0);
    assert_eq!(released["provider_messages_cache"]["json_bytes"], 0);

    let rebuilt = session.provider_messages();
    assert_eq!(rebuilt.len(), 1);
    assert_eq!(
        serde_json::to_value(rebuilt).unwrap(),
        serde_json::to_value(before).unwrap()
    );
}
