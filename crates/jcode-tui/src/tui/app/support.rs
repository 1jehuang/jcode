//! `/support` command: gather diagnostics and open a prefilled support email.
//!
//! The diagnostics builder is a pure function over [`SupportDiagnostics`] so it
//! can be unit-tested without an `App`. The command opens a `mailto:` URL via
//! the shared detached opener (which honors NO_BROWSER and test suppression)
//! and always prints the full email text in the TUI as a fallback for
//! terminals that cannot open mailto links.

use super::{App, DisplayMessage};

pub(super) const SUPPORT_EMAIL: &str = "support@jcode.sh";
pub(super) const SUPPORT_SUBJECT: &str = "jcode support";

/// Everything the support email body needs. Pure data so the body builder is
/// trivially testable.
#[derive(Debug, Clone, Default)]
pub(super) struct SupportDiagnostics {
    pub version: String,
    pub git_hash: String,
    pub build_channel: String,
    pub os: String,
    pub arch: String,
    pub telemetry_id: Option<String>,
    pub account_id: Option<String>,
    pub account_email: Option<String>,
    pub tier: Option<String>,
    pub provider: String,
    pub model: String,
    pub last_error: Option<String>,
}

/// Build the plain-text email body from diagnostics. Pure function.
pub(super) fn build_support_body(d: &SupportDiagnostics) -> String {
    let mut body = String::from("Describe your issue:\n\n\n");
    body.push_str("--- Diagnostics (auto-generated) ---\n");
    body.push_str(&format!("Version: {}\n", d.version));
    body.push_str(&format!("Git hash: {}\n", d.git_hash));
    body.push_str(&format!("Build channel: {}\n", d.build_channel));
    body.push_str(&format!("OS/Arch: {}/{}\n", d.os, d.arch));
    if let Some(id) = &d.telemetry_id {
        body.push_str(&format!("Telemetry ID: {}\n", id));
    }
    if let Some(id) = &d.account_id {
        body.push_str(&format!("Account ID: {}\n", id));
    }
    if let Some(email) = &d.account_email {
        body.push_str(&format!("Account email: {}\n", email));
    }
    if let Some(tier) = &d.tier {
        body.push_str(&format!("Tier: {}\n", tier));
    }
    body.push_str(&format!("Provider: {}\n", d.provider));
    body.push_str(&format!("Model: {}\n", d.model));
    if let Some(err) = &d.last_error {
        body.push_str(&format!("Last error: {}\n", err));
    }
    body
}

/// Build the `mailto:` URL with subject and body URL-encoded.
pub(super) fn build_mailto_url(body: &str) -> String {
    format!(
        "mailto:{}?subject={}&body={}",
        SUPPORT_EMAIL,
        urlencoding::encode(SUPPORT_SUBJECT),
        urlencoding::encode(body)
    )
}

fn build_channel() -> String {
    if std::env::var(jcode_selfdev_types::CLIENT_SELFDEV_ENV).is_ok() {
        return "selfdev".to_string();
    }
    if jcode_build_meta::is_release_build() {
        return "release".to_string();
    }
    if let Ok(exe) = std::env::current_exe() {
        let path = exe.to_string_lossy();
        if path.contains("/target/debug/") || path.contains("\\target\\debug\\") {
            return "debug".to_string();
        }
        if path.contains("/target/release/") || path.contains("\\target\\release\\") {
            return "local_build".to_string();
        }
    }
    "dev".to_string()
}

/// Read the persisted telemetry id without creating one (read-only, so
/// `/support` never mutates telemetry state).
fn read_telemetry_id() -> Option<String> {
    let path = crate::storage::jcode_dir().ok()?.join("telemetry_id");
    let id = std::fs::read_to_string(path).ok()?;
    let id = id.trim().to_string();
    if id.is_empty() { None } else { Some(id) }
}

fn gather_diagnostics(app: &App) -> SupportDiagnostics {
    use crate::provider_catalog::load_env_value_from_env_or_config;
    use crate::subscription_catalog as cat;

    // Account identity is only present when a jcode subscription is
    // configured; skip gracefully otherwise.
    let has_subscription = cat::has_credentials();
    let (account_id, account_email, tier) = if has_subscription {
        (
            load_env_value_from_env_or_config(cat::KCODE_ACCOUNT_ID_ENV, cat::KCODE_ENV_FILE),
            load_env_value_from_env_or_config(cat::KCODE_ACCOUNT_EMAIL_ENV, cat::KCODE_ENV_FILE),
            cat::cached_tier().map(|t| t.display_name().to_string()),
        )
    } else {
        (None, None, None)
    };

    let last_error = app
        .display_messages
        .iter()
        .rev()
        .find(|message| message.role == "error")
        .map(|message| {
            let mut content = message.content.trim().to_string();
            const MAX_ERROR_LEN: usize = 500;
            if content.chars().count() > MAX_ERROR_LEN {
                content = content.chars().take(MAX_ERROR_LEN).collect::<String>() + "…";
            }
            content
        });

    SupportDiagnostics {
        version: jcode_build_meta::version().to_string(),
        git_hash: jcode_build_meta::git_hash().to_string(),
        build_channel: build_channel(),
        os: std::env::consts::OS.to_string(),
        arch: std::env::consts::ARCH.to_string(),
        telemetry_id: read_telemetry_id(),
        account_id,
        account_email,
        tier,
        provider: app.provider_name().to_string(),
        model: app.provider_model(),
        last_error,
    }
}

pub(super) fn handle_support_command(app: &mut App, trimmed: &str) -> bool {
    if trimmed != "/support" && !trimmed.starts_with("/support ") {
        return false;
    }

    let diagnostics = gather_diagnostics(app);
    let body = build_support_body(&diagnostics);
    let mailto = build_mailto_url(&body);
    let opened = super::helpers::open_path_or_url_detached(&mailto).is_ok();

    let mut message = String::new();
    if opened {
        message.push_str("Opened a support email draft in your mail client.\n\n");
    } else {
        message
            .push_str("Could not open your mail client automatically. Copy the email below.\n\n");
    }
    message.push_str(&format!("To: {}\n", SUPPORT_EMAIL));
    message.push_str(&format!("Subject: {}\n\n", SUPPORT_SUBJECT));
    message.push_str(&body);

    app.push_display_message(DisplayMessage::system(message).with_title("Support"));
    app.set_status_notice(if opened {
        "Support email draft opened"
    } else {
        "Support email printed below"
    });
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_diagnostics() -> SupportDiagnostics {
        SupportDiagnostics {
            version: "v0.35.72-dev (abc1234)".to_string(),
            git_hash: "abc1234".to_string(),
            build_channel: "release".to_string(),
            os: "linux".to_string(),
            arch: "x86_64".to_string(),
            telemetry_id: Some("11111111-2222-3333-4444-555555555555".to_string()),
            account_id: Some("acct_42".to_string()),
            account_email: Some("user@example.com".to_string()),
            tier: Some("$100 Pro".to_string()),
            provider: "anthropic".to_string(),
            model: "claude-opus-4-8".to_string(),
            last_error: Some("request timed out".to_string()),
        }
    }

    #[test]
    fn support_body_includes_all_diagnostics() {
        let body = build_support_body(&sample_diagnostics());
        assert!(body.starts_with("Describe your issue:\n"));
        assert!(body.contains("Version: v0.35.72-dev (abc1234)"));
        assert!(body.contains("Git hash: abc1234"));
        assert!(body.contains("Build channel: release"));
        assert!(body.contains("OS/Arch: linux/x86_64"));
        assert!(body.contains("Telemetry ID: 11111111-2222-3333-4444-555555555555"));
        assert!(body.contains("Account ID: acct_42"));
        assert!(body.contains("Account email: user@example.com"));
        assert!(body.contains("Tier: $100 Pro"));
        assert!(body.contains("Provider: anthropic"));
        assert!(body.contains("Model: claude-opus-4-8"));
        assert!(body.contains("Last error: request timed out"));
    }

    #[test]
    fn support_body_skips_optional_fields_when_absent() {
        let body = build_support_body(&SupportDiagnostics {
            version: "v1".to_string(),
            git_hash: "deadbee".to_string(),
            build_channel: "dev".to_string(),
            os: "linux".to_string(),
            arch: "x86_64".to_string(),
            provider: "openai".to_string(),
            model: "gpt-5.5".to_string(),
            ..Default::default()
        });
        assert!(!body.contains("Telemetry ID:"));
        assert!(!body.contains("Account ID:"));
        assert!(!body.contains("Account email:"));
        assert!(!body.contains("Tier:"));
        assert!(!body.contains("Last error:"));
        assert!(body.contains("Provider: openai"));
    }

    #[test]
    fn support_mailto_url_is_correctly_encoded() {
        let url = build_mailto_url("Describe your issue:\n\nVersion: v1 (abc)");
        assert!(url.starts_with("mailto:support@jcode.sh?subject=jcode%20support&body="));
        // Newlines, spaces, colons, and parens must be percent-encoded.
        assert!(url.contains("Describe%20your%20issue%3A%0A%0AVersion%3A%20v1%20%28abc%29"));
        assert!(!url.contains(' '));
        assert!(!url.contains('\n'));
    }
}
