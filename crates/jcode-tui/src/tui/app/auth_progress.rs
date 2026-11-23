use super::*;
use crate::tui::ui::OAUTH_LOGIN_DETAILS_MARKER;

pub(super) const OPENAI_LOGIN_TITLE: &str = "OpenAI login";
pub(super) const CLAUDE_LOGIN_TITLE: &str = "Claude login";
pub(super) const GEMINI_LOGIN_TITLE: &str = "Gemini login";
pub(super) const ANTIGRAVITY_LOGIN_TITLE: &str = "Antigravity login";
pub(super) const COPILOT_LOGIN_TITLE: &str = "Copilot login";
pub(super) const JCODE_LOGIN_TITLE: &str = "Jcode login";
pub(super) const GROK_BUILD_LOGIN_TITLE: &str = "Grok Build login";
pub(super) const MUSE_LOGIN_TITLE: &str = "Muse login";

pub(in crate::tui::app) fn login_card_title_for_provider(provider: &str) -> Option<&'static str> {
    if provider.starts_with("muse_subscription_code#") || provider.starts_with("muse-subscription#")
    {
        return Some(MUSE_LOGIN_TITLE);
    }
    match provider {
        "openai" | "openai_device_code" => Some(OPENAI_LOGIN_TITLE),
        "claude" => Some(CLAUDE_LOGIN_TITLE),
        "gemini" => Some(GEMINI_LOGIN_TITLE),
        "antigravity" => Some(ANTIGRAVITY_LOGIN_TITLE),
        "copilot" | "copilot_code" => Some(COPILOT_LOGIN_TITLE),
        "jcode" => Some(JCODE_LOGIN_TITLE),
        "grok-build" => Some(GROK_BUILD_LOGIN_TITLE),
        "muse-subscription" => Some(MUSE_LOGIN_TITLE),
        _ => None,
    }
}

pub(in crate::tui::app) fn login_card_title_for_pending(
    pending: &PendingLogin,
) -> Option<&'static str> {
    match pending {
        PendingLogin::OpenAiAccount { .. } => Some(OPENAI_LOGIN_TITLE),
        PendingLogin::ClaudeAccount { .. } => Some(CLAUDE_LOGIN_TITLE),
        PendingLogin::Gemini { .. } => Some(GEMINI_LOGIN_TITLE),
        PendingLogin::Antigravity { .. } => Some(ANTIGRAVITY_LOGIN_TITLE),
        PendingLogin::Copilot => Some(COPILOT_LOGIN_TITLE),
        PendingLogin::GrokBuild => Some(GROK_BUILD_LOGIN_TITLE),
        PendingLogin::MuseSubscription { .. } => Some(MUSE_LOGIN_TITLE),
        PendingLogin::Exchanging { provider } => login_card_title_for_provider(provider),
        _ => None,
    }
}

pub(in crate::tui::app) fn login_retry_command_for_provider(provider: &str) -> &'static str {
    if provider.starts_with("muse-subscription") {
        return "/login muse-subscription";
    }
    match provider {
        "openai" => "/login openai",
        "claude" => "/login claude",
        "gemini" => "/login gemini",
        "antigravity" => "/login antigravity",
        "copilot" => "/login copilot",
        "jcode" => "/account jcode login",
        "grok-build" => "/login grok-build",
        _ => "/login",
    }
}

pub(in crate::tui::app) fn encode_oauth_login_card(
    provider_label: &str,
    account_label: Option<&str>,
    url: &str,
    browser_opened: bool,
    callback_available: bool,
    paste_hint: &str,
    details: &str,
) -> String {
    let browser = if browser_opened {
        "Browser launch requested"
    } else {
        "Open the sign-in link manually"
    };
    let waiting = if callback_available {
        "Waiting for browser approval and local callback"
    } else {
        "Waiting for pasted callback (local listener unavailable)"
    };
    let summary = format!(
        "✓ Sign-in link ready · {browser}\n◌ {waiting}\n[Open {provider_label} sign-in]({url})\n{paste_hint} · /cancel"
    );
    let account_section = account_label
        .map(|label| format!("Account: {label}\n\n"))
        .unwrap_or_default();
    let trimmed_details = details.trim();
    let extra = if trimmed_details.is_empty() {
        String::new()
    } else {
        format!("\n\n{trimmed_details}")
    };
    jcode_tui_messages::encode_collapsible_swarm_content(
        "Sign-in details",
        &format!(
            "{summary}{OAUTH_LOGIN_DETAILS_MARKER}{account_section}Full sign-in URL:\n{url}{extra}"
        ),
    )
}

#[allow(clippy::too_many_arguments)]
pub(in crate::tui::app) fn encode_device_login_card(
    provider_label: &str,
    url: &str,
    user_code: Option<&str>,
    clipboard_copied: bool,
    browser_opened: bool,
    waiting_line: &str,
    footer_line: &str,
    details: &str,
) -> String {
    let headline = if user_code.is_some() {
        if browser_opened {
            "✓ Device code ready · Browser launch requested"
        } else {
            "✓ Device code ready · Open on any browser or device"
        }
    } else if browser_opened {
        "✓ Sign-in link ready · Browser launch requested"
    } else {
        "✓ Sign-in link ready · Open the sign-in link manually"
    };
    let mut summary_lines = vec![headline.to_string()];
    if let Some(code) = user_code {
        if clipboard_copied {
            summary_lines.push(format!("Code: {code}  (copied to clipboard)"));
        } else {
            summary_lines.push(format!("Code: {code}"));
        }
    }
    summary_lines.push(format!("◌ {waiting_line}"));
    summary_lines.push(format!("[Open {provider_label} sign-in]({url})"));
    if !footer_line.is_empty() {
        summary_lines.push(footer_line.to_string());
    }
    let summary = summary_lines.join("\n");
    let trimmed_details = details.trim();
    let extra = if trimmed_details.is_empty() {
        String::new()
    } else {
        format!("\n\n{trimmed_details}")
    };
    jcode_tui_messages::encode_collapsible_swarm_content(
        "Sign-in details",
        &format!("{summary}{OAUTH_LOGIN_DETAILS_MARKER}Full sign-in URL:\n{url}{extra}"),
    )
}

impl App {
    pub(in crate::tui::app) fn upsert_login_progress_card(&mut self, title: &str, content: String) {
        if let Some(index) = self.display_messages.iter().rposition(|message| {
            message.role == "system" && message.title.as_deref() == Some(title)
        }) {
            self.replace_display_message_content(index, content);
            return;
        }
        let mut message = DisplayMessage::system(content);
        message.title = Some(title.to_string());
        self.push_display_message(message);
    }

    pub(in crate::tui::app) fn update_login_progress_exchange(
        &mut self,
        title: &str,
        account_label: Option<&str>,
    ) {
        let text = match account_label {
            Some(label) => {
                format!("{title} [{label}]: callback received, exchanging credentials...")
            }
            None => format!("{title}: callback received, exchanging credentials..."),
        };
        if let Some(index) = self.display_messages.iter().rposition(|message| {
            message.role == "system" && message.title.as_deref() == Some(title)
        }) {
            self.replace_display_message_content(index, text);
        } else {
            let mut message = DisplayMessage::system(text);
            message.title = Some(title.to_string());
            self.push_display_message(message);
        }
    }

    pub(in crate::tui::app) fn update_openai_login_exchange(&mut self, label: &str) {
        self.update_login_progress_exchange(OPENAI_LOGIN_TITLE, Some(label));
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "OAuth progress card accepts provider display metadata, browser/callback state, and details"
    )]
    pub(in crate::tui::app) fn show_oauth_login_progress(
        &mut self,
        title: &str,
        provider_label: &str,
        account_label: Option<&str>,
        url: &str,
        browser_opened: bool,
        callback_available: bool,
        paste_hint: &str,
        details: &str,
    ) {
        let content = encode_oauth_login_card(
            provider_label,
            account_label,
            url,
            browser_opened,
            callback_available,
            paste_hint,
            details,
        );
        self.upsert_login_progress_card(title, content);
    }

    pub(in crate::tui::app) fn show_openai_login_progress(
        &mut self,
        label: &str,
        url: &str,
        browser_opened: bool,
        callback_available: bool,
        details: &str,
    ) {
        self.show_oauth_login_progress(
            OPENAI_LOGIN_TITLE,
            "OpenAI",
            Some(label),
            url,
            browser_opened,
            callback_available,
            "Paste callback URL here if needed",
            details,
        );
    }

    /// Replace the pending card, removing stale links and instructions entirely.
    pub(in crate::tui::app) fn finish_login_progress(
        &mut self,
        title: &str,
        content: String,
    ) -> bool {
        let Some(index) = self.display_messages.iter().rposition(|message| {
            message.role == "system" && message.title.as_deref() == Some(title)
        }) else {
            return false;
        };
        self.replace_display_message_title_and_content(index, None, content)
    }

    pub(in crate::tui::app) fn finish_openai_login_progress(&mut self, content: String) -> bool {
        self.finish_login_progress(OPENAI_LOGIN_TITLE, content)
    }
}
