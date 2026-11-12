//! Observe terminal usage at the transport, never at the TUI consumer.
use super::*;
use jcode_base::auth::codex::OpenAiAccount;

pub(super) struct OAuthUsageRecorder {
    label: Option<String>,
    model: String,
    tier: Option<String>,
    recorded: bool,
}

// Match the actual request credential, not the mutable active-account setting.
// Prefer token identity over account_id because multiple logins may share an ID.
fn credential_label(creds: &CodexCredentials, accounts: &[OpenAiAccount]) -> Option<String> {
    for account in accounts {
        if (!creds.refresh_token.is_empty() && account.refresh_token == creds.refresh_token)
            || (!creds.access_token.is_empty() && account.access_token == creds.access_token)
        {
            return Some(account.label.clone());
        }
    }
    let mut matches = accounts
        .iter()
        .filter(|account| creds.account_id.is_some() && account.account_id == creds.account_id);
    if let Some(account) = matches.next() {
        return matches.next().is_none().then(|| account.label.clone());
    }
    // Legacy credentials outside the labeled account store belong to default.
    accounts.is_empty().then(|| "default".to_string())
}

impl OAuthUsageRecorder {
    pub(super) fn capture(creds: &CodexCredentials, request: &Value) -> Self {
        let label = if OpenAIProvider::is_chatgpt_mode(creds) {
            jcode_base::auth::codex::list_accounts()
                .ok()
                .and_then(|accounts| credential_label(creds, &accounts))
        } else {
            None
        };
        if label.is_none() && OpenAIProvider::is_chatgpt_mode(creds) {
            jcode_base::logging::warn(
                "ChatGPT usage not recorded: request credential could not be attributed to a unique account",
            );
        }
        Self {
            label,
            model: openai_request_model(request),
            tier: request
                .get("service_tier")
                .and_then(Value::as_str)
                .map(str::to_string),
            recorded: false,
        }
    }

    pub(super) async fn observe(&mut self, event: &StreamEvent) {
        if self.recorded {
            return;
        }
        // The Responses parser emits TokenUsage only from terminal
        // response.completed / response.incomplete, not incremental deltas.
        let StreamEvent::TokenUsage {
            input_tokens,
            output_tokens,
            cache_read_input_tokens,
            ..
        } = event
        else {
            return;
        };
        let Some(label) = self.label.clone() else {
            return;
        };
        self.recorded = true;
        let model = self.model.clone();
        let tier = self.tier.clone();
        let (input, output, cached) = (*input_tokens, *output_tokens, *cache_read_input_tokens);
        // Wait for durability before forwarding terminal usage. Blocking file IO
        // runs outside the async executor and survives a disconnected TUI.
        if let Err(error) = tokio::task::spawn_blocking(move || {
            jcode_base::provider_activity::record_openai_oauth_usage(
                &label,
                &model,
                tier.as_deref(),
                input,
                output,
                cached,
            );
        })
        .await
        {
            jcode_base::logging::warn(&format!("ChatGPT usage recorder task failed: {error}"));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn creds(access: &str, refresh: &str) -> CodexCredentials {
        CodexCredentials {
            access_token: access.into(),
            refresh_token: refresh.into(),
            id_token: None,
            account_id: Some("shared-id".into()),
            expires_at: None,
        }
    }
    fn account(label: &str, credentials: &CodexCredentials) -> OpenAiAccount {
        OpenAiAccount {
            label: label.into(),
            access_token: credentials.access_token.clone(),
            refresh_token: credentials.refresh_token.clone(),
            id_token: None,
            account_id: credentials.account_id.clone(),
            expires_at: None,
            email: None,
        }
    }

    #[test]
    fn attribution_uses_request_identity_not_active_account() {
        let one = creds("access-one", "refresh-one");
        let two = creds("access-two", "refresh-two");
        let accounts = vec![account("one", &one), account("two", &two)];
        assert_eq!(credential_label(&one, &accounts).as_deref(), Some("one"));
        assert_eq!(credential_label(&two, &accounts).as_deref(), Some("two"));
        assert_eq!(
            credential_label(&creds("rotated", "rotated"), &accounts),
            None
        );
        assert_eq!(
            credential_label(&creds("rotated", "rotated"), &accounts[..1]).as_deref(),
            Some("one")
        );
    }

    #[tokio::test]
    async fn api_credentials_do_not_record() {
        let mut recorder = OAuthUsageRecorder::capture(
            &creds("api-key", ""),
            &serde_json::json!({"model": "gpt-5.5"}),
        );
        assert!(recorder.label.is_none());
        recorder
            .observe(&StreamEvent::TokenUsage {
                input_tokens: Some(10),
                output_tokens: Some(2),
                cache_read_input_tokens: None,
                cache_creation_input_tokens: None,
            })
            .await;
        assert!(!recorder.recorded);
    }

    #[tokio::test]
    async fn terminal_usage_recorded_once_before_delivery() {
        let _lock = jcode_base::storage::lock_test_env();
        let temp = tempfile::tempdir().unwrap();
        let old = std::env::var_os("JCODE_HOME");
        jcode_base::env::set_var("JCODE_HOME", temp.path());
        let mut recorder = OAuthUsageRecorder {
            label: Some("captured".into()),
            model: "gpt-5.5".into(),
            tier: None,
            recorded: false,
        };
        let frame = r#"{"type":"response.completed","response":{"usage":{"input_tokens":100,"output_tokens":20,"input_tokens_details":{"cached_tokens":40}}}}"#;
        let mut text = false;
        let mut thinking = false;
        let mut tools = HashMap::new();
        let mut completed = HashSet::new();
        let mut pending = VecDeque::new();
        let event = parse_openai_response_event(
            frame,
            &mut text,
            &mut thinking,
            &mut tools,
            &mut completed,
            &mut pending,
        )
        .unwrap();
        recorder.observe(&event).await;
        recorder.observe(&event).await;
        for event in pending {
            recorder.observe(&event).await;
        }
        let rows = jcode_base::provider_activity::openai_oauth_usage_summary("captured");
        assert!(
            rows[1]
                .1
                .contains("100 input / 20 output tokens (40 cached input)"),
            "{rows:?}"
        );
        assert!(!temp.path().join("provider_activity.json").exists());
        if let Some(old) = old {
            jcode_base::env::set_var("JCODE_HOME", old);
        } else {
            jcode_base::env::remove_var("JCODE_HOME");
        }
    }
}
