//! `jcode login google`: guided bring-your-own OAuth app setup and login.
//!
//! The setup mirrors (and goes beyond) `gog auth setup`: it automates project
//! creation and API enabling through `gcloud` when available, opens console
//! pages with the project preselected otherwise, picks up the OAuth client
//! JSON from the Downloads folder automatically, and warns about the 7-day
//! Testing-mode token expiry.

use super::*;
use auth::google::setup;
use auth::google::{GmailAccessTier, GoogleCredentials, GoogleService};
use std::time::{Duration, SystemTime};

/// How long to watch Downloads for the OAuth client JSON.
const DOWNLOAD_WAIT: Duration = Duration::from_secs(600);

pub(super) async fn login_google_flow(
    no_browser: bool,
    access_tier: Option<GmailAccessTier>,
    requested_services: Option<Vec<GoogleService>>,
    force_setup: bool,
    client_json: Option<&str>,
) -> Result<()> {
    eprintln!("╔══════════════════════════════════════════╗");
    eprintln!("║       Google Integration Setup           ║");
    eprintln!("╚══════════════════════════════════════════╝\n");

    let existing = auth::google::load_tokens().ok();
    let services = match requested_services {
        Some(services) => auth::google::normalize_services(services),
        None => prompt_google_services(existing.as_ref().map(|t| t.services.as_slice()))?,
    };

    let staged = client_json
        .map(|source| import_google_client_json(source, false))
        .transpose()?;
    let have_creds = staged.or_else(|| auth::google::load_credentials().ok());
    let creds = match have_creds {
        Some(creds) if !force_setup || client_json.is_some() => {
            eprintln!(
                "✓ Google OAuth client selected ({}...)\n",
                &creds.client_id[..20.min(creds.client_id.len())]
            );
            creds
        }
        _ => {
            if have_creds.is_some() {
                eprintln!(
                    "Re-running setup. The OAuth client is replaced only after login succeeds.\n"
                );
            } else {
                eprintln!("No Google OAuth client yet. jcode uses your own Google Cloud app,");
                eprintln!("so your data goes straight from your machine to Google.\n");
            }
            run_client_setup(&services, no_browser)?
        }
    };

    let tier = choose_gmail_tier(&services, access_tier, existing.as_ref())?;

    eprintln!(
        "\nAuthorizing: {}",
        auth::google::describe_services(&services, tier)
    );
    eprintln!("Google will warn \"Google hasn't verified this app\". That is expected for your");
    eprintln!("own app: click Advanced, then Go to <app name>, then keep every box ticked.");
    eprintln!("\n── Logging in ──\n");

    let tokens = match auth::google::login_with_credentials(&creds, &services, tier, no_browser)
        .await
    {
        Ok(tokens) => tokens,
        Err(err) => {
            let text = format!("{err:#}");
            if text.contains("accessNotConfigured") || text.contains("has not been used") {
                eprintln!(
                    "\nAn API is not enabled yet. Enable it here, wait a minute, and retry:\n  {}",
                    setup::enable_apis_url(&services, None)
                );
            }
            return Err(err);
        }
    };

    print_summary(&services, &tokens)?;
    crate::telemetry::record_auth_success("google", "oauth");
    Ok(())
}

fn choose_gmail_tier(
    services: &[GoogleService],
    access_tier: Option<GmailAccessTier>,
    existing: Option<&auth::google::GoogleTokens>,
) -> Result<GmailAccessTier> {
    if !services.contains(&GoogleService::Gmail) {
        // Gmail scopes are not requested, so the tier is unused. Keep any
        // previous choice so re-adding Gmail later starts from it.
        return Ok(access_tier
            .or(existing.map(|t| t.tier))
            .unwrap_or(GmailAccessTier::Full));
    }
    if let Some(tier) = access_tier {
        return Ok(tier);
    }
    eprintln!("── Gmail Access Level ──\n");
    eprintln!("  [1] Full Access (recommended)");
    eprintln!("      Search, read, draft, send, and manage emails.");
    eprintln!("      Send and delete always require your confirmation.\n");
    eprintln!("  [2] Read & Draft Only");
    eprintln!("      Search, read emails, create drafts. Cannot send or delete.");
    eprintln!("      API-level restriction - impossible even if the AI tries.\n");
    let choice = prompt("Choose [1/2] (default: 1): ")?;
    Ok(match choice.as_str() {
        "" | "1" => GmailAccessTier::Full,
        "2" => GmailAccessTier::ReadOnly,
        _ => {
            eprintln!("Invalid choice, defaulting to Full Access.");
            GmailAccessTier::Full
        }
    })
}

fn print_summary(services: &[GoogleService], tokens: &auth::google::GoogleTokens) -> Result<()> {
    eprintln!("\n╔══════════════════════════════════════════╗");
    eprintln!("║  ✓ Google setup complete!                ║");
    eprintln!("╚══════════════════════════════════════════╝\n");
    if let Some(email) = &tokens.email {
        eprintln!("  Account:      {}", email);
    }
    eprintln!(
        "  Services:     {}",
        auth::google::describe_services(&tokens.services, tokens.tier)
    );
    let missing: Vec<_> = services
        .iter()
        .filter(|s| !tokens.services.contains(s))
        .map(|s| s.label())
        .collect();
    if !missing.is_empty() {
        eprintln!(
            "  Not granted:  {} (permission unticked on the consent screen)",
            missing.join(", ")
        );
    }
    eprintln!(
        "  Tokens:       {}\n",
        auth::google::tokens_path()?.display()
    );
    eprintln!("Reminder: {}", setup::PUBLISH_APP_NOTE);
    eprintln!("  {}\n", setup::audience_url(None));
    if tokens.has_service(GoogleService::Gmail) {
        eprintln!("Try asking: \"check my recent emails\" or \"search emails from ...\"");
    }
    if tokens.has_service(GoogleService::Calendar) {
        eprintln!("Try asking: \"what's on my calendar tomorrow?\" or \"remind me at 7pm to ...\"");
    }
    Ok(())
}

fn prompt(label: &str) -> Result<String> {
    eprint!("{label}");
    io::stderr().flush()?;
    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    Ok(input.trim().to_string())
}

fn pause(label: &str) -> Result<()> {
    prompt(&format!("{label} Press Enter to continue..."))?;
    Ok(())
}

/// Create (or reuse) a Google Cloud project, enable APIs, guide the OAuth
/// client creation, and import the downloaded client JSON.
fn run_client_setup(services: &[GoogleService], no_browser: bool) -> Result<GoogleCredentials> {
    // A client JSON downloaded earlier can be imported right away.
    if let Some((path, parsed)) = setup::find_client_json(&setup::default_download_dirs(), None) {
        eprintln!("Found an OAuth client file: {}", path.display());
        if prompt("Use it? [Y/n]: ")?
            .to_ascii_lowercase()
            .starts_with('n')
        {
            eprintln!();
        } else {
            return import_client(&parsed, services);
        }
    }

    eprintln!("How do you want to set up the OAuth client?\n");
    eprintln!("  [1] Guided setup (recommended): jcode opens each page and imports the download");
    eprintln!("  [2] I already have a client ID and secret to paste");
    eprintln!("  [3] I have a downloaded client JSON at a specific path\n");
    match prompt("Choose [1/2/3] (default: 1): ")?.as_str() {
        "2" => paste_credentials(),
        "3" => import_from_path(services),
        _ => guided_setup(services, no_browser),
    }
}

fn guided_setup(services: &[GoogleService], no_browser: bool) -> Result<GoogleCredentials> {
    let project = prepare_project(services, no_browser)?;
    let project = project.as_deref();

    eprintln!("\n── Configure the consent screen ──\n");
    let consent = setup::consent_screen_url(project);
    open_page(&consent, no_browser);
    eprintln!("  If it says \"Google Auth Platform not configured yet\", click Get Started:");
    eprintln!("  - App name: jcode. Support and contact email: yours.");
    eprintln!("  - Audience: External (or Internal for a Workspace account that allows it).");
    eprintln!("  - Agree to the policy, then Create. Skip adding scopes; jcode requests them.");
    pause("\n  When the consent screen is created:")?;

    eprintln!("\n── Publish the app ──\n");
    open_page(&setup::audience_url(project), no_browser);
    eprintln!(
        "  Click \"Publish app\" and confirm.\n  Why: in Testing mode Google expires the login every 7 days. Publishing\n  your own app needs no Google review."
    );
    pause("\n  When it shows \"In production\":")?;

    eprintln!("\n── Create the OAuth client ──\n");
    let since = SystemTime::now();
    open_page(&setup::create_client_url(project), no_browser);
    eprintln!("  - Application type: Desktop app. Name: jcode. Click Create.");
    eprintln!("  - In the dialog, click \"Download JSON\".");
    eprintln!("\n  Waiting for client_secret_*.json in your Downloads folder...");
    eprintln!("  (Press Ctrl+C to stop and use `jcode login google --setup` again later.)");

    let dirs = setup::default_download_dirs();
    let mut ticks = 0u32;
    match setup::wait_for_client_json(&dirs, since, DOWNLOAD_WAIT, || {
        ticks += 1;
        if ticks.is_multiple_of(30) {
            eprintln!("  Still waiting ({}s)...", ticks);
        }
        true
    }) {
        Some((path, parsed)) => {
            eprintln!("  ✓ Found {}", path.display());
            import_client(&parsed, services)
        }
        None => {
            eprintln!("\n  No download detected.");
            import_from_path(services)
        }
    }
}

/// Use gcloud to create a project and enable APIs when it is installed and
/// signed in. Otherwise open the console pages. Returns the project id when
/// known, so later links preselect it.
fn prepare_project(services: &[GoogleService], no_browser: bool) -> Result<Option<String>> {
    let gcloud = setup::gcloud_status();
    if gcloud.usable() {
        eprintln!(
            "\ngcloud is signed in as {}.",
            gcloud.account.as_deref().unwrap_or("?")
        );
        let default_choice = if gcloud.project.is_some() { "2" } else { "1" };
        eprintln!("  [1] Create a new project for jcode (recommended)");
        if let Some(p) = &gcloud.project {
            eprintln!("  [2] Use the current gcloud project: {p}");
        }
        eprintln!("  [3] Skip gcloud and do it in the browser");
        let choice = prompt(&format!("Choose (default: {default_choice}): "))?;
        let choice = if choice.is_empty() {
            default_choice.to_string()
        } else {
            choice
        };
        let project = match choice.as_str() {
            "2" if gcloud.project.is_some() => gcloud.project.clone(),
            "3" => None,
            _ => {
                let seed = (SystemTime::now()
                    .duration_since(SystemTime::UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or(0)
                    % 1_000_000) as u32;
                let id = setup::suggest_project_id(setup::DEFAULT_PROJECT_NAME, seed);
                eprintln!("Creating project {id}...");
                match setup::gcloud_create_project(&id, setup::DEFAULT_PROJECT_NAME) {
                    Ok(id) => Some(id),
                    Err(err) => {
                        eprintln!("{err:#}\nFalling back to the browser.");
                        None
                    }
                }
            }
        };
        if let Some(project) = &project {
            eprintln!("Enabling APIs in {project}...");
            match setup::gcloud_enable_apis(project, services) {
                Ok(()) => {
                    eprintln!("✓ Enabled {}", api_names(services));
                    return Ok(Some(project.clone()));
                }
                Err(err) => {
                    eprintln!("{err:#}\nEnable them in the browser instead.");
                    open_page(&setup::enable_apis_url(services, Some(project)), no_browser);
                    pause("  When the APIs are enabled:")?;
                    return Ok(Some(project.clone()));
                }
            }
        }
    } else if gcloud.installed {
        eprintln!("\nTip: run `gcloud auth login` first and jcode will create the project and");
        eprintln!("enable the APIs for you.");
    }

    eprintln!("\n── Create a Google Cloud project ──\n");
    open_page(setup::project_create_url(), no_browser);
    eprintln!("  Name it \"jcode\" and click Create. Skip this if you want to reuse a project.");
    let project = prompt("\n  Project ID (shown under the name; Enter to skip): ")?;
    let project = (!project.is_empty()).then_some(project);

    eprintln!("\n── Enable the APIs ──\n");
    open_page(
        &setup::enable_apis_url(services, project.as_deref()),
        no_browser,
    );
    eprintln!(
        "  Confirm the project and click Enable ({}).",
        api_names(services)
    );
    pause("\n  When the APIs are enabled:")?;
    Ok(project)
}

fn api_names(services: &[GoogleService]) -> String {
    services
        .iter()
        .map(|s| s.label())
        .collect::<Vec<_>>()
        .join(", ")
}

fn open_page(url: &str, no_browser: bool) {
    let opened = maybe_open_browser(url, no_browser);
    eprintln!("  {} {url}", if opened { "Opened:" } else { "Open:" });
}

fn import_client(
    parsed: &setup::ClientJson,
    services: &[GoogleService],
) -> Result<GoogleCredentials> {
    if !parsed.desktop {
        eprintln!("  Warning: this is a \"Web application\" client. jcode needs a \"Desktop app\"");
        eprintln!("  client for its local sign-in redirect. Create one with type Desktop app.");
        if !prompt("  Import anyway? [y/N]: ")?
            .to_ascii_lowercase()
            .starts_with('y')
        {
            anyhow::bail!(
                "Create a Desktop app OAuth client and run `jcode login google --setup`."
            );
        }
    }
    eprintln!("  ✓ OAuth client staged until login succeeds");
    if let Some(project) = &parsed.project_id {
        // The client JSON names its project: make sure the APIs are on there.
        eprintln!(
            "  Project: {project}. If an API is not enabled yet: {}\n",
            setup::enable_apis_url(services, Some(project))
        );
    }
    Ok(parsed.credentials.clone())
}

fn import_from_path(services: &[GoogleService]) -> Result<GoogleCredentials> {
    let raw = prompt("\nPath to the downloaded client JSON: ")?;
    let path = match raw.strip_prefix("~/") {
        Some(rest) => dirs::home_dir()
            .map(|h| h.join(rest))
            .unwrap_or_else(|| PathBuf::from(&raw)),
        None => PathBuf::from(&raw),
    };
    let data = std::fs::read_to_string(&path)
        .with_context(|| format!("Could not read file: {}", path.display()))?;
    let parsed = setup::parse_client_json(&data).context(
        "Not an OAuth client JSON. Download it from the OAuth client's page in Google Cloud Console.",
    )?;
    import_client(&parsed, services)
}

fn paste_credentials() -> Result<GoogleCredentials> {
    let client_id = prompt("\nClient ID (ends with .apps.googleusercontent.com): ")?;
    if client_id.is_empty() {
        anyhow::bail!("No client ID provided.");
    }
    eprint!("Client secret (input hidden): ");
    io::stderr().flush()?;
    let client_secret = read_secret_line()?.trim().to_string();
    if client_secret.is_empty() {
        anyhow::bail!("No client secret provided.");
    }
    eprintln!("✓ Credentials staged until login succeeds");
    Ok(GoogleCredentials {
        client_id,
        client_secret,
    })
}

pub(super) fn default_pending_google_services() -> Vec<GoogleService> {
    vec![GoogleService::Gmail]
}

/// `--google-client-json <path|auto>`: import an OAuth client JSON without
/// prompts, so an agent driving the console in a browser can hand the
/// download to jcode. Desktop clients only.
pub(super) fn import_google_client_json(source: &str, quiet: bool) -> Result<GoogleCredentials> {
    let parsed = read_google_client_json(source, &setup::default_download_dirs())?;
    if !quiet {
        eprintln!("✓ OAuth client staged until login succeeds");
    }
    Ok(parsed.credentials)
}

fn read_google_client_json(source: &str, download_dirs: &[PathBuf]) -> Result<setup::ClientJson> {
    let (path, parsed) = if source.eq_ignore_ascii_case("auto") {
        setup::find_client_json(download_dirs, None).ok_or_else(|| {
            anyhow::anyhow!(
                "No valid client_secret*.json found in Downloads. Download the OAuth client JSON \
                 from Google Cloud Console (Clients > your Desktop client > Download JSON)."
            )
        })?
    } else {
        let path = match source.strip_prefix("~/") {
            Some(rest) => dirs::home_dir()
                .map(|h| h.join(rest))
                .unwrap_or_else(|| PathBuf::from(source)),
            None => PathBuf::from(source),
        };
        let data = std::fs::read_to_string(&path)
            .with_context(|| format!("Could not read {}", path.display()))?;
        let parsed = setup::parse_client_json(&data)?;
        (path, parsed)
    };
    if !parsed.desktop {
        anyhow::bail!(
            "{} is a Web application client. jcode needs a Desktop app OAuth client: {}",
            path.display(),
            setup::create_client_url(parsed.project_id.as_deref())
        );
    }
    Ok(parsed)
}

/// Choose the new-login client independently of any client bound to old tokens.
pub(super) fn google_login_credentials(options: &LoginOptions) -> Result<GoogleCredentials> {
    match options.google_client_json.as_deref() {
        Some(source) => import_google_client_json(source, options.json),
        None => auth::google::load_credentials().context(
            "Google login requires an OAuth client. Use --google-client-json <path|auto> or run the interactive Google setup.",
        ),
    }
}

/// Services to authorize when the caller did not pass `--google-services`:
/// keep what is already granted, defaulting to Gmail for first-time setup.
pub(super) fn default_google_services(options: &LoginOptions) -> Vec<GoogleService> {
    options
        .google_services
        .clone()
        .or_else(|| auth::google::load_tokens().ok().map(|t| t.services))
        .map(auth::google::normalize_services)
        .filter(|services| !services.is_empty())
        .unwrap_or_else(default_pending_google_services)
}

#[cfg(test)]
mod transactional_tests {
    use super::*;

    const CLIENT_JSON: &str = r#"{"installed":{"client_id":"synthetic-other-project.apps.googleusercontent.com","client_secret":"synthetic-other-secret","project_id":"synthetic-other-project","redirect_uris":["http://localhost"]}}"#;
    const OLD_TOKENS: &str = r#"{"access_token":"synthetic-old-access","refresh_token":"synthetic-old-refresh","expires_at":4102444800000,"tier":"full","email":null}"#;

    struct TestHome(Option<std::ffi::OsString>);

    impl TestHome {
        fn new(path: &Path) -> Self {
            let previous = std::env::var_os("JCODE_HOME");
            crate::env::set_var("JCODE_HOME", path);
            Self(previous)
        }
    }

    impl Drop for TestHome {
        fn drop(&mut self) {
            match self.0.take() {
                Some(value) => crate::env::set_var("JCODE_HOME", value),
                None => crate::env::remove_var("JCODE_HOME"),
            }
        }
    }

    fn seed_old_login(home: &Path) -> (PathBuf, Vec<u8>, PathBuf, Vec<u8>) {
        let client = home.join("google_credentials.json");
        let tokens = home.join("google_oauth.json");
        std::fs::write(
            &client,
            r#"{"client_id":"synthetic-old-project","client_secret":"synthetic-old-secret"}"#,
        )
        .unwrap();
        std::fs::write(&tokens, OLD_TOKENS).unwrap();
        let client_bytes = std::fs::read(&client).unwrap();
        let token_bytes = std::fs::read(&tokens).unwrap();
        (client, client_bytes, tokens, token_bytes)
    }

    #[test]
    fn transactional_google_login_auto_discovery_only_stages_other_project() {
        let temp = tempfile::TempDir::new().unwrap();
        let (client, old_client, tokens, old_tokens) = seed_old_login(temp.path());
        let downloads = temp.path().join("Downloads");
        std::fs::create_dir(&downloads).unwrap();
        std::fs::write(downloads.join("client_secret_other.json"), CLIENT_JSON).unwrap();
        let staged = read_google_client_json("auto", &[downloads]).unwrap();
        assert_eq!(
            staged.project_id.as_deref(),
            Some("synthetic-other-project")
        );
        assert_eq!(
            staged.credentials.client_id,
            "synthetic-other-project.apps.googleusercontent.com"
        );
        assert_eq!(std::fs::read(client).unwrap(), old_client);
        assert_eq!(std::fs::read(tokens).unwrap(), old_tokens);
    }

    #[tokio::test]
    async fn transactional_google_login_scriptable_failure_and_cancel_preserve_active_files() {
        let _guard = crate::storage::lock_test_env();
        let temp = tempfile::TempDir::new().unwrap();
        let _home = TestHome::new(temp.path());
        let (client, old_client, tokens, old_tokens) = seed_old_login(temp.path());
        let imported = temp.path().join("client_secret_other.json");
        std::fs::write(&imported, CLIENT_JSON).unwrap();
        let provider = crate::provider_catalog::resolve_login_provider("google").unwrap();
        let begin = LoginOptions {
            google_client_json: Some(imported.display().to_string()),
            google_services: Some(vec![GoogleService::Gmail]),
            print_auth_url: true,
            flow_id: Some("synthetic-google-transaction".into()),
            no_browser: true,
            json: true,
            ..LoginOptions::default()
        };
        run_login_provider(provider, None, begin.clone())
            .await
            .unwrap();
        let pending = pending_login_path("google", begin.flow_id.as_deref()).unwrap();
        let pending_bytes = std::fs::read(&pending).unwrap();
        let PendingScriptableLogin::Google { credentials, .. } =
            load_pending_login(&pending, "google").unwrap()
        else {
            panic!("expected Google pending flow");
        };
        assert_eq!(
            credentials.unwrap().client_id,
            "synthetic-other-project.apps.googleusercontent.com"
        );
        assert_eq!(std::fs::read(&client).unwrap(), old_client);
        assert_eq!(std::fs::read(&tokens).unwrap(), old_tokens);

        // State validation rejects this synthetic callback before any HTTP.
        let complete = LoginOptions {
            callback_url: Some("http://127.0.0.1/?code=synthetic&state=wrong-state".into()),
            flow_id: begin.flow_id.clone(),
            no_browser: true,
            json: true,
            ..LoginOptions::default()
        };
        let error = run_login_provider(provider, None, complete)
            .await
            .unwrap_err();
        assert!(format!("{error:#}").contains("state mismatch"));
        assert_eq!(std::fs::read(&pending).unwrap(), pending_bytes);
        assert_eq!(std::fs::read(&client).unwrap(), old_client);
        assert_eq!(std::fs::read(&tokens).unwrap(), old_tokens);

        let cancel = LoginOptions {
            cancel: true,
            flow_id: begin.flow_id,
            json: true,
            ..LoginOptions::default()
        };
        run_login_provider(provider, None, cancel).await.unwrap();
        assert!(!pending.exists());
        assert_eq!(std::fs::read(&client).unwrap(), old_client);
        assert_eq!(std::fs::read(&tokens).unwrap(), old_tokens);
    }

    #[tokio::test]
    async fn transactional_google_login_mock_callback_failure_then_success_commits_pair() {
        let _guard = crate::storage::lock_test_env();
        let temp = tempfile::TempDir::new().unwrap();
        let _home = TestHome::new(temp.path());
        let (client_path, old_client, token_path, _) = seed_old_login(temp.path());
        let mut previous: auth::google::GoogleTokens = serde_json::from_str(OLD_TOKENS).unwrap();
        previous.credentials = Some(GoogleCredentials {
            client_id: "synthetic-bound-old-project".into(),
            client_secret: "synthetic-bound-old-secret".into(),
        });
        auth::google::save_tokens(&previous).unwrap();
        let old_tokens = std::fs::read(&token_path).unwrap();
        let imported = temp.path().join("client_secret_other.json");
        std::fs::write(&imported, CLIENT_JSON).unwrap();
        let provider = crate::provider_catalog::resolve_login_provider("google").unwrap();
        let begin = LoginOptions {
            google_client_json: Some(imported.display().to_string()),
            google_services: Some(vec![GoogleService::Gmail]),
            print_auth_url: true,
            flow_id: Some("synthetic-google-callback".into()),
            no_browser: true,
            json: true,
            ..LoginOptions::default()
        };
        run_login_provider(provider, None, begin.clone())
            .await
            .unwrap();
        let path = pending_login_path("google", begin.flow_id.as_deref()).unwrap();
        let PendingScriptableLogin::Google {
            state, credentials, ..
        } = load_pending_login(&path, "google").unwrap()
        else {
            panic!("expected Google pending flow");
        };
        let selected = credentials.unwrap();
        assert_eq!(
            selected.client_id,
            "synthetic-other-project.apps.googleusercontent.com"
        );
        assert_eq!(std::fs::read(&client_path).unwrap(), old_client);
        assert_eq!(std::fs::read(&token_path).unwrap(), old_tokens);
        let complete = LoginOptions {
            flow_id: begin.flow_id,
            json: true,
            ..LoginOptions::default()
        };
        let callback = format!("http://127.0.0.1/?code=synthetic-code&state={state}");
        let error = auth::google::with_test_token_exchange(
            selected.clone(),
            Err("synthetic endpoint failure".into()),
            complete_scriptable_google_login(
                "google",
                &complete,
                ProvidedAuthInput::CallbackUrl(callback.clone()),
            ),
        )
        .await
        .unwrap_err();
        assert!(error.to_string().contains("synthetic endpoint failure"));
        assert!(path.exists());
        assert_eq!(std::fs::read(&client_path).unwrap(), old_client);
        assert_eq!(std::fs::read(&token_path).unwrap(), old_tokens);

        let mut replacement: auth::google::GoogleTokens = serde_json::from_str(OLD_TOKENS).unwrap();
        replacement.access_token = "synthetic-new-access".into();
        replacement.refresh_token = "synthetic-new-refresh".into();
        let result = auth::google::with_test_token_exchange(
            selected.clone(),
            Ok(replacement),
            complete_scriptable_google_login(
                "google",
                &complete,
                ProvidedAuthInput::CallbackUrl(callback),
            ),
        )
        .await
        .unwrap();
        assert_eq!(result, LoginFlowOutcome::Completed);
        assert!(!path.exists());
        let stored = auth::google::load_tokens().unwrap();
        assert_eq!(stored.refresh_token, "synthetic-new-refresh");
        assert_eq!(stored.credentials.as_ref(), Some(&selected));
        assert_eq!(auth::google::load_credentials().unwrap(), selected);
        assert_eq!(std::fs::read(&client_path).unwrap(), old_client);
    }

    #[test]
    fn transactional_google_login_legacy_pending_client_defaults_to_none() {
        let pending: PendingScriptableLogin = serde_json::from_str(
            r#"{"provider":"google","verifier":"synthetic","state":"synthetic","redirect_uri":"http://127.0.0.1:8456","tier":"full"}"#,
        )
        .unwrap();
        assert!(matches!(
            pending,
            PendingScriptableLogin::Google {
                credentials: None,
                ..
            }
        ));
    }
}

fn prompt_google_services(existing: Option<&[GoogleService]>) -> Result<Vec<GoogleService>> {
    let default = existing
        .filter(|s| !s.is_empty())
        .map(|s| s.to_vec())
        .unwrap_or_else(|| GoogleService::ALL.to_vec());
    let default_ids = default
        .iter()
        .map(GoogleService::id)
        .collect::<Vec<_>>()
        .join(",");

    eprintln!("── Google Services ──\n");
    eprintln!("  gmail     Search, read, draft, and send email");
    eprintln!("  calendar  View and manage Google Calendar events\n");
    eprintln!("Enter a comma-separated list, or 'all'.");
    let input = prompt(&format!("Services (default: {default_ids}): "))?;
    if input.is_empty() {
        return Ok(default);
    }
    match GoogleService::parse_list(&input) {
        Ok(services) => Ok(services),
        Err(err) => {
            eprintln!("{err} Using {default_ids}.");
            Ok(default)
        }
    }
}
