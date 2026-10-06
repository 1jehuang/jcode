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
) -> Result<()> {
    eprintln!("╔══════════════════════════════════════════╗");
    eprintln!("║       Google Integration Setup           ║");
    eprintln!("╚══════════════════════════════════════════╝\n");

    let existing = auth::google::load_tokens().ok();
    let services = match requested_services {
        Some(services) => auth::google::normalize_services(services),
        None => prompt_google_services(existing.as_ref().map(|t| t.services.as_slice()))?,
    };

    let have_creds = auth::google::load_credentials().ok();
    match have_creds {
        Some(creds) if !force_setup => eprintln!(
            "✓ Google OAuth client found ({}...)\n",
            &creds.client_id[..20.min(creds.client_id.len())]
        ),
        _ => {
            if have_creds.is_some() {
                eprintln!("Re-running setup. The existing OAuth client is replaced on import.\n");
            } else {
                eprintln!("No Google OAuth client yet. jcode uses your own Google Cloud app,");
                eprintln!("so your data goes straight from your machine to Google.\n");
            }
            run_client_setup(&services, no_browser)?;
        }
    }

    let tier = choose_gmail_tier(&services, access_tier, existing.as_ref())?;

    eprintln!(
        "\nAuthorizing: {}",
        auth::google::describe_services(&services, tier)
    );
    eprintln!("Google will warn \"Google hasn't verified this app\". That is expected for your");
    eprintln!("own app: click Advanced, then Go to <app name>, then keep every box ticked.");
    eprintln!("\n── Logging in ──\n");

    let tokens = match auth::google::login(&services, tier, no_browser).await {
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
fn run_client_setup(services: &[GoogleService], no_browser: bool) -> Result<()> {
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

fn guided_setup(services: &[GoogleService], no_browser: bool) -> Result<()> {
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
        if ticks % 30 == 0 {
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

fn import_client(parsed: &setup::ClientJson, services: &[GoogleService]) -> Result<()> {
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
    let path = setup::save_client_json(parsed)?;
    eprintln!("  ✓ OAuth client saved to {}", path.display());
    if let Some(project) = &parsed.project_id {
        // The client JSON names its project: make sure the APIs are on there.
        eprintln!(
            "  Project: {project}. If an API is not enabled yet: {}\n",
            setup::enable_apis_url(services, Some(project))
        );
    }
    Ok(())
}

fn import_from_path(services: &[GoogleService]) -> Result<()> {
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

fn paste_credentials() -> Result<()> {
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
    auth::google::save_credentials(&GoogleCredentials {
        client_id,
        client_secret,
    })?;
    eprintln!(
        "✓ Credentials saved to {}",
        auth::google::credentials_path()?.display()
    );
    Ok(())
}

pub(super) fn default_pending_google_services() -> Vec<GoogleService> {
    vec![GoogleService::Gmail]
}

/// `--google-client-json <path|auto>`: import an OAuth client JSON without
/// prompts, so an agent driving the console in a browser can hand the
/// download to jcode. Desktop clients only.
pub(super) fn import_google_client_json(source: &str, quiet: bool) -> Result<()> {
    let (path, parsed) = if source.eq_ignore_ascii_case("auto") {
        setup::find_client_json(&setup::default_download_dirs(), None).ok_or_else(|| {
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
    let saved = setup::save_client_json(&parsed)?;
    if !quiet {
        eprintln!(
            "✓ Imported OAuth client from {} into {}",
            path.display(),
            saved.display()
        );
    }
    Ok(())
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
