use super::{
    auth::{CommandCodeAccount, CommandCodeStore},
    integration::{bounded_context, compose_provider, compose_provider_from_store},
};
#[test]
fn test_command_code_multi_turn_context_and_model_contract() {
    assert!(bounded_context().cwd.len() > 0);
    assert_eq!(
        super::models::canonicalize_command_code_model(
            "glm-5.3",
            &super::models::CommandCodeCatalog::new()
        ),
        Some("zai-org/GLM-5.3".into())
    );
}

#[test]
fn unverified_account_is_rejected_before_composition() {
    let account = CommandCodeAccount {
        label: None,
        api_key: "key".into(),
        user_id: String::new(),
        user_name: String::new(),
        org_id: None,
        key_name: None,
    };
    assert!(compose_provider(account, "glm").is_err());
}

#[test]
fn store_composition_honors_active_account() {
    let account = |label: &str, key: &str, user: &str| CommandCodeAccount {
        label: Some(label.into()),
        api_key: key.into(),
        user_id: user.into(),
        user_name: user.into(),
        org_id: None,
        key_name: None,
    };
    let store = CommandCodeStore {
        accounts: vec![
            account("first", "key-1", "u1"),
            account("second", "key-2", "u2"),
        ],
        active: Some("second".into()),
        imported_at: None,
    };
    let provider = compose_provider_from_store(&store, "glm-5.3").unwrap();
    assert_eq!(provider.active_key.read().unwrap().as_str(), "key-2");
}
