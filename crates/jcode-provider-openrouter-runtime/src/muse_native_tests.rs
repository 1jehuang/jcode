use super::*;
use jcode_base::auth::{muse, test_sandbox::AuthTestSandbox};
use jcode_base::provider_catalog::MUSE_SUBSCRIPTION_PROFILE;

fn save_native_test_login() {
    jcode_base::env::set_var(muse::EXPERIMENTAL_ENV_VAR, "1");
    muse::save_credentials(&muse::MuseCredentials {
        api_key: "native-runtime-test-key".into(),
        access_token: "dca:native-runtime-test-grant".into(),
        base_url: "https://api.meta.ai/v1".into(),
        user_email: None,
        user_full_name: None,
        is_subs_active: true,
        minted_at_ms: chrono_timestamp_ms(),
        subs_usage: None,
    })
    .unwrap();
}

fn chrono_timestamp_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64
}

#[test]
fn native_muse_provider_uses_jcode_tools_and_switches_models_without_host_state() {
    let _sandbox = AuthTestSandbox::new().unwrap();
    save_native_test_login();
    let provider =
        OpenRouterProvider::new_openai_compatible_profile_runtime(MUSE_SUBSCRIPTION_PROFILE)
            .unwrap();
    assert!(!provider.handles_tools_internally());
    assert_eq!(provider.model(), "muse-spark-1.3");
    assert_eq!(provider.context_window(), 1_048_576);
    provider.set_model("muse-wonder-1.0").unwrap();
    assert_eq!(provider.model(), "muse-wonder-1.0");
    assert_eq!(provider.context_window(), 1_048_576);
    assert_eq!(provider.fork().model(), "muse-wonder-1.0");
}

#[tokio::test]
async fn native_muse_provider_rechecks_opt_in_before_starting_a_turn() {
    let _sandbox = AuthTestSandbox::new().unwrap();
    save_native_test_login();
    let provider =
        OpenRouterProvider::new_openai_compatible_profile_runtime(MUSE_SUBSCRIPTION_PROFILE)
            .unwrap();
    jcode_base::env::remove_var(muse::EXPERIMENTAL_ENV_VAR);
    let result = provider
        .complete(&[Message::user("must not send")], &[], "system", None)
        .await;
    let error = match result {
        Ok(_) => panic!("native turn started after opt-in was disabled"),
        Err(error) => error.to_string(),
    };
    assert!(error.contains(muse::EXPERIMENTAL_ENV_VAR));
}

#[test]
fn native_muse_runtime_does_not_accept_a_paid_meta_key_as_login() {
    let _sandbox = AuthTestSandbox::new().unwrap();
    jcode_base::env::set_var(muse::EXPERIMENTAL_ENV_VAR, "1");
    jcode_base::env::set_var("META_MUSE_API_KEY", "paid-runtime-test-key");
    jcode_base::env::set_var(muse::API_KEY_ENV, "not-a-device-login");
    let result =
        OpenRouterProvider::new_openai_compatible_profile_runtime(MUSE_SUBSCRIPTION_PROFILE);
    let error = match result {
        Ok(_) => panic!("paid Meta key was accepted for native subscription login"),
        Err(error) => error.to_string(),
    };
    assert!(error.contains("No pay-as-you-go key fallback"));
}
