use anyhow::{Context, Result};
use std::io::{self, IsTerminal, Write};
use std::process::Command as ProcessCommand;

use crate::auth;
use crate::provider_catalog::{
    LoginProviderDescriptor, LoginProviderTarget, OPENAI_COMPAT_LOCAL_ENABLED_ENV,
    OpenAiCompatibleProfile, resolve_login_selection, resolve_openai_compatible_profile,
};

use super::provider_init::{ProviderChoice, login_provider_for_choice, save_named_api_key};

pub async fn run_login(choice: &ProviderChoice, account_label: Option<&str>) -> Result<()> {
    if let Some(provider) = login_provider_for_choice(choice) {
        if matches!(choice, ProviderChoice::ClaudeSubprocess) {
            eprintln!(
                "Warning: Claude subprocess transport is deprecated and will be removed. Direct Anthropic API is already the default for `--provider claude`."
            );
        }
        return run_login_provider(provider, account_label).await;
    }

    match choice {
        ProviderChoice::Auto => {
            let providers = crate::provider_catalog::cli_login_providers();
            if !io::stdin().is_terminal() {
                anyhow::bail!(
                    "`jcode login --provider auto` requires an interactive terminal. Use `jcode login --provider <provider>` in non-interactive mode."
                );
            }
            eprintln!("Choose a provider to log in:");
            for (index, provider) in providers.iter().enumerate() {
                eprintln!(
                    "  {}. {:<16} - {}",
                    index + 1,
                    provider.display_name,
                    provider.menu_detail
                );
            }
            eprintln!();
            let recommended = providers
                .iter()
                .filter(|provider| provider.recommended)
                .map(|provider| provider.display_name)
                .collect::<Vec<_>>();
            if !recommended.is_empty() {
                eprintln!(
                    "  Recommended if you have a subscription: {}.",
                    recommended.join(", ")
                );
            }
            eprint!("\nEnter 1-{}: ", providers.len());
            io::stdout().flush()?;

            let mut input = String::new();
            io::stdin().read_line(&mut input)?;
            if let Some(provider) = resolve_login_selection(input.trim(), &providers) {
                run_login_provider(provider, account_label).await?;
            } else {
                let valid = providers
                    .iter()
                    .map(|provider| provider.id)
                    .collect::<Vec<_>>()
                    .join("|");
                anyhow::bail!("Invalid choice. Use --provider {}", valid);
            }
        }
        _ => unreachable!("handled above"),
    }
    Ok(())
}

pub async fn run_login_provider(
    provider: LoginProviderDescriptor,
    account_label: Option<&str>,
) -> Result<()> {
    match provider.target {
        LoginProviderTarget::Jcode => login_jcode_flow()?,
        LoginProviderTarget::Claude => login_claude_flow(account_label).await?,
        LoginProviderTarget::OpenAi => login_openai_flow(account_label).await?,
        LoginProviderTarget::OpenRouter => login_openrouter_flow()?,
        LoginProviderTarget::Azure => login_azure_flow()?,
        LoginProviderTarget::OpenAiCompatible(profile) => login_openai_compatible_flow(&profile)?,
        LoginProviderTarget::Cursor => login_cursor_flow()?,
        LoginProviderTarget::Copilot => login_copilot_flow()?,
        LoginProviderTarget::Gemini => login_gemini_flow().await?,
        LoginProviderTarget::Antigravity => login_antigravity_flow().await?,
        LoginProviderTarget::Google => login_google_flow().await?,
    }
    auth::AuthStatus::invalidate_cache();
    Ok(())
}

fn login_jcode_flow() -> Result<()> {
    eprintln!("Setting up Jcode subscription access...");
    eprintln!(
        "Paste the jcode subscription API key from your account portal. This key is used for your curated jcode router access.\n"
    );
    eprint!("Paste your Jcode API key: ");
    io::stdout().flush()?;

    let key = read_secret_line()?;
    if key.is_empty() {
        anyhow::bail!("No API key provided.");
    }

    eprint!("Optional router base URL (press Enter to use the default placeholder): ");
    io::stdout().flush()?;
    let api_base = read_secret_line()?;

    let mut content = format!(
        "{}={}\n",
        crate::subscription_catalog::JCODE_API_KEY_ENV,
        key
    );
    if !api_base.trim().is_empty() {
        content.push_str(&format!(
            "{}={}\n",
            crate::subscription_catalog::JCODE_API_BASE_ENV,
            api_base.trim()
        ));
    }

    let config_dir = crate::storage::app_config_dir()?;
    let file_path = config_dir.join(crate::subscription_catalog::JCODE_ENV_FILE);
    crate::storage::write_text_secret(&file_path, &content)?;

    crate::env::set_var(crate::subscription_catalog::JCODE_API_KEY_ENV, key);
    if !api_base.trim().is_empty() {
        crate::env::set_var(
            crate::subscription_catalog::JCODE_API_BASE_ENV,
            api_base.trim(),
        );
    }

    eprintln!("\nSuccessfully saved Jcode subscription credentials!");
    eprintln!(
        "Stored at ~/.config/jcode/{}",
        crate::subscription_catalog::JCODE_ENV_FILE
    );
    eprintln!(
        "Curated models available now: {}",
        crate::subscription_catalog::curated_models()
            .iter()
            .map(|model| model.display_name)
            .collect::<Vec<_>>()
            .join(", ")
    );
    crate::telemetry::record_auth_success("jcode", "api_key");
    Ok(())
}

async fn login_claude_flow(requested_label: Option<&str>) -> Result<()> {
    let label = auth::claude::login_target_label(requested_label)?;
    eprintln!("Logging in to Claude (account: {})...", label);
    let tokens = auth::oauth::login_claude().await?;
    auth::oauth::save_claude_tokens_for_account(&tokens, &label)?;
    let profile_email =
        match auth::oauth::update_claude_account_profile(&label, &tokens.access_token).await {
            Ok(email) => email,
            Err(e) => {
                eprintln!(
                    "Warning: logged in but failed to fetch profile metadata: {}",
                    e
                );
                None
            }
        };
    eprintln!("Successfully logged in to Claude!");
    eprintln!(
        "Account '{}' stored at {}",
        label,
        auth::claude::jcode_path()?.display()
    );
    if let Some(email) = profile_email {
        eprintln!("Profile email: {}", email);
    }
    crate::telemetry::record_auth_success("claude", "oauth");
    Ok(())
}

async fn login_openai_flow(requested_label: Option<&str>) -> Result<()> {
    let label = auth::codex::login_target_label(requested_label)?;
    eprintln!("Logging in to OpenAI/Codex (account: {})...", label);
    let tokens = auth::oauth::login_openai().await?;
    auth::oauth::save_openai_tokens_for_account(&tokens, &label)?;
    eprintln!(
        "Successfully logged in to OpenAI! Account '{}' saved to {}",
        label,
        crate::storage::jcode_dir()?
            .join("openai-auth.json")
            .display()
    );
    crate::telemetry::record_auth_success("openai", "oauth");
    Ok(())
}

fn login_openrouter_flow() -> Result<()> {
    eprintln!("Setting up OpenRouter...");
    eprintln!("Get your API key from: https://openrouter.ai/keys\n");
    eprint!("Paste your OpenRouter API key: ");
    io::stdout().flush()?;

    let key = read_secret_line()?;

    if key.is_empty() {
        anyhow::bail!("No API key provided.");
    }

    if !key.starts_with("sk-or-") {
        eprintln!("Warning: OpenRouter API keys typically start with 'sk-or-'. Saving anyway.");
    }

    save_named_api_key("openrouter.env", "OPENROUTER_API_KEY", &key)?;
    eprintln!("\nSuccessfully saved OpenRouter API key!");
    eprintln!("Stored at ~/.config/jcode/openrouter.env");
    crate::telemetry::record_auth_success("openrouter", "api_key");
    Ok(())
}

fn login_azure_flow() -> Result<()> {
    use crate::auth::azure;

    eprintln!("Setting up Azure OpenAI...");
    eprintln!(
        "Reference: OpenCode supports Azure OpenAI with Entra credentials. jcode uses Azure OpenAI's newer `/openai/v1` API with either Microsoft Entra ID or an API key.\n"
    );

    let endpoint_raw = read_line_trimmed(
        "Azure OpenAI endpoint (for example `https://your-resource.openai.azure.com`): ",
    )?;
    let endpoint = azure::normalize_endpoint(&endpoint_raw).ok_or_else(|| {
        anyhow::anyhow!(
            "Invalid Azure OpenAI endpoint. Use https://<resource>.openai.azure.com (or the full /openai/v1 URL)."
        )
    })?;

    let model =
        read_line_trimmed("Azure deployment/model name (required, for example `gpt-4.1-nano`): ")?;
    if model.is_empty() {
        anyhow::bail!("No deployment/model name provided.");
    }

    eprintln!("\nAuthentication method:");
    eprintln!("  1. Microsoft Entra ID (recommended)");
    eprintln!("  2. API key");
    let auth_choice = read_line_trimmed("Enter 1-2 [1]: ")?;
    let use_entra = match auth_choice.trim() {
        "" | "1" => true,
        "2" => false,
        other if other.eq_ignore_ascii_case("entra") || other.eq_ignore_ascii_case("oauth") => true,
        other if other.eq_ignore_ascii_case("key") || other.eq_ignore_ascii_case("api-key") => {
            false
        }
        other => anyhow::bail!("Invalid auth choice '{}'. Use 1 or 2.", other),
    };

    let mut assignments = vec![
        (azure::ENDPOINT_ENV, endpoint),
        (azure::MODEL_ENV, model),
        (
            azure::USE_ENTRA_ENV,
            if use_entra { "1" } else { "0" }.to_string(),
        ),
    ];

    if use_entra {
        eprintln!();
        eprintln!("Using Microsoft Entra ID via Azure's DefaultAzureCredential chain.");
        eprintln!(
            "That means jcode can authenticate via `az login`, managed identity, or Azure environment credentials."
        );
    } else {
        eprint!("Paste your Azure OpenAI API key: ");
        io::stdout().flush()?;
        let key = read_secret_line()?;
        if key.is_empty() {
            anyhow::bail!("No API key provided.");
        }
        assignments.push((azure::API_KEY_ENV, key));
    }

    save_named_env_vars(azure::ENV_FILE, &assignments)?;
    azure::apply_runtime_env()?;

    eprintln!("\nSuccessfully saved Azure OpenAI configuration!");
    eprintln!("Stored at ~/.config/jcode/{}", azure::ENV_FILE);
    eprintln!("Base URL: {}", azure::load_endpoint().unwrap_or_default());
    if let Some(model) = azure::load_model() {
        eprintln!("Default deployment/model: {}", model);
    }
    if use_entra {
        eprintln!(
            "Next step: if you're using Azure CLI auth, run `az login` (and ensure your identity has the Cognitive Services OpenAI User role)."
        );
    }
    crate::telemetry::record_auth_success("azure", if use_entra { "entra_id" } else { "api_key" });
    Ok(())
}

fn login_openai_compatible_flow(profile: &OpenAiCompatibleProfile) -> Result<()> {
    let resolved = resolve_openai_compatible_profile(*profile);

    eprintln!("Setting up {}...", resolved.display_name);
    eprintln!("See setup details: {}\n", resolved.setup_url);
    eprintln!("Endpoint: {}", resolved.api_base);

    if resolved.requires_api_key {
        eprintln!("API key env variable: {}\n", resolved.api_key_env);
        eprint!("Paste your {} API key: ", resolved.display_name);
        io::stdout().flush()?;

        let key = read_secret_line()?;
        if key.is_empty() {
            anyhow::bail!("No API key provided.");
        }

        save_named_api_key(&resolved.env_file, &resolved.api_key_env, &key)?;
        eprintln!("\nSuccessfully saved {} API key!", resolved.display_name);
    } else {
        eprintln!("This provider uses a local OpenAI-compatible endpoint.");
        eprintln!(
            "An API key is optional here. Press Enter to skip if your local server does not require one.\n"
        );
        eprint!("Optional {} API key: ", resolved.display_name);
        io::stdout().flush()?;

        let key = read_secret_line()?;
        crate::provider_catalog::save_env_value_to_env_file(
            OPENAI_COMPAT_LOCAL_ENABLED_ENV,
            &resolved.env_file,
            Some("1"),
        )?;
        if key.trim().is_empty() {
            crate::provider_catalog::save_env_value_to_env_file(
                &resolved.api_key_env,
                &resolved.env_file,
                None,
            )?;
            eprintln!("\nSaved {} local endpoint setup.", resolved.display_name);
        } else {
            crate::provider_catalog::save_env_value_to_env_file(
                &resolved.api_key_env,
                &resolved.env_file,
                Some(key.trim()),
            )?;
            eprintln!(
                "\nSaved {} local endpoint setup and optional API key.",
                resolved.display_name
            );
        }
    }

    eprintln!("Stored at ~/.config/jcode/{}", resolved.env_file);
    if let Some(default_model) = resolved.default_model {
        eprintln!("Default model hint: {}", default_model);
    }
    crate::telemetry::record_auth_success(&resolved.id, "api_key");
    Ok(())
}

pub fn read_secret_line() -> Result<String> {
    use crossterm::terminal;

    let was_raw = crossterm::terminal::is_raw_mode_enabled().unwrap_or(false);
    if !was_raw {
        terminal::enable_raw_mode()?;
    }

    let mut input = String::new();
    loop {
        if let crossterm::event::Event::Key(key_event) =
            crossterm::event::read().context("Failed to read key input")?
        {
            use crossterm::event::{KeyCode, KeyModifiers};
            match key_event.code {
                KeyCode::Enter => {
                    eprintln!();
                    break;
                }
                KeyCode::Char('c') if key_event.modifiers.contains(KeyModifiers::CONTROL) => {
                    if !was_raw {
                        terminal::disable_raw_mode()?;
                    }
                    anyhow::bail!("Cancelled.");
                }
                KeyCode::Backspace => {
                    input.pop();
                }
                KeyCode::Char(c) => {
                    input.push(c);
                }
                _ => {}
            }
        }
    }

    if !was_raw {
        terminal::disable_raw_mode()?;
    }

    Ok(input.trim().to_string())
}

fn read_line_trimmed(prompt: &str) -> Result<String> {
    print!("{}", prompt);
    io::stdout().flush()?;

    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    Ok(input.trim().to_string())
}

fn save_named_env_vars(env_file: &str, vars: &[(&str, String)]) -> Result<()> {
    if !crate::provider_catalog::is_safe_env_file_name(env_file) {
        anyhow::bail!("Invalid env file name: {}", env_file);
    }

    for (key, _) in vars {
        if !crate::provider_catalog::is_safe_env_key_name(key) {
            anyhow::bail!("Invalid API key variable name: {}", key);
        }
    }

    let config_dir = crate::storage::app_config_dir()?;
    std::fs::create_dir_all(&config_dir)?;
    crate::platform::set_directory_permissions_owner_only(&config_dir)?;

    let file_path = config_dir.join(env_file);
    let mut content = String::new();
    for (key, value) in vars {
        content.push_str(&format!("{}={}\n", key, value));
    }
    std::fs::write(&file_path, &content)?;
    crate::platform::set_permissions_owner_only(&file_path)?;

    for (key, value) in vars {
        crate::env::set_var(key, value);
    }

    Ok(())
}

fn login_cursor_flow() -> Result<()> {
    eprintln!("Starting Cursor login...");
    let binary = crate::auth::cursor::cursor_agent_cli_path();

    if crate::auth::cursor::has_cursor_agent_cli() {
        match run_external_login_command(&binary, &["login"]) {
            Ok(()) => {
                eprintln!("Cursor login command completed.");
                crate::telemetry::record_auth_success("cursor", "oauth");
                crate::auth::AuthStatus::invalidate_cache();
                return Ok(());
            }
            Err(err) => {
                eprintln!("Cursor browser login failed: {}", err);
                eprintln!();
                eprintln!("Falling back to Cursor API key setup.");
            }
        }
    } else {
        eprintln!("Cursor Agent CLI was not found on PATH.");
        eprintln!("You can still save a Cursor API key now and install Cursor Agent later.");
    }

    eprintln!("Get your API key from: https://cursor.com/settings");
    eprintln!("(Dashboard > Integrations > User API Keys)\n");
    eprint!("Paste your Cursor API key: ");
    io::stdout().flush()?;

    let key = read_secret_line()?;
    if key.is_empty() {
        anyhow::bail!("No API key provided.");
    }

    save_named_api_key("cursor.env", "CURSOR_API_KEY", &key)?;
    crate::auth::AuthStatus::invalidate_cache();
    eprintln!("\nSuccessfully saved Cursor API key!");
    eprintln!("Stored at ~/.config/jcode/cursor.env");
    eprintln!("jcode will pass it to `cursor-agent` automatically.");
    if !crate::auth::cursor::has_cursor_agent_cli() {
        eprintln!("Install Cursor Agent to use the Cursor provider:");
        eprintln!("  - macOS/Linux/WSL: curl https://cursor.com/install -fsS | bash");
        eprintln!("  - Windows (PowerShell): irm 'https://cursor.com/install?win32=true' | iex");
    }
    crate::telemetry::record_auth_success("cursor", "api_key");
    Ok(())
}

fn login_copilot_flow() -> Result<()> {
    eprintln!("Starting GitHub Copilot login...");

    tokio::task::block_in_place(|| {
        tokio::runtime::Handle::current().block_on(login_copilot_device_flow())
    })
}

async fn login_copilot_device_flow() -> Result<()> {
    let client = reqwest::Client::new();

    let device_resp = crate::auth::copilot::initiate_device_flow(&client).await?;

    eprintln!();
    eprintln!("  Open this URL in your browser:");
    eprintln!("    {}", device_resp.verification_uri);
    eprintln!();
    if let Some(qr) = crate::login_qr::indented_section(
        &device_resp.verification_uri,
        "  Or scan this QR on another device to open the verification page:",
        "    ",
    ) {
        eprintln!("{qr}");
        eprintln!();
    }
    eprintln!("  Enter code: {}", device_resp.user_code);
    eprintln!();
    eprintln!("  Waiting for authorization...");

    let _ = open::that(&device_resp.verification_uri);

    let token = crate::auth::copilot::poll_for_access_token(
        &client,
        &device_resp.device_code,
        device_resp.interval,
    )
    .await?;

    let username = crate::auth::copilot::fetch_github_username(&client, &token)
        .await
        .unwrap_or_else(|_| "unknown".to_string());

    crate::auth::copilot::save_github_token(&token, &username)?;

    eprintln!("  ✓ Authenticated as {} via GitHub Copilot", username);
    crate::telemetry::record_auth_success("copilot", "oauth_device_code");
    Ok(())
}

async fn login_antigravity_flow() -> Result<()> {
    eprintln!("Starting native Antigravity login...");
    eprintln!(
        "jcode will authenticate directly with Google Antigravity; the Antigravity desktop app is not required."
    );
    eprintln!(
        "If browser launch fails, set `NO_BROWSER=true` and jcode will prompt for the callback URL instead."
    );
    eprintln!(
        "If the browser later shows a loopback/callback error page, copy the full URL from the address bar and re-run with `NO_BROWSER=true`."
    );
    eprintln!();

    let tokens = crate::auth::antigravity::login().await?;

    eprintln!("Successfully logged in to Antigravity!");
    eprintln!(
        "Tokens saved to {}",
        crate::auth::antigravity::tokens_path()?.display()
    );
    if let Some(email) = tokens.email.as_deref() {
        eprintln!("Google account: {}", email);
    }
    if let Some(project_id) = tokens.project_id.as_deref() {
        eprintln!("Resolved Antigravity project: {}", project_id);
    }
    crate::telemetry::record_auth_success("antigravity", "oauth");
    Ok(())
}

async fn login_gemini_flow() -> Result<()> {
    eprintln!("Starting native Gemini login...");
    eprintln!(
        "If your student/education plan is attached to your Google account, use that account in the browser flow."
    );
    eprintln!(
        "If browser launch fails, set `NO_BROWSER=true` and jcode will prompt for the manual authorization code."
    );
    eprintln!(
        "Note: school / Workspace Google accounts may also require GOOGLE_CLOUD_PROJECT and GOOGLE_CLOUD_LOCATION for Code Assist entitlement checks."
    );
    eprintln!();

    let tokens = crate::auth::gemini::login().await?;

    eprintln!("Successfully logged in to Gemini!");
    eprintln!(
        "Tokens saved to {}",
        crate::auth::gemini::tokens_path()?.display()
    );
    if let Some(email) = tokens.email.as_deref() {
        eprintln!("Google account: {}", email);
    }
    crate::telemetry::record_auth_success("gemini", "oauth");
    Ok(())
}

async fn login_google_flow() -> Result<()> {
    use auth::google::{GmailAccessTier, GoogleCredentials};

    eprintln!("╔══════════════════════════════════════════╗");
    eprintln!("║       Gmail Integration Setup            ║");
    eprintln!("╚══════════════════════════════════════════╝\n");

    let _creds = match auth::google::load_credentials() {
        Ok(creds) => {
            eprintln!(
                "✓ Google credentials found (client_id: {}...)\n",
                &creds.client_id[..20.min(creds.client_id.len())]
            );
            creds
        }
        Err(_) => {
            eprintln!("No Google credentials found. Let's set them up.\n");
            eprintln!("You need OAuth credentials from Google Cloud Console.");
            eprintln!("How would you like to provide them?\n");
            eprintln!("  [1] Paste client ID and secret directly (easiest)");
            eprintln!("  [2] Provide path to downloaded JSON credentials file");
            eprintln!("  [3] I need help creating credentials (opens setup guide)\n");
            eprint!("Choose [1/2/3]: ");
            io::stdout().flush()?;

            let mut input = String::new();
            io::stdin().read_line(&mut input)?;

            match input.trim() {
                "1" => {
                    eprintln!("\nPaste your Google OAuth Client ID:");
                    eprintln!("  (looks like: 123456789-abc.apps.googleusercontent.com)\n");
                    eprint!("> ");
                    io::stdout().flush()?;
                    let mut client_id = String::new();
                    io::stdin().read_line(&mut client_id)?;
                    let client_id = client_id.trim().to_string();

                    if client_id.is_empty() {
                        anyhow::bail!("No client ID provided.");
                    }

                    eprintln!("\nPaste your Google OAuth Client Secret:");
                    eprintln!("  (looks like: GOCSPX-...)\n");
                    eprint!("> ");
                    io::stdout().flush()?;
                    let mut client_secret = String::new();
                    io::stdin().read_line(&mut client_secret)?;
                    let client_secret = client_secret.trim().to_string();

                    if client_secret.is_empty() {
                        anyhow::bail!("No client secret provided.");
                    }

                    let creds = GoogleCredentials {
                        client_id,
                        client_secret,
                    };
                    auth::google::save_credentials(&creds)?;
                    eprintln!(
                        "\n✓ Credentials saved to {}\n",
                        auth::google::credentials_path()?.display()
                    );
                    creds
                }
                "2" => {
                    eprintln!("\nPaste the path to your downloaded JSON file:\n");
                    eprint!("> ");
                    io::stdout().flush()?;
                    let mut path_input = String::new();
                    io::stdin().read_line(&mut path_input)?;
                    let path_str = path_input.trim();

                    let path_str = if let Some(stripped) = path_str.strip_prefix("~/") {
                        if let Some(home) = dirs::home_dir() {
                            home.join(stripped).to_string_lossy().to_string()
                        } else {
                            path_str.to_string()
                        }
                    } else {
                        path_str.to_string()
                    };

                    let data = std::fs::read_to_string(&path_str)
                        .with_context(|| format!("Could not read file: {}", path_str))?;

                    let dest = auth::google::credentials_path()?;
                    if let Some(parent) = dest.parent() {
                        std::fs::create_dir_all(parent)?;
                        crate::platform::set_directory_permissions_owner_only(parent)?;
                    }
                    std::fs::write(&dest, &data)?;
                    crate::platform::set_permissions_owner_only(&dest)?;

                    let creds = auth::google::load_credentials()
                        .context("Could not parse the credentials file. Make sure it's the OAuth client JSON from Google Cloud Console.")?;

                    eprintln!("\n✓ Credentials imported to {}\n", dest.display());
                    creds
                }
                "3" => {
                    eprintln!("\n── Step-by-step Google Cloud setup ──\n");

                    eprintln!("1. Open Google Cloud Console and create a project:");
                    eprintln!("   Opening: https://console.cloud.google.com/projectcreate\n");
                    let _ = open::that("https://console.cloud.google.com/projectcreate");
                    eprint!("   Press Enter when your project is created...");
                    io::stdout().flush()?;
                    let mut wait = String::new();
                    io::stdin().read_line(&mut wait)?;

                    eprintln!("\n2. Enable the Gmail API:");
                    eprintln!("   Opening: Gmail API library page\n");
                    let _ = open::that(
                        "https://console.cloud.google.com/apis/library/gmail.googleapis.com",
                    );
                    eprintln!("   Click the blue 'Enable' button.");
                    eprint!("   Press Enter when done...");
                    io::stdout().flush()?;
                    io::stdin().read_line(&mut wait)?;

                    eprintln!("\n3. Configure OAuth consent screen:");
                    eprintln!("   Opening: OAuth consent screen\n");
                    let _ = open::that("https://console.cloud.google.com/apis/credentials/consent");
                    eprintln!("   - Choose 'External' user type");
                    eprintln!("   - Fill in app name (e.g. 'jcode') and your email");
                    eprintln!("   - Skip scopes (we'll request them during login)");
                    eprintln!("   - Add your email as a test user");
                    eprintln!("   - Save and continue through all steps");
                    eprint!("   Press Enter when done...");
                    io::stdout().flush()?;
                    io::stdin().read_line(&mut wait)?;

                    eprintln!("\n4. Create OAuth credentials:");
                    eprintln!("   Opening: Credentials page\n");
                    let _ = open::that("https://console.cloud.google.com/apis/credentials");
                    eprintln!("   - Click '+ Create Credentials' > 'OAuth client ID'");
                    eprintln!("   - Application type: 'Desktop app'");
                    eprintln!("   - Name: 'jcode'");
                    eprintln!("   - Click 'Create'\n");
                    eprintln!("   A dialog will show your Client ID and Client Secret.\n");

                    eprintln!("Paste your Client ID:");
                    eprint!("> ");
                    io::stdout().flush()?;
                    let mut client_id = String::new();
                    io::stdin().read_line(&mut client_id)?;
                    let client_id = client_id.trim().to_string();

                    if client_id.is_empty() {
                        anyhow::bail!("No client ID provided.");
                    }

                    eprintln!("\nPaste your Client Secret:");
                    eprint!("> ");
                    io::stdout().flush()?;
                    let mut client_secret = String::new();
                    io::stdin().read_line(&mut client_secret)?;
                    let client_secret = client_secret.trim().to_string();

                    if client_secret.is_empty() {
                        anyhow::bail!("No client secret provided.");
                    }

                    let creds = GoogleCredentials {
                        client_id,
                        client_secret,
                    };
                    auth::google::save_credentials(&creds)?;
                    eprintln!("\n✓ Credentials saved!\n");
                    creds
                }
                _ => {
                    eprintln!("\nInvalid choice. Please enter 1, 2, or 3.\n");
                    std::process::exit(1);
                }
            }
        }
    };

    eprintln!("── Gmail Access Level ──\n");
    eprintln!("  [1] Full Access (recommended)");
    eprintln!("      Search, read, draft, send, and manage emails.");
    eprintln!("      Send and delete always require your confirmation.\n");
    eprintln!("  [2] Read & Draft Only");
    eprintln!("      Search, read emails, create drafts. Cannot send or delete.");
    eprintln!("      API-level restriction - impossible even if the AI tries.\n");
    eprint!("Choose [1/2] (default: 1): ");
    io::stdout().flush()?;

    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    let tier = match input.trim() {
        "" | "1" => GmailAccessTier::Full,
        "2" => GmailAccessTier::ReadOnly,
        _ => {
            eprintln!("Invalid choice, defaulting to Full Access.");
            GmailAccessTier::Full
        }
    };

    eprintln!("\nAccess level: {}", tier.label());

    eprintln!("\n── Logging in ──\n");

    let tokens = auth::google::login(tier).await?;

    eprintln!("\n╔══════════════════════════════════════════╗");
    eprintln!("║  ✓ Gmail setup complete!                 ║");
    eprintln!("╚══════════════════════════════════════════╝\n");
    if let Some(email) = &tokens.email {
        eprintln!("  Account:      {}", email);
    }
    eprintln!("  Access tier:  {}", tokens.tier.label());
    eprintln!(
        "  Credentials:  {}",
        auth::google::credentials_path()?.display()
    );
    eprintln!(
        "  Tokens:       {}\n",
        auth::google::tokens_path()?.display()
    );
    eprintln!("The 'gmail' tool is now available to the AI agent.");
    eprintln!("Try asking: \"check my recent emails\" or \"search emails from ...\"");

    crate::telemetry::record_auth_success("google", "oauth");
    Ok(())
}

pub fn run_external_login_command(program: &str, args: &[&str]) -> Result<()> {
    let status = ProcessCommand::new(program)
        .args(args)
        .status()
        .with_context(|| format!("Failed to start command: {} {}", program, args.join(" ")))?;
    if !status.success() {
        anyhow::bail!(
            "Command exited with non-zero status: {} {} ({})",
            program,
            args.join(" "),
            status
        );
    }
    Ok(())
}

#[allow(dead_code)]
pub fn run_external_login_command_owned(program: &str, args: &[String]) -> Result<()> {
    let status = ProcessCommand::new(program)
        .args(args)
        .status()
        .with_context(|| format!("Failed to start command: {} {}", program, args.join(" ")))?;
    if !status.success() {
        anyhow::bail!(
            "Command exited with non-zero status: {} {} ({})",
            program,
            args.join(" "),
            status
        );
    }
    Ok(())
}
