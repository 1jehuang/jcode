use super::*;

fn gemini_profile() -> ResolvedOpenAiCompatibleProfile {
    crate::provider_catalog::resolve_openai_compatible_profile(
        crate::provider_catalog::GEMINI_OPENAI_COMPAT_PROFILE,
    )
}

/// Point config resolution at a scratch home with no Gemini key anywhere.
fn isolated_home() -> tempfile::TempDir {
    let home = tempfile::tempdir().expect("tempdir");
    crate::env::set_var("JCODE_HOME", home.path());
    crate::env::remove_var("GEMINI_API_KEY");
    crate::env::remove_var("GOOGLE_API_KEY");
    home
}

#[test]
fn no_notice_when_nothing_is_configured() {
    let _guard = crate::storage::lock_test_env();
    let _home = isolated_home();

    assert_eq!(existing_api_key_notice(&gemini_profile()), None);

    crate::env::remove_var("JCODE_HOME");
}

#[test]
fn notice_names_the_environment_variable_when_the_env_wins() {
    let _guard = crate::storage::lock_test_env();
    let _home = isolated_home();
    crate::env::set_var("GEMINI_API_KEY", "AIza-from-env");

    let notice = existing_api_key_notice(&gemini_profile()).expect("configured key");
    assert!(
        notice.contains("GEMINI_API_KEY environment variable"),
        "{notice}"
    );
    // The point of the notice: say the prompt is not stuck, and how to keep
    // the existing key.
    assert!(notice.contains("Ctrl+C"), "{notice}");

    crate::env::remove_var("GEMINI_API_KEY");
    crate::env::remove_var("JCODE_HOME");
}

#[test]
fn notice_names_the_config_file_when_only_the_file_has_a_key() {
    let _guard = crate::storage::lock_test_env();
    let _home = isolated_home();

    let config_dir = crate::storage::app_config_dir().expect("config dir");
    std::fs::create_dir_all(&config_dir).expect("create config dir");
    std::fs::write(
        config_dir.join("gemini.env"),
        "GEMINI_API_KEY=AIza-from-file\n",
    )
    .expect("write env file");

    let notice = existing_api_key_notice(&gemini_profile()).expect("configured key");
    assert!(notice.contains("gemini.env"), "{notice}");
    assert!(!notice.contains("environment variable"), "{notice}");

    crate::env::remove_var("JCODE_HOME");
}
