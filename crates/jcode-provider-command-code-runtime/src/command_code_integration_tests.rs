use super::{
    auth::CommandCodeAccount,
    integration::{bounded_context, compose_provider},
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
