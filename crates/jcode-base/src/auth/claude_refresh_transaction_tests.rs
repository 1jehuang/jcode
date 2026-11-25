use super::*;
use crate::auth::claude::{self, AnthropicAccount, JcodeAuthFile};

struct EnvGuard(&'static str, Option<std::ffi::OsString>);
impl EnvGuard {
    fn set(key: &'static str, value: Option<&std::ffi::OsStr>) -> Self {
        let saved = std::env::var_os(key);
        match value {
            Some(value) => crate::env::set_var(key, value),
            None => crate::env::remove_var(key),
        }
        Self(key, saved)
    }
}
impl Drop for EnvGuard {
    fn drop(&mut self) {
        match &self.1 {
            Some(value) => crate::env::set_var(self.0, value),
            None => crate::env::remove_var(self.0),
        }
    }
}

fn login(label: &str, refresh: &str) -> AnthropicAccount {
    AnthropicAccount {
        label: label.into(),
        access: "original-access".into(),
        refresh: refresh.into(),
        expires: 1,
        email: Some("original@example.invalid".into()),
        subscription_type: Some("max".into()),
        scopes: vec!["user:inference".into()],
    }
}
fn rotated() -> OAuthTokens {
    OAuthTokens {
        access_token: "rotated-access".into(),
        refresh_token: "rotated-refresh".into(),
        expires_at: 9000,
        id_token: None,
        scopes: vec![],
    }
}
fn seed() {
    let mut auth = JcodeAuthFile::default();
    auth.anthropic_accounts = vec![login("work", "original-refresh")];
    auth.active_anthropic_account = Some("work".into());
    claude::save_auth_file(&auth).unwrap();
}
fn auth_bytes() -> Vec<u8> {
    std::fs::read(crate::storage::jcode_dir().unwrap().join("auth.json")).unwrap()
}

#[tokio::test]
async fn claude_refresh_cas_delayed_replacement_removal_and_ambiguity_preserve_disk() {
    let _sandbox = crate::auth::test_sandbox::AuthTestSandbox::new().unwrap();
    for change in ["replacement", "removed", "ambiguous", "reidentified"] {
        seed();
        let mut before = Vec::new();
        let result = refresh_claude_tokens_with("original-refresh", "work", async {
            let mut auth = claude::load_auth_file().unwrap();
            match change {
                "replacement" => auth.anthropic_accounts[0].refresh = "new-login-refresh".into(),
                "removed" => {
                    auth.anthropic_accounts.clear();
                    auth.active_anthropic_account = None;
                }
                "ambiguous" => auth
                    .anthropic_accounts
                    .push(login("other", "original-refresh")),
                "reidentified" => {
                    auth.anthropic_accounts[0].email = Some("other@example.invalid".into())
                }
                _ => unreachable!(),
            }
            claude::save_auth_file(&auth).unwrap();
            before = auth_bytes();
            Ok(rotated())
        })
        .await;
        let error = result.unwrap_err().to_string();
        assert!(!error.contains("original-refresh"));
        assert_eq!(auth_bytes(), before, "{change}");
    }
}

#[tokio::test]
async fn claude_refresh_cas_follows_rename_preserving_default_metadata_and_scopes() {
    let _sandbox = crate::auth::test_sandbox::AuthTestSandbox::new().unwrap();
    seed();
    refresh_claude_tokens_with("original-refresh", "work", async {
        let mut auth = claude::load_auth_file().unwrap();
        auth.anthropic_accounts[0].label = "renamed-work".into();
        auth.anthropic_accounts
            .push(login("work", "replacement-refresh"));
        auth.active_anthropic_account = Some("work".into());
        claude::save_auth_file(&auth).unwrap();
        Ok(rotated())
    })
    .await
    .unwrap();
    let saved = claude::load_auth_file().unwrap();
    let account = &saved.anthropic_accounts[0];
    assert_eq!(account.label, "renamed-work");
    assert_eq!(account.refresh, "rotated-refresh");
    assert_eq!(account.email.as_deref(), Some("original@example.invalid"));
    assert_eq!(account.subscription_type.as_deref(), Some("max"));
    assert_eq!(account.scopes, vec!["user:inference"]);
    assert_eq!(saved.active_anthropic_account.as_deref(), Some("work"));
    assert_eq!(saved.anthropic_accounts[1].refresh, "replacement-refresh");
}

#[tokio::test]
async fn claude_refresh_cas_rejects_superseded_or_empty_token_before_polling_request() {
    let _sandbox = crate::auth::test_sandbox::AuthTestSandbox::new().unwrap();
    seed();
    let before = auth_bytes();
    for token in ["stale-refresh", ""] {
        let mut polled = false;
        let result = refresh_claude_tokens_with(token, "work", async {
            polled = true;
            Ok(rotated())
        })
        .await;
        assert!(result.is_err());
        assert!(!polled);
        assert_eq!(auth_bytes(), before);
    }
}

#[tokio::test]
async fn claude_refresh_cas_failure_and_cancel_do_not_write() {
    let _sandbox = crate::auth::test_sandbox::AuthTestSandbox::new().unwrap();
    seed();
    let before = auth_bytes();
    assert!(
        refresh_claude_tokens_with("original-refresh", "work", async {
            anyhow::bail!("synthetic network failure")
        })
        .await
        .is_err()
    );
    let (started_tx, started_rx) = tokio::sync::oneshot::channel();
    let task = tokio::spawn(async {
        refresh_claude_tokens_with("original-refresh", "work", async {
            started_tx.send(()).unwrap();
            std::future::pending::<Result<OAuthTokens>>().await
        })
        .await
    });
    started_rx.await.unwrap();
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    assert_eq!(auth_bytes(), before);
}

#[tokio::test]
async fn claude_refresh_cas_external_source_requires_current_approval_and_unclaimed_label() {
    let sandbox = crate::auth::test_sandbox::AuthTestSandbox::new().unwrap();
    let config_dir = sandbox.root().join("claude-config");
    let _config_dir = EnvGuard::set("CLAUDE_CONFIG_DIR", Some(config_dir.as_os_str()));
    let _trust = EnvGuard::set("JCODE_TRUSTED_EXTERNAL_AUTH_SOURCES", None);
    let _native = EnvGuard::set("CLAUDE_CODE_OAUTH_TOKEN", None);
    std::fs::create_dir_all(&config_dir).unwrap();
    let external = config_dir.join(".credentials.json");
    std::fs::write(&external,
        r#"{"claudeAiOauth":{"accessToken":"external-access","refreshToken":"external-refresh","expiresAt":1}}"#,
    ).unwrap();
    claude::save_auth_file(&JcodeAuthFile::default()).unwrap();
    let before = auth_bytes();
    let mut polled = false;
    assert!(
        refresh_claude_tokens_with("external-refresh", "work", async {
            polled = true;
            Ok(rotated())
        })
        .await
        .is_err()
    );
    assert!(!polled);
    assert_eq!(auth_bytes(), before);
    use crate::config::Config;
    for change in ["revoke", "source-replaced", "label-claimed", "success"] {
        claude::save_auth_file(&JcodeAuthFile::default()).unwrap();
        Config::allow_external_auth_source_for_path(claude::CLAUDE_CODE_AUTH_SOURCE_ID, &external)
            .unwrap();
        let original = std::fs::read(&external).unwrap();
        let mut before = Vec::new();
        let result = refresh_claude_tokens_with("external-refresh", "work", async {
            match change {
                "revoke" => Config::revoke_external_auth_source_for_path(claude::CLAUDE_CODE_AUTH_SOURCE_ID, &external).unwrap(),
                "source-replaced" => std::fs::write(&external, r#"{"claudeAiOauth":{"accessToken":"new-login","refreshToken":"replacement-refresh","expiresAt":1}}"#).unwrap(),
                "label-claimed" => seed(),
                "success" => (),
                _ => unreachable!(),
            }
            before = auth_bytes();
            Ok(rotated())
        }).await;
        std::fs::write(&external, original).unwrap();
        if change == "success" {
            result.unwrap();
            let saved = claude::load_auth_file().unwrap();
            assert_eq!(saved.anthropic_accounts.len(), 1);
            assert_eq!(saved.anthropic_accounts[0].refresh, "rotated-refresh");
        } else {
            assert!(result.is_err(), "{change}");
            assert_eq!(auth_bytes(), before, "{change}");
        }
    }
}

#[test]
fn claude_refresh_cas_config_directory_must_be_absolute() {
    let sandbox = crate::auth::test_sandbox::AuthTestSandbox::new().unwrap();
    let _config = EnvGuard::set(
        "CLAUDE_CONFIG_DIR",
        Some(std::ffi::OsStr::new("relative-config")),
    );
    assert!(claude::ExternalClaudeAuthSource::ClaudeCode.path().is_err());
    drop(_config);
    let absolute = sandbox.root().join("native");
    let _config = EnvGuard::set("CLAUDE_CONFIG_DIR", Some(absolute.as_os_str()));
    assert_eq!(
        claude::ExternalClaudeAuthSource::ClaudeCode.path().unwrap(),
        absolute.join(".credentials.json")
    );
}
