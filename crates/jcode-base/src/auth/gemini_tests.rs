use super::*;
use crate::storage::lock_test_env;

struct GeminiOauthEnvReset {
    prev_client_id: Option<String>,
    prev_client_secret: Option<String>,
}

impl Drop for GeminiOauthEnvReset {
    fn drop(&mut self) {
        if let Some(value) = &self.prev_client_id {
            crate::env::set_var(GEMINI_CLIENT_ID_ENV, value);
        } else {
            crate::env::remove_var(GEMINI_CLIENT_ID_ENV);
        }

        if let Some(value) = &self.prev_client_secret {
            crate::env::set_var(GEMINI_CLIENT_SECRET_ENV, value);
        } else {
            crate::env::remove_var(GEMINI_CLIENT_SECRET_ENV);
        }
    }
}

fn set_test_gemini_oauth_env() -> GeminiOauthEnvReset {
    let reset = GeminiOauthEnvReset {
        prev_client_id: std::env::var(GEMINI_CLIENT_ID_ENV).ok(),
        prev_client_secret: std::env::var(GEMINI_CLIENT_SECRET_ENV).ok(),
    };
    crate::env::set_var(
        GEMINI_CLIENT_ID_ENV,
        "test-gemini-client.apps.googleusercontent.com",
    );
    crate::env::set_var(GEMINI_CLIENT_SECRET_ENV, "test-gemini-client-secret");
    reset
}

#[test]
fn parses_env_command_with_args() {
    let resolved =
        resolve_gemini_cli_command_with(Some("npx @google/gemini-cli --proxy test"), |_| false);
    assert_eq!(
        resolved,
        GeminiCliCommand {
            program: "npx".to_string(),
            args: vec![
                "@google/gemini-cli".to_string(),
                "--proxy".to_string(),
                "test".to_string(),
            ],
        }
    );
}

#[test]
fn falls_back_to_gemini_binary_when_available() {
    let resolved = resolve_gemini_cli_command_with(None, |cmd| cmd == "gemini");
    assert_eq!(resolved.program, "gemini");
    assert!(resolved.args.is_empty());
}

#[test]
fn falls_back_to_npx_when_gemini_binary_missing() {
    let resolved = resolve_gemini_cli_command_with(None, |cmd| cmd == "npx");
    assert_eq!(resolved.program, "npx");
    assert_eq!(resolved.args, vec!["@google/gemini-cli"]);
}

#[test]
fn display_includes_args_when_present() {
    let command = GeminiCliCommand {
        program: "npx".to_string(),
        args: vec!["@google/gemini-cli".to_string()],
    };
    assert_eq!(command.display(), "npx @google/gemini-cli");
}

#[test]
fn build_manual_auth_url_contains_expected_redirect_uri() {
    let _guard = lock_test_env();
    let _env = set_test_gemini_oauth_env();
    let url = build_manual_auth_url(GEMINI_MANUAL_REDIRECT_URI, "challenge-123", "state-123")
        .expect("manual auth url");
    assert!(url.contains("codeassist.google.com%2Fauthcode"));
    assert!(url.contains("code_challenge=challenge-123"));
    assert!(url.contains("state=state-123"));
}

#[test]
fn build_web_auth_url_includes_pkce_parameters() {
    let _guard = lock_test_env();
    let _env = set_test_gemini_oauth_env();
    let url = build_web_auth_url(
        "http://127.0.0.1:45619/oauth2callback",
        "challenge-123",
        "state-123",
    )
    .expect("web auth url");
    assert!(url.contains("127.0.0.1%3A45619%2Foauth2callback"));
    assert!(url.contains("state=state-123"));
    assert!(url.contains("code_challenge=challenge-123"));
    assert!(url.contains("code_challenge_method=S256"));
    assert!(!url.contains("code_verifier="));
}

#[test]
fn resolve_callback_or_manual_code_accepts_manual_code_with_expected_state() {
    let code = resolve_callback_or_manual_code("  authcode-123  ", Some("state-123"))
        .expect("manual code should be accepted");
    assert_eq!(code, "authcode-123");
}

#[test]
fn resolve_callback_or_manual_code_validates_state_for_callback_input() {
    let code = resolve_callback_or_manual_code(
        "http://127.0.0.1:1455/callback?code=authcode-123&state=state-123",
        Some("state-123"),
    )
    .expect("callback should parse");
    assert_eq!(code, "authcode-123");

    let err = resolve_callback_or_manual_code(
        "http://127.0.0.1:1455/callback?code=authcode-123&state=wrong-state",
        Some("state-123"),
    )
    .expect_err("mismatched state should fail");
    assert!(err.to_string().contains("OAuth state mismatch"));
}

#[test]
fn uses_hardcoded_credentials_when_env_missing() {
    let _guard = lock_test_env();
    crate::env::remove_var(GEMINI_CLIENT_ID_ENV);
    crate::env::remove_var(GEMINI_CLIENT_SECRET_ENV);

    // Should succeed with hardcoded credentials
    let url = build_manual_auth_url(GEMINI_MANUAL_REDIRECT_URI, "challenge-123", "state-123")
        .expect("should use hardcoded credentials");
    assert!(url.contains("codeassist.google.com%2Fauthcode"));
    assert!(url.contains("code_challenge=challenge-123"));
    // Should contain the hardcoded client ID
    assert!(
        url.contains("681255809395-oo8ft2oprdrnp9e3aqf6av3hmdib135j.apps.googleusercontent.com")
    );
}

#[test]
fn imports_cli_oauth_tokens_when_native_tokens_missing() {
    let _guard = lock_test_env();
    let temp = tempfile::TempDir::new().expect("tempdir");
    let prev_home = std::env::var_os("JCODE_HOME");
    crate::env::set_var("JCODE_HOME", temp.path());

    let cli_path = gemini_cli_oauth_path().expect("cli path");
    std::fs::create_dir_all(cli_path.parent().unwrap()).expect("create cli dir");
    std::fs::write(
        &cli_path,
        r#"{"access_token":"at-123","refresh_token":"rt-456","expiry_date":4102444800000}"#,
    )
    .expect("write cli token file");
    crate::config::Config::allow_external_auth_source_for_path(
        GEMINI_CLI_AUTH_SOURCE_ID,
        &cli_path,
    )
    .expect("trust cli auth path");

    let tokens = load_tokens().expect("load tokens");
    assert_eq!(tokens.access_token, "at-123");
    assert_eq!(tokens.refresh_token, "rt-456");
    assert_eq!(tokens.expires_at, 4102444800000);

    if let Some(prev_home) = prev_home {
        crate::env::set_var("JCODE_HOME", prev_home);
    } else {
        crate::env::remove_var("JCODE_HOME");
    }
}

#[cfg(unix)]
#[test]
fn imports_cli_oauth_tokens_without_changing_external_permissions() {
    use std::os::unix::fs::PermissionsExt;

    let _guard = lock_test_env();
    let temp = tempfile::TempDir::new().expect("tempdir");
    let prev_home = std::env::var_os("JCODE_HOME");
    crate::env::set_var("JCODE_HOME", temp.path());

    let cli_path = gemini_cli_oauth_path().expect("cli path");
    std::fs::create_dir_all(cli_path.parent().unwrap()).expect("create cli dir");
    std::fs::write(
        &cli_path,
        r#"{"access_token":"at-123","refresh_token":"rt-456","expiry_date":4102444800000}"#,
    )
    .expect("write cli token file");
    std::fs::set_permissions(
        cli_path.parent().unwrap(),
        std::fs::Permissions::from_mode(0o755),
    )
    .expect("set dir perms");
    std::fs::set_permissions(&cli_path, std::fs::Permissions::from_mode(0o644))
        .expect("set file perms");
    crate::config::Config::allow_external_auth_source_for_path(
        GEMINI_CLI_AUTH_SOURCE_ID,
        &cli_path,
    )
    .expect("trust cli auth path");

    let tokens = load_tokens().expect("load tokens");
    assert_eq!(tokens.access_token, "at-123");

    let dir_mode = std::fs::metadata(cli_path.parent().unwrap())
        .expect("stat dir")
        .permissions()
        .mode()
        & 0o777;
    let file_mode = std::fs::metadata(&cli_path)
        .expect("stat file")
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(dir_mode, 0o755);
    assert_eq!(file_mode, 0o644);

    if let Some(prev_home) = prev_home {
        crate::env::set_var("JCODE_HOME", prev_home);
    } else {
        crate::env::remove_var("JCODE_HOME");
    }
}
