use super::*;
use std::sync::{
    Arc, Weak,
    atomic::{AtomicBool, Ordering},
};

pub(super) fn selected_login() -> Option<(String, String)> {
    match crate::auth::claude::load_credentials() {
        Ok(credentials)
            if !credentials.access_token.is_empty()
                && credentials.expires_at > chrono::Utc::now().timestamp_millis() =>
        {
            Some((credentials.access_token, credentials.refresh_token))
        }
        _ => None,
    }
}

pub(super) async fn wait_for_login(
    before: Option<(String, String)>,
    cancelled: Weak<AtomicBool>,
    timeout: std::time::Duration,
) -> Option<bool> {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        if cancelled
            .upgrade()
            .is_none_or(|cancelled| cancelled.load(Ordering::Acquire))
        {
            return None;
        }
        if selected_login().is_some_and(|current| Some(current) != before) {
            return Some(true);
        }
        if tokio::time::Instant::now() >= deadline {
            return Some(false);
        }
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    }
}

impl App {
    pub(super) fn track_claude_cli_login(&mut self) {
        let before = selected_login();
        let attempt = format!("{:032x}", rand::random::<u128>());
        let cancelled = Arc::new(AtomicBool::new(false));
        let weak = Arc::downgrade(&cancelled);
        self.begin_pending_login(PendingLogin::ClaudeCli {
            attempt: attempt.clone(),
            cancelled,
        });
        tokio::spawn(async move {
            if let Some(success) =
                wait_for_login(before, weak, std::time::Duration::from_secs(600)).await
            {
                Bus::global().publish(BusEvent::LoginCompleted(LoginCompleted {
                    provider: format!("claude_cli#{attempt}"),
                    success,
                    message: if success { "Claude Code login is ready in Jcode." } else { "No completed Claude Code login was detected. Retry /login or choose Jcode OAuth." }.into(),
                }));
            }
        });
    }

    pub(super) fn accept_claude_cli_completion(&self, login: &mut LoginCompleted) -> bool {
        let Some(attempt) = login.provider.strip_prefix("claude_cli#") else {
            return true;
        };
        if !matches!(&self.pending_login, Some(PendingLogin::ClaudeCli { attempt: pending, cancelled }) if pending == attempt && !cancelled.load(Ordering::Acquire))
        {
            return false;
        }
        login.provider = "claude".into();
        true
    }

    pub(super) fn handle_claude_cli_input(&mut self, pending: PendingLogin) {
        self.pending_login = Some(pending);
        if selected_login().is_some() {
            self.handle_login_completed(LoginCompleted {
                provider: "claude".into(),
                success: true,
                message: "Claude Code login is ready in Jcode.".into(),
            });
        } else {
            self.set_status_notice("Complete Claude Code sign-in in the terminal, then press Enter here; /cancel stops waiting");
        }
    }
}
