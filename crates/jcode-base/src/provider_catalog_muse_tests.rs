use super::*;
use crate::auth::test_sandbox::AuthTestSandbox;
use crate::auth::{AuthCredentialSource, AuthExpiryConfidence, AuthStatus, muse};

fn native_credentials() -> muse::MuseCredentials {
    muse::MuseCredentials {
        api_key: "native-test-key".to_string(),
        access_token: "dca:native-test-grant".to_string(),
        base_url: "https://api.meta.ai/v1".to_string(),
        user_email: None,
        user_full_name: None,
        is_subs_active: true,
        minted_at_ms: chrono::Utc::now().timestamp_millis(),
        subs_usage: None,
    }
}

#[test]
fn native_muse_never_borrows_paid_or_named_profile_credentials() {
    let sandbox = AuthTestSandbox::new().unwrap();
    crate::env::set_var(muse::EXPERIMENTAL_ENV_VAR, "1");
    crate::env::set_var("META_MUSE_API_KEY", "paid-test-key");
    crate::env::set_var(muse::API_KEY_ENV, "not-a-native-login");
    sandbox
        .write_env_file(muse::ENV_FILE, muse::API_KEY_ENV, "not-a-native-login")
        .unwrap();
    crate::env::set_var("JCODE_NAMED_PROVIDER_PROFILE", "unrelated");
    crate::env::set_var("JCODE_OPENROUTER_API_KEY_NAME", "META_MUSE_API_KEY");
    crate::env::set_var("JCODE_OPENROUTER_ENV_FILE", "meta-muse.env");
    let resolved = resolve_openai_compatible_profile(MUSE_SUBSCRIPTION_PROFILE);
    assert!(load_openai_compatible_profile_api_key(&resolved).is_none());
    assert!(!openai_compatible_profile_is_configured(
        MUSE_SUBSCRIPTION_PROFILE
    ));
    assert_eq!(
        load_api_key_from_env_or_config("META_MUSE_API_KEY", "meta-muse.env").as_deref(),
        Some("paid-test-key")
    );
}

#[test]
fn native_muse_credentials_are_bound_to_official_profile_without_opt_in() {
    let _sandbox = AuthTestSandbox::new().unwrap();
    crate::env::set_var(muse::EXPERIMENTAL_ENV_VAR, "1");
    muse::save_credentials(&native_credentials()).unwrap();
    let resolved = resolve_openai_compatible_profile(MUSE_SUBSCRIPTION_PROFILE);
    assert_eq!(
        load_openai_compatible_profile_api_key(&resolved).as_deref(),
        Some("native-test-key")
    );
    for field in ["base", "env", "file"] {
        let mut changed = resolved.clone();
        match field {
            "base" => changed.api_base = "https://example.com/v1".to_string(),
            "env" => changed.api_key_env = "META_MUSE_API_KEY".to_string(),
            _ => changed.env_file = "meta-muse.env".to_string(),
        }
        assert!(load_openai_compatible_profile_api_key(&changed).is_none());
    }
    crate::env::remove_var(muse::EXPERIMENTAL_ENV_VAR);
    assert_eq!(
        load_openai_compatible_profile_api_key(&resolved).as_deref(),
        Some("native-test-key")
    );
    assert!(openai_compatible_profile_is_configured(
        MUSE_SUBSCRIPTION_PROFILE
    ));
}

#[test]
fn native_muse_status_reports_local_presence_not_verified_billing() {
    let _sandbox = AuthTestSandbox::new().unwrap();
    crate::env::set_var(muse::EXPERIMENTAL_ENV_VAR, "1");
    muse::save_credentials(&native_credentials()).unwrap();
    let provider = resolve_login_provider("muse-subscription").unwrap();
    let assessment = AuthStatus::default().assessment_for_provider(provider);
    assert_eq!(assessment.state, crate::auth::AuthState::Available);
    assert_eq!(
        assessment.credential_source,
        AuthCredentialSource::JcodeManagedFile
    );
    assert_eq!(assessment.expiry_confidence, AuthExpiryConfidence::Unknown);
    assert!(!assessment.method_detail.contains("experimental"));
    assert!(assessment.method_detail.contains("third-party client"));
    assert!(assessment.method_detail.contains("billing unverified"));
    assert!(assessment.method_detail.contains("quota unknown"));
}

#[test]
fn native_muse_default_is_standard_and_context_is_not_200k() {
    let _sandbox = AuthTestSandbox::new().unwrap();
    let resolved = resolve_openai_compatible_profile(MUSE_SUBSCRIPTION_PROFILE);
    assert_eq!(resolved.default_model.as_deref(), Some("muse-spark-1.3"));
    assert_eq!(
        openai_compatible_profile_context_limit("muse-subscription", "muse-spark-1.3"),
        Some(1_048_576)
    );
    assert_eq!(
        openai_compatible_profile_context_limit("muse-subscription", "muse-wonder-1.0"),
        Some(1_048_576)
    );
    assert!(
        !openai_compatible_profile_static_models(MUSE_SUBSCRIPTION_PROFILE)
            .iter()
            .any(|model| model.contains("wonder-full"))
    );
    assert_eq!(resolve_login_provider("muse").unwrap().id, "meta-muse");
}
