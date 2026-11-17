use super::*;

#[test]
fn materialized_provider_messages_share_payload_and_detach_on_edit() {
    let mut app = create_test_app();
    app.messages = vec![Message::assistant_text("retained provider payload")];

    let mut materialized = app.materialized_provider_messages();
    assert!(
        materialized[0]
            .content
            .shares_storage_with(&app.messages[0].content)
    );

    let original = serde_json::to_string(&app.messages[0]).unwrap();
    if let ContentBlock::Text { text, .. } = &mut materialized[0].content[0] {
        text.push_str(" independently edited");
    } else {
        panic!("expected text payload");
    }
    assert!(
        !materialized[0]
            .content
            .shares_storage_with(&app.messages[0].content)
    );
    assert_eq!(serde_json::to_string(&app.messages[0]).unwrap(), original);
    assert_ne!(serde_json::to_string(&materialized[0]).unwrap(), original);
}

#[test]
fn kv_cache_signature_prefix_match_allows_appended_messages() {
    let baseline_messages = vec![
        crate::message::Message::user("first prompt"),
        crate::message::Message::assistant_text("first answer"),
    ];
    let mut current_messages = baseline_messages.clone();
    current_messages.push(crate::message::Message::user("follow up"));

    let baseline = App::kv_cache_request_signature(&baseline_messages, &[], "system", "memory a");
    let current = App::kv_cache_request_signature(&current_messages, &[], "system", "memory b");

    assert!(App::kv_cache_signatures_prefix_match(&current, &baseline));
    assert_eq!(
        App::kv_cache_common_prefix_messages(&current, &baseline),
        baseline_messages.len()
    );
    assert_ne!(baseline.ephemeral_hash, current.ephemeral_hash);
}

#[test]
fn kv_cache_signature_prefix_match_detects_prefix_mutation() {
    let baseline_messages = vec![
        crate::message::Message::user("first prompt"),
        crate::message::Message::assistant_text("first answer"),
    ];
    let current_messages = vec![
        crate::message::Message::user("changed first prompt"),
        crate::message::Message::assistant_text("first answer"),
        crate::message::Message::user("follow up"),
    ];

    let baseline = App::kv_cache_request_signature(&baseline_messages, &[], "system", "");
    let current = App::kv_cache_request_signature(&current_messages, &[], "system", "");

    assert!(!App::kv_cache_signatures_prefix_match(&current, &baseline));
    assert_eq!(App::kv_cache_common_prefix_messages(&current, &baseline), 0);
}
