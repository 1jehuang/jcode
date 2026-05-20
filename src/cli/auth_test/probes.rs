fn generic_credential_paths_for_provider(
    provider: crate::provider_catalog::LoginProviderDescriptor,
) -> Vec<String> {
    let Ok(config_dir) = crate::storage::app_config_dir() else {
        return Vec::new();
    };

    match provider.target {
        crate::provider_catalog::LoginProviderTarget::Jcode => {
            vec![config_dir.join(crate::subscription_catalog::JCODE_ENV_FILE)]
        }
        crate::provider_catalog::LoginProviderTarget::OpenRouter => {
            vec![config_dir.join("openrouter.env")]
        }
        crate::provider_catalog::LoginProviderTarget::OpenAiApiKey => {
            vec![config_dir.join("openai.env")]
        }
        crate::provider_catalog::LoginProviderTarget::Azure => {
            vec![config_dir.join(crate::auth::azure::ENV_FILE)]
        }
        crate::provider_catalog::LoginProviderTarget::Bedrock => {
            vec![config_dir.join(crate::provider::bedrock::ENV_FILE)]
        }
        crate::provider_catalog::LoginProviderTarget::OpenAiCompatible(profile) => {
            let resolved = crate::provider_catalog::resolve_openai_compatible_profile(profile);
            vec![config_dir.join(resolved.env_file)]
        }
        _ => Vec::new(),
    }
    .into_iter()
    .map(|path| path.display().to_string())
    .collect()
}

fn auth_state_label(state: crate::auth::AuthState) -> &'static str {
    match state {
        crate::auth::AuthState::Available => "available",
        crate::auth::AuthState::Expired => "expired",
        crate::auth::AuthState::NotConfigured => "not_configured",
    }
}

fn probe_generic_provider_auth(
    provider: crate::provider_catalog::LoginProviderDescriptor,
    report: &mut AuthTestProviderReport,
) {
    // Keep generic provider probes provider-local. A DeepSeek/Z.AI/OpenRouter
    // auth-test should never be delayed or wedged by an unrelated Cursor/Gemini
    // external auth probe.
    let status = crate::auth::AuthStatus::check_fast();
    let assessment = status.assessment_for_provider(provider);
    report.push_step(
        "credential_probe",
        assessment.is_available(),
        format!(
            "{} auth status is {} ({}).",
            provider.display_name,
            auth_state_label(assessment.state),
            assessment.method_detail,
        ),
    );
    report.push_step(
        "refresh_probe",
        true,
        "Skipped: provider does not expose a dedicated refresh probe in jcode today.".to_string(),
    );
}

async fn probe_claude_auth(report: &mut AuthTestProviderReport) {
    if let Some(creds) = push_result_step(
        report,
        "credential_probe",
        crate::auth::claude::load_credentials(),
        |creds| {
            format!(
                "Loaded Claude credentials (expires_at={}).",
                creds.expires_at
            )
        },
    ) {
        push_result_step(
            report,
            "refresh_probe",
            crate::auth::oauth::refresh_claude_tokens(&creds.refresh_token).await,
            |tokens| {
                format!(
                    "Claude token refresh succeeded (new_expires_at={}).",
                    tokens.expires_at
                )
            },
        );
    }
}

async fn probe_openai_auth(report: &mut AuthTestProviderReport) {
    if let Some(creds) = push_result_step(
        report,
        "credential_probe",
        crate::auth::codex::load_credentials(),
        |creds| {
            if creds.refresh_token.trim().is_empty() {
                "Loaded OpenAI API key credentials (no refresh token present).".to_string()
            } else {
                format!(
                    "Loaded OpenAI OAuth credentials (expires_at={:?}).",
                    creds.expires_at
                )
            }
        },
    ) {
        if creds.refresh_token.trim().is_empty() {
            report.push_step(
                "refresh_probe",
                true,
                "Skipped: OpenAI is using API key auth, not OAuth.",
            );
        } else {
            push_result_step(
                report,
                "refresh_probe",
                crate::auth::oauth::refresh_openai_tokens(&creds.refresh_token).await,
                |tokens| {
                    format!(
                        "OpenAI token refresh succeeded (new_expires_at={}).",
                        tokens.expires_at
                    )
                },
            );
        }
    }
}

async fn probe_gemini_auth(report: &mut AuthTestProviderReport) {
    if push_result_step(
        report,
        "credential_probe",
        crate::auth::gemini::load_tokens(),
        |tokens| {
            format!(
                "Loaded Gemini tokens{} (expires_at={}).",
                auth_email_suffix(tokens.email.as_deref()),
                tokens.expires_at
            )
        },
    )
    .is_some()
    {
        push_result_step(
            report,
            "refresh_probe",
            crate::auth::gemini::load_or_refresh_tokens().await,
            |tokens| {
                format!(
                    "Gemini token load/refresh succeeded (expires_at={}).",
                    tokens.expires_at
                )
            },
        );
    }
}

async fn probe_antigravity_auth(report: &mut AuthTestProviderReport) {
    if push_result_step(
        report,
        "credential_probe",
        crate::auth::antigravity::load_tokens(),
        |tokens| {
            format!(
                "Loaded Antigravity OAuth tokens{} (expires_at={}).",
                auth_email_suffix(tokens.email.as_deref()),
                tokens.expires_at
            )
        },
    )
    .is_some()
    {
        push_result_step(
            report,
            "refresh_probe",
            crate::auth::antigravity::load_or_refresh_tokens().await,
            |tokens| {
                format!(
                    "Antigravity token load/refresh succeeded (expires_at={}).",
                    tokens.expires_at
                )
            },
        );
    }
}

async fn probe_google_auth(report: &mut AuthTestProviderReport) {
    let creds_result = crate::auth::google::load_credentials();
    let tokens_result = crate::auth::google::load_tokens();
    match (creds_result, tokens_result) {
        (Ok(creds), Ok(tokens)) => {
            report.push_step(
                "credential_probe",
                true,
                format!(
                    "Loaded Google credentials (client_id={}...) and Gmail tokens{}.",
                    &creds.client_id[..20.min(creds.client_id.len())],
                    auth_email_suffix(tokens.email.as_deref())
                ),
            );
            match crate::auth::google::get_valid_token().await {
                Ok(_) => report.push_step(
                    "refresh_probe",
                    true,
                    "Google/Gmail token load/refresh succeeded.".to_string(),
                ),
                Err(err) => report.push_step("refresh_probe", false, err.to_string()),
            }
        }
        (Err(err), _) => report.push_step("credential_probe", false, err.to_string()),
        (_, Err(err)) => report.push_step("credential_probe", false, err.to_string()),
    }
}

async fn probe_copilot_auth(report: &mut AuthTestProviderReport) {
    if let Some(token) = push_result_step(
        report,
        "credential_probe",
        crate::auth::copilot::load_github_token(),
        |token| {
            format!(
                "Loaded GitHub OAuth token for Copilot ({} chars).",
                token.len()
            )
        },
    ) {
        let client = crate::provider::shared_http_client();
        push_result_step(
            report,
            "refresh_probe",
            crate::auth::copilot::exchange_github_token(&client, &token).await,
            |api_token| {
                format!(
                    "Exchanged GitHub token for Copilot API token (expires_at={}).",
                    api_token.expires_at
                )
            },
        );
    }
}

async fn probe_cursor_auth(report: &mut AuthTestProviderReport) {
    let has_api_key = crate::auth::cursor::has_cursor_api_key();
    let has_auth_file = crate::auth::cursor::has_cursor_auth_file_token();
    let has_vscdb = crate::auth::cursor::has_cursor_vscdb_token();
    let ok = has_api_key || has_auth_file || has_vscdb;
    report.push_step(
        "credential_probe",
        ok,
        format!(
            "Cursor native auth sources: api_key={}, auth_json={}, vscdb_token={}",
            has_api_key, has_auth_file, has_vscdb
        ),
    );
    report.push_step(
        "refresh_probe",
        true,
        "Skipped: Cursor provider does not expose a native refresh-token probe in jcode today."
            .to_string(),
    );
}
