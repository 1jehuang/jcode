#[derive(Debug)]
pub(crate) enum AuthTestChoicePlan {
    Run { model: Option<String> },
    Skip(String),
}

#[derive(Debug, Deserialize)]
struct OpenAiCompatibleModelsResponse {
    #[serde(default)]
    data: Vec<OpenAiCompatibleModelInfo>,
}

#[derive(Debug, Deserialize)]
struct OpenAiCompatibleModelInfo {
    id: String,
}

pub(crate) async fn auth_test_choice_plan(
    choice: &super::provider_init::ProviderChoice,
    model: Option<&str>,
) -> Result<AuthTestChoicePlan> {
    if let Some(model) = model.map(str::trim).filter(|model| !model.is_empty()) {
        return Ok(AuthTestChoicePlan::Run {
            model: Some(model.to_string()),
        });
    }

    let Some(profile) = super::provider_init::profile_for_choice(choice) else {
        return Ok(AuthTestChoicePlan::Run { model: None });
    };
    let resolved = crate::provider_catalog::resolve_openai_compatible_profile(profile);
    if resolved.default_model.is_some() {
        return Ok(AuthTestChoicePlan::Run { model: None });
    }

    crate::provider_catalog::apply_openai_compatible_profile_env(Some(profile));
    let discovered_model = discover_openai_compatible_validation_model(&resolved).await?;
    if let Some(model) = discovered_model {
        return Ok(AuthTestChoicePlan::Run { model: Some(model) });
    }

    Ok(AuthTestChoicePlan::Skip(format!(
        "Skipped: {} local endpoint reported no models. Re-run `jcode auth-test --provider {} --model <local-model>` or set a default model first.",
        resolved.display_name,
        choice.as_arg_value()
    )))
}

pub(crate) fn tool_smoke_skip_detail_for_choice(
    choice: &super::provider_init::ProviderChoice,
    model: Option<&str>,
) -> Option<String> {
    if matches!(choice, super::provider_init::ProviderChoice::Fpt) {
        let model = effective_openai_compatible_auth_test_model(
            crate::provider_catalog::FPT_PROFILE,
            model,
        )
        .unwrap_or_else(|| "the selected model".to_string());
        return Some(format!(
            "Skipped: FPT model '{}' is hosted on an OpenAI-compatible/vLLM-style endpoint that rejects OpenAI tool-choice requests unless server-side auto-tool parsing is enabled. Basic provider smoke still validates chat.",
            model
        ));
    }

    if !matches!(choice, super::provider_init::ProviderChoice::NvidiaNim) {
        return None;
    }

    let model = effective_openai_compatible_auth_test_model(
        crate::provider_catalog::NVIDIA_NIM_PROFILE,
        model,
    )?;
    if !nvidia_nim_model_supports_openai_tools(&model) {
        return Some(format!(
            "Skipped: NVIDIA NIM model '{}' is documented by NVIDIA as a request/status-polling model rather than a standard OpenAI tool-calling chat model. Basic provider smoke still validates chat; choose a NIM model with OpenAI tool support to run tool_smoke.",
            model
        ));
    }

    None
}

fn effective_openai_compatible_auth_test_model(
    profile: crate::provider_catalog::OpenAiCompatibleProfile,
    model: Option<&str>,
) -> Option<String> {
    model
        .map(str::trim)
        .filter(|model| !model.is_empty())
        .map(ToString::to_string)
        .or_else(|| {
            std::env::var("JCODE_OPENROUTER_MODEL")
                .ok()
                .map(|model| model.trim().to_string())
                .filter(|model| !model.is_empty())
        })
        .or_else(|| {
            let cfg = crate::config::config();
            let default_provider_is_profile = cfg
                .provider
                .default_provider
                .as_deref()
                .map(str::trim)
                .map(|provider| {
                    provider == profile.id
                        || crate::provider_catalog::resolve_openai_compatible_profile_selection(
                            provider,
                        )
                        .map(|resolved| resolved.id == profile.id)
                        .unwrap_or(false)
                })
                .unwrap_or(false);
            default_provider_is_profile
                .then(|| cfg.provider.default_model.clone())
                .flatten()
        })
        .or_else(|| {
            crate::provider_catalog::resolve_openai_compatible_profile(profile).default_model
        })
}

fn nvidia_nim_model_supports_openai_tools(model: &str) -> bool {
    let normalized = model.trim().to_ascii_lowercase().replace('_', "-");
    // NVIDIA documents moonshotai/kimi-k2.6 under the request/status-polling
    // visual-model API family. The OpenAI-compatible chat endpoint accepts basic
    // prompts for this model, but tool-enabled smoke has been observed returning
    // a server-side `unhashable type: 'dict'` 500 when sent OpenAI tools.
    !(normalized.contains("moonshotai/kimi-k2.6") || normalized.contains("moonshotai/kimi-k2-6"))
}

async fn discover_openai_compatible_validation_model(
    profile: &crate::provider_catalog::ResolvedOpenAiCompatibleProfile,
) -> Result<Option<String>> {
    let url = format!("{}/models", profile.api_base.trim_end_matches('/'));
    let mut request = crate::provider::shared_http_client().get(&url);
    if matches!(profile.id.as_str(), "kimi" | "alibaba-coding-plan" | "zai") {
        request = request
            .header("User-Agent", "claude-cli/1.0.0")
            .header("x-app", "cli");
    }
    if let Some(api_key) = crate::provider_catalog::load_api_key_from_env_or_config(
        &profile.api_key_env,
        &profile.env_file,
    ) {
        request = request.bearer_auth(api_key);
    }

    let response = request.send().await.with_context(|| {
        format!(
            "Failed to query {} models from {} during auth-test validation",
            profile.display_name, url
        )
    })?;
    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    if !status.is_success() {
        anyhow::bail!(
            "{} model discovery failed (HTTP {}): {}",
            profile.display_name,
            status,
            body.trim()
        );
    }

    let parsed: OpenAiCompatibleModelsResponse =
        serde_json::from_str(&body).with_context(|| {
            format!(
                "Failed to parse {} model discovery response from {}",
                profile.display_name, url
            )
        })?;
    Ok(parsed
        .data
        .into_iter()
        .map(|model| model.id.trim().to_string())
        .find(|model| !model.is_empty()))
}

#[cfg(test)]
mod nvidia_nim_tool_smoke_tests {
    use super::*;

    #[test]
    fn skips_kimi_k2_6_tool_smoke_for_nvidia_nim() {
        let detail = tool_smoke_skip_detail_for_choice(
            &super::super::provider_init::ProviderChoice::NvidiaNim,
            Some("moonshotai/kimi-k2.6"),
        )
        .expect("kimi-k2.6 should skip NIM tool smoke");

        assert!(detail.contains("NVIDIA NIM model 'moonshotai/kimi-k2.6'"));
        assert!(detail.contains("tool_smoke"));
    }

    #[test]
    fn allows_other_nvidia_nim_models_to_attempt_tool_smoke() {
        assert!(
            tool_smoke_skip_detail_for_choice(
                &super::super::provider_init::ProviderChoice::NvidiaNim,
                Some("nvidia/llama-3.1-nemotron-ultra-253b-v1"),
            )
            .is_none()
        );
    }

    #[test]
    fn does_not_apply_nvidia_skip_to_other_providers() {
        assert!(
            tool_smoke_skip_detail_for_choice(
                &super::super::provider_init::ProviderChoice::Groq,
                Some("moonshotai/kimi-k2.6"),
            )
            .is_none()
        );
    }

    #[test]
    fn skips_fpt_tool_smoke_for_vllm_auto_tool_choice_gap() {
        let detail = tool_smoke_skip_detail_for_choice(
            &super::super::provider_init::ProviderChoice::Fpt,
            Some("FPT.AI-KIE-v1.7"),
        )
        .expect("FPT should skip tool smoke when server-side auto tool parsing is unavailable");

        assert!(detail.contains("FPT model 'FPT.AI-KIE-v1.7'"));
        assert!(detail.contains("vLLM-style endpoint"));
        assert!(detail.contains("Basic provider smoke still validates chat"));
    }

    #[test]
    fn allows_cerebras_models_to_attempt_tool_smoke() {
        assert!(
            tool_smoke_skip_detail_for_choice(
                &super::super::provider_init::ProviderChoice::Cerebras,
                Some("gpt-oss-120b"),
            )
            .is_none()
        );
        assert!(
            tool_smoke_skip_detail_for_choice(
                &super::super::provider_init::ProviderChoice::Cerebras,
                Some("zai-glm-4.7"),
            )
            .is_none()
        );
    }
}

async fn run_provider_smoke_for_choice(
    choice: &super::provider_init::ProviderChoice,
    model: Option<&str>,
    prompt: &str,
) -> Result<String> {
    run_auth_test_with_retry(async || {
        let provider = super::provider_init::init_provider_for_validation(choice, model)
            .await
            .with_context(|| format!("Failed to initialize {} provider", choice.as_arg_value()))?;
        let output = provider
            .complete_simple(prompt, "")
            .await
            .with_context(|| format!("{} provider smoke prompt failed", choice.as_arg_value()))?;
        Ok(output.trim().to_string())
    })
    .await
}

async fn run_provider_tool_smoke_for_choice(
    choice: &super::provider_init::ProviderChoice,
    model: Option<&str>,
    prompt: &str,
) -> Result<String> {
    run_auth_test_with_retry(async || {
        let (provider, registry) =
            super::provider_init::init_provider_and_registry_for_validation(choice, model)
                .await
                .with_context(|| {
                    format!("Failed to initialize {} provider", choice.as_arg_value())
                })?;
        registry
            .register_mcp_tools(None, None, Some("auth-test".to_string()))
            .await;

        let allowed_tools = HashSet::from([AUTH_TEST_TOOL_NAME.to_string()]);
        let mut agent = crate::agent::Agent::new_with_session(
            provider,
            registry,
            crate::session::Session::create(None, None),
            Some(allowed_tools),
        );
        let transcript_start = agent.messages().len();
        let output = agent.run_once_capture(prompt).await.with_context(|| {
            format!(
                "{} tool-enabled smoke prompt failed during agent turn execution",
                choice.as_arg_value()
            )
        })?;
        validate_auth_test_tool_smoke_transcript(&agent.messages()[transcript_start..], &output)
            .with_context(|| {
                format!(
                    "{} tool-enabled smoke prompt did not complete a valid real Jcode tool loop",
                    choice.as_arg_value()
                )
            })?;

        Ok(output.trim().to_string())
    })
    .await
}

fn validate_auth_test_tool_smoke_transcript(
    messages: &[crate::session::StoredMessage],
    output: &str,
) -> Result<()> {
    if output.trim() != "AUTH_TEST_OK" {
        anyhow::bail!(
            "tool smoke final response was {:?}, expected exactly AUTH_TEST_OK",
            output.trim()
        );
    }

    let mut tool_uses = Vec::new();
    let mut tool_results = Vec::new();
    for message in messages {
        for block in &message.content {
            match block {
                crate::message::ContentBlock::ToolUse { id, name, input, .. } => {
                    tool_uses.push((id.as_str(), name.as_str(), input));
                }
                crate::message::ContentBlock::ToolResult {
                    tool_use_id,
                    content,
                    is_error,
                } => {
                    tool_results.push((tool_use_id.as_str(), content.as_str(), *is_error));
                }
                _ => {}
            }
        }
    }

    ensure_auth_test_tool_count(tool_uses.len())?;
    let (tool_id, tool_name, input) = tool_uses[0];
    if tool_id.trim().is_empty() {
        anyhow::bail!("tool smoke emitted tool call with empty id");
    }
    let tool_call = crate::message::ToolCall {
        id: tool_id.to_string(),
        name: tool_name.to_string(),
        input: input.clone(),
        intent: None, thought_signature: None, };
    if let Some(error) = tool_call.validation_error() {
        anyhow::bail!("tool smoke emitted invalid tool call: {error}");
    }
    if tool_name != AUTH_TEST_TOOL_NAME {
        anyhow::bail!(
            "tool smoke used unexpected tool {:?}; expected {:?}",
            tool_name,
            AUTH_TEST_TOOL_NAME
        );
    }
    let command = input
        .get("command")
        .and_then(|value| value.as_str())
        .unwrap_or_default()
        .trim();
    if command != AUTH_TEST_TOOL_COMMAND {
        anyhow::bail!(
            "tool smoke used unsafe or unexpected command {:?}; expected {:?}",
            command,
            AUTH_TEST_TOOL_COMMAND
        );
    }

    let matching_results = tool_results
        .iter()
        .filter(|(tool_use_id, _, _)| *tool_use_id == tool_id)
        .collect::<Vec<_>>();
    if matching_results.len() != 1 {
        anyhow::bail!(
            "tool smoke expected exactly one matching tool result for {}, got {}",
            tool_id,
            matching_results.len()
        );
    }
    let (_, content, is_error) = matching_results[0];
    if is_error.unwrap_or(false) {
        anyhow::bail!("tool smoke tool result was marked as an error: {content}");
    }
    if !content.contains(AUTH_TEST_TOOL_OUTPUT_MARKER) {
        anyhow::bail!(
            "tool smoke result did not contain marker {:?}: {}",
            AUTH_TEST_TOOL_OUTPUT_MARKER,
            crate::util::truncate_str(content, 500)
        );
    }

    let errored_results = tool_results
        .iter()
        .filter(|(_, content, is_error)| {
            is_error.unwrap_or(false) || content.contains("Invalid tool call")
        })
        .count();
    if errored_results > 0 {
        anyhow::bail!("tool smoke transcript contained {errored_results} errored tool result(s)");
    }

    Ok(())
}

fn ensure_auth_test_tool_count(count: usize) -> Result<()> {
    match count {
        1 => Ok(()),
        0 => anyhow::bail!("tool smoke did not emit any tool call"),
        other => anyhow::bail!("tool smoke emitted {other} tool calls; expected exactly one"),
    }
}

async fn run_auth_test_with_retry<F, Fut>(mut f: F) -> Result<String>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<String>>,
{
    const RETRY_DELAYS: &[Duration] = &[Duration::from_secs(3), Duration::from_secs(8)];

    let mut last_err = None;
    for (attempt, delay) in RETRY_DELAYS.iter().enumerate() {
        match f().await {
            Ok(output) => return Ok(output),
            Err(err) if auth_test_error_is_retryable(&err) => {
                last_err = Some(err);
                crate::logging::warn(&format!(
                    "auth-test transient failure on attempt {} - retrying in {}s",
                    attempt + 1,
                    delay.as_secs()
                ));
                tokio::time::sleep(*delay).await;
            }
            Err(err) => return Err(err),
        }
    }

    match f().await {
        Ok(output) => Ok(output),
        Err(err) if last_err.is_some() => Err(err),
        Err(err) => Err(err),
    }
}

pub(crate) fn auth_test_error_is_retryable(err: &anyhow::Error) -> bool {
    let text = format!("{err:#}").to_ascii_lowercase();
    [
        "http 429",
        "too many requests",
        "resource_exhausted",
        "rate_limit_exceeded",
        "rate limit",
        "temporarily unavailable",
        "timeout",
        "connection reset",
        "service unavailable",
        "http 500",
        "http 502",
        "http 503",
        "http 504",
    ]
    .iter()
    .any(|needle| text.contains(needle))
}

fn print_auth_test_reports(reports: &[AuthTestProviderReport]) {
    for report in reports {
        println!("=== auth-test: {} ===", report.provider);
        if !report.credential_paths.is_empty() {
            println!("credential paths:");
            for path in &report.credential_paths {
                println!("  - {}", path);
            }
        }
        for step in &report.steps {
            let marker = if step.ok { "✓" } else { "✗" };
            println!("{} {} — {}", marker, step.name, step.detail);
        }
        if let Some(output) = report.smoke_output.as_deref() {
            println!("smoke output: {}", output);
        }
        if let Some(output) = report.tool_smoke_output.as_deref() {
            println!("tool smoke output: {}", output);
        }
        println!("result: {}\n", if report.success { "PASS" } else { "FAIL" });
    }
}

#[cfg(test)]
mod auth_tool_smoke_tests {
    use super::*;

    fn stored_message(
        role: crate::message::Role,
        content: Vec<crate::message::ContentBlock>,
    ) -> crate::session::StoredMessage {
        crate::session::StoredMessage {
            id: "msg_test".to_string(),
            role,
            content,
            display_role: None,
            timestamp: None,
            tool_duration_ms: None,
            token_usage: None,
        }
    }

    fn valid_tool_transcript() -> Vec<crate::session::StoredMessage> {
        vec![
            stored_message(
                crate::message::Role::Assistant,
                vec![crate::message::ContentBlock::ToolUse {
                    id: "call_1".to_string(),
                    name: AUTH_TEST_TOOL_NAME.to_string(),
                    input: serde_json::json!({"command": AUTH_TEST_TOOL_COMMAND}), thought_signature: None, }],
            ),
            stored_message(
                crate::message::Role::User,
                vec![crate::message::ContentBlock::ToolResult {
                    tool_use_id: "call_1".to_string(),
                    content: format!("{}\n", AUTH_TEST_TOOL_OUTPUT_MARKER),
                    is_error: None,
                }],
            ),
        ]
    }

    #[test]
    fn auth_test_tool_smoke_validation_accepts_real_successful_loop() {
        validate_auth_test_tool_smoke_transcript(&valid_tool_transcript(), "AUTH_TEST_OK")
            .expect("valid tool smoke transcript");
    }

    #[test]
    fn auth_test_tool_smoke_validation_rejects_no_tool_call() {
        let err = validate_auth_test_tool_smoke_transcript(&[], "AUTH_TEST_OK")
            .expect_err("missing tool call should fail")
            .to_string();
        assert!(err.contains("did not emit any tool call"), "{err}");
    }

    #[test]
    fn auth_test_tool_smoke_validation_rejects_empty_tool_name() {
        let mut messages = valid_tool_transcript();
        if let crate::message::ContentBlock::ToolUse { name, .. } = &mut messages[0].content[0] {
            name.clear();
        }
        let err = validate_auth_test_tool_smoke_transcript(&messages, "AUTH_TEST_OK")
            .expect_err("empty tool name should fail")
            .to_string();
        assert!(err.contains("tool name must not be empty"), "{err}");
    }

    #[test]
    fn auth_test_tool_smoke_validation_rejects_unexpected_command() {
        let mut messages = valid_tool_transcript();
        if let crate::message::ContentBlock::ToolUse { input, .. } = &mut messages[0].content[0] {
            *input = serde_json::json!({"command": "ls"});
        }
        let err = validate_auth_test_tool_smoke_transcript(&messages, "AUTH_TEST_OK")
            .expect_err("unexpected command should fail")
            .to_string();
        assert!(err.contains("unsafe or unexpected command"), "{err}");
    }

    #[test]
    fn auth_test_tool_smoke_validation_rejects_tool_result_error() {
        let mut messages = valid_tool_transcript();
        if let crate::message::ContentBlock::ToolResult { is_error, .. } =
            &mut messages[1].content[0]
        {
            *is_error = Some(true);
        }
        let err = validate_auth_test_tool_smoke_transcript(&messages, "AUTH_TEST_OK")
            .expect_err("errored tool result should fail")
            .to_string();
        assert!(err.contains("marked as an error"), "{err}");
    }
}
