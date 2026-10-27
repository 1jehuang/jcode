use super::*;

impl AuthStatus {
    pub fn method_detail_for_provider(&self, provider: LoginProviderDescriptor) -> String {
        if provider.id == "muse-subscription" {
            return if self.state_for_provider(provider) == AuthState::Available {
                let usage = muse::usage_summary()
                    .map(|usage| format!("; last account snapshot: {usage}"))
                    .unwrap_or_else(|| "; quota unknown".to_string());
                format!("Device OAuth + minted key (third-party client, billing unverified){usage}")
            } else {
                "Native device login not configured".to_string()
            };
        }
        match provider.target {
            crate::provider_catalog::LoginProviderTarget::AutoImport => {
                if Self::has_any_untrusted_external_auth() {
                    "Existing external logins detected".to_string()
                } else {
                    "No importable external logins found".to_string()
                }
            }
            crate::provider_catalog::LoginProviderTarget::Jcode => {
                if self.state_for_provider(provider) == AuthState::Available {
                    if crate::subscription_catalog::has_router_base() {
                        format!(
                            "API key (`{}`) + router base",
                            crate::subscription_catalog::JCODE_API_KEY_ENV
                        )
                    } else {
                        format!(
                            "API key (`{}`), router base pending",
                            crate::subscription_catalog::JCODE_API_KEY_ENV
                        )
                    }
                } else {
                    "not configured".to_string()
                }
            }
            crate::provider_catalog::LoginProviderTarget::OpenRouter => {
                if self.state_for_provider(provider) == AuthState::Available {
                    "API key (`OPENROUTER_API_KEY`)".to_string()
                } else {
                    "not configured".to_string()
                }
            }
            crate::provider_catalog::LoginProviderTarget::OpenAiApiKey => {
                if self.state_for_provider(provider) == AuthState::Available {
                    "API key (`OPENAI_API_KEY`)".to_string()
                } else {
                    "not configured".to_string()
                }
            }
            crate::provider_catalog::LoginProviderTarget::ClaudeApiKey => {
                if self.state_for_provider(provider) == AuthState::Available {
                    "API key (`ANTHROPIC_API_KEY`)".to_string()
                } else {
                    "not configured".to_string()
                }
            }
            crate::provider_catalog::LoginProviderTarget::Bedrock => {
                if self.state_for_provider(provider) == AuthState::Available {
                    if crate::provider::bedrock::BedrockProvider::configured_bearer_token()
                        .is_some()
                    {
                        "Bedrock API key (`AWS_BEARER_TOKEN_BEDROCK`)".to_string()
                    } else {
                        "AWS credential chain".to_string()
                    }
                } else {
                    "not configured".to_string()
                }
            }
            crate::provider_catalog::LoginProviderTarget::GrokBuild => {
                if self.grok_build == AuthState::Available {
                    "Grok CLI subscription login (xAI OIDC, auto-refreshed)".to_string()
                } else {
                    "not configured (run `jcode login --provider grok-build`)".to_string()
                }
            }
            crate::provider_catalog::LoginProviderTarget::OpenAiCompatible(profile) => {
                let resolved = crate::provider_catalog::resolve_openai_compatible_profile(profile);
                if self.state_for_provider(provider) == AuthState::Available {
                    if resolved.requires_api_key {
                        format!("API key (`{}`)", resolved.api_key_env)
                    } else if crate::provider_catalog::load_api_key_from_env_or_config(
                        &resolved.api_key_env,
                        &resolved.env_file,
                    )
                    .is_some()
                    {
                        format!(
                            "local endpoint (`{}`) + optional API key (`{}`)",
                            resolved.api_base, resolved.api_key_env
                        )
                    } else {
                        format!("local endpoint (`{}`)", resolved.api_base)
                    }
                } else {
                    "not configured".to_string()
                }
            }
            _ => match provider.auth_state_key {
                // The `claude` login provider is the OAuth/subscription path;
                // the API key reports through the separate `anthropic-api`
                // provider (handled above via `LoginProviderTarget::ClaudeApiKey`).
                // Describe the OAuth credential alone so the two rows never
                // blur together as "OAuth + API key".
                LoginProviderAuthStateKey::Anthropic => {
                    let detail = match self.anthropic.oauth_state {
                        AuthState::Available => "OAuth",
                        AuthState::Expired => "OAuth (expired)",
                        AuthState::NotConfigured => "not configured",
                    };

                    let accounts = crate::auth::claude::list_accounts().unwrap_or_default();
                    if accounts.len() > 1 {
                        let active = crate::auth::claude::active_account_label()
                            .unwrap_or_else(|| "?".to_string());
                        format!(
                            "{detail} ({} accounts, active: `{}`)",
                            accounts.len(),
                            active
                        )
                    } else if accounts.len() == 1 {
                        format!("{detail} (account: `{}`)", accounts[0].label)
                    } else {
                        detail.to_string()
                    }
                }
                // Same split for OpenAI: this is the ChatGPT/Codex OAuth login;
                // `openai-api` (handled above) owns the API-key answer.
                LoginProviderAuthStateKey::OpenAi => {
                    let detail = match self.openai_oauth_state {
                        AuthState::Available => "OAuth",
                        AuthState::Expired => "OAuth (expired)",
                        AuthState::NotConfigured => "not configured",
                    };

                    let accounts = crate::auth::codex::list_accounts().unwrap_or_default();
                    if accounts.len() > 1 {
                        let active = crate::auth::codex::active_account_label()
                            .unwrap_or_else(|| "?".to_string());
                        format!(
                            "{detail} ({} accounts, active: `{}`)",
                            accounts.len(),
                            active
                        )
                    } else if accounts.len() == 1 {
                        format!("{detail} (account: `{}`)", accounts[0].label)
                    } else {
                        detail.to_string()
                    }
                }
                _ => provider.auth_status_method.to_string(),
            },
        }
    }
}
