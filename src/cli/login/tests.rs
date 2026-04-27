use super::*;

fn set_or_clear_env(key: &str, value: Option<std::ffi::OsString>) {
    if let Some(value) = value {
        crate::env::set_var(key, value);
    } else {
        crate::env::remove_var(key);
    }
}

#[test]
fn scriptable_resume_command_matches_input_kind() {
    assert_eq!(
        scriptable_resume_command("openai", "callback_url"),
        "jcode login --provider openai --callback-url '<url-or-query>'"
    );
    assert_eq!(
        scriptable_resume_command("gemini", "auth_code"),
        "jcode login --provider gemini --auth-code '<code>'"
    );
    assert_eq!(
        scriptable_resume_command("copilot", "complete"),
        "jcode login --provider copilot --complete"
    );
}

#[test]
fn load_pending_login_removes_expired_record() {
    let _guard = crate::storage::lock_test_env();
    let temp = tempfile::TempDir::new().expect("temp dir");
    let prev_home = std::env::var_os("JCODE_HOME");
    crate::env::set_var("JCODE_HOME", temp.path());

    let path = pending_login_path("openai").expect("pending path");
    let record = PendingScriptableLoginRecord {
        expires_at_ms: current_time_ms() - 1,
        login: PendingScriptableLogin::Openai {
            account_label: "default".to_string(),
            verifier: "verifier".to_string(),
            state: "state".to_string(),
            redirect_uri: "http://localhost:1455/auth/callback".to_string(),
        },
    };
    crate::storage::write_json_secret(&path, &record).expect("write pending login");

    let err = load_pending_login(&path, "openai").expect_err("expected expired state");
    assert!(err.to_string().contains("expired"));
    assert!(!path.exists(), "expired pending login should be removed");

    set_or_clear_env("JCODE_HOME", prev_home);
}

#[test]
fn load_pending_login_accepts_legacy_format() {
    let _guard = crate::storage::lock_test_env();
    let temp = tempfile::TempDir::new().expect("temp dir");
    let prev_home = std::env::var_os("JCODE_HOME");
    crate::env::set_var("JCODE_HOME", temp.path());

    let path = pending_login_path("gemini").expect("pending path");
    let legacy = PendingScriptableLogin::Gemini {
        verifier: "verifier".to_string(),
        redirect_uri: auth::gemini::GEMINI_MANUAL_REDIRECT_URI.to_string(),
    };
    crate::storage::write_json_secret(&path, &legacy).expect("write legacy pending login");

    let loaded = load_pending_login(&path, "gemini").expect("load legacy pending login");
    match loaded {
        PendingScriptableLogin::Gemini {
            verifier,
            redirect_uri,
        } => {
            assert_eq!(verifier, "verifier");
            assert_eq!(redirect_uri, auth::gemini::GEMINI_MANUAL_REDIRECT_URI);
        }
        other => panic!("unexpected login variant: {:?}", other),
    }

    set_or_clear_env("JCODE_HOME", prev_home);
}

#[test]
fn uses_scriptable_flow_detects_dash_input_without_consuming_stdin() {
    let options = LoginOptions {
        callback_url: Some("-".to_string()),
        ..LoginOptions::default()
    };
    assert!(
        options
            .uses_scriptable_flow()
            .expect("uses scriptable flow")
    );
    assert!(options.has_provided_input());
}

#[test]
fn auto_scriptable_flow_reason_prefers_non_interactive_for_oauth_provider() {
    let provider =
        crate::provider_catalog::resolve_login_provider("openai").expect("resolve openai provider");
    let reason = auto_scriptable_flow_reason(provider, &LoginOptions::default(), false);
    assert_eq!(reason, Some("non_interactive_terminal"));
}

#[test]
fn auto_scriptable_flow_reason_uses_no_browser_reason_when_requested() {
    let provider =
        crate::provider_catalog::resolve_login_provider("claude").expect("resolve claude provider");
    let reason = auto_scriptable_flow_reason(
        provider,
        &LoginOptions {
            no_browser: true,
            ..LoginOptions::default()
        },
        true,
    );
    assert_eq!(reason, Some("no_browser_requested"));
}

#[test]
fn auto_scriptable_flow_reason_skips_api_key_only_provider() {
    let provider = crate::provider_catalog::resolve_login_provider("openrouter")
        .expect("resolve openrouter provider");
    let reason = auto_scriptable_flow_reason(provider, &LoginOptions::default(), false);
    assert_eq!(reason, None);
}

#[test]
fn auto_scriptable_flow_reason_skips_when_scriptable_input_already_explicit() {
    let provider =
        crate::provider_catalog::resolve_login_provider("openai").expect("resolve openai provider");
    let reason = auto_scriptable_flow_reason(
        provider,
        &LoginOptions {
            print_auth_url: true,
            ..LoginOptions::default()
        },
        false,
    );
    assert_eq!(reason, None);
}
