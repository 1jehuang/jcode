//! Guided setup for the user's own Google Cloud OAuth app.
//!
//! jcode's Google integrations use a bring-your-own OAuth client. This module
//! holds the reusable, UI-free pieces of that setup so the CLI wizard, the
//! agent-driven browser flow, and Desktop can share them:
//!
//! - building Google Cloud Console deep links with the project preselected,
//! - automating project creation and API enabling through `gcloud` when it is
//!   installed and signed in,
//! - finding, validating, and importing the OAuth client JSON the user
//!   downloads from the console.

use super::{GoogleCredentials, GoogleService};
use anyhow::{Context, Result};
use serde::Deserialize;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, SystemTime};

/// Default display name and id prefix for a project created by setup.
pub const DEFAULT_PROJECT_NAME: &str = "jcode";

/// Console page for creating a project.
pub fn project_create_url() -> &'static str {
    "https://console.cloud.google.com/projectcreate"
}

fn with_project(url: &str, project: Option<&str>) -> String {
    match project.filter(|p| !p.trim().is_empty()) {
        Some(p) => {
            let sep = if url.contains('?') { '&' } else { '?' };
            format!("{url}{sep}project={}", urlencoding::encode(p.trim()))
        }
        None => url.to_string(),
    }
}

/// One link that enables every API for the selected services at once.
pub fn enable_apis_url(services: &[GoogleService], project: Option<&str>) -> String {
    let ids = services
        .iter()
        .map(|s| s.api_service_name())
        .collect::<Vec<_>>()
        .join(",");
    with_project(
        &format!("https://console.cloud.google.com/flows/enableapi?apiid={ids}"),
        project,
    )
}

/// OAuth consent screen ("Branding") page.
pub fn consent_screen_url(project: Option<&str>) -> String {
    with_project("https://console.cloud.google.com/auth/branding", project)
}

/// Audience page, where the app is published out of Testing mode.
pub fn audience_url(project: Option<&str>) -> String {
    with_project("https://console.cloud.google.com/auth/audience", project)
}

/// Page that creates a new OAuth client.
pub fn create_client_url(project: Option<&str>) -> String {
    with_project(
        "https://console.cloud.google.com/auth/clients/create",
        project,
    )
}

/// Why the app has to be published, shown wherever setup finishes.
pub const PUBLISH_APP_NOTE: &str = "Publish the app (Audience page > Publish app). While it stays in \
     Testing mode Google expires the login every 7 days. Publishing your own app needs no \
     Google review.";

/// Whether an OAuth error looks like the 7-day Testing-mode expiry (or any
/// revoked refresh token), so callers can point at [`PUBLISH_APP_NOTE`].
pub fn looks_like_expired_grant(error: &str) -> bool {
    let lower = error.to_ascii_lowercase();
    lower.contains("invalid_grant")
        || lower.contains("token has been expired or revoked")
        || lower.contains("expired or revoked")
}

// ---------------------------------------------------------------------------
// gcloud automation
// ---------------------------------------------------------------------------

/// State of the local `gcloud` CLI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GcloudStatus {
    pub installed: bool,
    /// Signed-in account, if any.
    pub account: Option<String>,
    /// Currently selected default project, if any.
    pub project: Option<String>,
}

impl GcloudStatus {
    pub fn usable(&self) -> bool {
        self.installed && self.account.is_some()
    }
}

fn gcloud_output(args: &[&str]) -> Result<std::process::Output> {
    Command::new("gcloud")
        .args(args)
        .arg("--quiet")
        .output()
        .with_context(|| format!("failed to run gcloud {}", args.join(" ")))
}

fn non_empty(value: &[u8]) -> Option<String> {
    let s = String::from_utf8_lossy(value).trim().to_string();
    (!s.is_empty() && s != "(unset)").then_some(s)
}

/// Detect `gcloud`, its signed-in account, and default project.
pub fn gcloud_status() -> GcloudStatus {
    let installed = Command::new("gcloud")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    if !installed {
        return GcloudStatus {
            installed: false,
            account: None,
            project: None,
        };
    }
    // `config get-value account` reports a configured account even when it
    // has no credentials (verified on gcloud 587), so ask for accounts with
    // usable credentials instead.
    let account = gcloud_output(&[
        "auth",
        "list",
        "--filter=status:ACTIVE",
        "--format=value(account)",
    ])
    .ok()
    .filter(|o| o.status.success())
    .and_then(|o| non_empty(&o.stdout));
    let project = gcloud_output(&["config", "get-value", "project"])
        .ok()
        .and_then(|o| non_empty(&o.stdout));
    GcloudStatus {
        installed,
        account,
        project,
    }
}

fn stderr_summary(output: &std::process::Output) -> String {
    summarize_gcloud_error(&String::from_utf8_lossy(&output.stderr))
}

/// Short form of gcloud's stderr: the `ERROR:` headline plus a few lines of
/// guidance. gcloud's own errors put the headline first and remedies after,
/// so keep the start rather than the tail.
fn summarize_gcloud_error(stderr: &str) -> String {
    let lines: Vec<&str> = stderr.lines().filter(|l| !l.trim().is_empty()).collect();
    let start = lines
        .iter()
        .position(|l| l.trim_start().starts_with("ERROR:"))
        .unwrap_or(0);
    lines[start..]
        .iter()
        .take(6)
        .copied()
        .collect::<Vec<_>>()
        .join("\n")
}

/// Derive a valid, likely-unique project id from a display name.
///
/// Project ids are 6-30 chars of lowercase letters, digits, and hyphens,
/// starting with a letter and not ending with a hyphen.
pub fn suggest_project_id(name: &str, suffix: u32) -> String {
    let mut base: String = name
        .to_ascii_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    while base.contains("--") {
        base = base.replace("--", "-");
    }
    let mut base = base.trim_matches('-').to_string();
    if !base.starts_with(|c: char| c.is_ascii_lowercase()) {
        base = format!("j-{base}");
    }
    let suffix = format!("-{:06}", suffix % 1_000_000);
    base.truncate(30 - suffix.len());
    let base = base.trim_end_matches('-');
    format!("{base}{suffix}")
}

/// Create a project with `gcloud`. Returns the project id.
pub fn gcloud_create_project(project_id: &str, name: &str) -> Result<String> {
    let output = gcloud_output(&["projects", "create", project_id, "--name", name])?;
    if !output.status.success() {
        anyhow::bail!(
            "gcloud could not create project {project_id}:\n{}",
            stderr_summary(&output)
        );
    }
    Ok(project_id.to_string())
}

/// Enable the APIs for `services` in `project` with `gcloud`.
pub fn gcloud_enable_apis(project: &str, services: &[GoogleService]) -> Result<()> {
    let mut args = vec!["services", "enable"];
    let names: Vec<&str> = services.iter().map(|s| s.api_service_name()).collect();
    args.extend(names.iter().copied());
    args.extend(["--project", project]);
    let output = gcloud_output(&args)?;
    if !output.status.success() {
        anyhow::bail!(
            "gcloud could not enable {} in {project}:\n{}",
            names.join(", "),
            stderr_summary(&output)
        );
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// OAuth client JSON import
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct ClientFile {
    installed: Option<ClientInner>,
    web: Option<ClientInner>,
}

#[derive(Deserialize)]
struct ClientInner {
    client_id: String,
    client_secret: String,
    #[serde(default)]
    project_id: Option<String>,
}

/// A parsed OAuth client JSON downloaded from Google Cloud Console.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientJson {
    pub credentials: GoogleCredentials,
    pub project_id: Option<String>,
    /// True for "Desktop app" clients, which support jcode's loopback
    /// redirect without registering redirect URIs.
    pub desktop: bool,
}

/// Parse the JSON Google Cloud Console offers as a download for an OAuth client.
pub fn parse_client_json(data: &str) -> Result<ClientJson> {
    let file: ClientFile = serde_json::from_str(data).context("not an OAuth client JSON file")?;
    let (inner, desktop) = match (file.installed, file.web) {
        (Some(inner), _) => (inner, true),
        (None, Some(inner)) => (inner, false),
        (None, None) => anyhow::bail!("OAuth client JSON has no 'installed' or 'web' section"),
    };
    if !inner.client_id.ends_with(".apps.googleusercontent.com") || inner.client_secret.is_empty() {
        anyhow::bail!("OAuth client JSON is missing a valid client_id or client_secret");
    }
    Ok(ClientJson {
        credentials: GoogleCredentials {
            client_id: inner.client_id,
            client_secret: inner.client_secret,
        },
        project_id: inner.project_id,
        desktop,
    })
}

/// Folders Google's download usually lands in.
pub fn default_download_dirs() -> Vec<PathBuf> {
    let mut dirs_out = Vec::new();
    if let Some(d) = dirs::download_dir() {
        dirs_out.push(d);
    }
    if let Some(home) = dirs::home_dir() {
        let d = home.join("Downloads");
        if !dirs_out.contains(&d) {
            dirs_out.push(d);
        }
    }
    dirs_out
}

fn is_client_secret_name(path: &Path) -> bool {
    path.file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|n| n.starts_with("client_secret") && n.ends_with(".json"))
}

/// Newest valid `client_secret*.json` in `dirs` modified at or after `since`.
pub fn find_client_json(
    dirs: &[PathBuf],
    since: Option<SystemTime>,
) -> Option<(PathBuf, ClientJson)> {
    let mut candidates: Vec<(SystemTime, PathBuf)> = dirs
        .iter()
        .filter_map(|d| std::fs::read_dir(d).ok())
        .flat_map(|entries| entries.flatten())
        .map(|e| e.path())
        .filter(|p| p.is_file() && is_client_secret_name(p))
        .filter_map(|p| {
            let modified = std::fs::metadata(&p).and_then(|m| m.modified()).ok()?;
            Some((modified, p))
        })
        .filter(|(modified, _)| since.is_none_or(|s| *modified >= s))
        .collect();
    candidates.sort_by(|a, b| b.0.cmp(&a.0));
    candidates.into_iter().find_map(|(_, path)| {
        let data = std::fs::read_to_string(&path).ok()?;
        parse_client_json(&data).ok().map(|parsed| (path, parsed))
    })
}

/// Poll `dirs` until a valid client JSON newer than `since` appears, or the
/// timeout passes. `on_tick` runs between polls (for progress output) and can
/// return `false` to stop early.
pub fn wait_for_client_json(
    dirs: &[PathBuf],
    since: SystemTime,
    timeout: Duration,
    mut on_tick: impl FnMut() -> bool,
) -> Option<(PathBuf, ClientJson)> {
    let deadline = std::time::Instant::now() + timeout;
    loop {
        if let Some(found) = find_client_json(dirs, Some(since)) {
            return Some(found);
        }
        if std::time::Instant::now() >= deadline || !on_tick() {
            return None;
        }
        std::thread::sleep(Duration::from_secs(1));
    }
}

/// Save imported credentials to jcode's credentials file (owner-only).
pub fn save_client_json(parsed: &ClientJson) -> Result<PathBuf> {
    super::save_credentials(&parsed.credentials)?;
    super::credentials_path()
}

#[cfg(test)]
mod tests {
    use super::*;

    const DESKTOP_JSON: &str = r#"{"installed":{"client_id":"123-abc.apps.googleusercontent.com","project_id":"jcode-123456","auth_uri":"https://accounts.google.com/o/oauth2/auth","client_secret":"GOCSPX-x","redirect_uris":["http://localhost"]}}"#;

    #[test]
    fn enable_url_lists_all_selected_apis_with_project() {
        let url = enable_apis_url(
            &[GoogleService::Gmail, GoogleService::Calendar],
            Some("jcode-123456"),
        );
        assert_eq!(
            url,
            "https://console.cloud.google.com/flows/enableapi?apiid=gmail.googleapis.com,calendar-json.googleapis.com&project=jcode-123456"
        );
        assert!(!enable_apis_url(&[GoogleService::Gmail], None).contains("project="));
    }

    #[test]
    fn console_links_preselect_project() {
        assert_eq!(
            create_client_url(Some("p-1")),
            "https://console.cloud.google.com/auth/clients/create?project=p-1"
        );
        assert_eq!(
            audience_url(None),
            "https://console.cloud.google.com/auth/audience"
        );
    }

    #[test]
    fn project_ids_match_gclouds_create_validator() {
        // Exact validator from gcloud 587 surface/projects/create.py
        // (RegexpValidator anchors the whole value), plus the API's
        // no-trailing-hyphen rule from the non-default-universe pattern.
        let gcloud = regex::Regex::new(r"^[a-z][a-z0-9-]{5,29}$").unwrap();
        let names = [
            "jcode",
            "J",
            "9",
            "a-b",
            "My  Project!!",
            "ünïcode",
            "-x-",
            "",
            "abcdefghijklmnopqrstuvwxyz0123456789",
        ];
        for name in names {
            for suffix in [0, 7, 42, 999_999, 1_000_000, u32::MAX] {
                let id = suggest_project_id(name, suffix);
                assert!(gcloud.is_match(&id), "{name:?} -> {id}");
                assert!(!id.ends_with('-'), "{name:?} -> {id}");
            }
        }
    }

    #[test]
    fn project_ids_are_valid() {
        for (name, suffix) in [
            ("jcode", 42),
            ("My Project!!", 999_999),
            ("9lives", 1),
            (&"x".repeat(60)[..], 123),
        ] {
            let id = suggest_project_id(name, suffix);
            assert!((6..=30).contains(&id.len()), "{id}");
            assert!(id.starts_with(|c: char| c.is_ascii_lowercase()), "{id}");
            assert!(!id.ends_with('-'), "{id}");
            assert!(!id.contains("--"), "{id}");
            assert!(
                id.chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'),
                "{id}"
            );
        }
        assert_eq!(suggest_project_id("jcode", 42), "jcode-000042");
    }

    #[test]
    fn parses_desktop_and_web_clients() {
        let parsed = parse_client_json(DESKTOP_JSON).unwrap();
        assert!(parsed.desktop);
        assert_eq!(parsed.project_id.as_deref(), Some("jcode-123456"));
        assert_eq!(parsed.credentials.client_secret, "GOCSPX-x");

        let web = DESKTOP_JSON.replace("installed", "web");
        assert!(!parse_client_json(&web).unwrap().desktop);

        assert!(parse_client_json("{}").is_err());
        assert!(
            parse_client_json(r#"{"installed":{"client_id":"bad","client_secret":"s"}}"#).is_err()
        );
    }

    #[test]
    fn finds_newest_valid_client_json_since_cutoff() {
        let dir = tempfile::tempdir().unwrap();
        let old = dir.path().join("client_secret_old.json");
        std::fs::write(&old, DESKTOP_JSON).unwrap();
        std::fs::write(dir.path().join("client_secret_bad.json"), "not json").unwrap();
        std::fs::write(dir.path().join("other.json"), DESKTOP_JSON).unwrap();
        let dirs = vec![dir.path().to_path_buf()];

        let (path, _) = find_client_json(&dirs, None).unwrap();
        assert_eq!(path, old);

        let future = SystemTime::now() + Duration::from_secs(3600);
        assert!(find_client_json(&dirs, Some(future)).is_none());
    }

    #[test]
    fn wait_returns_none_on_timeout() {
        let dir = tempfile::tempdir().unwrap();
        let found = wait_for_client_json(
            &[dir.path().to_path_buf()],
            SystemTime::now(),
            Duration::from_millis(0),
            || true,
        );
        assert!(found.is_none());
    }

    #[test]
    fn gcloud_error_summary_keeps_headline() {
        // Verbatim from gcloud 587.0.0 `projects create` while signed out.
        let real = "WARNING: some noise\nERROR: (gcloud.projects.create) You do not currently have an active account selected.\nPlease run:\n\n  $ gcloud auth login\n\nto obtain new credentials.\n\nIf you have already logged in with a different account, run:\n\n  $ gcloud config set account ACCOUNT\n\nto select an already authenticated account to use.\n";
        let summary = summarize_gcloud_error(real);
        assert!(
            summary.starts_with("ERROR: (gcloud.projects.create)"),
            "{summary}"
        );
        assert!(summary.contains("gcloud auth login"), "{summary}");
        assert!(!summary.contains("WARNING"), "{summary}");
        assert_eq!(summarize_gcloud_error("plain failure"), "plain failure");
    }

    #[test]
    fn detects_expired_grant_errors() {
        assert!(looks_like_expired_grant(
            r#"Google token refresh failed: {"error": "invalid_grant", "error_description": "Token has been expired or revoked."}"#
        ));
        assert!(!looks_like_expired_grant("network unreachable"));
    }
}
