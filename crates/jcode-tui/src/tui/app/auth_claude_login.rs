use super::*;

impl App {
    pub(in crate::tui::app) fn start_claude_login(&mut self) {
        let prompt = if crate::auth::claude::prefer_claude_code_cli_login() {
            "Claude login method\n\n1. Jcode OAuth: sign in directly in a browser.\n2. Claude Code CLI (default): sign in with `claude auth login` in a new terminal, then approve Jcode's access to that login.\n\nReply 1 or 2 (Enter for 2), or /cancel."
        } else {
            "Claude login method\n\n1. Jcode OAuth (default): sign in directly in a browser.\n2. Claude Code CLI: sign in with `claude auth login` in a new terminal, then approve Jcode's access to that login.\n\nReply 1 or 2 (Enter for 1), or /cancel."
        };
        self.push_display_message(DisplayMessage::system(prompt.to_string()));
        self.set_status_notice("Claude login: choose 1 or 2");
        self.begin_pending_login(PendingLogin::ClaudeMethodChoice);
    }

    pub(super) fn start_jcode_claude_login(&mut self) {
        let label = crate::auth::claude::login_target_label(None)
            .unwrap_or_else(|_| crate::auth::claude::primary_account_label());
        self.start_claude_login_for_account(&label);
    }

    pub(super) fn start_claude_code_cli_login(&mut self) {
        if crate::tui::is_ssh_remote() {
            self.push_display_message(DisplayMessage::error(
                "Claude Code CLI login must run on the remote host. SSH into that host and run `jcode login --provider claude --claude-code` there."
                    .to_string(),
            ));
            return;
        }
        self.track_claude_cli_login();
        let command = "jcode login --provider claude --claude-code";
        // macOS terminal launchers start a fresh login shell rather than
        // inheriting this process's environment. Do not silently sign into a
        // different Claude profile or Jcode home.
        if [
            "CLAUDE_CONFIG_DIR",
            "JCODE_HOME",
            "XDG_CONFIG_HOME",
            "JCODE_GLOBAL_HOME",
        ]
        .iter()
        .any(|name| std::env::var_os(name).is_some_and(|value| !value.is_empty()))
        {
            self.push_display_message(DisplayMessage::system(format!(
                "This Jcode session uses an explicit home or configuration directory. Open a terminal with the same environment settings, then run:\n\n  {command}\n\nReturn here after sign-in and press Enter; /cancel stops waiting. A new terminal here may lose those settings and sign into the wrong profile."
            )));
            self.set_status_notice(
                "Claude Code CLI login: run on Jcode host with same environment",
            );
            return;
        }
        let opened = if cfg!(test) {
            Ok(false)
        } else {
            std::env::current_exe().and_then(|exe| {
                let cwd = std::env::current_dir()?;
                let invocation = crate::terminal_launch::TerminalCommand::new(
                    exe,
                    vec![
                        "login".into(),
                        "--provider".into(),
                        "claude".into(),
                        "--claude-code".into(),
                    ],
                )
                .title("jcode · Claude Code login".to_string());
                crate::terminal_launch::spawn_command_in_new_terminal(&invocation, &cwd)
                    .map_err(std::io::Error::other)
            })
        };
        match opened {
            Ok(true) => self.push_display_message(DisplayMessage::system(
                "Opened Claude Code login in a new terminal. Complete sign-in and approve credential reuse there; return here after it finishes and press Enter if Jcode has not advanced automatically. /cancel stops waiting."
                    .to_string(),
            )),
            Ok(false) => self.push_display_message(DisplayMessage::system(format!(
                "No new terminal was available. Run this on the Jcode host to sign in, then return here and press Enter (/cancel stops waiting):\n\n  {command}"
            ))),
            Err(error) => self.push_display_message(DisplayMessage::error(format!(
                "Could not open a terminal ({error}). Run this on the Jcode host, then return here and press Enter (/cancel stops waiting):\n\n  {command}"
            ))),
        }
        self.set_status_notice("Claude Code CLI login: complete in terminal");
    }

    pub(in crate::tui::app) fn start_claude_login_for_account(&mut self, label: &str) {
        use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
        use sha2::{Digest, Sha256};

        let verifier: String = {
            use rand::Rng;
            const CHARSET: &[u8] =
                b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
            let mut rng = rand::rng();
            (0..64)
                .map(|_| {
                    let idx = rng.random_range(0..CHARSET.len());
                    CHARSET[idx] as char
                })
                .collect()
        };

        let mut hasher = Sha256::new();
        hasher.update(verifier.as_bytes());
        let hash = hasher.finalize();
        let challenge = URL_SAFE_NO_PAD.encode(hash);

        // Try a loopback callback first so the user never has to copy/paste the
        // authorization code (mirrors the OpenAI/Gemini flows). Claude uses the
        // PKCE verifier as the OAuth `state`, so we wait for that on the
        // listener. If binding fails we fall back to manual paste with the
        // hosted redirect page.
        let callback_listener = match crate::auth::oauth::bind_callback_listener(0) {
            Ok(listener) => Some(listener),
            Err(error) => {
                crate::logging::warn(&format!(
                    "Claude OAuth callback listener unavailable: {error}"
                ));
                None
            }
        };
        let callback_port = match callback_listener
            .as_ref()
            .map(|listener| listener.local_addr())
        {
            Some(Ok(address)) => Some(address.port()),
            Some(Err(error)) => {
                crate::logging::warn(&format!(
                    "Claude OAuth callback address unavailable: {error}"
                ));
                None
            }
            None => None,
        };
        let callback_available = callback_listener.is_some() && callback_port.is_some();

        let (auth_url, redirect_uri) = match callback_port {
            Some(port) if callback_available => {
                let redirect_uri = format!("http://localhost:{}/callback", port);
                let auth_url =
                    crate::auth::oauth::claude_auth_url(&redirect_uri, &challenge, &verifier);
                (auth_url, redirect_uri)
            }
            _ => {
                let redirect_uri = crate::auth::oauth::claude::REDIRECT_URI.to_string();
                let auth_url =
                    crate::auth::oauth::claude_auth_url(&redirect_uri, &challenge, &verifier);
                (auth_url, redirect_uri)
            }
        };
        let qr_section = jcode_terminal_prompt::qr::markdown_section_for_tui(
            &auth_url,
            "Scan this on another device if this machine has no browser:",
        )
        .map_or_else(String::new, |section| format!("\n\n{section}"));

        let browser_opened = Self::open_auth_browser(&auth_url);
        let preflight = Self::record_oauth_preflight(
            "claude",
            browser_opened,
            callback_port.map(|p| format!("localhost:{}", p)).as_deref(),
            Some(callback_available),
        );

        // Spawn the loopback waiter. On success it publishes LoginCompleted just
        // like the manual paste path, so onboarding and account UI react
        // identically.
        if let (Some(listener), true) = (callback_listener, callback_available) {
            let verifier_clone = verifier.clone();
            let label_clone = label.to_string();
            let redirect_clone = redirect_uri.clone();
            tokio::spawn(async move {
                match Self::claude_login_callback(
                    verifier_clone,
                    label_clone,
                    redirect_clone,
                    listener,
                )
                .await
                {
                    Ok(msg) => {
                        crate::logging::info(&format!("Claude login: {}", msg));
                        Bus::global().publish(BusEvent::LoginCompleted(LoginCompleted {
                            provider: "claude".to_string(),
                            success: true,
                            message: msg,
                        }));
                    }
                    Err(e) => {
                        crate::logging::info(&format!(
                            "Claude automatic callback did not complete: {}",
                            e
                        ));
                    }
                }
            });
        }

        let callback_line = if callback_available {
            "Waiting for the browser callback... (this completes automatically)\n".to_string()
        } else {
            "After logging in, copy the callback URL or authorization code and paste it here.\n"
                .to_string()
        };
        let details = format!(
            "{}{}{}{}",
            if preflight.is_empty() {
                String::new()
            } else {
                format!("{preflight}\n")
            },
            callback_line,
            if preflight.is_empty() {
                String::new()
            } else {
                "Manual-safe fallback is already active here.\n".to_string()
            },
            qr_section.trim_start_matches('\n')
        );

        self.show_oauth_login_progress(
            auth_progress::CLAUDE_LOGIN_TITLE,
            "Claude",
            Some(label),
            &auth_url,
            browser_opened,
            callback_available,
            "Paste callback URL or code here if needed",
            &details,
        );
        if callback_available {
            self.set_status_notice(format!("Login [{}]: waiting...", label));
        } else {
            self.set_status_notice(format!("Login [{}]: paste code...", label));
        }
        self.begin_pending_login(PendingLogin::ClaudeAccount {
            verifier,
            label: label.to_string(),
            redirect_uri: if callback_available {
                Some(redirect_uri)
            } else {
                None
            },
        });
    }
}

#[cfg(test)]
#[path = "auth_claude_login_tests.rs"]
mod tests;
