use crate::provider_catalog::{
    LoginProviderDescriptor, LoginProviderTarget, OpenAiCompatibleProfile,
};
use anyhow::{Result, bail};
use std::time::Duration;

const VALIDATION_TIMEOUT: Duration = Duration::from_secs(6);

/// Outcome of an immediate lightweight API key validation probe.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ApiKeyValidationOutcome {
    /// Endpoint returned 2xx; credentials are valid.
    Verified,
    /// Endpoint explicitly rejected the credentials (401/403).
    Rejected { status: u16, detail: Option<String> },
    /// Probe could not determine validity (offline, timeout, 404/5xx, or unsupported provider).
    Unverified { reason: String },
}

/// Summary of where an API key was saved.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SavedApiKeyLogin {
    pub provider_id: &'static str,
    pub display_name: &'static str,
    pub env_file: String,
    pub env_var: String,
    pub openai_compatible_profile: Option<OpenAiCompatibleProfile>,
    pub default_model: Option<String>,
}

/// Validate an API key against the provider's lightweight auth-checked endpoint.
///
/// Never returns an error for network/offline issues; instead returns
/// [`ApiKeyValidationOutcome::Unverified`] so offline users are never blocked,
/// while explicit 401/403 responses return [`ApiKeyValidationOutcome::Rejected`].
pub async fn validate_api_key_for_provider(
    provider: LoginProviderDescriptor,
    api_key: &str,
) -> ApiKeyValidationOutcome {
    let client = crate::provider::shared_http_client();
    validate_api_key_with_client(&client, provider, api_key, None).await
}

pub async fn validate_api_key_with_client(
    client: &reqwest::Client,
    provider: LoginProviderDescriptor,
    api_key: &str,
    base_url_override: Option<&str>,
) -> ApiKeyValidationOutcome {
    let key = api_key.trim();
    if key.is_empty() {
        return ApiKeyValidationOutcome::Rejected {
            status: 400,
            detail: Some("API key is empty".to_string()),
        };
    }

    let request = match provider.target {
        LoginProviderTarget::ClaudeApiKey => {
            let base = base_url_override
                .unwrap_or("https://api.anthropic.com/v1")
                .trim_end_matches('/');
            client
                .get(format!("{base}/models"))
                .header("x-api-key", key)
                .header("anthropic-version", "2023-06-01")
        }
        LoginProviderTarget::OpenAiApiKey => {
            let base = base_url_override
                .unwrap_or("https://api.openai.com/v1")
                .trim_end_matches('/');
            client.get(format!("{base}/models")).bearer_auth(key)
        }
        LoginProviderTarget::OpenRouter => {
            let base = base_url_override
                .unwrap_or("https://openrouter.ai/api/v1")
                .trim_end_matches('/');
            client.get(format!("{base}/auth/key")).bearer_auth(key)
        }
        LoginProviderTarget::OpenAiCompatible(profile) => {
            if profile.id == "muse-subscription" {
                return ApiKeyValidationOutcome::Unverified {
                    reason: "provider uses device login".to_string(),
                };
            }
            let resolved = crate::provider_catalog::resolve_openai_compatible_profile(profile);
            let base = base_url_override
                .unwrap_or(resolved.api_base.as_str())
                .trim_end_matches('/');
            client.get(format!("{base}/models")).bearer_auth(key)
        }
        LoginProviderTarget::Bedrock | LoginProviderTarget::Cursor => {
            return ApiKeyValidationOutcome::Unverified {
                reason: "provider has no lightweight key probe".to_string(),
            };
        }
        _ => {
            return ApiKeyValidationOutcome::Unverified {
                reason: "provider does not use API key login".to_string(),
            };
        }
    };

    let response = match request.timeout(VALIDATION_TIMEOUT).send().await {
        Ok(resp) => resp,
        Err(err) => {
            return ApiKeyValidationOutcome::Unverified {
                reason: format!("could not reach provider endpoint: {err}"),
            };
        }
    };

    let status = response.status().as_u16();
    if response.status().is_success() {
        return ApiKeyValidationOutcome::Verified;
    }

    if status == 401 || status == 403 {
        let body = response.text().await.unwrap_or_default();
        let detail = extract_safe_error_detail(&body, key);
        return ApiKeyValidationOutcome::Rejected { status, detail };
    }

    ApiKeyValidationOutcome::Unverified {
        reason: format!("endpoint returned HTTP {status}"),
    }
}

fn extract_safe_error_detail(body: &str, api_key: &str) -> Option<String> {
    let parsed: serde_json::Value = serde_json::from_str(body).ok()?;
    let raw = parsed
        .get("error")
        .and_then(|err| {
            err.get("message")
                .and_then(|m| m.as_str())
                .or_else(|| err.as_str())
        })
        .or_else(|| parsed.get("message").and_then(|m| m.as_str()))
        .or_else(|| parsed.get("detail").and_then(|m| m.as_str()))?;
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    // Never echo the raw secret back if a server included it in an error body.
    let redacted = if !api_key.is_empty() && trimmed.contains(api_key) {
        trimmed.replace(api_key, "[redacted]")
    } else {
        trimmed.to_string()
    };
    let single_line = redacted.split_whitespace().collect::<Vec<_>>().join(" ");
    let truncated = if single_line.chars().count() > 140 {
        format!("{}...", single_line.chars().take(137).collect::<String>())
    } else {
        single_line
    };
    Some(truncated)
}

/// Persist an API key for any supported API-key login provider.
pub fn save_api_key_for_provider(
    provider: LoginProviderDescriptor,
    raw_key: &str,
) -> Result<SavedApiKeyLogin> {
    let key = crate::provider_catalog::normalize_pasted_api_key(raw_key);
    anyhow::ensure!(!key.is_empty(), "API key cannot be empty.");

    let saved = match provider.target {
        LoginProviderTarget::ClaudeApiKey => {
            crate::provider_catalog::save_named_api_key(
                "anthropic.env",
                "ANTHROPIC_API_KEY",
                &key,
            )?;
            SavedApiKeyLogin {
                provider_id: provider.id,
                display_name: provider.display_name,
                env_file: "anthropic.env".to_string(),
                env_var: "ANTHROPIC_API_KEY".to_string(),
                openai_compatible_profile: None,
                default_model: None,
            }
        }
        LoginProviderTarget::OpenAiApiKey => {
            crate::provider_catalog::save_named_api_key("openai.env", "OPENAI_API_KEY", &key)?;
            SavedApiKeyLogin {
                provider_id: provider.id,
                display_name: provider.display_name,
                env_file: "openai.env".to_string(),
                env_var: "OPENAI_API_KEY".to_string(),
                openai_compatible_profile: None,
                default_model: None,
            }
        }
        LoginProviderTarget::OpenRouter => {
            crate::provider_catalog::save_named_api_key(
                "openrouter.env",
                "OPENROUTER_API_KEY",
                &key,
            )?;
            SavedApiKeyLogin {
                provider_id: provider.id,
                display_name: provider.display_name,
                env_file: "openrouter.env".to_string(),
                env_var: "OPENROUTER_API_KEY".to_string(),
                openai_compatible_profile: None,
                default_model: None,
            }
        }
        LoginProviderTarget::Bedrock => {
            crate::provider_catalog::save_named_api_key(
                "bedrock.env",
                "AWS_BEARER_TOKEN_BEDROCK",
                &key,
            )?;
            crate::provider_catalog::save_env_value_to_env_file(
                "AWS_REGION",
                "bedrock.env",
                Some("us-east-2"),
            )?;
            SavedApiKeyLogin {
                provider_id: provider.id,
                display_name: provider.display_name,
                env_file: "bedrock.env".to_string(),
                env_var: "AWS_BEARER_TOKEN_BEDROCK".to_string(),
                openai_compatible_profile: None,
                default_model: None,
            }
        }
        LoginProviderTarget::Cursor => {
            crate::auth::cursor::save_api_key(&key)?;
            SavedApiKeyLogin {
                provider_id: provider.id,
                display_name: provider.display_name,
                env_file: "cursor.env".to_string(),
                env_var: "CURSOR_API_KEY".to_string(),
                openai_compatible_profile: None,
                default_model: None,
            }
        }
        LoginProviderTarget::OpenAiCompatible(profile) => {
            if profile.id == "muse-subscription" {
                bail!(
                    "{} uses device code login, not an API key.",
                    provider.display_name
                );
            }
            let resolved = crate::provider_catalog::resolve_openai_compatible_profile(profile);
            if resolved.requires_api_key {
                crate::provider_catalog::save_env_value_to_env_file(
                    crate::provider_catalog::OPENAI_COMPAT_LOCAL_ENABLED_ENV,
                    &resolved.env_file,
                    None,
                )?;
            } else {
                crate::provider_catalog::save_env_value_to_env_file(
                    crate::provider_catalog::OPENAI_COMPAT_LOCAL_ENABLED_ENV,
                    &resolved.env_file,
                    Some("1"),
                )?;
            }
            crate::provider_catalog::save_named_api_key(
                &resolved.env_file,
                &resolved.api_key_env,
                &key,
            )?;
            SavedApiKeyLogin {
                provider_id: provider.id,
                display_name: provider.display_name,
                env_file: resolved.env_file,
                env_var: resolved.api_key_env,
                openai_compatible_profile: Some(profile),
                default_model: resolved.default_model,
            }
        }
        _ => bail!("{} does not use API key login.", provider.display_name),
    };

    crate::auth::AuthStatus::invalidate_cache();
    Ok(saved)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    async fn spawn_mock_status_server(status_line: &'static str, body: &'static str) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("local_addr");
        tokio::spawn(async move {
            if let Ok((mut stream, _)) = listener.accept().await {
                let mut buf = [0u8; 2048];
                let _ = stream.read(&mut buf).await;
                let response = format!(
                    "HTTP/1.1 {status_line}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(response.as_bytes()).await;
            }
        });
        format!("http://{addr}")
    }

    #[tokio::test]
    async fn validation_returns_verified_on_200() {
        let base = spawn_mock_status_server("200 OK", r#"{"data":[]}"#).await;
        let provider = crate::provider_catalog::resolve_login_provider("anthropic-api").unwrap();
        let client = reqwest::Client::new();
        let outcome =
            validate_api_key_with_client(&client, provider, "sk-ant-test-key", Some(&base)).await;
        assert_eq!(outcome, ApiKeyValidationOutcome::Verified);
    }

    #[tokio::test]
    async fn validation_returns_rejected_on_401_and_redacts_key() {
        let base = spawn_mock_status_server(
            "401 Unauthorized",
            r#"{"error":{"message":"Invalid key sk-ant-secret-123"}}"#,
        )
        .await;
        let provider = crate::provider_catalog::resolve_login_provider("anthropic-api").unwrap();
        let client = reqwest::Client::new();
        let outcome =
            validate_api_key_with_client(&client, provider, "sk-ant-secret-123", Some(&base)).await;
        match outcome {
            ApiKeyValidationOutcome::Rejected { status, detail } => {
                assert_eq!(status, 401);
                let detail = detail.expect("detail");
                assert!(!detail.contains("sk-ant-secret-123"));
                assert!(detail.contains("[redacted]"));
            }
            other => panic!("expected Rejected, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn validation_returns_unverified_on_500() {
        let base = spawn_mock_status_server("500 Internal Server Error", "{}").await;
        let provider = crate::provider_catalog::resolve_login_provider("openai-api").unwrap();
        let client = reqwest::Client::new();
        let outcome =
            validate_api_key_with_client(&client, provider, "sk-proj-test", Some(&base)).await;
        assert!(matches!(
            outcome,
            ApiKeyValidationOutcome::Unverified { .. }
        ));
    }
}
