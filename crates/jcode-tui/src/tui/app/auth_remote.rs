//! SSH-native login. No provider/credential APIs from the local machine belong here.
mod command;
#[cfg(test)]
mod tests;
use super::{App, DisplayMessage, PendingLogin};
use command::{Operation, Reply, Target, Task};
use crossterm::event::{KeyCode, KeyModifiers};

const PROVIDERS: [&str; 6] = [
    "openai",
    "claude",
    "gemini",
    "antigravity",
    "google",
    "copilot",
];

#[derive(PartialEq, Eq)]
enum Phase {
    Choosing,
    Starting,
    Input,
    Completing,
    Cancelling,
    Finished,
}

// Intentionally not Debug/Clone: the input buffer can contain an OAuth secret.
pub(super) struct RemoteLogin {
    target: Target,
    provider: String,
    flow: String,
    phase: Phase,
    input_kind: String,
    input: String,
    task: Option<Task>,
    operation: Option<Operation>,
    quit_after_cancel: bool,
}
impl RemoteLogin {
    fn run(&mut self, operation: Operation, payload: Option<String>) {
        self.operation = Some(operation);
        self.task = Some(Task::spawn(
            self.target.clone(),
            self.provider.clone(),
            self.flow.clone(),
            operation,
            payload,
        ));
    }
}

impl Drop for RemoteLogin {
    fn drop(&mut self) {
        // A waiting-for-paste flow has no Task to cancel. Graceful teardown still
        // invalidates its pending remote authorization, without touching credentials.
        if self.task.is_none() && !self.provider.is_empty() && self.phase != Phase::Finished {
            command::cleanup_detached(
                self.target.clone(),
                self.provider.clone(),
                self.flow.clone(),
            );
        }
    }
}

impl App {
    pub(super) fn handle_ssh_login_command(&mut self, input: &str) -> bool {
        if !crate::tui::is_ssh_remote() || input.split_whitespace().next() != Some("/login") {
            return false;
        }
        if self.remote_login.is_some() {
            self.set_status_notice(
                "SSH login already in progress. Press Esc or type /cancel first.",
            );
            return true;
        }
        let words: Vec<_> = input.split_whitespace().collect();
        let provider = words.get(1).copied();
        if words.len() > 2 || provider.is_some_and(|p| !PROVIDERS.contains(&p)) {
            self.push_display_message(DisplayMessage::system("SSH login supports: openai, claude, gemini, antigravity, google, copilot. Use /login to choose. No local credentials were accessed."));
            return true;
        }
        let target = match Target::from_env() {
            Ok(target) => target,
            Err(message) => {
                self.push_display_message(DisplayMessage::error(format!(
                    "SSH login failed: {message}"
                )));
                return true;
            }
        };
        // Clear every generic input retention surface before accepting any secret.
        self.input.clear();
        self.cursor_pos = 0;
        self.pasted_contents.clear();
        self.clear_input_undo_history();
        self.inline_interactive_state = None;
        self.pending_login = Some(PendingLogin::Remote);
        self.remote_login = Some(RemoteLogin {
            target,
            provider: String::new(),
            flow: hex::encode(rand::random::<[u8; 16]>()),
            phase: Phase::Choosing,
            input_kind: String::new(),
            input: String::new(),
            task: None,
            operation: None,
            quit_after_cancel: false,
        });
        if let Some(provider) = provider {
            self.start_ssh_login(provider);
        } else {
            self.push_display_message(DisplayMessage::system(
                "SSH login: choose a provider\n\n1. openai\n2. claude\n3. gemini\n4. antigravity\n5. google\n6. copilot\n\nEnter a number or provider name. Esc or /cancel cancels. Credentials are saved only on the remote host."
            ));
            self.set_status_notice("SSH login: choose a provider");
        }
        true
    }

    fn start_ssh_login(&mut self, provider: &str) {
        if tokio::runtime::Handle::try_current().is_err() {
            self.finish_ssh_login_ui();
            self.push_display_message(DisplayMessage::error(
                "SSH login failed: async runtime unavailable",
            ));
            return;
        }
        let Some(login) = self.remote_login.as_mut() else {
            return;
        };
        login.provider = provider.to_string();
        login.phase = Phase::Starting;
        login.input.clear();
        login.run(Operation::Begin, None);
        self.input.clear();
        self.cursor_pos = 0;
        self.set_status_notice(format!("SSH login: {provider} starting. Esc cancels."));
    }

    fn finish_ssh_login_ui(&mut self) {
        if let Some(login) = self.remote_login.as_mut() {
            login.phase = Phase::Finished;
        }
        self.remote_login = None;
        self.pending_login = None;
        self.input.clear();
        self.cursor_pos = 0;
        self.pasted_contents.clear();
        self.clear_input_undo_history();
    }

    pub(super) fn cancel_ssh_login(&mut self) {
        let Some(login) = self.remote_login.as_mut() else {
            return;
        };
        if login.phase == Phase::Choosing {
            let quit = login.quit_after_cancel;
            self.finish_ssh_login_ui();
            self.should_quit |= quit;
            self.push_display_message(DisplayMessage::system(
                "SSH login cancelled. No authorization was started.",
            ));
            self.set_status_notice("SSH login cancelled");
            return;
        }
        if login.phase == Phase::Cancelling {
            return;
        }
        login.input.clear();
        login.phase = Phase::Cancelling;
        if let Some(task) = login.task.as_mut() {
            task.cancel();
        } else {
            login.run(Operation::Cancel, None);
        }
        self.input.clear();
        self.cursor_pos = 0;
        self.set_status_notice("SSH login cancelling on remote host...");
    }

    /// Secrets never enter App::input, paste placeholders, undo, history or debug snapshots.
    pub(super) fn append_ssh_login_input(&mut self, text: &str) -> bool {
        let Some(login) = self.remote_login.as_mut() else {
            return false;
        };
        if login.phase == Phase::Cancelling {
            return true;
        }
        if login.input.len().saturating_add(text.len()) > command::INPUT_LIMIT {
            self.set_status_notice(
                "SSH login input too long. Press Ctrl+U to clear or Esc to cancel.",
            );
            return true;
        }
        login.input.extend(text.chars().filter(|c| !c.is_control()));
        self.sync_ssh_login_input_mask();
        true
    }

    fn sync_ssh_login_input_mask(&mut self) {
        let Some(login) = self.remote_login.as_ref() else {
            return;
        };
        self.input = if login.phase == Phase::Choosing
            && (PROVIDERS
                .iter()
                .any(|provider| provider.starts_with(&login.input))
                || matches!(login.input.as_str(), "1" | "2" | "3" | "4" | "5" | "6"))
        {
            login.input.clone()
        } else if login.input.is_empty() {
            String::new()
        } else {
            "[hidden login input]".into()
        };
        self.cursor_pos = self.input.len();
    }

    pub(super) fn handle_ssh_login_key(
        &mut self,
        code: KeyCode,
        modifiers: KeyModifiers,
        text: Option<&str>,
    ) -> bool {
        if self.remote_login.is_none() {
            return false;
        }
        match code {
            KeyCode::Esc => self.cancel_ssh_login(),
            KeyCode::Char('c') if modifiers.contains(KeyModifiers::CONTROL) => {
                self.cancel_ssh_login()
            }
            KeyCode::Char('u') if modifiers.contains(KeyModifiers::CONTROL) => {
                self.remote_login.as_mut().unwrap().input.clear();
                self.sync_ssh_login_input_mask();
            }
            KeyCode::Char('v')
                if modifiers.contains(KeyModifiers::CONTROL)
                    || modifiers.contains(KeyModifiers::SUPER) =>
            {
                // Explicit text clipboard paste only. Never invoke smart file/image paste.
                if let Ok(mut clipboard) = arboard::Clipboard::new() {
                    if let Ok(text) = clipboard.get_text() {
                        self.append_ssh_login_input(&text);
                    }
                }
            }
            KeyCode::Backspace => {
                self.remote_login.as_mut().unwrap().input.pop();
                self.sync_ssh_login_input_mask();
            }
            KeyCode::Enter => self.submit_ssh_login_input(),
            KeyCode::Char(c)
                if !modifiers.intersects(
                    KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SUPER,
                ) =>
            {
                self.append_ssh_login_input(text.unwrap_or(&c.to_string()));
            }
            _ => {}
        }
        true
    }

    fn submit_ssh_login_input(&mut self) {
        let Some(login) = self.remote_login.as_mut() else {
            return;
        };
        let input = std::mem::take(&mut login.input);
        self.input.clear();
        self.cursor_pos = 0;
        let input = input.trim();
        if matches!(input, "/quit" | "/exit") {
            login.quit_after_cancel = true;
            self.cancel_ssh_login();
            return;
        }
        if matches!(input, "/cancel" | "/stop" | "cancel") {
            self.cancel_ssh_login();
            return;
        }
        if login.phase == Phase::Choosing {
            let provider = input
                .parse::<usize>()
                .ok()
                .and_then(|n| n.checked_sub(1))
                .and_then(|n| PROVIDERS.get(n))
                .copied()
                .or_else(|| {
                    PROVIDERS
                        .iter()
                        .copied()
                        .find(|p| *p == input.strip_prefix("/login ").unwrap_or(input))
                });
            if let Some(provider) = provider {
                self.start_ssh_login(provider);
            } else {
                self.set_status_notice("Choose 1-6 or a provider name. Esc cancels.");
            }
            return;
        }
        if login.phase != Phase::Input {
            self.set_status_notice("SSH login is working. Esc cancels.");
            return;
        }
        if input.is_empty() && login.input_kind != "complete" {
            self.set_status_notice("Paste the browser callback URL or authorization code, then press Enter. Esc cancels.");
            return;
        }
        let operation = match login.input_kind.as_str() {
            "complete" => Operation::Complete,
            "auth_code" => Operation::Code,
            "auth_code_or_callback_url"
                if !input.starts_with("http") && !input.starts_with('?') =>
            {
                Operation::Code
            }
            _ => Operation::Callback,
        };
        login.phase = Phase::Completing;
        login.run(
            operation,
            (operation != Operation::Complete).then(|| input.to_string()),
        );
        self.set_status_notice("SSH login: completing on remote host. Esc cancels.");
    }

    pub(super) async fn poll_ssh_login(
        &mut self,
        remote: &mut crate::tui::backend::RemoteConnection,
    ) -> bool {
        let Some(login) = self.remote_login.as_mut() else {
            return false;
        };
        let Some(task) = login.task.as_mut() else {
            return false;
        };
        let reply = match task.reply.try_recv() {
            Ok(reply) => reply,
            Err(tokio::sync::oneshot::error::TryRecvError::Empty) => return false,
            Err(_) => Err("Remote login task stopped"),
        };
        login.task = None;
        let provider = login.provider.clone();
        let quit_after_cancel = login.quit_after_cancel;
        match reply {
            Ok(Reply::Pending {
                auth_url,
                input_kind,
                user_code,
            }) => {
                if login.phase == Phase::Cancelling {
                    login.run(Operation::Cancel, None);
                    return true;
                }
                login.phase = Phase::Input;
                login.input_kind = input_kind.clone();
                let opened = Self::open_auth_browser(&auth_url);
                let host = crate::tui::ssh_remote_host().unwrap_or_else(|| "remote host".into());
                let instructions = if input_kind == "complete" {
                    format!(
                        "Enter code {} in your browser, then press Enter here to continue.",
                        user_code.as_deref().unwrap_or("shown by your provider")
                    )
                } else {
                    "After approval, paste the full callback URL or authorization code here and press Enter. If localhost refuses the browser connection, copy that URL from the address bar. Input is hidden and never sent to chat.".into()
                };
                self.push_display_message(DisplayMessage::system(format!(
                    "SSH login: {provider} on {host}\n\n{auth_url}\n\n{}\n{instructions}\nEsc or /cancel cancels. Credentials stay on {host}.",
                    if opened { "Opened authorization in your browser." } else { "Open the URL above in your browser." },
                )));
                self.set_status_notice(
                    "SSH login: waiting for browser approval. Paste completion here. Esc cancels.",
                );
            }
            Ok(Reply::Authenticated { validation_warning }) => {
                self.finish_ssh_login_ui();
                self.should_quit |= quit_after_cancel;
                self.recent_authenticated_provider =
                    Some((provider.clone(), std::time::Instant::now()));
                self.reset_credential_failure_breaker();
                self.invalidate_model_picker_cache();
                // The CLI notifies its daemon too. Notify this exact attached daemon and ask it
                // for fresh catalog data, never call local AuthStatus or provider activation.
                let refreshed = remote
                    .notify_auth_changed_event(Some(&provider), None, false)
                    .await
                    .is_ok();
                let catalog_requested = remote.request_model_catalog().await.is_ok();
                self.push_display_message(DisplayMessage::system(format!(
                    "SSH login: {provider} authenticated on the remote host.{}",
                    if refreshed && catalog_requested {
                        " Refreshing remote provider and model state."
                    } else {
                        " Reconnect to refresh remote provider and model state."
                    }
                )));
                self.set_status_notice(format!("SSH login: {provider} authenticated"));
                if validation_warning {
                    self.push_display_message(DisplayMessage::system("Remote credentials were saved, but post-login validation did not complete successfully. Use /model to choose a remote model and retry. Do not paste the authorization code again."));
                }
            }
            Ok(Reply::Cancelled) => {
                self.finish_ssh_login_ui();
                self.should_quit |= quit_after_cancel;
                self.push_display_message(DisplayMessage::system(
                    "SSH login cancelled. Pending authorization was removed on the remote host. Previously issued credentials are not revoked, and an already-running token exchange may still finish.",
                ));
                self.set_status_notice("SSH login cancelled");
            }
            Err(message) => {
                if login.phase == Phase::Cancelling {
                    // Esc can race a result already queued by a finished task. In
                    // that case the task never receives its cancellation signal.
                    if login.operation != Some(Operation::Cancel) {
                        login.run(Operation::Cancel, None);
                        return true;
                    }
                    self.finish_ssh_login_ui();
                    self.should_quit |= quit_after_cancel;
                    self.push_display_message(DisplayMessage::error("SSH login cancelled locally, but remote cleanup could not be confirmed. The pending authorization expires automatically. No local credentials were changed."));
                } else {
                    // Preserve the scoped flow so invalid callback input can be retried or cancelled.
                    login.phase = Phase::Input;
                    self.push_display_message(DisplayMessage::error(format!("SSH login failed: {message}\nPaste a fresh completion to retry, or /cancel and /login to restart.")));
                }
                self.set_status_notice("SSH login failed");
            }
        }
        true
    }
}
