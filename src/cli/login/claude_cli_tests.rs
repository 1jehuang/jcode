use super::*;

#[test]
fn claude_code_choice_ignores_ambient_browser_suppression() {
    let _env = TestHome::new();
    let old_no_browser = std::env::var_os("NO_BROWSER");
    crate::env::set_var("NO_BROWSER", "1");
    let claude = login_provider_for_choice(&ProviderChoice::Claude).unwrap();
    assert_eq!(
        auto_scriptable_flow_reason(claude, &LoginOptions::default(), true),
        Some("no_browser_requested")
    );
    assert_eq!(
        auto_scriptable_flow_reason(
            claude,
            &LoginOptions {
                claude_code: true,
                ..LoginOptions::default()
            },
            true,
        ),
        None
    );
    set_or_clear_env("NO_BROWSER", old_no_browser);
}

#[test]
fn unchanged_configured_file_with_native_credentials_cannot_claim_new_login() {
    let temp = tempfile::TempDir::new().unwrap();
    let file = temp.path().join(".credentials.json");
    std::fs::write(&file, "old synthetic credentials").unwrap();
    let modified = std::fs::metadata(&file).unwrap().modified().unwrap();

    let error =
        configured_claude_file_is_unambiguous(&file, true, Some(modified), true).unwrap_err();
    assert!(error.to_string().contains("cannot tell which account"));
    assert!(configured_claude_file_is_unambiguous(&file, true, Some(modified), false).is_ok());
    assert!(configured_claude_file_is_unambiguous(&file, false, None, true).is_ok());
    assert!(
        configured_claude_file_is_unambiguous(
            &file,
            true,
            Some(std::time::SystemTime::UNIX_EPOCH),
            true,
        )
        .is_ok()
    );
}

#[test]
fn claude_code_method_rejects_other_provider_account_and_noninteractive_options() {
    let claude = login_provider_for_choice(&ProviderChoice::Claude).unwrap();
    let openai = login_provider_for_choice(&ProviderChoice::Openai).unwrap();
    let selected = LoginOptions {
        claude_code: true,
        ..LoginOptions::default()
    };
    assert!(
        validate_claude_code_method(openai, None, &selected)
            .unwrap_err()
            .to_string()
            .contains("only available with --provider claude")
    );
    assert!(
        validate_claude_code_method(claude, Some("work"), &selected)
            .unwrap_err()
            .to_string()
            .contains("cannot select a Jcode account label")
    );

    for incompatible in [
        LoginOptions {
            no_browser: true,
            ..selected.clone()
        },
        LoginOptions {
            json: true,
            ..selected.clone()
        },
        LoginOptions {
            print_auth_url: true,
            ..selected.clone()
        },
        LoginOptions {
            callback_url: Some("test-callback".into()),
            ..selected.clone()
        },
        LoginOptions {
            auth_code: Some("test-code".into()),
            ..selected.clone()
        },
        LoginOptions {
            complete: true,
            ..selected.clone()
        },
        LoginOptions {
            flow_id: Some("test-flow".into()),
            ..selected.clone()
        },
        LoginOptions {
            cancel: true,
            ..selected
        },
    ] {
        assert!(
            validate_claude_code_method(claude, None, &incompatible)
                .unwrap_err()
                .to_string()
                .contains("requires an interactive terminal")
        );
    }
}

#[test]
fn claude_code_native_import_refuses_previously_trusted_file_shadow() {
    let _env = TestHome::new();
    let temp = tempfile::TempDir::new().unwrap();
    let old_home = std::env::var_os("JCODE_HOME");
    let old_config = std::env::var_os("CLAUDE_CONFIG_DIR");
    let old_native = std::env::var_os("CLAUDE_CODE_OAUTH_TOKEN");
    crate::env::set_var("JCODE_HOME", temp.path());
    let config_dir = temp.path().join("claude-config");
    crate::env::set_var("CLAUDE_CONFIG_DIR", &config_dir);
    crate::env::set_var(
        "CLAUDE_CODE_OAUTH_TOKEN",
        r#"{"claudeAiOauth":{"accessToken":"native-token","refreshToken":"native-refresh","expiresAt":4102444800000}}"#,
    );
    std::fs::create_dir_all(&config_dir).unwrap();
    let file = config_dir.join(".credentials.json");
    std::fs::write(&file, r#"{"claudeAiOauth":{"accessToken":"file-token","refreshToken":"file-refresh","expiresAt":4102444800000}}"#).unwrap();
    crate::config::Config::allow_external_auth_source_for_path(
        auth::claude::CLAUDE_CODE_AUTH_SOURCE_ID,
        &file,
    )
    .unwrap();
    crate::config::Config::allow_external_auth_source(
        auth::claude::CLAUDE_CODE_NATIVE_AUTH_SOURCE_ID,
    )
    .unwrap();

    let error = reuse_claude_code_native(&file).unwrap_err();
    assert!(error.to_string().contains("takes precedence"));
    assert!(auth::claude::list_accounts().unwrap().is_empty());
    assert_eq!(
        auth::claude::load_credentials().unwrap().access_token,
        "file-token"
    );

    set_or_clear_env("CLAUDE_CODE_OAUTH_TOKEN", old_native);
    set_or_clear_env("CLAUDE_CONFIG_DIR", old_config);
    set_or_clear_env("JCODE_HOME", old_home);
}

#[test]
fn expired_claude_code_file_does_not_shadow_fresh_native_login() {
    let _env = TestHome::new();
    let temp = tempfile::TempDir::new().unwrap();
    let old_home = std::env::var_os("JCODE_HOME");
    let old_config = std::env::var_os("CLAUDE_CONFIG_DIR");
    let old_native = std::env::var_os("CLAUDE_CODE_OAUTH_TOKEN");
    crate::env::set_var("JCODE_HOME", temp.path());
    let config_dir = temp.path().join("claude-config");
    crate::env::set_var("CLAUDE_CONFIG_DIR", &config_dir);
    crate::env::set_var(
        "CLAUDE_CODE_OAUTH_TOKEN",
        r#"{"claudeAiOauth":{"accessToken":"native-token","refreshToken":"native-refresh","expiresAt":4102444800000}}"#,
    );
    std::fs::create_dir_all(&config_dir).unwrap();
    let file = config_dir.join(".credentials.json");
    std::fs::write(&file, r#"{"claudeAiOauth":{"accessToken":"old-file-token","refreshToken":"old-file-refresh","expiresAt":1}}"#).unwrap();
    crate::config::Config::allow_external_auth_source_for_path(
        auth::claude::CLAUDE_CODE_AUTH_SOURCE_ID,
        &file,
    )
    .unwrap();
    crate::config::Config::allow_external_auth_source(
        auth::claude::CLAUDE_CODE_NATIVE_AUTH_SOURCE_ID,
    )
    .unwrap();

    assert!(reuse_claude_code_native(&file).unwrap().is_some());
    assert_eq!(
        auth::claude::load_credentials().unwrap().access_token,
        "native-token"
    );

    set_or_clear_env("CLAUDE_CODE_OAUTH_TOKEN", old_native);
    set_or_clear_env("CLAUDE_CONFIG_DIR", old_config);
    set_or_clear_env("JCODE_HOME", old_home);
}

#[test]
fn shadowed_claude_code_file_login_does_not_keep_new_trust() {
    let _env = TestHome::new();
    let temp = tempfile::TempDir::new().unwrap();
    let old_home = std::env::var_os("JCODE_HOME");
    let old_config = std::env::var_os("CLAUDE_CONFIG_DIR");
    let old_native = std::env::var_os("CLAUDE_CODE_OAUTH_TOKEN");
    crate::env::set_var("JCODE_HOME", temp.path());
    crate::env::remove_var("CLAUDE_CODE_OAUTH_TOKEN");
    let config_dir = temp.path().join("claude-config");
    crate::env::set_var("CLAUDE_CONFIG_DIR", &config_dir);
    std::fs::create_dir_all(&config_dir).unwrap();
    let file = config_dir.join(".credentials.json");
    // Expired access token with a refresh token: usable, but a valid saved
    // Jcode credential takes precedence when Jcode selects credentials.
    std::fs::write(&file, r#"{"claudeAiOauth":{"accessToken":"file-token","refreshToken":"file-refresh","expiresAt":1}}"#).unwrap();
    auth::claude::set_active_account_override(None);
    auth::claude::upsert_account(auth::claude::AnthropicAccount {
        label: "saved".to_string(),
        access: "saved-token".to_string(),
        refresh: "saved-refresh".to_string(),
        expires: 4_102_444_800_000,
        email: None,
        subscription_type: None,
        scopes: Vec::new(),
    })
    .unwrap();

    CLAUDE_CODE_REUSE_TEST_APPROVAL.with(|approval| approval.set(Some(true)));
    let result = reuse_claude_code_file(auth::claude::ExternalClaudeAuthSource::ClaudeCode, &file);
    CLAUDE_CODE_REUSE_TEST_APPROVAL.with(|approval| approval.set(None));
    let error = result.unwrap_err();
    assert!(error.to_string().contains("taking precedence"));
    assert!(
        !crate::config::Config::external_auth_source_allowed_for_path(
            auth::claude::CLAUDE_CODE_AUTH_SOURCE_ID,
            &file,
        ),
        "a failed login must not leave the Claude Code file trusted"
    );

    auth::claude::set_active_account_override(None);
    set_or_clear_env("CLAUDE_CODE_OAUTH_TOKEN", old_native);
    set_or_clear_env("CLAUDE_CONFIG_DIR", old_config);
    set_or_clear_env("JCODE_HOME", old_home);
}

#[test]
fn unusable_claude_code_file_falls_back_to_native_snapshot() {
    let _env = TestHome::new();
    let temp = tempfile::TempDir::new().unwrap();
    let old_home = std::env::var_os("JCODE_HOME");
    let old_config = std::env::var_os("CLAUDE_CONFIG_DIR");
    let old_native = std::env::var_os("CLAUDE_CODE_OAUTH_TOKEN");
    crate::env::set_var("JCODE_HOME", temp.path());
    let config_dir = temp.path().join("claude-config");
    crate::env::set_var("CLAUDE_CONFIG_DIR", &config_dir);
    crate::env::set_var(
        "CLAUDE_CODE_OAUTH_TOKEN",
        r#"{"claudeAiOauth":{"accessToken":"native-token","refreshToken":"native-refresh","expiresAt":4102444800000}}"#,
    );
    std::fs::create_dir_all(&config_dir).unwrap();
    let file = config_dir.join(".credentials.json");
    std::fs::write(&file, "not an OAuth credential").unwrap();
    crate::config::Config::allow_external_auth_source_for_path(
        auth::claude::CLAUDE_CODE_AUTH_SOURCE_ID,
        &file,
    )
    .unwrap();
    crate::config::Config::allow_external_auth_source(
        auth::claude::CLAUDE_CODE_NATIVE_AUTH_SOURCE_ID,
    )
    .unwrap();

    assert!(
        reuse_claude_code_file(auth::claude::ExternalClaudeAuthSource::ClaudeCode, &file)
            .unwrap()
            .is_none()
    );
    assert!(reuse_claude_code_native(&file).unwrap().is_some());
    crate::env::remove_var("CLAUDE_CODE_OAUTH_TOKEN");
    auth::claude::set_active_account_override(None);
    assert_eq!(
        auth::claude::load_credentials().unwrap().access_token,
        "native-token"
    );

    set_or_clear_env("CLAUDE_CODE_OAUTH_TOKEN", old_native);
    set_or_clear_env("CLAUDE_CONFIG_DIR", old_config);
    set_or_clear_env("JCODE_HOME", old_home);
}

fn set_or_clear_env(key: &str, value: Option<std::ffi::OsString>) {
    match value {
        Some(value) => crate::env::set_var(key, value),
        None => crate::env::remove_var(key),
    }
}

struct TestHome {
    _guards: Vec<(&'static str, Option<std::ffi::OsString>)>,
    _sandbox: auth::test_sandbox::AuthTestSandbox,
    active: Option<String>,
}
impl TestHome {
    fn new() -> Self {
        let sandbox = auth::test_sandbox::AuthTestSandbox::new().unwrap();
        let active = auth::claude::get_active_account_override();
        auth::claude::set_active_account_override(None);
        let mut guards = Vec::new();
        for (key, value) in [
            ("HOME", sandbox.root().to_path_buf()),
            ("XDG_CONFIG_HOME", sandbox.root().join("xdg")),
            ("JCODE_GLOBAL_HOME", sandbox.root().join("global")),
            ("CLAUDE_CONFIG_DIR", sandbox.root().join("claude")),
        ] {
            guards.push((key, std::env::var_os(key)));
            crate::env::set_var(key, value);
        }
        for (key, value) in [
            ("JCODE_TEST_SESSION", "1"),
            ("CLAUDE_CODE_OAUTH_TOKEN", ""),
            ("JCODE_TRUSTED_EXTERNAL_AUTH_SOURCES", ""),
        ] {
            guards.push((key, std::env::var_os(key)));
            if key == "JCODE_TRUSTED_EXTERNAL_AUTH_SOURCES" {
                crate::env::remove_var(key);
            } else {
                crate::env::set_var(key, value);
            }
        }
        Self {
            _guards: guards,
            _sandbox: sandbox,
            active,
        }
    }
}
impl Drop for TestHome {
    fn drop(&mut self) {
        auth::claude::set_active_account_override(self.active.take());
        for (key, value) in self._guards.drain(..) {
            set_or_clear_env(key, value)
        }
    }
}

#[cfg(unix)]
struct FakeCliEnv(Vec<(&'static str, Option<std::ffi::OsString>)>);
#[cfg(unix)]
impl FakeCliEnv {
    fn set(&mut self, key: &'static str, value: impl AsRef<std::ffi::OsStr>) {
        self.0.push((key, std::env::var_os(key)));
        crate::env::set_var(key, value);
    }
    fn new(root: &Path, write_file: bool) -> Self {
        use std::os::unix::fs::PermissionsExt;
        let bin = root.join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        let cli = bin.join("claude");
        std::fs::write(
            &cli,
            r#"#!/bin/sh
[ "$1" = auth ] && [ "$2" = login ] || exit 42
printf '%s %s' "$1" "$2" > "$JCODE_FIXTURE_INVOCATION"
if [ -n "$JCODE_FIXTURE_AUTH_FILE" ]; then
  printf '%s' "$JCODE_FIXTURE_AUTH_JSON" > "$JCODE_FIXTURE_AUTH_FILE"
fi
"#,
        )
        .unwrap();
        std::fs::set_permissions(&cli, std::fs::Permissions::from_mode(0o700)).unwrap();
        let mut env = Self(Vec::new());
        let mut paths = vec![bin];
        paths.extend(std::env::split_paths(&std::env::var_os("PATH").unwrap()));
        env.set("PATH", std::env::join_paths(paths).unwrap());
        env.set("JCODE_FIXTURE_INVOCATION", root.join("invocation"));
        let dir = std::env::var_os("CLAUDE_CONFIG_DIR").unwrap();
        std::fs::create_dir_all(&dir).unwrap();
        env.set(
            "JCODE_FIXTURE_AUTH_FILE",
            if write_file {
                std::path::PathBuf::from(dir)
                    .join(".credentials.json")
                    .into_os_string()
            } else {
                std::ffi::OsString::new()
            },
        );
        env.set("JCODE_FIXTURE_AUTH_JSON",r#"{"claudeAiOauth":{"accessToken":"fake-cli-access","refreshToken":"fake-cli-refresh","expiresAt":4102444800000}}"#);
        env
    }
}
#[cfg(unix)]
impl Drop for FakeCliEnv {
    fn drop(&mut self) {
        for (key, value) in self.0.drain(..).rev() {
            set_or_clear_env(key, value)
        }
    }
}
#[cfg(unix)]
struct PromptAnswer;
#[cfg(unix)]
impl PromptAnswer {
    fn set(approved: bool) -> Self {
        CLAUDE_CODE_REUSE_TEST_APPROVAL.with(|answer| answer.set(Some(approved)));
        Self
    }
}
#[cfg(unix)]
impl Drop for PromptAnswer {
    fn drop(&mut self) {
        CLAUDE_CODE_REUSE_TEST_APPROVAL.with(|answer| answer.set(None));
    }
}

#[cfg(unix)]
#[tokio::test]
async fn fake_cli_sign_in_imports_a_validated_native_snapshot_only_after_approval() {
    let home = TestHome::new();
    let mut env = FakeCliEnv::new(home._sandbox.root(), false);
    env.set("CLAUDE_CODE_OAUTH_TOKEN",r#"{"claudeAiOauth":{"accessToken":"fake-native-access","refreshToken":"fake-native-refresh","expiresAt":4102444800000}}"#);
    let _answer = PromptAnswer::set(true);
    login_claude_code_flow(async { Ok(()) }).await.unwrap();
    assert_eq!(
        std::fs::read_to_string(home._sandbox.root().join("invocation")).unwrap(),
        "auth login"
    );
    let accounts = auth::claude::list_accounts().unwrap();
    assert_eq!(accounts.len(), 1);
    assert_eq!(accounts[0].access, "fake-native-access");
    assert!(auth::claude::native_source_allowed());
}

#[cfg(unix)]
#[tokio::test]
async fn fake_cli_file_sign_in_declined_approval_does_not_save_jcode_trust_or_accounts() {
    let home = TestHome::new();
    let _env = FakeCliEnv::new(home._sandbox.root(), true);
    let _answer = PromptAnswer::set(false);
    assert!(
        login_claude_code_flow(async { panic!("unapproved login must not validate") })
            .await
            .unwrap_err()
            .to_string()
            .contains("not granted")
    );
    let source = auth::claude::ExternalClaudeAuthSource::ClaudeCode;
    assert!(
        !crate::config::Config::external_auth_source_allowed_for_path(
            source.source_id(),
            &source.path().unwrap()
        )
    );
    assert!(auth::claude::list_accounts().unwrap().is_empty());
}

#[cfg(unix)]
#[tokio::test]
async fn fake_cli_completion_captures_tokens_after_validation_refresh() {
    let home = TestHome::new();
    let mut env = FakeCliEnv::new(home._sandbox.root(), false);
    env.set("CLAUDE_CODE_OAUTH_TOKEN",r#"{"claudeAiOauth":{"accessToken":"fake-native-access","refreshToken":"fake-native-refresh","expiresAt":4102444800000}}"#);
    let _answer = PromptAnswer::set(true);
    let flow = auth::claude_cli_completion::LoginFlow::new().unwrap();
    let attempt = auth::claude_cli_completion::claim(&flow.path).unwrap();
    let completed = login_claude_code_flow(async {
        assert!(flow.result().unwrap().is_none());
        let account = auth::claude::list_accounts().unwrap().remove(0);
        // Model a one-time native import without a live environment override.
        env.set("CLAUDE_CODE_OAUTH_TOKEN", "");
        auth::claude::update_account_tokens(
            &account.label,
            "validation-refreshed-access",
            "validation-refreshed-refresh",
            account.expires,
        )?;
        Ok(())
    })
    .await
    .unwrap();
    let final_credentials = auth::claude::load_credentials().unwrap();
    assert_eq!(
        final_credentials.access_token,
        "validation-refreshed-access"
    );
    assert_eq!(
        completed.fingerprint,
        auth::claude_cli_completion::fingerprint(&final_credentials)
    );
    auth::claude_cli_completion::finish(&flow.path, &attempt, Some(completed.clone())).unwrap();
    auth::claude_cli_completion::with_verified_receipt(
        &flow.path,
        &attempt,
        &completed.fingerprint,
        || Ok(()),
    )
    .unwrap();
}

#[cfg(unix)]
#[tokio::test]
async fn fake_cli_completion_validation_failure_never_reports_success() {
    let home = TestHome::new();
    let mut env = FakeCliEnv::new(home._sandbox.root(), false);
    env.set("CLAUDE_CODE_OAUTH_TOKEN",r#"{"claudeAiOauth":{"accessToken":"fake-native-access","refreshToken":"fake-native-refresh","expiresAt":4102444800000}}"#);
    let _answer = PromptAnswer::set(true);
    let flow = auth::claude_cli_completion::LoginFlow::new().unwrap();
    let attempt = auth::claude_cli_completion::claim(&flow.path).unwrap();
    let result = login_claude_code_flow(async {
        assert!(flow.result().unwrap().is_none());
        anyhow::bail!("synthetic validation failure")
    })
    .await;
    assert!(
        result
            .unwrap_err()
            .to_string()
            .contains("synthetic validation failure")
    );
    auth::claude_cli_completion::finish(&flow.path, &attempt, None).unwrap();
    assert!(matches!(flow.result().unwrap(), Some(None)));
}

#[cfg(unix)]
#[tokio::test]
async fn fake_cli_completion_preserves_account_choice_during_validation() {
    let home = TestHome::new();
    let mut env = FakeCliEnv::new(home._sandbox.root(), false);
    env.set("CLAUDE_CODE_OAUTH_TOKEN",r#"{"claudeAiOauth":{"accessToken":"fake-native-access","refreshToken":"fake-native-refresh","expiresAt":4102444800000}}"#);
    let _answer = PromptAnswer::set(true);
    let result = login_claude_code_flow(async {
        auth::claude::set_active_account_override(Some("later-manual-choice".into()));
        Ok(())
    })
    .await;
    assert!(
        result
            .unwrap_err()
            .to_string()
            .contains("selection changed during validation")
    );
    assert_eq!(
        auth::claude::get_active_account_override().as_deref(),
        Some("later-manual-choice")
    );
}

#[test]
fn credential_file_preflight_distinguishes_missing_regular_and_invalid_paths() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("credential");
    assert!(credential_file_modified(&path).unwrap().is_none());
    std::fs::write(&path, "synthetic credential").unwrap();
    assert!(credential_file_modified(&path).unwrap().is_some());
    assert!(
        credential_file_modified(temp.path())
            .unwrap_err()
            .to_string()
            .contains("regular file")
    );
}
#[cfg(unix)]
#[test]
fn credential_file_preflight_rejects_symlinks_before_cli_sign_in() {
    let temp = tempfile::tempdir().unwrap();
    let target = temp.path().join("target");
    let link = temp.path().join("credential");
    std::fs::write(&target, "synthetic credential").unwrap();
    std::os::unix::fs::symlink(target, &link).unwrap();
    assert!(
        credential_file_modified(&link)
            .unwrap_err()
            .to_string()
            .contains("regular file")
    );
}
