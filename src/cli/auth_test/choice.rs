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
    if resolved.requires_api_key || resolved.default_model.is_some() {
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

async fn discover_openai_compatible_validation_model(
    profile: &crate::provider_catalog::ResolvedOpenAiCompatibleProfile,
) -> Result<Option<String>> {
    let url = format!("{}/models", profile.api_base.trim_end_matches('/'));
    let mut request = crate::provider::shared_http_client().get(&url);
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

        let mut agent = crate::agent::Agent::new(provider, registry);
        let output = agent.run_once_capture(prompt).await.with_context(|| {
            format!(
                "{} tool-enabled smoke prompt failed during agent turn execution",
                choice.as_arg_value()
            )
        })?;

        Ok(output.trim().to_string())
    })
    .await
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
