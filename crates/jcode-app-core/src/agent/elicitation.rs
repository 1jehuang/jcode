//! Register structured questions when a form-capable client is attached.
use super::*;
impl Agent {
    /// Register `ask_user` once any form-capable client attaches, before the
    /// `(desktop, applet)` early return: a TUI reports `(false, false)`.
    ///
    /// The registry is the source of truth, so `Agent` needs no extra field and
    /// detaching never unregisters the tool. Registration invalidates the locked
    /// surface once, because the first request may already have locked it.
    pub(super) async fn register_ask_user(&mut self) {
        if !crate::elicitation::global().forms(&self.session.id) {
            return;
        }
        if self
            .registry
            .tool_names()
            .await
            .iter()
            .any(|name| name == "ask_user")
        {
            return;
        }
        self.registry
            .register(
                "ask_user".into(),
                Arc::new(crate::elicitation::ask_user::AskUserTool),
            )
            .await;
        self.invalidate_sdk_tools();
    }
}
