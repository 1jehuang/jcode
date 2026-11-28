use super::*;
use crate::auth::account_pins::tests::TestHome;

fn pinned_session(id: &str) -> Session {
    let mut session = Session::create_with_id(id.into(), None, None);
    session.model = Some("gpt-5.5".into());
    session.route_api_method = Some("openai-oauth@openai-fox".into());
    for (provider, label, identity) in [
        ("openai", "openai-fox", "id:workspace"),
        ("claude", "claude-otter", "person@example.test"),
    ] {
        session.account_pins.insert(
            provider.into(),
            jcode_provider_core::AccountPin {
                label: label.into(),
                identity: Some(identity.into()),
                record_id: Some(uuid::Uuid::new_v4().to_string()),
            },
        );
    }
    session
}

#[test]
fn account_pins_survive_normal_resume_and_both_startup_snapshots() {
    let _home = TestHome::new();
    let mut session = pinned_session("account_pin_snapshot_roundtrip");
    session.save_prepared().unwrap();
    for restored in [
        Session::load(&session.id).unwrap(),
        Session::load_for_resume(&session.id).unwrap(),
        Session::load_startup_stub(&session.id).unwrap(),
        Session::load_for_remote_startup(&session.id).unwrap(),
    ] {
        assert_eq!(restored.account_pins, session.account_pins);
        assert_eq!(restored.route_api_method, session.route_api_method);
    }
}

#[test]
fn old_journal_metadata_preserves_pins_and_an_explicit_empty_map_clears_them() {
    let _home = TestHome::new();
    let mut session = pinned_session("account_pin_journal_roundtrip");
    session.save_prepared().unwrap();
    session.append_stored_message(StoredMessage {
        id: "offline-message".into(),
        role: Role::Assistant,
        content: vec![ContentBlock::Text {
            text: "private fixture".into(),
            cache_control: None,
        }]
        .into(),
        display_role: None,
        timestamp: None,
        tool_duration_ms: None,
        token_usage: None,
    });
    session.save().unwrap();
    let path = session_journal_path(&session.id).unwrap();
    let contents = std::fs::read_to_string(&path).unwrap();
    let mut entry: serde_json::Value =
        serde_json::from_str(contents.lines().last().unwrap()).unwrap();
    assert_eq!(
        entry["meta"]["account_pins"],
        serde_json::to_value(&session.account_pins).unwrap()
    );
    entry["meta"]
        .as_object_mut()
        .unwrap()
        .remove("account_pins");
    std::fs::write(
        &path,
        format!("{}\n", serde_json::to_string(&entry).unwrap()),
    )
    .unwrap();
    for restored in [
        Session::load(&session.id).unwrap(),
        Session::load_startup_stub(&session.id).unwrap(),
        Session::load_for_remote_startup(&session.id).unwrap(),
    ] {
        assert_eq!(restored.account_pins, session.account_pins);
    }
    entry["meta"]["account_pins"] = serde_json::json!({});
    std::fs::write(
        &path,
        format!("{}\n", serde_json::to_string(&entry).unwrap()),
    )
    .unwrap();
    assert!(Session::load(&session.id).unwrap().account_pins.is_empty());
    assert!(
        Session::load_startup_stub(&session.id)
            .unwrap()
            .account_pins
            .is_empty()
    );
}
