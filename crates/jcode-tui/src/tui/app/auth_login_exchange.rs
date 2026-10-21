use super::*;

impl App {
    pub(super) async fn claude_login_callback(
        verifier: String,
        label: String,
        redirect_uri: String,
        listener: tokio::net::TcpListener,
        target_snapshot: (bool, Option<String>),
        completion: &mut crate::bus::SavedAccountLoginCompleted,
    ) -> Result<String, String> {
        // Claude uses the PKCE verifier as the OAuth `state` value.
        let code = tokio::time::timeout(
            std::time::Duration::from_secs(300),
            crate::auth::oauth::wait_for_callback_async_on_listener(listener, &verifier),
        )
        .await
        .map_err(|_| "Login timed out after 5 minutes. Please try again.".to_string())?
        .map_err(|e| format!("Callback failed: {}", e))?;

        Self::claude_token_exchange(
            verifier,
            code,
            &label,
            Some(redirect_uri),
            target_snapshot,
            completion,
        )
        .await
    }

    pub(super) async fn openai_login_callback(
        verifier: String,
        expected_state: String,
        label: Option<String>,
        listener: tokio::net::TcpListener,
        target_snapshot: (bool, Option<String>),
        completion: &mut crate::bus::SavedAccountLoginCompleted,
    ) -> Result<String, String> {
        let port = crate::auth::oauth::openai::DEFAULT_PORT;
        let redirect_uri = crate::auth::oauth::openai::redirect_uri(port);
        let code = tokio::time::timeout(
            std::time::Duration::from_secs(300),
            crate::auth::oauth::wait_for_callback_async_on_listener(listener, &expected_state),
        )
        .await
        .map_err(|_| "Login timed out after 5 minutes. Please try again.".to_string())?
        .map_err(|e| format!("Callback failed: {}", e))?;

        Self::openai_token_exchange(
            verifier,
            code,
            label,
            None,
            &redirect_uri,
            target_snapshot,
            completion,
        )
        .await
    }

    pub(super) async fn openai_token_exchange(
        verifier: String,
        input: String,
        label: Option<String>,
        expected_state: Option<String>,
        redirect_uri: &str,
        target_snapshot: (bool, Option<String>),
        completion: &mut crate::bus::SavedAccountLoginCompleted,
    ) -> Result<String, String> {
        if !completion.exchange_claimed {
            Self::begin_saved_login_exchange(completion)?;
        }
        if completion
            .attempt
            .load(std::sync::atomic::Ordering::Acquire)
            != 4
        {
            return Err("This login was cancelled. No credentials were replaced.".to_string());
        }
        let oauth_tokens = if let Some(expected_state) = expected_state {
            crate::auth::oauth::exchange_openai_callback_input(
                &verifier,
                input.trim(),
                &expected_state,
                redirect_uri,
            )
            .await
            .map_err(|e| e.to_string())?
        } else {
            crate::auth::oauth::exchange_openai_code(&input, &verifier, redirect_uri)
                .await
                .map_err(|e| e.to_string())?
        };

        let label = label.unwrap_or_else(crate::auth::codex::primary_account_label);
        if completion.replace {
            Self::verify_login_target_snapshot("openai", &label, &target_snapshot)?;
            if !target_snapshot.0 {
                return Err(
                    "The saved login no longer exists. No credentials were replaced.".to_string(),
                );
            }
            let replacement = crate::auth::codex::OpenAiAccount {
                label: label.clone(),
                access_token: oauth_tokens.access_token.clone(),
                refresh_token: oauth_tokens.refresh_token.clone(),
                account_id: oauth_tokens
                    .id_token
                    .as_deref()
                    .and_then(crate::auth::codex::extract_account_id),
                email: oauth_tokens
                    .id_token
                    .as_deref()
                    .and_then(crate::auth::codex::extract_email),
                id_token: oauth_tokens.id_token.clone(),
                expires_at: Some(oauth_tokens.expires_at),
            };
            completion.new_identity = crate::auth::codex::account_identity(&replacement);
            Self::claim_saved_login_commit(completion)?;
            crate::auth::codex::replace_account_if_unchanged(
                &label,
                target_snapshot.1.as_deref(),
                replacement,
            )
            .map_err(|e| format!("Failed to replace the saved login: {e}"))?;
            return Ok(format!("Replaced OpenAI login (account: {label})."));
        }
        Self::verify_saved_openai_login(
            &label,
            oauth_tokens.id_token.as_deref(),
            &target_snapshot,
        )?;
        Self::claim_saved_login_commit(completion)?;
        if target_snapshot.0 {
            crate::auth::codex::update_account_tokens(
                &label,
                &oauth_tokens.access_token,
                &oauth_tokens.refresh_token,
                oauth_tokens.id_token.clone(),
                oauth_tokens
                    .id_token
                    .as_deref()
                    .and_then(crate::auth::codex::extract_account_id),
                Some(oauth_tokens.expires_at),
            )
            .map_err(|e| format!("Failed to update the saved login: {e}"))?;
        } else {
            crate::auth::oauth::save_openai_tokens_for_account(&oauth_tokens, &label)
                .map_err(|e| format!("Failed to save tokens: {}", e))?;
        }

        Ok(format!(
            "Successfully logged in to OpenAI! (account: {})",
            label
        ))
    }

    pub(super) async fn antigravity_token_exchange(
        verifier: String,
        input: String,
        expected_state: Option<String>,
        redirect_uri: String,
    ) -> Result<String, String> {
        let trimmed = input.trim();
        let tokens =
            if antigravity_input_requires_state_validation(trimmed, expected_state.as_deref()) {
                crate::auth::antigravity::exchange_callback_input(
                    &verifier,
                    trimmed,
                    expected_state.as_deref(),
                    &redirect_uri,
                )
                .await
            } else {
                crate::auth::antigravity::exchange_callback_code(trimmed, &verifier, &redirect_uri)
                    .await
            }
            .map_err(|e| e.to_string())?;

        let mut msg = if let Some(email) = tokens.email.as_deref() {
            format!(
                "Successfully logged in to Antigravity! (account: {})",
                email
            )
        } else {
            "Successfully logged in to Antigravity!".to_string()
        };
        if let Some(project_id) = tokens.project_id.as_deref() {
            msg.push_str(&format!(" (project: {})", project_id));
        }
        Ok(msg)
    }

    pub(super) async fn claude_token_exchange(
        verifier: String,
        input: String,
        label: &str,
        redirect_uri: Option<String>,
        target_snapshot: (bool, Option<String>),
        completion: &mut crate::bus::SavedAccountLoginCompleted,
    ) -> Result<String, String> {
        if !completion.exchange_claimed {
            Self::begin_saved_login_exchange(completion)?;
        }
        if completion
            .attempt
            .load(std::sync::atomic::Ordering::Acquire)
            != 4
        {
            return Err("This login was cancelled. No credentials were replaced.".to_string());
        }
        let fallback_redirect_uri =
            redirect_uri.unwrap_or_else(|| crate::auth::oauth::claude::REDIRECT_URI.to_string());
        let redirect_uri =
            crate::auth::oauth::claude_redirect_uri_for_input(input.trim(), &fallback_redirect_uri);
        let oauth_tokens =
            crate::auth::oauth::exchange_claude_code(&verifier, input.trim(), &redirect_uri)
                .await
                .map_err(|e| e.to_string())?;

        if completion.replace {
            Self::verify_login_target_snapshot("claude", label, &target_snapshot)?;
            if !target_snapshot.0 {
                return Err(
                    "The saved login no longer exists. No credentials were replaced.".to_string(),
                );
            }
            let email = crate::auth::oauth::fetch_claude_profile_email(&oauth_tokens.access_token)
                .await
                .map_err(|e| {
                    format!(
                        "Could not verify replacement identity: {e}. No credentials were replaced."
                    )
                })?;
            Self::verify_login_target_snapshot("claude", label, &target_snapshot)?;
            let replacement = crate::auth::claude::AnthropicAccount {
                label: label.to_string(),
                access: oauth_tokens.access_token.clone(),
                refresh: oauth_tokens.refresh_token.clone(),
                expires: oauth_tokens.expires_at,
                email,
                subscription_type: None,
                scopes: oauth_tokens.scopes.clone(),
            };
            completion.new_identity = crate::auth::claude::account_identity(&replacement);
            Self::claim_saved_login_commit(completion)?;
            crate::auth::claude::replace_account_if_unchanged(
                label,
                target_snapshot.1.as_deref(),
                replacement,
            )
            .map_err(|e| format!("Failed to replace the saved login: {e}"))?;
            return Ok(format!("Replaced Claude login (account: {label})."));
        }
        Self::verify_saved_claude_login(label, &oauth_tokens.access_token, &target_snapshot)
            .await?;
        Self::claim_saved_login_commit(completion)?;
        if target_snapshot.0 {
            crate::auth::claude::update_account_tokens(
                label,
                &oauth_tokens.access_token,
                &oauth_tokens.refresh_token,
                oauth_tokens.expires_at,
            )
            .map_err(|e| format!("Failed to update the saved login: {e}"))?;
        } else {
            crate::auth::oauth::save_claude_tokens_for_account(&oauth_tokens, label)
                .map_err(|e| format!("Failed to save tokens: {}", e))?;
        }

        let profile_suffix = if target_snapshot.0 {
            // Identity was verified before persistence. Do not perform another
            // profile fetch that could erase its email if metadata is omitted.
            String::new()
        } else {
            match crate::auth::oauth::update_claude_account_profile(
                label,
                &oauth_tokens.access_token,
            )
            .await
            {
                Ok(Some(email)) => format!(" (email: {})", mask_email(&email)),
                Ok(None) => String::new(),
                Err(e) => {
                    crate::logging::warn(&format!(
                        "Claude login [{}] profile fetch failed: {}",
                        label, e
                    ));
                    String::new()
                }
            }
        };

        Ok(format!(
            "Successfully logged in to Claude! (account: {}){}",
            label, profile_suffix
        ))
    }
}
