use super::*;

pub(super) const OPENAI_LOGIN_TITLE: &str = "OpenAI login";

impl App {
    pub(in crate::tui::app) fn update_openai_login_exchange(&mut self, label: &str) {
        if let Some(index) = self.display_messages.iter().rposition(|message| {
            message.role == "system" && message.title.as_deref() == Some(OPENAI_LOGIN_TITLE)
        }) {
            self.replace_display_message_content(
                index,
                format!("OpenAI login [{label}]: callback received, exchanging credentials..."),
            );
        }
    }

    pub(in crate::tui::app) fn show_openai_login_progress(
        &mut self,
        label: &str,
        url: &str,
        browser_opened: bool,
        callback_available: bool,
        details: &str,
    ) {
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
            "✓ Sign-in link ready · {browser}\n◌ {waiting}\n[Open OpenAI sign-in]({url})\nPaste callback URL here if needed · /cancel"
        );
        let content = jcode_tui_messages::encode_collapsible_swarm_content(
            "Sign-in details",
            &format!(
                "{summary}\n\n<!-- oauth-details -->\n\nAccount: {label}\n\nFull sign-in URL:\n{url}\n\n{details}"
            ),
        );
        let mut message = DisplayMessage::system(content);
        message.title = Some(OPENAI_LOGIN_TITLE.to_string());
        self.push_display_message(message);
    }

    /// Replace the pending card, removing stale links and instructions entirely.
    pub(in crate::tui::app) fn finish_openai_login_progress(&mut self, content: String) -> bool {
        let Some(index) = self.display_messages.iter().rposition(|message| {
            message.role == "system" && message.title.as_deref() == Some(OPENAI_LOGIN_TITLE)
        }) else {
            return false;
        };
        self.replace_display_message_title_and_content(index, None, content)
    }
}
