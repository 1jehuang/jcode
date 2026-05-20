use super::*;

pub(super) fn auto_scriptable_flow_reason(
    provider: LoginProviderDescriptor,
    options: &LoginOptions,
    stdin_is_terminal: bool,
) -> Option<&'static str> {
    if options.print_auth_url || options.complete || options.has_provided_input() {
        return None;
    }

    let supports_scriptable = matches!(
        provider.target,
        LoginProviderTarget::Claude
            | LoginProviderTarget::OpenAi
            | LoginProviderTarget::Gemini
            | LoginProviderTarget::Antigravity
            | LoginProviderTarget::Google
            | LoginProviderTarget::Copilot
    );
    if !supports_scriptable {
        return None;
    }

    if !stdin_is_terminal {
        Some("non_interactive_terminal")
    } else if auth::browser_suppressed(options.no_browser) {
        Some("no_browser_requested")
    } else {
        None
    }
}

pub(super) async fn run_scriptable_login_provider(
    provider: LoginProviderDescriptor,
    account_label: Option<&str>,
    options: &LoginOptions,
) -> Result<LoginFlowOutcome> {
    if options.print_auth_url {
        return start_scriptable_login(provider, account_label, options).await;
    }

    let input = options.resolve_provided_input()?;
    if options.complete && input.is_some() {
        anyhow::bail!(
            "Use either --complete or an explicit --callback-url / --auth-code input, not both."
        );
    }
    complete_scriptable_login(provider, account_label, options, input).await
}

pub(super) async fn start_scriptable_login(
    provider: LoginProviderDescriptor,
    account_label: Option<&str>,
    options: &LoginOptions,
) -> Result<LoginFlowOutcome> {
    let (pending, auth_url, input_kind, user_code, expires_at_ms) = match provider.target {
        LoginProviderTarget::Claude => {
            let label = auth::claude::login_target_label(account_label)?;
            let (verifier, challenge) = auth::oauth::generate_pkce_public();
            let redirect_uri = auth::oauth::claude::REDIRECT_URI.to_string();
            let auth_url = auth::oauth::claude_auth_url(&redirect_uri, &challenge, &verifier);
            (
                PendingScriptableLogin::Claude {
                    account_label: label,
                    verifier,
                    redirect_uri,
                },
                auth_url,
                "auth_code_or_callback_url",
                None,
                PendingScriptableLogin::Claude {
                    account_label: String::new(),
                    verifier: String::new(),
                    redirect_uri: String::new(),
                }
                .default_expires_at_ms(),
            )
        }
        LoginProviderTarget::OpenAi => {
            let label = auth::codex::login_target_label(account_label)?;
            let (verifier, challenge) = auth::oauth::generate_pkce_public();
            let state = auth::oauth::generate_state_public();
            let redirect_uri = auth::oauth::openai::default_redirect_uri();
            let auth_url = auth::oauth::openai_auth_url_with_prompt(
                &redirect_uri,
                &challenge,
                &state,
                Some("login"),
            );
            (
                PendingScriptableLogin::Openai {
                    account_label: label,
                    verifier,
                    state,
                    redirect_uri,
                },
                auth_url,
                "callback_url",
                None,
                PendingScriptableLogin::Openai {
                    account_label: String::new(),
                    verifier: String::new(),
                    state: String::new(),
                    redirect_uri: String::new(),
                }
                .default_expires_at_ms(),
            )
        }
        LoginProviderTarget::Gemini => {
            let (verifier, challenge) = auth::oauth::generate_pkce_public();
            let state = auth::oauth::generate_state_public();
            let redirect_uri = auth::gemini::GEMINI_MANUAL_REDIRECT_URI.to_string();
            let auth_url = auth::gemini::build_manual_auth_url(&redirect_uri, &challenge, &state)?;
            (
                PendingScriptableLogin::Gemini {
                    verifier,
                    redirect_uri,
                },
                auth_url,
                "auth_code",
                None,
                PendingScriptableLogin::Gemini {
                    verifier: String::new(),
                    redirect_uri: String::new(),
                }
                .default_expires_at_ms(),
            )
        }
        LoginProviderTarget::Antigravity => {
            let (verifier, challenge) = auth::oauth::generate_pkce_public();
            let state = auth::oauth::generate_state_public();
            let redirect_uri = auth::antigravity::redirect_uri(auth::antigravity::DEFAULT_PORT);
            let auth_url = auth::antigravity::build_auth_url(&redirect_uri, &challenge, &state)?;
            (
                PendingScriptableLogin::Antigravity {
                    verifier,
                    state,
                    redirect_uri,
                },
                auth_url,
                "callback_url",
                None,
                PendingScriptableLogin::Antigravity {
                    verifier: String::new(),
                    state: String::new(),
                    redirect_uri: String::new(),
                }
                .default_expires_at_ms(),
            )
        }
        LoginProviderTarget::Google => {
            let creds = auth::google::load_credentials().context(
                "Google/Gmail scriptable auth requires saved OAuth credentials first. Run `jcode login --provider google` once or save google credentials manually.",
            )?;
            let tier = options
                .google_access_tier
                .unwrap_or(auth::google::GmailAccessTier::Full);
            let (verifier, challenge) = auth::oauth::generate_pkce_public();
            let state = auth::oauth::generate_state_public();
            let redirect_uri = format!("http://127.0.0.1:{}", auth::google::DEFAULT_PORT);
            let auth_url =
                auth::google::build_auth_url(&creds, tier, &redirect_uri, &challenge, &state);
            (
                PendingScriptableLogin::Google {
                    verifier,
                    state,
                    redirect_uri,
                    tier,
                },
                auth_url,
                "callback_url",
                None,
                PendingScriptableLogin::Google {
                    verifier: String::new(),
                    state: String::new(),
                    redirect_uri: String::new(),
                    tier,
                }
                .default_expires_at_ms(),
            )
        }
        LoginProviderTarget::Copilot => {
            let client = crate::provider::shared_http_client();
            let device_resp = auth::copilot::initiate_device_flow(&client).await?;
            (
                PendingScriptableLogin::Copilot {
                    device_code: device_resp.device_code.clone(),
                    user_code: device_resp.user_code.clone(),
                    verification_uri: device_resp.verification_uri.clone(),
                    interval: device_resp.interval,
                },
                device_resp.verification_uri,
                "complete",
                Some(device_resp.user_code),
                current_time_ms() + (device_resp.expires_in as i64 * 1000),
            )
        }
        _ => {
            anyhow::bail!(
                "`--print-auth-url` is currently supported for: claude, openai, gemini, antigravity, google, copilot."
            )
        }
    };

    let pending_path = pending.pending_path()?;
    cleanup_stale_pending_login_files()?;
    let record = PendingScriptableLoginRecord {
        expires_at_ms,
        login: pending,
    };
    crate::storage::write_json_secret(&pending_path, &record)?;
    emit_scriptable_auth_prompt(
        provider.id,
        &auth_url,
        input_kind,
        &pending_path,
        user_code.as_deref(),
        expires_at_ms,
        options.json,
    )?;
    Ok(LoginFlowOutcome::Deferred)
}

pub(super) async fn complete_scriptable_login(
    provider: LoginProviderDescriptor,
    account_label: Option<&str>,
    options: &LoginOptions,
    input: Option<ProvidedAuthInput>,
) -> Result<LoginFlowOutcome> {
    if account_label.is_some() {
        anyhow::bail!(
            "Do not pass --account when completing a scriptable login. The pending login already stores the target account."
        );
    }

    match provider.target {
        LoginProviderTarget::Claude => {
            complete_scriptable_claude_login(provider.id, options, require_scriptable_input(input)?)
                .await
        }
        LoginProviderTarget::OpenAi => {
            complete_scriptable_openai_login(provider.id, options, require_scriptable_input(input)?)
                .await
        }
        LoginProviderTarget::Gemini => {
            complete_scriptable_gemini_login(provider.id, options, require_scriptable_input(input)?)
                .await
        }
        LoginProviderTarget::Antigravity => {
            complete_scriptable_antigravity_login(
                provider.id,
                options,
                require_scriptable_input(input)?,
            )
            .await
        }
        LoginProviderTarget::Google => {
            complete_scriptable_google_login(provider.id, options, require_scriptable_input(input)?)
                .await
        }
        LoginProviderTarget::Copilot => {
            if input.is_some() {
                anyhow::bail!(
                    "Copilot completion uses `--complete` and does not accept --callback-url or --auth-code."
                )
            }
            if !options.complete {
                anyhow::bail!("Copilot completion requires `--complete`.")
            }
            complete_scriptable_copilot_login(provider.id, options).await
        }
        _ => anyhow::bail!(
            "Scriptable completion is currently supported for: claude, openai, gemini, antigravity, google, copilot."
        ),
    }
}

pub(super) async fn complete_scriptable_claude_login(
    provider_id: &str,
    options: &LoginOptions,
    input: ProvidedAuthInput,
) -> Result<LoginFlowOutcome> {
    let pending_path = pending_login_path("claude")?;
    let PendingScriptableLogin::Claude {
        account_label,
        verifier,
        redirect_uri,
    } = load_pending_login(&pending_path, "claude")?
    else {
        anyhow::bail!("Pending Claude login state is invalid.");
    };

    let raw_input = match input {
        ProvidedAuthInput::CallbackUrl(value) | ProvidedAuthInput::AuthCode(value) => value,
    };
    let selected_redirect_uri =
        auth::oauth::claude_redirect_uri_for_input(&raw_input, &redirect_uri);
    let tokens =
        auth::oauth::exchange_claude_code(&verifier, &raw_input, &selected_redirect_uri).await?;
    auth::oauth::save_claude_tokens_for_account(&tokens, &account_label)?;
    let profile_email =
        auth::oauth::update_claude_account_profile(&account_label, &tokens.access_token)
            .await
            .unwrap_or(None);
    clear_pending_login(&pending_path);
    crate::telemetry::record_auth_success(provider_id, "oauth");
    emit_scriptable_auth_success(
        options.json,
        ScriptableAuthSuccess {
            status: "authenticated",
            provider: provider_id.to_string(),
            account_label: Some(account_label.clone()),
            credentials_path: Some(auth::claude::jcode_path()?.display().to_string()),
            email: profile_email.clone(),
        },
    )?;
    if !options.json {
        eprintln!("Successfully logged in to Claude!");
        eprintln!(
            "Account '{}' stored at {}",
            account_label,
            auth::claude::jcode_path()?.display()
        );
        if let Some(email) = profile_email {
            eprintln!("Profile email: {}", email);
        }
    }
    Ok(LoginFlowOutcome::Completed)
}

pub(super) async fn complete_scriptable_openai_login(
    provider_id: &str,
    options: &LoginOptions,
    input: ProvidedAuthInput,
) -> Result<LoginFlowOutcome> {
    let pending_path = pending_login_path("openai")?;
    let PendingScriptableLogin::Openai {
        account_label,
        verifier,
        state,
        redirect_uri,
    } = load_pending_login(&pending_path, "openai")?
    else {
        anyhow::bail!("Pending OpenAI login state is invalid.");
    };

    let callback_input = match input {
        ProvidedAuthInput::CallbackUrl(value) => value,
        ProvidedAuthInput::AuthCode(_) => {
            anyhow::bail!(
                "OpenAI completion requires --callback-url because state validation is required."
            )
        }
    };
    let tokens = auth::oauth::exchange_openai_callback_input(
        &verifier,
        &callback_input,
        &state,
        &redirect_uri,
    )
    .await?;
    auth::oauth::save_openai_tokens_for_account(&tokens, &account_label)?;
    clear_pending_login(&pending_path);
    crate::telemetry::record_auth_success(provider_id, "oauth");
    let credentials_path = crate::storage::jcode_dir()?.join("openai-auth.json");
    emit_scriptable_auth_success(
        options.json,
        ScriptableAuthSuccess {
            status: "authenticated",
            provider: provider_id.to_string(),
            account_label: Some(account_label.clone()),
            credentials_path: Some(credentials_path.display().to_string()),
            email: None,
        },
    )?;
    if !options.json {
        eprintln!(
            "Successfully logged in to OpenAI! Account '{}' saved to {}",
            account_label,
            credentials_path.display()
        );
    }
    Ok(LoginFlowOutcome::Completed)
}

pub(super) async fn complete_scriptable_gemini_login(
    provider_id: &str,
    options: &LoginOptions,
    input: ProvidedAuthInput,
) -> Result<LoginFlowOutcome> {
    let pending_path = pending_login_path("gemini")?;
    let PendingScriptableLogin::Gemini {
        verifier,
        redirect_uri,
    } = load_pending_login(&pending_path, "gemini")?
    else {
        anyhow::bail!("Pending Gemini login state is invalid.");
    };

    let auth_code = match input {
        ProvidedAuthInput::AuthCode(value) => value,
        ProvidedAuthInput::CallbackUrl(_) => {
            anyhow::bail!("Gemini completion requires --auth-code.")
        }
    };
    let tokens = auth::gemini::exchange_callback_code(&auth_code, &verifier, &redirect_uri).await?;
    clear_pending_login(&pending_path);
    crate::telemetry::record_auth_success(provider_id, "oauth");
    emit_scriptable_auth_success(
        options.json,
        ScriptableAuthSuccess {
            status: "authenticated",
            provider: provider_id.to_string(),
            account_label: None,
            credentials_path: Some(auth::gemini::tokens_path()?.display().to_string()),
            email: tokens.email.clone(),
        },
    )?;
    if !options.json {
        eprintln!("Successfully logged in to Gemini!");
        eprintln!("Tokens saved to {}", auth::gemini::tokens_path()?.display());
        if let Some(email) = tokens.email.as_deref() {
            eprintln!("Google account: {}", email);
        }
    }
    Ok(LoginFlowOutcome::Completed)
}

pub(super) async fn complete_scriptable_antigravity_login(
    provider_id: &str,
    options: &LoginOptions,
    input: ProvidedAuthInput,
) -> Result<LoginFlowOutcome> {
    let pending_path = pending_login_path("antigravity")?;
    let PendingScriptableLogin::Antigravity {
        verifier,
        state,
        redirect_uri,
    } = load_pending_login(&pending_path, "antigravity")?
    else {
        anyhow::bail!("Pending Antigravity login state is invalid.");
    };

    let callback_input = match input {
        ProvidedAuthInput::CallbackUrl(value) => value,
        ProvidedAuthInput::AuthCode(_) => {
            anyhow::bail!("Antigravity completion requires --callback-url.")
        }
    };
    let tokens = auth::antigravity::exchange_callback_input(
        &verifier,
        &callback_input,
        Some(&state),
        &redirect_uri,
    )
    .await?;
    clear_pending_login(&pending_path);
    crate::telemetry::record_auth_success(provider_id, "oauth");
    emit_scriptable_auth_success(
        options.json,
        ScriptableAuthSuccess {
            status: "authenticated",
            provider: provider_id.to_string(),
            account_label: None,
            credentials_path: Some(auth::antigravity::tokens_path()?.display().to_string()),
            email: tokens.email.clone(),
        },
    )?;
    if !options.json {
        eprintln!("Successfully logged in to Antigravity!");
        eprintln!(
            "Tokens saved to {}",
            auth::antigravity::tokens_path()?.display()
        );
        if let Some(email) = tokens.email.as_deref() {
            eprintln!("Google account: {}", email);
        }
        if let Some(project_id) = tokens.project_id.as_deref() {
            eprintln!("Resolved Antigravity project: {}", project_id);
        }
    }
    Ok(LoginFlowOutcome::Completed)
}

pub(super) async fn complete_scriptable_google_login(
    provider_id: &str,
    options: &LoginOptions,
    input: ProvidedAuthInput,
) -> Result<LoginFlowOutcome> {
    let pending_path = pending_login_path("google")?;
    let PendingScriptableLogin::Google {
        verifier,
        state,
        redirect_uri,
        tier,
    } = load_pending_login(&pending_path, "google")?
    else {
        anyhow::bail!("Pending Google login state is invalid.");
    };

    let callback_input = match input {
        ProvidedAuthInput::CallbackUrl(value) => value,
        ProvidedAuthInput::AuthCode(_) => {
            anyhow::bail!("Google completion requires --callback-url.")
        }
    };
    let creds = auth::google::load_credentials().context(
        "Google/Gmail completion requires saved OAuth credentials first. Run `jcode login --provider google` once or save google credentials manually.",
    )?;
    let tokens = auth::google::exchange_callback_input(
        &creds,
        &verifier,
        &callback_input,
        &state,
        &redirect_uri,
        tier,
    )
    .await?;
    clear_pending_login(&pending_path);
    crate::telemetry::record_auth_success(provider_id, "oauth");
    emit_scriptable_auth_success(
        options.json,
        ScriptableAuthSuccess {
            status: "authenticated",
            provider: provider_id.to_string(),
            account_label: None,
            credentials_path: Some(auth::google::tokens_path()?.display().to_string()),
            email: tokens.email.clone(),
        },
    )?;
    if !options.json {
        eprintln!("Successfully logged in to Google/Gmail!");
        if let Some(email) = tokens.email.as_deref() {
            eprintln!("Account: {}", email);
        }
        eprintln!("Access tier: {}", tokens.tier.label());
        eprintln!("Tokens saved to {}", auth::google::tokens_path()?.display());
    }
    Ok(LoginFlowOutcome::Completed)
}

pub(super) async fn complete_scriptable_copilot_login(
    provider_id: &str,
    options: &LoginOptions,
) -> Result<LoginFlowOutcome> {
    let pending_path = pending_login_path("copilot")?;
    let PendingScriptableLogin::Copilot {
        device_code,
        interval,
        ..
    } = load_pending_login(&pending_path, "copilot")?
    else {
        anyhow::bail!("Pending Copilot login state is invalid.");
    };

    let client = crate::provider::shared_http_client();
    let token = auth::copilot::poll_for_access_token(&client, &device_code, interval).await?;
    let username = auth::copilot::fetch_github_username(&client, &token)
        .await
        .unwrap_or_else(|_| "unknown".to_string());
    auth::copilot::save_github_token(&token, &username)?;
    clear_pending_login(&pending_path);
    crate::telemetry::record_auth_success(provider_id, "oauth_device_code");
    emit_scriptable_auth_success(
        options.json,
        ScriptableAuthSuccess {
            status: "authenticated",
            provider: provider_id.to_string(),
            account_label: Some(username.clone()),
            credentials_path: Some(auth::copilot::saved_hosts_path().display().to_string()),
            email: None,
        },
    )?;
    if !options.json {
        eprintln!("✓ Authenticated as {} via GitHub Copilot", username);
        eprintln!("Saved at {}", auth::copilot::saved_hosts_path().display());
    }
    Ok(LoginFlowOutcome::Completed)
}

pub(super) fn pending_login_path(key: &str) -> Result<PathBuf> {
    Ok(crate::storage::jcode_dir()?
        .join("pending-login")
        .join(format!("{key}.json")))
}

pub(super) fn pending_login_dir() -> Result<PathBuf> {
    Ok(crate::storage::jcode_dir()?.join("pending-login"))
}

pub(super) fn require_scriptable_input(
    input: Option<ProvidedAuthInput>,
) -> Result<ProvidedAuthInput> {
    input.ok_or_else(|| anyhow::anyhow!("No scriptable auth input was provided."))
}

pub(super) fn load_pending_login(path: &PathBuf, provider: &str) -> Result<PendingScriptableLogin> {
    if !path.exists() {
        anyhow::bail!(
            "No pending {} login state found. Run `jcode login --provider {} --print-auth-url` first.",
            provider,
            provider
        );
    }
    crate::storage::harden_secret_file_permissions(path);
    let data = std::fs::read_to_string(path).with_context(|| {
        format!(
            "Failed to read pending {} login state from {}",
            provider,
            path.display()
        )
    })?;
    if let Ok(record) = serde_json::from_str::<PendingScriptableLoginRecord>(&data) {
        if record.expires_at_ms <= current_time_ms() {
            clear_pending_login(path);
            anyhow::bail!(
                "Pending {} login state expired. Run `jcode login --provider {} --print-auth-url` again.",
                provider,
                provider
            );
        }
        cleanup_stale_pending_login_files()?;
        return Ok(record.login);
    }
    let login = serde_json::from_str::<PendingScriptableLogin>(&data).with_context(|| {
        format!(
            "Failed to load pending {} login state from {}",
            provider,
            path.display()
        )
    })?;
    cleanup_stale_pending_login_files()?;
    Ok(login)
}

pub(super) fn clear_pending_login(path: &PathBuf) {
    let _ = std::fs::remove_file(path);
}

pub(super) fn cleanup_stale_pending_login_files() -> Result<()> {
    let dir = pending_login_dir()?;
    if !dir.exists() {
        return Ok(());
    }
    for entry in std::fs::read_dir(&dir)? {
        let entry = match entry {
            Ok(entry) => entry,
            Err(_) => continue,
        };
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
            continue;
        }
        let Ok(data) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Ok(record) = serde_json::from_str::<PendingScriptableLoginRecord>(&data) else {
            continue;
        };
        if record.expires_at_ms <= current_time_ms() {
            let _ = std::fs::remove_file(&path);
        }
    }
    Ok(())
}

pub(super) fn current_time_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

pub(super) fn resolve_auth_input(value: &str) -> Result<String> {
    if value != "-" {
        return Ok(value.to_string());
    }

    let mut input = String::new();
    std::io::stdin()
        .read_line(&mut input)
        .context("Failed to read auth input from stdin")?;
    let trimmed = input.trim();
    if trimmed.is_empty() {
        anyhow::bail!("No auth input was provided on stdin.");
    }
    Ok(trimmed.to_string())
}

pub(super) fn emit_scriptable_auth_prompt(
    provider: &str,
    auth_url: &str,
    input_kind: &str,
    pending_path: &Path,
    user_code: Option<&str>,
    expires_at_ms: i64,
    json: bool,
) -> Result<()> {
    let resume_command = scriptable_resume_command(provider, input_kind);
    let prompt = ScriptableAuthPrompt {
        status: "pending",
        provider: provider.to_string(),
        auth_url: auth_url.to_string(),
        input_kind: input_kind.to_string(),
        pending_path: pending_path.display().to_string(),
        user_code: user_code.map(str::to_string),
        expires_at_ms,
        resume_command: resume_command.clone(),
    };
    if json {
        println!("{}", serde_json::to_string(&prompt)?);
    } else {
        println!("{}", auth_url);
        if let Some(user_code) = user_code {
            eprintln!("User code: {}", user_code);
        }
        eprintln!("Auth URL printed to stdout.");
        eprintln!("Complete this login later with `{}`.", resume_command);
        eprintln!(
            "This pending login expires at {} ms since epoch.",
            expires_at_ms
        );
        eprintln!("Pending login state saved at {}", pending_path.display());
    }
    Ok(())
}

pub(super) fn scriptable_resume_command(provider: &str, input_kind: &str) -> String {
    match input_kind {
        "callback_url" => {
            format!(
                "jcode login --provider {} --callback-url '<url-or-query>'",
                provider
            )
        }
        "auth_code" => format!("jcode login --provider {} --auth-code '<code>'", provider),
        "complete" => format!("jcode login --provider {} --complete", provider),
        _ => format!(
            "jcode login --provider {} --callback-url '<url>'  # or --auth-code '<code>'",
            provider
        ),
    }
}

pub(super) fn emit_scriptable_auth_success(
    json: bool,
    success: ScriptableAuthSuccess,
) -> Result<()> {
    if json {
        println!("{}", serde_json::to_string(&success)?);
    }
    Ok(())
}
