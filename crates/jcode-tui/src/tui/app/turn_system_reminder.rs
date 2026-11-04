use super::*;

impl App {
    /// Append a hidden `<system-reminder>` message after the turn's prompt.
    /// Per-turn reminders must never go in the system prompt: it precedes the
    /// whole conversation, so changing it invalidates every cached message.
    pub(in crate::tui::app) fn append_turn_system_reminder(&mut self, reminder: Option<String>) {
        let Some(reminder) = reminder
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        else {
            return;
        };
        let text = format!("<system-reminder>\n{reminder}\n</system-reminder>");
        self.add_provider_message(Message::user(&text));
        self.session.add_message_with_display_role(
            Role::User,
            vec![ContentBlock::Text {
                text,
                cache_control: None,
            }],
            Some(crate::session::StoredDisplayRole::System),
        );
    }
}
