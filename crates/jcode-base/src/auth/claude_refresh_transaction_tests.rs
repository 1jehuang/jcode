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
        record_id: Some(uuid::Uuid::new_v4().to_string()),
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

#[tokio::test]
async fn claude_cli_validation_follows_only_its_original_refresh_chain() {
    let _sandbox = crate::auth::test_sandbox::AuthTestSandbox::new().unwrap();
    for replacement in [false, true] {
        seed();
        let credentials = claude::load_credentials_for_account("work").unwrap();
        let validated =
            crate::auth::claude_cli_completion::with_validation_refreshes(credentials, async {
                let refresh = if replacement {
                    claude::upsert_account(login("work", "replacement-refresh"))?;
                    "replacement-refresh"
                } else {
                    "original-refresh"
                };
                refresh_claude_tokens_with(refresh, "work", async { Ok(rotated()) }).await?;
                Ok(())
            })
            .await
            .unwrap();
        assert_eq!(
            validated.refresh_token,
            if replacement {
                "original-refresh"
            } else {
                "rotated-refresh"
            }
        );
        assert_eq!(
            claude::load_credentials_for_account("work")
                .unwrap()
                .refresh_token,
            "rotated-refresh"
        );
    }
}
#[tokio::test]
async fn claude_cli_validation_follows_owned_spawned_refreshes_only() {
    use crate::auth::claude_cli_completion::{
        inherit_validation_refreshes, with_validation_refreshes,
    };
    let _sandbox = crate::auth::test_sandbox::AuthTestSandbox::new().unwrap();
    for owned in [true, false] {
        seed();
        let credentials = claude::load_credentials_for_account("work").unwrap();
        let validated = with_validation_refreshes(credentials, async {
            let refresh = async {
                refresh_claude_tokens_with("original-refresh", "work", async { Ok(rotated()) })
                    .await
                    .unwrap();
            };
            if owned {
                tokio::spawn(inherit_validation_refreshes(refresh))
                    .await
                    .unwrap();
            } else {
                tokio::spawn(refresh).await.unwrap();
            }
            Ok(())
        })
        .await
        .unwrap();
        assert_eq!(
            validated.refresh_token,
            if owned {
                "rotated-refresh"
            } else {
                "original-refresh"
            }
        );
        assert_eq!(
            claude::load_credentials_for_account("work")
                .unwrap()
                .refresh_token,
            "rotated-refresh"
        );
    }
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

#[tokio::test]
async fn claude_refresh_cas_background_profile_hydration_keeps_the_rotation() {
    let _sandbox = crate::auth::test_sandbox::AuthTestSandbox::new().unwrap();
    seed();
    let mut auth = claude::load_auth_file().unwrap();
    auth.anthropic_accounts[0].email = None;
    claude::save_auth_file(&auth).unwrap();
    refresh_claude_tokens_with("original-refresh", "work", async {
        claude::update_account_profile_metadata(
            "work",
            Some("hydrated@example.invalid".into()),
            Some("new-plan".into()),
        )
        .unwrap();
        Ok(rotated())
    })
    .await
    .unwrap();
    let saved = claude::load_auth_file().unwrap();
    let account = &saved.anthropic_accounts[0];
    assert_eq!(account.email.as_deref(), Some("hydrated@example.invalid"));
    assert_eq!(account.subscription_type.as_deref(), Some("new-plan"));
    assert_eq!(account.refresh, "rotated-refresh");
    assert_eq!(account.scopes, ["user:inference"]);
    assert_eq!(saved.active_anthropic_account.as_deref(), Some("work"));
}

#[tokio::test]
async fn claude_refresh_cas_all_generic_external_sources_observe_approval_and_token_changes() {
    use crate::auth::external::ExternalAuthSource;
    use crate::config::Config;
    let sandbox = crate::auth::test_sandbox::AuthTestSandbox::new().unwrap();
    let _trust = EnvGuard::set("JCODE_TRUSTED_EXTERNAL_AUTH_SOURCES", None);
    let _native = EnvGuard::set("CLAUDE_CODE_OAUTH_TOKEN", None);
    let config_dir = sandbox.root().join("unused-native-config");
    let _config = EnvGuard::set("CLAUDE_CONFIG_DIR", Some(config_dir.as_os_str()));
    let competing_path = ExternalAuthSource::OpenCode.path().unwrap();
    let add_competing_source = || {
        std::fs::create_dir_all(competing_path.parent().unwrap()).unwrap();
        std::fs::write(
            &competing_path,
            serde_json::json!({"anthropic":{"type":"oauth", "access":"competing-access",
                "refresh":"competing-refresh", "expires":i64::MAX}})
            .to_string(),
        )
        .unwrap();
        Config::allow_external_auth_source_for_path(
            ExternalAuthSource::OpenCode.source_id(),
            &competing_path,
        )
        .unwrap();
        assert_eq!(
            crate::auth::external::load_anthropic_oauth_tokens()
                .unwrap()
                .refresh_token,
            "competing-refresh"
        );
    };
    for source in [
        ExternalAuthSource::Pi,
        ExternalAuthSource::OpenClaw,
        ExternalAuthSource::Hermes,
    ] {
        let entry = serde_json::json!({"type":"oauth", "provider":"anthropic",
            "access":"external-access", "refresh":"external-refresh", "expires":1});
        let body = match source {
            ExternalAuthSource::Pi => serde_json::json!({"anthropic":entry}),
            ExternalAuthSource::OpenClaw => serde_json::json!({"version":1,
                "profiles":{"anthropic:default":entry}}),
            ExternalAuthSource::Hermes => serde_json::json!({"version":1,
                "credential_pool":{"anthropic":[{"auth_type":"oauth_external",
                    "access_token":"external-access", "refresh_token":"external-refresh",
                    "expires_at_ms":1}]}}),
            _ => unreachable!(),
        };
        let path = if source == ExternalAuthSource::OpenClaw {
            sandbox
                .root()
                .join("external/.openclaw/agents/main/agent/auth-profiles.json")
        } else {
            source.path().unwrap()
        };
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        for change in [
            "revoke",
            "source-replaced",
            "label-claimed",
            "success",
            "shadowed-before",
            "shadowed-during",
            "shadowed-revoke",
            "shadowed-replace",
        ] {
            if competing_path.exists() {
                Config::revoke_external_auth_source_for_path(
                    ExternalAuthSource::OpenCode.source_id(),
                    &competing_path,
                )
                .unwrap();
                std::fs::remove_file(&competing_path).unwrap();
            }
            claude::save_auth_file(&JcodeAuthFile::default()).unwrap();
            std::fs::write(&path, body.to_string()).unwrap();
            Config::allow_external_auth_source_for_path(source.source_id(), &path).unwrap();
            assert_eq!(
                claude::load_credentials().unwrap().refresh_token,
                "external-refresh"
            );
            if matches!(
                change,
                "shadowed-before" | "shadowed-revoke" | "shadowed-replace"
            ) {
                add_competing_source();
            }
            let mut before = Vec::new();
            let mut polled = false;
            let result = refresh_claude_tokens_with("external-refresh", "work", async {
                polled = true;
                if change == "shadowed-during" {
                    add_competing_source();
                }
                match change {
                    "revoke" | "shadowed-revoke" => {
                        Config::revoke_external_auth_source_for_path(source.source_id(), &path)
                            .unwrap()
                    }
                    "source-replaced" | "shadowed-replace" => std::fs::write(
                        &path,
                        body.to_string()
                            .replace("external-refresh", "replacement-refresh"),
                    )
                    .unwrap(),
                    "label-claimed" => seed(),
                    "success" | "shadowed-before" | "shadowed-during" => (),
                    _ => unreachable!(),
                }
                before = auth_bytes();
                Ok(rotated())
            })
            .await;
            assert!(
                polled,
                "{source:?} {change}: refresh was rejected before the request"
            );
            if matches!(change, "success" | "shadowed-before" | "shadowed-during") {
                result.unwrap();
                assert_eq!(
                    claude::load_auth_file().unwrap().anthropic_accounts[0].refresh,
                    "rotated-refresh"
                );
            } else {
                let error = result.unwrap_err().to_string();
                assert!(!error.contains("external-refresh"));
                assert_eq!(auth_bytes(), before, "{source:?} {change}");
            }
        }
        Config::revoke_external_auth_source_for_path(source.source_id(), &path).unwrap();
        std::fs::remove_file(path).unwrap();
    }
}
