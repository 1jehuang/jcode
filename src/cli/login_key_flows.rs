use super::*;

/// Resolve an API key for a non-interactive-capable login flow.
///
/// Order: `--api-key <value>` (`-` reads one line from stdin), then
/// `--api-key-env <NAME>` (reads the key from that environment variable, so
/// secrets never appear in argv), then an interactive/piped secret prompt.
pub(super) fn obtain_api_key(options: &LoginOptions, display_name: &str) -> Result<String> {
    if let Some(value) = options.openai_compatible_api_key.as_deref() {
        let key = resolve_auth_input(value.trim())?;
        let key = key.trim().to_string();
        anyhow::ensure!(!key.is_empty(), "No API key provided.");
        return Ok(key);
    }
    if let Some(name) = options
        .openai_compatible_api_key_env
        .as_deref()
        .map(str::trim)
        .filter(|name| !name.is_empty())
    {
        anyhow::ensure!(
            crate::provider_catalog::is_safe_env_key_name(name),
            "Invalid API key environment variable name: {}",
            name
        );
        let key = match std::env::var(name) {
            Ok(value) => value.trim().to_string(),
            Err(_) => String::new(),
        };
        anyhow::ensure!(
            !key.is_empty(),
            "--api-key-env {} is not set or empty in this environment.",
            name
        );
        return Ok(key);
    }
    eprint!("Paste your {} API key: ", display_name);
    io::stdout().flush()?;
    let key = read_secret_line()?;
    if key.is_empty() {
        anyhow::bail!(
            "No API key provided. Pass --api-key <KEY>, --api-key - (stdin), or --api-key-env <VAR>."
        );
    }
    Ok(key)
}

/// Print the machine-readable result of an API-key style login.
pub(super) fn emit_key_login_success(
    options: &LoginOptions,
    provider: &str,
    env_file: &str,
) -> Result<()> {
    let path = crate::storage::app_config_dir()?.join(env_file);
    emit_scriptable_auth_success(
        options.json,
        ScriptableAuthSuccess {
            status: "authenticated",
            provider: provider.to_string(),
            account_label: None,
            credentials_path: Some(path.display().to_string()),
            email: None,
        },
    )
}

pub(super) fn login_openai_api_key_flow(options: &LoginOptions) -> Result<()> {
    eprintln!("Setting up OpenAI API key...");
    eprintln!("Get your API key from: https://platform.openai.com/api-keys\n");
    let key = obtain_api_key(options, "OpenAI")?;
    if !key.starts_with("sk-") {
        eprintln!("Warning: OpenAI API keys usually start with 'sk-'. Saving anyway.");
    }

    save_named_api_key("openai.env", "OPENAI_API_KEY", &key)?;
    eprintln!("\nSuccessfully saved OpenAI API key!");
    eprintln!(
        "Stored at {}",
        crate::storage::app_config_dir()?
            .join("openai.env")
            .display()
    );
    eprintln!("Provider: openai-api (native OpenAI Responses API)");
    crate::telemetry::record_auth_success("openai-api", "api_key");
    emit_key_login_success(options, "openai-api", "openai.env")
}

pub(super) fn login_anthropic_api_key_flow(options: &LoginOptions) -> Result<()> {
    eprintln!("Setting up Anthropic API...");
    eprintln!("Get your API key from: https://console.anthropic.com/settings/keys\n");
    let key = obtain_api_key(options, "Anthropic")?;

    if !key.starts_with("sk-ant-") {
        eprintln!("Warning: Anthropic API keys typically start with 'sk-ant-'. Saving anyway.");
    }

    save_named_api_key("anthropic.env", "ANTHROPIC_API_KEY", &key)?;
    eprintln!("\nSuccessfully saved Anthropic API key!");
    eprintln!(
        "Stored at {}",
        crate::storage::app_config_dir()?
            .join("anthropic.env")
            .display()
    );
    eprintln!("Provider: claude (native Anthropic Messages API)");
    crate::telemetry::record_auth_success("anthropic-api", "api_key");
    emit_key_login_success(options, "anthropic-api", "anthropic.env")
}

pub(super) fn login_openrouter_flow(options: &LoginOptions) -> Result<()> {
    eprintln!("Setting up OpenRouter...");
    eprintln!("Get your API key from: https://openrouter.ai/keys\n");
    let key = obtain_api_key(options, "OpenRouter")?;

    if !key.starts_with("sk-or-") {
        eprintln!("Warning: OpenRouter API keys typically start with 'sk-or-'. Saving anyway.");
    }

    save_named_api_key("openrouter.env", "OPENROUTER_API_KEY", &key)?;
    eprintln!("\nSuccessfully saved OpenRouter API key!");
    eprintln!(
        "Stored at {}",
        crate::storage::app_config_dir()?
            .join("openrouter.env")
            .display()
    );
    crate::telemetry::record_auth_success("openrouter", "api_key");
    emit_key_login_success(options, "openrouter", "openrouter.env")
}

pub(super) fn login_bedrock_flow(options: &LoginOptions) -> Result<()> {
    eprintln!("Setting up AWS Bedrock...");
    eprintln!(
        "Generate a Bedrock API key in the AWS Bedrock console: https://console.aws.amazon.com/bedrock/home#/api-keys"
    );
    eprintln!("Short-term keys are recommended for onboarding/testing.\n");

    let region = match options.region.as_deref() {
        Some(region) => region.to_string(),
        None if options.openai_compatible_api_key.is_some()
            || options.openai_compatible_api_key_env.is_some()
            || !io::stdin().is_terminal() =>
        {
            String::new()
        }
        None => read_line_trimmed("AWS region [us-east-2]: ")?,
    };
    let region = if region.trim().is_empty() {
        "us-east-2".to_string()
    } else {
        region.trim().to_string()
    };

    let key = obtain_api_key(options, "Bedrock")?;

    save_named_api_key(
        crate::provider::bedrock::ENV_FILE,
        crate::provider::bedrock::API_KEY_ENV,
        &key,
    )?;
    crate::provider_catalog::save_env_value_to_env_file(
        crate::provider::bedrock::REGION_ENV,
        crate::provider::bedrock::ENV_FILE,
        Some(&region),
    )?;

    eprintln!("\nSuccessfully saved AWS Bedrock API key!");
    eprintln!(
        "Stored at {}",
        crate::storage::app_config_dir()?
            .join(crate::provider::bedrock::ENV_FILE)
            .display()
    );
    eprintln!("Region: {}", region);
    eprintln!("Provider: bedrock (native AWS Bedrock Converse API)");
    crate::telemetry::record_auth_success("bedrock", "api_key");
    emit_key_login_success(options, "bedrock", crate::provider::bedrock::ENV_FILE)
}
